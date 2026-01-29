use std::fs;
use std::path::Path;
pub mod kernel_version;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId {
    #[cfg(unix)]
    pub dev: u64,
    #[cfg(unix)]
    pub ino: u64,
    #[cfg(windows)]
    pub volume_serial_number: u32,
    #[cfg(windows)]
    pub file_index: u64,
}

pub fn get_file_id(path: &Path, meta: &fs::Metadata) -> Option<FileId> {
    #[cfg(unix)]
    {
        let _ = path; // Unused on Unix
        Some(FileId {
            dev: meta.dev(),
            ino: meta.ino(),
        })
    }
    #[cfg(windows)]
    {
        let _ = meta; // Unused on Windows
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };

        // Open file to get handle
        let file = match fs::File::open(path) {
            Ok(f) => f,
            Err(_) => return None,
        };

        let handle = file.as_raw_handle() as HANDLE;
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };

        let result = unsafe { GetFileInformationByHandle(handle, &mut info) };

        if result == 0 {
            return None;
        }

        // Combine nFileIndexHigh and nFileIndexLow into a u64
        let file_index = ((info.nFileIndexHigh as u64) << 32) | (info.nFileIndexLow as u64);

        Some(FileId {
            volume_serial_number: info.dwVolumeSerialNumber,
            file_index,
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, meta);
        None
    }
}

pub fn get_mode(meta: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        meta.permissions().mode()
    }
    #[cfg(not(unix))]
    {
        if meta.is_dir() {
            0o755
        } else {
            if meta.permissions().readonly() {
                0o444
            } else {
                0o644
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FileMetadata {
    pub mode: u32,
    pub mtime: u64,
    pub uid: u64,
    pub gid: u64,
}

pub fn get_file_metadata(path: &Path, meta: &fs::Metadata) -> FileMetadata {
    let mode = get_mode(meta);

    // mtime
    let mtime = meta
        .modified()
        .unwrap_or_else(|_| std::time::SystemTime::now())
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    #[cfg(unix)]
    {
        let _ = path;
        FileMetadata {
            mode,
            mtime,
            uid: meta.uid() as u64,
            gid: meta.gid() as u64,
        }
    }

    #[cfg(windows)]
    {
        let _ = path;
        // On Windows, strict validation isn't as critical for tar compatibility usually,
        // but we default to root (0) to avoid issues.
        FileMetadata {
            mode,
            mtime,
            uid: 0,
            gid: 0,
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        FileMetadata {
            mode,
            mtime,
            uid: 0,
            gid: 0,
        }
    }
}
