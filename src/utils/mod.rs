use std::fs;
pub mod kernel_version;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId {
    #[cfg(unix)]
    pub dev: u64,
    #[cfg(unix)]
    pub ino: u64,
    #[cfg(windows)]
    pub volume_serial_number: Option<u32>,
    #[cfg(windows)]
    pub file_index: Option<u64>,
}

pub fn get_file_id(meta: &fs::Metadata) -> Option<FileId> {
    #[cfg(unix)]
    {
        Some(FileId {
            dev: meta.dev(),
            ino: meta.ino(),
        })
    }
    #[cfg(windows)]
    {
        Some(FileId {
            volume_serial_number: meta.volume_serial_number(),
            file_index: meta.file_index(),
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
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
