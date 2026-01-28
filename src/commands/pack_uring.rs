#[cfg(target_os = "linux")]
use crate::commands::pack::{LARGE_FILE_THRESHOLD, TarEntry};
#[cfg(target_os = "linux")]
use crate::utils::{FileId, get_file_id, get_mode};
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
use tokio::sync::Semaphore;

#[cfg(target_os = "linux")]
pub fn start_uring_worker(
    path_rx: Receiver<PathBuf>,
    content_tx: Sender<Result<TarEntry>>,
    pool_rx: Receiver<Vec<u8>>,
    input_dir: PathBuf,
    pb: Arc<ProgressBar>,
    inode_cache: Arc<DashMap<FileId, PathBuf>>,
    ignore_errors: bool,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        tokio_uring::start(async move {
            let semaphore = Arc::new(Semaphore::new(128)); // Dispatch up to 128 IOs

            // We need to bridge Sync Channel -> Async Stream or Loop
            // Using spawn_blocking for the recv() is the standard way to not block the Runtime
            // However, tokio-uring is single-threaded and doesn't have spawn_blocking in the same way regular tokio does?
            // Wait, tokio-uring creates a runtime. usage of tokio::task::spawn_blocking is valid if it's available.
            // tokio-uring implies using the uring runtime.

            loop {
                // Acquire permit first to limit concurrency
                let permit = match semaphore.clone().acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => break, // Closed
                };

                let rx = path_rx.clone();
                // We use standard tokio spawn_blocking if available or we just block?
                // tokio-uring is a runtime.
                // NOTE: Using standard tokio functions might require the standard tokio reactor references which might not be there?
                // Correction: tokio-uring allows standard tokio types but uses its own driver.
                // Use tokio::task::spawn_blocking safely.

                let path_res = tokio::task::spawn_blocking(move || rx.recv()).await;

                match path_res {
                    Ok(Ok(path)) => {
                        // Got path, spawn local task for IO
                        let content_tx = content_tx.clone();
                        let pool_rx = pool_rx.clone();
                        let base_path = input_dir.clone();
                        let p_bar = pb.clone();
                        let i_cache = inode_cache.clone();

                        tokio::task::spawn_local(async move {
                            let _permit = permit; // Hold until done

                            process_path_uring(
                                path,
                                base_path,
                                content_tx,
                                pool_rx,
                                p_bar,
                                i_cache,
                                ignore_errors,
                            )
                            .await;
                        });
                    }
                    Ok(Err(_)) => break, // Channel closed (Done)
                    Err(_) => break,     // Join Error
                }
            }
        });
    })
}

#[cfg(target_os = "linux")]
async fn process_path_uring(
    path: PathBuf,
    base_path: PathBuf,
    content_tx: Sender<Result<TarEntry>>,
    pool_rx: Receiver<Vec<u8>>,
    pb: Arc<ProgressBar>,
    inode_cache: Arc<DashMap<FileId, PathBuf>>,
    ignore_errors: bool,
) {
    let process = async {
        let parent = base_path.parent().unwrap_or(&base_path);
        let relative_path = match path.strip_prefix(parent) {
            Ok(p) => p.to_path_buf(),
            Err(_) => path.clone(),
        };

        // We still use blocking fs::symlink_metadata because tokio-uring doesn't have metadata yet?
        // tokio-uring is strictly for Ring IO (Read/Write). Metadata is usually fast enough or we use tokio::fs which is threadpool.
        // For strict Uring purity we would use `statx` via uring, but tokio-uring doesn't expose it easily.
        // We will use standard threadpool blocking for metadata to be safe and compatible.
        // It's technically NOT uring, but the heavy lifting (Reading Content) IS uring.

        // Blocking Metadata
        let (meta, mode, file_type) = match tokio::task::spawn_blocking({
            let p = path.clone();
            move || -> Result<(std::fs::Metadata, u32, std::fs::FileType)> {
                let m = std::fs::symlink_metadata(&p)?;
                let mo = get_mode(&m);
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
            content_tx.send(Ok(TarEntry::Dir(relative_path.clone(), mode)))?;
        } else if file_type.is_symlink() {
            // Read link is also metadata-ish
            let target = tokio::fs::read_link(&path).await?;
            content_tx.send(Ok(TarEntry::Symlink(relative_path.clone(), target, mode)))?;
        } else {
            // Check Hardlinks (CPU/Memory op)
            if let Some(fid) = get_file_id(&meta) {
                if let Some(existing_entry) = inode_cache.get(&fid) {
                    let target = existing_entry.value().clone();
                    content_tx.send(Ok(TarEntry::HardLink(relative_path.clone(), target)))?;
                    pb.inc(1);
                    pb.set_message(format!("{:?}", relative_path));
                    return Ok(());
                } else {
                    inode_cache.insert(fid, relative_path.clone());
                }
            }

            let len = meta.len();
            if len > LARGE_FILE_THRESHOLD {
                content_tx.send(Ok(TarEntry::LargeFile(
                    relative_path.clone(),
                    len,
                    path.clone(),
                    mode,
                )))?;
            } else {
                // Small File: Read using IO URING
                let mut buf = pool_rx
                    .try_recv()
                    .unwrap_or_else(|_| Vec::with_capacity(len as usize));
                if buf.capacity() < len as usize {
                    buf.reserve(len as usize - buf.capacity());
                }
                // Resize to len effectively for reading? No, we need a buffer.
                // tokio-uring uses owned buffers.
                // We need to pass the buffer to the operation.

                // Open file using tokio_uring
                let file = tokio_uring::fs::File::open(&path).await?;

                // Read
                // tokio_uring::fs::File::read_at takes (buf, pos).
                // It returns (res, buf).
                if buf.len() < len as usize {
                    buf.resize(len as usize, 0);
                }

                let (res, buf_ret) = file.read_at(buf, 0).await;
                let mut valid_buf = buf_ret;
                res?; // check error

                // The buffer might be larger than len if reused.
                // Should truncate to actual read size?
                // read_at returns bytes read.
                if valid_buf.len() > len as usize {
                    valid_buf.truncate(len as usize);
                }

                content_tx.send(Ok(TarEntry::SmallFile(
                    relative_path.clone(),
                    valid_buf,
                    mode,
                )))?;
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
            let _ = content_tx.send(Err(anyhow::anyhow!("Failed to process {:?}: {}", path, e)));
        }
    }
}
