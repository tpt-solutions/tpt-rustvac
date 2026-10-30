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
///
/// On Windows an ordinary file costs no extra open. Its length is rounded up to the volume's
/// cluster size, which is what the allocation is for a plain file. Only a file whose
/// attributes say it may differ (see [`SIZE_NEEDS_PROBE`]) asks `GetCompressedFileSizeW`.
#[cfg(windows)]
pub fn disk_bytes(path: &Path, md: &Metadata) -> u64 {
    use std::os::windows::fs::MetadataExt;
    if md.file_attributes() & SIZE_NEEDS_PROBE == 0 {
        return round_to_cluster(md.len(), cluster_size(path));
    }
    probe_allocated(path, md)
}

/// Attributes that mean the allocated size can differ from the length: compressed (0x800),
/// sparse (0x200), reparse point (0x400, which includes OneDrive placeholders), offline (0x1000),
/// and recall on open or on data access (0x40000, 0x400000).
#[cfg(windows)]
const SIZE_NEEDS_PROBE: u32 = 0x800 | 0x200 | 0x400 | 0x1000 | 0x40000 | 0x400000;

/// The allocation `length` takes when rounded up to whole clusters. Zero stays zero.
pub fn round_to_cluster(length: u64, cluster: u64) -> u64 {
    if length == 0 || cluster == 0 {
        return length;
    }
    length.div_ceil(cluster) * cluster
}

/// The cluster size of the volume `path` is on, read once per volume. Falls back to 4 KiB.
#[cfg(windows)]
fn cluster_size(path: &Path) -> u64 {
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};

    static CACHE: OnceLock<Mutex<HashMap<OsString, u64>>> = OnceLock::new();
    let key = path
        .components()
        .next()
        .map(|c| c.as_os_str().to_os_string())
        .unwrap_or_default();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(size) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return *size;
    }
    let size = query_cluster_size(path).unwrap_or(4096);
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, size);
    size
}

#[cfg(windows)]
fn query_cluster_size(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceW;

    // The volume root, such as `C:\`. Any path on the volume gives the same answer.
    let root = path.ancestors().last()?;
    let wide: Vec<u16> = root.as_os_str().encode_wide().chain(Some(0)).collect();
    let (mut sectors, mut bytes, mut free, mut total) = (0u32, 0u32, 0u32, 0u32);
    // SAFETY: `wide` is a valid, NUL-terminated UTF-16 string that outlives the call, and every
    // out-pointer is a valid local.
    let ok = unsafe {
        GetDiskFreeSpaceW(
            wide.as_ptr(),
            &mut sectors,
            &mut bytes,
            &mut free,
            &mut total,
        )
    };
    (ok != 0).then(|| sectors as u64 * bytes as u64)
}

#[cfg(windows)]
fn probe_allocated(path: &Path, md: &Metadata) -> u64 {
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

#[cfg(test)]
mod tests {
    use super::round_to_cluster;

    #[test]
    fn lengths_round_up_to_whole_clusters() {
        assert_eq!(round_to_cluster(0, 4096), 0);
        assert_eq!(round_to_cluster(1, 4096), 4096);
        assert_eq!(round_to_cluster(4096, 4096), 4096);
        assert_eq!(round_to_cluster(4097, 4096), 8192);
        assert_eq!(round_to_cluster(10, 0), 10);
    }
}
