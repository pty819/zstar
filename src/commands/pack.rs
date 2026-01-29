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

pub const LARGE_FILE_THRESHOLD: u64 = 1024 * 1024; // 1MB

pub enum TarEntry {
    SmallFile(PathBuf, Vec<u8>, FileMetadata),
    LargeFile(PathBuf, u64, PathBuf, FileMetadata),
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
    // Readers -> Writer
    let (content_tx, content_rx) = bounded::<Result<TarEntry>>(100);
    // Buffer Pool - Unbounded to prevent deadlocks.
    // Readers will try to pop from here, if empty they allocate new.
    // Writer will push back used buffers here.
    let (pool_tx, pool_rx) = unbounded::<Vec<u8>>();

    // 4. Start Scanner Thread
    // Canonicalize input path to ensure consistent absolute paths
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
            let base_path = input_dir.clone();
            let pool_rx = pool_rx.clone();
            let pb = pb.clone();
            let inode_cache = inode_cache.clone();
            let ignore_errors = options.ignore_errors;

            reader_handles.push(thread::spawn(move || {
                for path in path_rx {
                    // Normalize entry path to match base_path (WalkDir usually returns abs path if input is abs)
                    // But to be safe, we're relying on WalkDir returning paths consistent with input.
                    // Since we canonicalized input, WalkDir might still return raw paths from readdir?
                    // No, jwalk/WalkDir usually joins with root.
                    // Let's canonicalize the entry path too just to be safe?
                    // No, canonicalize involves syscalls and resolving symlinks which we might NOT want to follow (if we want to archive the link itself).
                    // Wait, we WANT to archive the symlink itself, not target.
                    // So we should NOT canonicalize `path` here if it resolves symlinks!
                    // Correct approach: Use strict prefix stripping.

                    let relative_path = match path.strip_prefix(&base_path) {
                        Ok(p) => p.to_path_buf(),
                        Err(_) => {
                            // Fallback: This happens if path is not cleaner or canonicalized same way.
                            // If base is /a/b and path is /a/b/../c (unlikely from walker but possible), strip fails.
                            // We construct relative by simple filename if all else fails, or error out?
                            // Safe default: use path file name
                            path.file_name()
                                .map(PathBuf::from)
                                .unwrap_or_else(|| PathBuf::from("unknown"))
                        }
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
                            // Regular file - check Hardlinks
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

                            if len > LARGE_FILE_THRESHOLD {
                                content_tx.send(Ok(TarEntry::LargeFile(
                                    relative_path.clone(),
                                    len,
                                    path.clone(),
                                    metadata,
                                )))?;
                            } else {
                                let mut buf = pool_rx
                                    .try_recv()
                                    .unwrap_or_else(|_| Vec::with_capacity(len as usize));
                                buf.clear();

                                let f = File::open(&path)?;
                                f.take(len).read_to_end(&mut buf)?;

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

    // 6. Writer Current Thread
    for entry in content_rx {
        let entry = entry?;
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
            TarEntry::LargeFile(path, len, abs_path, metadata) => {
                let mut header = tar::Header::new_gnu();
                header.set_size(len);
                header.set_mode(metadata.mode);
                header.set_uid(metadata.uid);
                header.set_gid(metadata.gid);
                header.set_mtime(metadata.mtime);
                header.set_cksum();
                let mut f = File::open(abs_path)?;
                tar.append_data(&mut header, &path, &mut f)?;
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
