use anyhow::{Context, Result};
use crossbeam_channel::Receiver;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime};
use tar::Archive;

const LARGE_FILE_THRESHOLD: u64 = 10 * 1024 * 1024; // 10MB

enum UnpackTask {
    File {
        path: PathBuf,
        data: Vec<u8>,
        mode: u32,
        mtime: u64,
    },
}

struct DirMetadata {
    path: PathBuf,
    mode: u32,
    mtime: u64,
}

struct SymlinkTask {
    path: PathBuf,
    target: PathBuf,
    #[allow(dead_code)] // mtime for symlinks is hard to set portably
    mtime: u64,
}

pub fn execute(input: &Path, output: &Path, threads: u32) -> Result<()> {
    let file = File::open(input).context("Failed to open input file")?;
    let decoder = zstd::Decoder::new(file)?;
    let mut archive = Archive::new(decoder);

    // Bounded channel to prevent reading the whole archive into memory
    let (tx, rx) = crossbeam_channel::bounded::<UnpackTask>(threads as usize * 16);
    let rx = Arc::new(rx);

    let mut handles = vec![];

    // Spawn workers
    for _ in 0..threads {
        let rx_worker = rx.clone();
        handles.push(thread::spawn(move || worker_loop(rx_worker)));
    }

    // Deferred tasks
    let mut dirs_metadata = Vec::new();
    let mut symlinks = Vec::new();
    let mut hardlinks = Vec::new();

    // Iterate entries
    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_path = entry.path()?.to_path_buf();
        let target_path = output.join(&entry_path);

        let header = entry.header();
        let entry_type = header.entry_type();
        let size = header.size()?;
        let mode = header.mode()?;
        let mtime = header.mtime()?;

        // Basic path sanitization check (tar-rs usually handles this)
        if target_path.strip_prefix(output).is_err() {
            eprintln!("Skipping unsafe path: {:?}", entry_path);
            continue;
        }

        match entry_type {
            tar::EntryType::Directory => {
                // Determine actual disk path (ensure it exists now so files can be written)
                fs::create_dir_all(&target_path)?;
                dirs_metadata.push(DirMetadata {
                    path: target_path,
                    mode,
                    mtime,
                });
            }
            tar::EntryType::Link => {
                if let Some(target) = entry.link_name()? {
                    // Hardlinks must be created at the end to ensure targets exist
                    hardlinks.push((target_path, output.join(target)));
                }
            }
            tar::EntryType::Symlink => {
                if let Some(target) = entry.link_name()? {
                    symlinks.push(SymlinkTask {
                        path: target_path,
                        target: target.to_path_buf(),
                        mtime,
                    });
                }
            }
            _ => {
                // Regular file (or contiguous, etc.)
                if size > LARGE_FILE_THRESHOLD {
                    // Process large files immediately in main thread to save memory
                    // We use entry.unpack_in which handles reading and writing
                    // Note: This relies on tar-rs internal logic, which is fine
                    entry.unpack_in(output)?;
                } else {
                    // Small file: buffer and send to worker
                    let mut data = Vec::with_capacity(size as usize);
                    entry.read_to_end(&mut data)?;

                    tx.send(UnpackTask::File {
                        path: target_path,
                        data,
                        mode,
                        mtime,
                    })
                    .context("Failed to send task to worker")?;
                }
            }
        }
    }

    // Drop sender to signal workers to finish
    drop(tx);

    // Wait for file workers
    for handle in handles {
        handle.join().unwrap()?;
    }

    // --- Post Processing ---

    // 1. Create Symlinks
    for link in symlinks {
        if let Some(parent) = link.path.parent() {
            fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&link.target, &link.path).or_else(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    Ok(())
                } else {
                    Err(e)
                }
            })?;
        }
        #[cfg(windows)]
        {
            // Windows symlinks are tricky. We try file symlink first.
            // Note: This requires Developer Mode or Admin usually.
            std::os::windows::fs::symlink_file(&link.target, &link.path)
                .or_else(|_| std::os::windows::fs::symlink_dir(&link.target, &link.path))
                .ok(); // Ignore failure for now to avoid crashing on non-admin Windows
        }
    }

    // 2. Create Hardlinks (Targets should exist now)
    for (path, target) in hardlinks {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        // Remove existing file if any (tar overwrite behavior)
        if path.exists() {
            fs::remove_file(&path).ok();
        }
        fs::hard_link(&target, &path).with_context(|| {
            format!("Failed to create hardlink from {:?} to {:?}", target, path)
        })?;
    }

    // 3. Restore Directory Metadata (Deepest first to avoid modifying parent mtimes by accident)
    // Sort by number of components to ensure we do children before parents
    dirs_metadata.sort_by(|a, b| {
        b.path
            .components()
            .count()
            .cmp(&a.path.components().count())
    });

    for dir in dirs_metadata {
        set_permissions_and_times(&dir.path, dir.mode, dir.mtime).ok();
        // Ignore errors for dirs (e.g. if removed or permission issues)
    }

    Ok(())
}

fn worker_loop(rx: Arc<Receiver<UnpackTask>>) -> Result<()> {
    let mut created_dirs = std::collections::HashSet::new();
    while let Ok(task) = rx.recv() {
        match task {
            UnpackTask::File {
                path,
                data,
                mode,
                mtime,
            } => {
                if let Some(parent) = path.parent()
                    && !created_dirs.contains(parent) {
                        fs::create_dir_all(parent)?;
                        created_dirs.insert(parent.to_path_buf());
                    }

                {
                    let mut file = File::create(&path)?;
                    file.write_all(&data)?;
                } // File closed here

                set_permissions_and_times(&path, mode, mtime)?;
            }
        }
    }
    Ok(())
}

fn set_permissions_and_times(path: &Path, mode: u32, mtime: u64) -> Result<()> {
    // 1. Permissions
    let mut perms = fs::metadata(path)?.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(mode);
    }
    // On Windows, mode is limited (read-only or not). We can try mapping basic bits.
    #[cfg(windows)]
    {
        // Simple mapping: if write bit is missing, set readonly
        let readonly = mode & 0o222 == 0;
        perms.set_readonly(readonly);
    }
    fs::set_permissions(path, perms)?;

    // 2. Times (mtime)
    let mtime_system = SystemTime::UNIX_EPOCH + Duration::from_secs(mtime);
    let file = File::open(path)?;
    file.set_modified(mtime_system)?;

    Ok(())
}
