#[cfg(target_os = "linux")]
use crate::commands::pack::{CHUNK_SIZE, MEMORY_FILE_THRESHOLD, TarEntry};
#[cfg(target_os = "linux")]
use crate::utils::{FileId, FileMetadata, get_file_id, get_file_metadata};
#[cfg(target_os = "linux")]
use anyhow::Result;
#[cfg(target_os = "linux")]
use crossbeam_channel::{Receiver, Sender};
#[cfg(target_os = "linux")]
use dashmap::DashMap;
#[cfg(target_os = "linux")]
use indicatif::ProgressBar;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use tokio::sync::{Mutex, Semaphore};

#[cfg(target_os = "linux")]
pub fn start_uring_worker(
    path_rx: Receiver<PathBuf>,
    content_tx: Sender<Result<TarEntry>>,
    chunk_tx: Sender<Result<TarEntry>>, // Added argument
    pool_rx: Receiver<Vec<u8>>,
    input_dir: PathBuf,
    pb: Arc<ProgressBar>,
    inode_cache: Arc<DashMap<FileId, PathBuf>>,
    ignore_errors: bool,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        // Create an Async-to-Sync Bridge
        // uring tasks will send to async_tx (non-blocking yield on full)
        // bridge thread will forward to content_tx (blocking)
        let (async_tx, mut async_rx) = tokio::sync::mpsc::channel::<Result<TarEntry>>(100);

        // Spawn Bridge Thread
        let bridge_handle = std::thread::spawn(move || {
            while let Some(res) = async_rx.blocking_recv() {
                match res {
                    Ok(entry) => match entry {
                        TarEntry::LargeFileChunk(_) | TarEntry::LargeFileEnd => {
                            if chunk_tx.send(Ok(entry)).is_err() {
                                break;
                            }
                        }
                        _ => {
                            if content_tx.send(Ok(entry)).is_err() {
                                break;
                            }
                        }
                    },
                    Err(e) => {
                        // Forward errors to content channel
                        if content_tx.send(Err(e)).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        // Large File Serializer Mutex (Async)
        let large_file_mutex = Arc::new(tokio::sync::Mutex::new(()));

        // Start uring Runtime on this thread
        tokio_uring::start(async move {
            let semaphore = Arc::new(Semaphore::new(128)); // Dispatch up to 128 IOs

            loop {
                // Acquire permit
                let permit = match semaphore.clone().acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => break, // Closed
                };

                // Receive path from Crossbeam (blocking involved? No, we spawn blocking)
                let rx = path_rx.clone();
                let path_res = tokio::task::spawn_blocking(move || rx.recv()).await;

                match path_res {
                    Ok(Ok(path)) => {
                        // Got path, spawn local task for IO
                        let async_tx = async_tx.clone();
                        let pool_rx = pool_rx.clone();
                        let base_path = input_dir.clone();
                        let p_bar = pb.clone();
                        let i_cache = inode_cache.clone();
                        let lf_mutex = large_file_mutex.clone();

                        tokio::task::spawn_local(async move {
                            let _permit = permit; // Hold until done

                            process_path_uring(
                                path,
                                base_path,
                                async_tx,
                                pool_rx,
                                p_bar,
                                i_cache,
                                lf_mutex,
                                ignore_errors,
                            )
                            .await;
                        });
                    }
                    Ok(Err(_)) => break, // Channel closed (Done)
                    Err(_) => break,     // Join Error
                }
            }

            // Wait for all in-flight tasks to complete
            // We do this by re-acquiring all semaphore permits.
            // This ensures all spawned tasks have dropped their permits.
            // We use a loop 128 times.
            for _ in 0..128 {
                let _ = semaphore.acquire().await;
            }
        });

        // Wait for bridge to finish (it finishes when async_tx is dropped by uring runtime)
        let _ = bridge_handle.join();
    })
}

#[cfg(target_os = "linux")]
async fn process_path_uring(
    path: PathBuf,
    base_path: PathBuf,
    content_tx: tokio::sync::mpsc::Sender<Result<TarEntry>>,
    pool_rx: Receiver<Vec<u8>>,
    pb: Arc<ProgressBar>,
    inode_cache: Arc<DashMap<FileId, PathBuf>>,
    large_file_mutex: Arc<tokio::sync::Mutex<()>>,
    ignore_errors: bool,
) {
    let process = async {
        let parent = base_path.parent().unwrap_or(&base_path);
        let relative_path = match path.strip_prefix(parent) {
            Ok(p) => p.to_path_buf(),
            Err(_) => path
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("unknown")),
        };

        // We still use blocking fs::symlink_metadata because tokio-uring doesn't have metadata yet?
        // tokio-uring is strictly for Ring IO (Read/Write). Metadata is usually fast enough or we use tokio::fs which is threadpool.
        // For strict Uring purity we would use `statx` via uring, but tokio-uring doesn't expose it easily.
        // We will use standard threadpool blocking for metadata to be safe and compatible.
        // It's technically NOT uring, but the heavy lifting (Reading Content) IS uring.

        // Blocking Metadata
        let (meta, metadata, file_type) = match tokio::task::spawn_blocking({
            let p = path.clone();
            move || -> Result<(std::fs::Metadata, FileMetadata, std::fs::FileType)> {
                let m = std::fs::symlink_metadata(&p)?;
                let mo = get_file_metadata(&p, &m);
                let ft = m.file_type();
                Ok((m, mo, ft))
            }
        })
        .await
        .unwrap()
        {
            Ok(v) => v,
            Err(e) => {
                if ignore_errors {
                    eprintln!("Warning: Skipping unreadable file {:?}: {}", path, e);
                    return Ok::<(), anyhow::Error>(());
                } else {
                    return Err(e.into());
                }
            }
        };

        if file_type.is_dir() {
            content_tx
                .send(Ok(TarEntry::Dir(relative_path.clone(), metadata)))
                .await
                .map_err(|_| anyhow::anyhow!("Channel closed"))?;
        } else if file_type.is_symlink() {
            // Read link is also metadata-ish
            let target = tokio::fs::read_link(&path).await?;
            content_tx
                .send(Ok(TarEntry::Symlink(
                    relative_path.clone(),
                    target,
                    metadata,
                )))
                .await
                .map_err(|_| anyhow::anyhow!("Channel closed"))?;
        } else {
            // Check Hardlinks (CPU/Memory op)
            if let Some(fid) = get_file_id(&path, &meta) {
                if let Some(existing_entry) = inode_cache.get(&fid) {
                    let target = existing_entry.value().clone();
                    content_tx
                        .send(Ok(TarEntry::HardLink(relative_path.clone(), target)))
                        .await
                        .map_err(|_| anyhow::anyhow!("Channel closed"))?;
                    pb.inc(1);
                    pb.set_message(format!("{:?}", relative_path));
                    return Ok(());
                } else {
                    inode_cache.insert(fid, relative_path.clone());
                }
            }

            let len = meta.len();
            if len >= MEMORY_FILE_THRESHOLD {
                // Large File Chunking
                let _lock = large_file_mutex.lock().await;

                content_tx
                    .send(Ok(TarEntry::LargeFileStart(
                        relative_path.clone(),
                        len,
                        metadata,
                    )))
                    .await
                    .map_err(|_| anyhow::anyhow!("Channel closed"))?;

                let file = tokio_uring::fs::File::open(&path).await?;
                let mut pos = 0;
                while pos < len {
                    let chunk_size = std::cmp::min(len - pos, CHUNK_SIZE);
                    let mut buf = pool_rx
                        .try_recv()
                        .unwrap_or_else(|_| Vec::with_capacity(chunk_size as usize));
                    if buf.capacity() < chunk_size as usize {
                        buf.reserve(chunk_size as usize - buf.capacity());
                    }
                    // tokio-uring needs full buffer to read into? No, it takes buffer by value.
                    // But we must correct size.
                    if buf.len() < chunk_size as usize {
                        buf.resize(chunk_size as usize, 0);
                    }

                    let (res, buf_ret) = file.read_at(buf, pos).await;
                    let mut valid_buf = buf_ret;
                    res?;

                    if valid_buf.len() > chunk_size as usize {
                        valid_buf.truncate(chunk_size as usize);
                    }

                    content_tx
                        .send(Ok(TarEntry::LargeFileChunk(valid_buf)))
                        .await
                        .map_err(|_| anyhow::anyhow!("Channel closed"))?;
                    pos += chunk_size;
                }

                content_tx
                    .send(Ok(TarEntry::LargeFileEnd))
                    .await
                    .map_err(|_| anyhow::anyhow!("Channel closed"))?;
                // Unlock
            } else {
                // Small File: Read using IO URING
                let mut buf = pool_rx
                    .try_recv()
                    .unwrap_or_else(|_| Vec::with_capacity(len as usize));
                if buf.capacity() < len as usize {
                    buf.reserve(len as usize - buf.capacity());
                }
                if buf.len() < len as usize {
                    buf.resize(len as usize, 0);
                }

                let file = tokio_uring::fs::File::open(&path).await?;
                let (res, buf_ret) = file.read_at(buf, 0).await;
                let mut valid_buf = buf_ret;
                res?;

                if valid_buf.len() > len as usize {
                    valid_buf.truncate(len as usize);
                }

                content_tx
                    .send(Ok(TarEntry::SmallFile(
                        relative_path.clone(),
                        valid_buf,
                        metadata,
                    )))
                    .await
                    .map_err(|_| anyhow::anyhow!("Channel closed"))?;
            }
        }
        pb.inc(1);
        pb.set_message(format!("{:?}", relative_path));
        Ok(())
    };

    if let Err(e) = process.await {
        if ignore_errors {
            eprintln!("Warning: Failed to process {:?}: {}", path, e);
        } else {
            let _ = content_tx
                .send(Err(anyhow::anyhow!("Failed to process {:?}: {}", path, e)))
                .await;
        }
    }
}
