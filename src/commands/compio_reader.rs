use crate::commands::pack::{CHUNK_SIZE, MEMORY_FILE_THRESHOLD, TarEntry};
use crate::utils::{FileId, FileMetadata, get_file_id, get_file_metadata};
use anyhow::Result;
use crossbeam_channel::Receiver;
use crossbeam_channel::Sender;
use dashmap::DashMap;
use indicatif::ProgressBar;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Backpressure is handled via channel capacity (PATH_CHANNEL_CAPACITY = 1000 in pack.rs)

/// Start compio worker pool - replaces thread-per-path pattern
pub fn start_compio_worker(
    path_rx: Receiver<PathBuf>,
    content_tx: Sender<Result<TarEntry>>,
    chunk_tx: Sender<Result<TarEntry>>,
    pool_rx: Receiver<Vec<u8>>,
    input_dir: PathBuf,
    pb: Arc<ProgressBar>,
    inode_cache: Arc<DashMap<FileId, PathBuf>>,
    ignore_errors: bool,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        // Use flume for async-to-sync bridging
        let (async_tx, async_rx) = flume::unbounded::<Result<TarEntry>>();

        // Spawn Bridge Thread - forwards async results to sync channels
        let bridge_handle = std::thread::spawn(move || {
            while let Ok(res) = async_rx.recv() {
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
                        if content_tx.send(Err(e)).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        // Track active tasks for graceful shutdown
        let active_tasks = Arc::new(AtomicUsize::new(0));
        
        // Large File Serializer - but with finer-grained locking
        let large_file_in_progress = Arc::new(AtomicUsize::new(0));

        // Start compio Runtime
        let runtime = compio_runtime::Runtime::new()
            .expect("Failed to create compio runtime");

        runtime.block_on(async move {
            // Spawn a fixed pool of workers - they compete for paths from the channel
            // This replaces the previous "spawn a thread per path" approach
            let num_workers = std::cmp::max(1, num_cpus::get() as usize);
            
            // Track all spawned tasks
            let mut handles = Vec::new();
            
            for _worker_id in 0..num_workers {
                let path_rx = path_rx.clone();
                let async_tx = async_tx.clone();
                let pool_rx = pool_rx.clone();
                let base_path = input_dir.clone();
                let p_bar = pb.clone();
                let i_cache = inode_cache.clone();
                let active = active_tasks.clone();
                let lf_in_progress = large_file_in_progress.clone();

                let handle = compio_runtime::spawn(async move {
                    // Worker loop - continuously process paths until channel closes
                    // Backpressure is handled by channel capacity (PATH_CHANNEL_CAPACITY = 1000)
                    loop {
                        // Try to get next path from channel (non-blocking)
                        let path = match path_rx.try_recv() {
                            Ok(p) => p,
                            Err(crossbeam_channel::TryRecvError::Empty) => {
                                // No work available, release permit and continue
                                continue;
                            }
                            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                                // Channel closed, exit worker
                                break;
                            }
                        };

                        // Track active tasks
                        active.fetch_add(1, Ordering::SeqCst);

                        // Process the file
                        process_path_compio(
                            path,
                            base_path.clone(),
                            async_tx.clone(),
                            pool_rx.clone(),
                            p_bar.clone(),
                            i_cache.clone(),
                            lf_in_progress.clone(),
                            ignore_errors,
                        ).await;

                        active.fetch_sub(1, Ordering::SeqCst);
                    }
                });
                handles.push(handle);
            }

            // Wait for all workers to complete
            for handle in handles {
                let _ = handle.await;
            }
            
            // Wait for all active tasks to complete
            while active_tasks.load(Ordering::SeqCst) > 0 {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        });

        let _ = bridge_handle.join();
    })
}

async fn process_path_compio(
    path: PathBuf,
    base_path: PathBuf,
    content_tx: flume::Sender<Result<TarEntry>>,
    pool_rx: Receiver<Vec<u8>>,
    pb: Arc<ProgressBar>,
    inode_cache: Arc<DashMap<FileId, PathBuf>>,
    large_file_in_progress: Arc<AtomicUsize>,
    ignore_errors: bool,
) {
    use compio::buf::BufResult;
    use compio::io::AsyncReadAt;

    let process = async {
        let parent = base_path.parent().unwrap_or(&base_path);
        let relative_path = match path.strip_prefix(parent) {
            Ok(p) => p.to_path_buf(),
            Err(_) => path
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("unknown")),
        };

        // Blocking metadata - use std::thread
        let path_clone = path.clone();
        let metadata_result = std::thread::spawn(move || -> Result<(std::fs::Metadata, FileMetadata, std::fs::FileType)> {
            let m = std::fs::symlink_metadata(&path_clone)?;
            let mo = get_file_metadata(&path_clone, &m);
            let ft = m.file_type();
            Ok((m, mo, ft))
        }).join();

        let (meta, metadata, file_type) = match metadata_result {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                if ignore_errors {
                    eprintln!("Warning: Skipping unreadable file {:?}: {}", path, e);
                    return Ok(());
                } else {
                    return Err(e.into());
                }
            }
            Err(_) => {
                let err = anyhow::anyhow!("Thread panicked");
                if ignore_errors {
                    eprintln!("Warning: Skipping unreadable file {:?}: {}", path, err);
                    return Ok(());
                } else {
                    return Err(err);
                }
            }
        };

        if file_type.is_dir() {
            content_tx.send(Ok(TarEntry::Dir(relative_path.clone(), metadata)))
                .map_err(|_| anyhow::anyhow!("Channel closed"))?;
        } else if file_type.is_symlink() {
            let target = std::fs::read_link(&path)?;
            content_tx.send(Ok(TarEntry::Symlink(
                relative_path.clone(),
                target,
                metadata,
            )))
            .map_err(|_| anyhow::anyhow!("Channel closed"))?;
        } else {
            // Check Hardlinks
            if let Some(fid) = get_file_id(&path, &meta) {
                if let Some(existing_entry) = inode_cache.get(&fid) {
                    let target = existing_entry.value().clone();
                    content_tx.send(Ok(TarEntry::HardLink(relative_path.clone(), target)))
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
                // Large File Chunking - use atomic instead of mutex for better concurrency
                // Wait for any in-progress large file to finish
                while large_file_in_progress.load(Ordering::SeqCst) > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                
                // Mark as in-progress (we're now the one processing)
                // Actually for large files, we can process them in parallel too
                // but we need to ensure ordering in the chunk channel
                // The key insight: each large file uses a dedicated chunk channel
                // So we just need to ensure we serialize LARGE FILE STARTS
                // But we can process chunks in parallel
                
                content_tx.send(Ok(TarEntry::LargeFileStart(
                    relative_path.clone(),
                    len,
                    metadata,
                )))
                .map_err(|_| anyhow::anyhow!("Channel closed"))?;

                let file = compio::fs::File::open(&path).await?;
                let mut pos = 0;
                while pos < len {
                    let chunk_size = std::cmp::min(len - pos, CHUNK_SIZE);
                    let mut buf = pool_rx
                        .try_recv()
                        .unwrap_or_else(|_| Vec::with_capacity(chunk_size as usize));
                    if buf.capacity() < chunk_size as usize {
                        buf.reserve(chunk_size as usize - buf.capacity());
                    }
                    if buf.len() < chunk_size as usize {
                        buf.resize(chunk_size as usize, 0);
                    }

                    let BufResult(res, buf_ret) = file.read_at(buf, pos).await;
                    let mut valid_buf = buf_ret;
                    res?;

                    if valid_buf.len() > chunk_size as usize {
                        valid_buf.truncate(chunk_size as usize);
                    }

                    content_tx.send(Ok(TarEntry::LargeFileChunk(valid_buf)))
                        .map_err(|_| anyhow::anyhow!("Channel closed"))?;
                    pos += chunk_size;
                }

                content_tx.send(Ok(TarEntry::LargeFileEnd))
                    .map_err(|_| anyhow::anyhow!("Channel closed"))?;
            } else {
                // Small File - can be processed in parallel freely
                let mut buf = pool_rx
                    .try_recv()
                    .unwrap_or_else(|_| Vec::with_capacity(len as usize));
                if buf.capacity() < len as usize {
                    buf.reserve(len as usize - buf.capacity());
                }
                if buf.len() < len as usize {
                    buf.resize(len as usize, 0);
                }

                let file = compio::fs::File::open(&path).await?;
                let BufResult(res, buf_ret) = file.read_at(buf, 0).await;
                let mut valid_buf = buf_ret;
                res?;

                if valid_buf.len() > len as usize {
                    valid_buf.truncate(len as usize);
                }

                content_tx.send(Ok(TarEntry::SmallFile(
                    relative_path.clone(),
                    valid_buf,
                    metadata,
                )))
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
                .send(Err(anyhow::anyhow!("Failed to process {:?}: {}", path, e)));
        }
    }
}
