#[cfg(target_os = "linux")]
use anyhow::{Context, Result};
#[cfg(target_os = "linux")]
use std::ffi::CStr;
#[cfg(target_os = "linux")]
use std::mem;

#[cfg(target_os = "linux")]
pub fn is_io_uring_supported() -> bool {
    // Check if kernel version >= 6.0
    match get_kernel_version() {
        Ok((major, _)) => major >= 6,
        Err(_) => false,
    }
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
pub fn is_io_uring_supported() -> bool {
    false
}

#[cfg(target_os = "linux")]
fn get_kernel_version() -> Result<(u32, u32)> {
    let mut uname_data: libc::utsname = unsafe { mem::zeroed() };
    let res = unsafe { libc::uname(&mut uname_data) };
    if res < 0 {
        return Err(anyhow::anyhow!("Failed to call uname"));
    }

    let release = unsafe { CStr::from_ptr(uname_data.release.as_ptr()) }.to_string_lossy();
    // Parse "6.5.0-..."
    let parts: Vec<&str> = release.split('.').collect();
    if parts.len() < 2 {
        return Err(anyhow::anyhow!("Invalid release format"));
    }

    let major: u32 = parts[0].parse().context("Failed to parse major version")?;
    let minor: u32 = parts[1].parse().unwrap_or(0); // Optional parsing checking

    Ok((major, minor))
}
