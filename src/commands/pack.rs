use anyhow::{Context, Result};
use crossbeam_channel::{bounded, unbounded};
use dashmap::DashMap;
use indicatif::{ProgressBar, ProgressStyle};
use jwalk::WalkDir;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::utils::{FileId, FileMetadata};

// ============== Constants ==============
/// Chunk size for large file streaming (4MB)
pub const CHUNK_SIZE: u64 = 4 * 1024 * 1024;

/// Files larger than this threshold are streamed in chunks (128MB)
pub const MEMORY_FILE_THRESHOLD: u64 = 128 * 1024 * 1024;

/// Channel capacity for scanner -> reader (path distribution)
pub const PATH_CHANNEL_CAPACITY: usize = 1000;

/// Channel capacity for reader -> writer (metadata and small files)
pub const CONTENT_CHANNEL_CAPACITY: usize = 100;

/// Channel capacity for large file chunks (dedicated)
pub const CHUNK_CHANNEL_CAPACITY: usize = 100;

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

// ChannelReader for streaming large files from dedicated chunk channel
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
                let b = std::mem::take(&mut self.buffer);
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
            Ok(Err(e)) => Err(std::io::Error::other(e)),
            Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Chunk Channel closed unexpectedly",
            )),
        }
    }
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
    let (path_tx, path_rx) = bounded::<PathBuf>(PATH_CHANNEL_CAPACITY);
    // Readers -> Writer (Metadata & Small Files)
    let (content_tx, content_rx) = bounded::<Result<TarEntry>>(CONTENT_CHANNEL_CAPACITY);
    // Large File Data Channel (Dedicated to prevent interleaving)
    let (chunk_tx, chunk_rx) = bounded::<Result<TarEntry>>(CHUNK_CHANNEL_CAPACITY);

    // Buffer Pool - Unbounded to prevent deadlocks.
    let (pool_tx, pool_rx) = unbounded::<Vec<u8>>();

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

    // 5. Start Reader Threads - Using Compio for unified cross-platform async I/O
    let mut reader_handles = Vec::new();

    // Use compio - unified API that automatically selects:
    // - io_uring on Linux
    // - IOCP on Windows
    // - Polling on other Unix systems (macOS)
    reader_handles.push(crate::commands::compio_reader::start_compio_worker(
        path_rx,
        content_tx.clone(),
        chunk_tx.clone(),
        pool_rx,
        input_dir.clone(),
        pb.clone(),
        inode_cache,
        options.ignore_errors,
    ));

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
