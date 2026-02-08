use anyhow::{Context, Result};
use crossbeam_channel::{bounded, unbounded};
use dashmap::DashMap;
use indicatif::{ProgressBar, ProgressStyle};
use jwalk::WalkDir;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::utils::{FileId, FileMetadata, get_file_id, get_file_metadata};

pub const CHUNK_SIZE: u64 = 4 * 1024 * 1024; // 4MB
pub const MEMORY_FILE_THRESHOLD: u64 = 128 * 1024 * 1024; // 128MB

pub enum TarEntry {
    SmallFile(PathBuf, Vec<u8>, FileMetadata),
    LargeFileStart(PathBuf, u64 /* total_size */, FileMetadata),
    LargeFileChunk(Vec<u8>),
    LargeFileEnd,
    Symlink(PathBuf, PathBuf, FileMetadata),
    HardLink(PathBuf, PathBuf),
    Dir(PathBuf, FileMetadata),
}

pub struct PackOptions {
    pub level: i32,
    pub threads: u32,
    pub long_distance: bool,
    pub ignore_errors: bool,
}

pub fn execute(input: &Path, output: &Path, options: PackOptions) -> Result<()> {
    // 1. Setup Zstd Encoder
    let file = File::create(output).context("Failed to create output file")?;
    let mut encoder = zstd::Encoder::new(file, options.level)?;
    encoder.multithread(options.threads)?;
    let _ = encoder.long_distance_matching(options.long_distance);
    let encoder = encoder.auto_finish();

    let mut tar = tar::Builder::new(encoder);

    // 2. Setup Progress Bar & Caches
    let pb = Arc::new(ProgressBar::new_spinner());
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] {pos} files processed ({msg})",
        )?
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
    );
    pb.enable_steady_tick(Duration::from_millis(100));

    let inode_cache = Arc::new(DashMap::<FileId, PathBuf>::new());

    // 3. Setup Channels
    // Scanner -> Readers
    let (path_tx, path_rx) = bounded::<PathBuf>(1000);
    // Readers -> Writer (Metadata & Small Files)
    let (content_tx, content_rx) = bounded::<Result<TarEntry>>(100);
    // Large File Data Channel (Dedicated to prevent interleaving)
    let (chunk_tx, chunk_rx) = bounded::<Result<TarEntry>>(100);

    // Buffer Pool - Unbounded to prevent deadlocks.
    let (pool_tx, pool_rx) = unbounded::<Vec<u8>>();

    // Global Mutex for Large File Serialization (Threaded Mode Only)
    let large_file_mutex = Arc::new(std::sync::Mutex::new(()));
    // For async uring, we need an async mutex. We will pass a separate one or let uring create its own?
    // PackUring needs to share global serialization if we mixed threaded and uring?
    // PackUring is exclusive with Threaded. So we can use separate mutexes.
    // We will let pack_uring create its own tokio Mutex inside start_uring_worker?
    // No, pack_uring::start_uring_worker is called once. The mutex must be shared among uring tasks.
    // So uring worker will create its own Arc<tokio::Mutex>.

    // 4. Start Scanner Thread
    let input_dir = input.canonicalize().unwrap_or_else(|_| input.to_path_buf());
    let input_dir_clone = input_dir.clone();
    let scanner_handle = thread::spawn(move || {
        for entry in WalkDir::new(&input_dir_clone).skip_hidden(false) {
            match entry {
                Ok(entry) => {
                    let path = entry.path();
                    if path == input_dir_clone {
                        continue;
                    }
                    if path_tx.send(path).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("Error walking directory: {}", e);
                }
            }
        }
    });

    // 5. Start Reader Threads
    let use_uring = {
        #[cfg(target_os = "linux")]
        {
            crate::utils::kernel_version::is_io_uring_supported()
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    };

    let mut reader_handles = Vec::new();

    if use_uring {
        #[cfg(target_os = "linux")]
        reader_handles.push(crate::commands::pack_uring::start_uring_worker(
            path_rx,
            content_tx.clone(),
            chunk_tx.clone(),
            pool_rx,
            input_dir.clone(),
            pb.clone(),
            inode_cache,
            options.ignore_errors,
        ));
    } else {
        let num_readers = num_cpus::get();
        for _ in 0..num_readers {
            let path_rx = path_rx.clone();
            let content_tx = content_tx.clone();
            let chunk_tx = chunk_tx.clone();
            let base_path = input_dir.clone();
            let pool_rx = pool_rx.clone();
            let pb = pb.clone();
            let inode_cache = inode_cache.clone();
            let ignore_errors = options.ignore_errors;
            let large_file_mutex = large_file_mutex.clone();

            reader_handles.push(thread::spawn(move || {
                for path in path_rx {
                    // Safe Relative Path Logic
                    let relative_path = match path.strip_prefix(&base_path) {
                        Ok(p) => p.to_path_buf(),
                        Err(_) => path
                            .file_name()
                            .map(PathBuf::from)
                            .unwrap_or_else(|| PathBuf::from("unknown")),
                    };

                    let process_entry = || -> Result<()> {
                        let meta = match fs::symlink_metadata(&path) {
                            Ok(m) => m,
                            Err(e) => {
                                if ignore_errors {
                                    eprintln!(
                                        "Warning: Skipping unreadable file {:?}: {}",
                                        path, e
                                    );
                                    return Ok(());
                                } else {
                                    return Err(e.into());
                                }
                            }
                        };

                        let metadata = get_file_metadata(&path, &meta);
                        let file_type = meta.file_type();

                        if file_type.is_dir() {
                            content_tx.send(Ok(TarEntry::Dir(relative_path.clone(), metadata)))?;
                        } else if file_type.is_symlink() {
                            let target = fs::read_link(&path)?;
                            content_tx.send(Ok(TarEntry::Symlink(
                                relative_path.clone(),
                                target,
                                metadata,
                            )))?;
                        } else {
                            if let Some(fid) = get_file_id(&path, &meta) {
                                let is_hardlink = {
                                    if let Some(existing_entry) = inode_cache.get(&fid) {
                                        let target = existing_entry.value().clone();
                                        content_tx.send(Ok(TarEntry::HardLink(
                                            relative_path.clone(),
                                            target,
                                        )))?;
                                        true
                                    } else {
                                        inode_cache.insert(fid, relative_path.clone());
                                        false
                                    }
                                };
                                if is_hardlink {
                                    pb.inc(1);
                                    pb.set_message(format!("{:?}", relative_path));
                                    return Ok(());
                                }
                            }

                            let len = meta.len();

                            if len >= MEMORY_FILE_THRESHOLD {
                                // Large File: Sequential Chunking with Lock
                                let _lock = large_file_mutex.lock().unwrap();

                                content_tx.send(Ok(TarEntry::LargeFileStart(
                                    relative_path.clone(),
                                    len,
                                    metadata,
                                )))?;

                                let mut f = File::open(&path)?;
                                let mut remain = len;
                                while remain > 0 {
                                    let chunk_size = std::cmp::min(remain, CHUNK_SIZE);
                                    let mut buf = pool_rx.try_recv().unwrap_or_else(|_| {
                                        Vec::with_capacity(chunk_size as usize)
                                    });
                                    if buf.capacity() < chunk_size as usize {
                                        buf.reserve(chunk_size as usize - buf.capacity());
                                    }
                                    unsafe {
                                        buf.set_len(chunk_size as usize);
                                    } // Unsafe set len? Or just clear and read?
                                    // Safety: read_exact/read usually fine. But take().read_to_end is safe.
                                    buf.clear();
                                    let mut chunk_reader = (&mut f).take(chunk_size);
                                    chunk_reader.read_to_end(&mut buf)?;

                                    chunk_tx.send(Ok(TarEntry::LargeFileChunk(buf)))?;
                                    remain -= chunk_size;
                                }

                                chunk_tx.send(Ok(TarEntry::LargeFileEnd))?;
                                // Lock released here
                            } else {
                                // Small File: Read All
                                let mut buf = pool_rx
                                    .try_recv()
                                    .unwrap_or_else(|_| Vec::with_capacity(len as usize));
                                buf.clear();

                                let mut f = File::open(&path)?;
                                f.read_to_end(&mut buf)?; // Read whole file

                                content_tx.send(Ok(TarEntry::SmallFile(
                                    relative_path.clone(),
                                    buf,
                                    metadata,
                                )))?;
                            }
                        }
                        pb.inc(1);
                        pb.set_message(format!("{:?}", relative_path));
                        Ok(())
                    };

                    if let Err(e) = process_entry() {
                        if ignore_errors {
                            eprintln!("Warning: Failed to process {:?}: {}", path, e);
                        } else {
                            let _ = content_tx.send(Err(anyhow::anyhow!(
                                "Failed to process {:?}: {}",
                                path,
                                e
                            )));
                        }
                    }
                }
            }));
        }
    }

    drop(content_tx);
    drop(chunk_tx); // Important: drop writer's sender handle so rx can close

    // 6. Writer Current Thread
    loop {
        let entry_result = content_rx.recv();
        if entry_result.is_err() {
            break; // Channel closed and empty
        }
        let entry = entry_result.unwrap()?;

        match entry {
            TarEntry::Dir(path, metadata) => {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Directory);
                header.set_mode(metadata.mode);
                header.set_uid(metadata.uid);
                header.set_gid(metadata.gid);
                header.set_mtime(metadata.mtime);
                header.set_size(0);
                header.set_cksum();
                tar.append_dir(&path, ".")?;
            }
            TarEntry::SmallFile(path, buf, metadata) => {
                let mut header = tar::Header::new_gnu();
                header.set_size(buf.len() as u64);
                header.set_mode(metadata.mode);
                header.set_uid(metadata.uid);
                header.set_gid(metadata.gid);
                header.set_mtime(metadata.mtime);
                header.set_cksum();
                tar.append_data(&mut header, &path, &buf[..])?;
                let _ = pool_tx.send(buf);
            }
            TarEntry::LargeFileStart(path, len, metadata) => {
                let mut header = tar::Header::new_gnu();
                header.set_size(len);
                header.set_mode(metadata.mode);
                header.set_uid(metadata.uid);
                header.set_gid(metadata.gid);
                header.set_mtime(metadata.mtime);
                header.set_cksum();

                // Construct Reader that pulls subsequent chunks from CHUNK_RX (Dedicated channel)
                struct ChannelReader<'a> {
                    rx: &'a crossbeam_channel::Receiver<Result<TarEntry>>,
                    buffer: Vec<u8>,
                    cursor: usize,
                    exhausted: bool,
                    total_read: u64,
                    expected: u64,
                    pool_tx: &'a crossbeam_channel::Sender<Vec<u8>>,
                }

                impl<'a> Read for ChannelReader<'a> {
                    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                        if self.exhausted {
                            return Ok(0);
                        }

                        // Serve from buffer
                        if self.cursor < self.buffer.len() {
                            let available = self.buffer.len() - self.cursor;
                            let to_read = std::cmp::min(available, out.len());
                            out[..to_read]
                                .copy_from_slice(&self.buffer[self.cursor..self.cursor + to_read]);
                            self.cursor += to_read;
                            self.total_read += to_read as u64;

                            if self.cursor == self.buffer.len() {
                                // Recycle buffer
                                let b = std::mem::replace(&mut self.buffer, Vec::new());
                                if b.capacity() > 0 {
                                    let _ = self.pool_tx.send(b);
                                }
                            }
                            return Ok(to_read);
                        }

                        // Need new chunk
                        match self.rx.recv() {
                            Ok(Ok(entry)) => match entry {
                                TarEntry::LargeFileChunk(buf) => {
                                    self.buffer = buf;
                                    self.cursor = 0;
                                    self.read(out) // Recurse to copy
                                }
                                TarEntry::LargeFileEnd => {
                                    self.exhausted = true;
                                    if self.total_read != self.expected {
                                        return Err(std::io::Error::new(
                                            std::io::ErrorKind::UnexpectedEof,
                                            format!(
                                                "Size mismatch: expected {}, got {}",
                                                self.expected, self.total_read
                                            ),
                                        ));
                                    }
                                    Ok(0)
                                }
                                _ => Err(std::io::Error::new(
                                    std::io::ErrorKind::InvalidData,
                                    "Unexpected entry type, expected chunk from dedicated channel",
                                )),
                            },
                            Ok(Err(e)) => Err(std::io::Error::new(std::io::ErrorKind::Other, e)),
                            Err(_) => Err(std::io::Error::new(
                                std::io::ErrorKind::BrokenPipe,
                                "Chunk Channel closed unexpectedly",
                            )),
                        }
                    }
                }

                let mut reader = ChannelReader {
                    rx: &chunk_rx, // Read from dedicated chunk channel
                    buffer: Vec::new(),
                    cursor: 0,
                    exhausted: false,
                    total_read: 0,
                    expected: len,
                    pool_tx: &pool_tx,
                };

                // If append_data returns error (e.g. read error), we should handle it.
                // But we are in a loop handling entries.
                tar.append_data(&mut header, &path, &mut reader)?;
            }
            TarEntry::LargeFileChunk(_) | TarEntry::LargeFileEnd => {
                // We should NEVER receive Chunk/End on content_rx!
                // This confirms separation works.
                anyhow::bail!("Protocol Error: chunk received on metadata channel");
            }
            TarEntry::Symlink(path, target, metadata) => {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                header.set_mode(metadata.mode);
                header.set_uid(metadata.uid);
                header.set_gid(metadata.gid);
                header.set_mtime(metadata.mtime);
                header.set_link_name(&target).unwrap_or(());
                header.set_cksum();
                tar.append_data(&mut header, &path, &mut std::io::empty())?;
            }
            TarEntry::HardLink(path, target) => {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Link);
                header.set_size(0);
                header.set_mode(0o644);
                header.set_link_name(&target).unwrap_or(());
                header.set_cksum();
                tar.append_data(&mut header, &path, &mut std::io::empty())?;
            }
        }
    }

    pb.finish_with_message("Done");
    scanner_handle.join().unwrap();
    for handle in reader_handles {
        handle.join().unwrap();
    }

    tar.finish().context("Failed to finish writing archive")?;

    Ok(())
}
