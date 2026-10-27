//! File identity for hardlink deduplication.
//!
//! Returns `Some((volume, index))` only for files with more than one link.
//! Single-link files never need tracking, so the common case stays cheap.

use std::fs::Metadata;
use std::path::Path;

#[cfg(unix)]
pub fn hardlink_identity(path: &Path, md: &Metadata) -> Option<(u64, u64)> {
    let _ = path;
    use std::os::unix::fs::MetadataExt;
    if md.nlink() > 1 {
        Some((md.dev(), md.ino()))
    } else {
        None
    }
}

/// Bytes this file takes on disk. A cloud placeholder (OneDrive Files On-Demand) takes none,
/// and a sparse or compressed file takes its allocated size. On Windows this is the allocated
/// size from `GetCompressedFileSizeW`. Elsewhere it is the logical length, for now (see todo.md,
/// Phase 8, "Sizes are logical").
#[cfg(windows)]
pub fn disk_bytes(path: &Path, md: &Metadata) -> u64 {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetCompressedFileSizeW;

    let wide: Vec<u16> = crate::safety::long_path(path)
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut high: u32 = 0;
    // SAFETY: `wide` is a valid, NUL-terminated UTF-16 string that outlives the call, and
    // `high` is a valid out-pointer for the high 32 bits.
    let low = unsafe { GetCompressedFileSizeW(wide.as_ptr(), &mut high) };
    // u32::MAX is the failure value. It is only an error when GetLastError is set too.
    if low == u32::MAX && std::io::Error::last_os_error().raw_os_error().unwrap_or(0) != 0 {
        return md.len();
    }
    ((high as u64) << 32) | low as u64
}

#[cfg(not(windows))]
pub fn disk_bytes(_path: &Path, md: &Metadata) -> u64 {
    md.len()
}

#[cfg(windows)]
pub fn hardlink_identity(path: &Path, _md: &Metadata) -> Option<(u64, u64)> {
    // Stable std does not expose the link count or file index on Windows,
    // so the file has to be opened to ask the kernel. Access is zero, which
    // needs no read permission and does not lock the file.
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_NORMAL,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let wide: Vec<u16> = crate::safety::long_path(path)
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: `wide` is a valid, NUL-terminated UTF-16 string that outlives the call.
    // The handle is checked against INVALID_HANDLE_VALUE and closed on every path below.
    unsafe {
        let handle = CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        let ok = GetFileInformationByHandle(handle, &mut info);
        CloseHandle(handle);
        if ok == 0 || info.nNumberOfLinks <= 1 {
            return None;
        }
        let index = ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64;
        Some((info.dwVolumeSerialNumber as u64, index))
    }
}
