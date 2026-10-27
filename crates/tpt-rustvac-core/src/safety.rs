//! Guardrails for destructive and broad operations, plus path helpers.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Refuses filesystem roots and the home directory unless `force` is set.
/// This tool deletes things, so the footgun should be hard to pull.
pub fn check_scan_root(path: &Path, force: bool) -> Result<()> {
    if force {
        return Ok(());
    }
    let abs = absolute(path);
    let reason = if abs.parent().is_none() {
        Some("it is a filesystem root")
    } else if crate::config::home_dir()
        .map(|h| absolute(&h) == abs)
        .unwrap_or(false)
    {
        Some("it is your home directory")
    } else {
        None
    };
    match reason {
        Some(reason) => Err(Error::Guardrail {
            path: abs,
            reason: reason.into(),
        }),
        None => Ok(()),
    }
}

/// Lexically absolute path. Falls back to the input if the working directory cannot be read.
pub fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// True if `path` is at or below any of the keep paths.
pub fn is_kept(path: &Path, keep: &[PathBuf]) -> bool {
    let abs = absolute(path);
    keep.iter().any(|k| abs.starts_with(absolute(k)))
}

/// On Windows, prefixes absolute paths with `\\?\` so paths over MAX_PATH work.
/// Forward slashes are converted first, since the prefix does not accept them.
/// Elsewhere this returns the path unchanged.
pub fn long_path(path: &Path) -> PathBuf {
    if !cfg!(windows) {
        return path.to_path_buf();
    }
    let abs = absolute(path);
    let s = abs.to_string_lossy().replace('/', "\\");
    if s.starts_with(r"\\?\") {
        return PathBuf::from(s);
    }
    if let Some(unc) = s.strip_prefix(r"\\") {
        return PathBuf::from(format!(r"\\?\UNC\{unc}"));
    }
    PathBuf::from(format!(r"\\?\{s}"))
}

/// Why a path must not be trashed or archived, or `None` if it may be. On Windows a path is
/// refused when it is under a OneDrive folder, or when it or any folder above it is a reparse
/// point. Cloud sync folders are reparse points, and so are symlinks and junctions. Removing
/// a file in a sync folder can remove the cloud copy too. Elsewhere nothing is refused here.
#[cfg(windows)]
pub fn sync_managed(path: &Path) -> Option<String> {
    use std::os::windows::fs::MetadataExt;
    const REPARSE_POINT: u32 = 0x400;

    let abs = absolute(path);
    let text = abs.to_string_lossy().to_lowercase();
    for var in ["OneDrive", "OneDriveCommercial", "OneDriveConsumer"] {
        if let Some(root) = std::env::var_os(var).filter(|v| !v.is_empty()) {
            let root = root.to_string_lossy().to_lowercase();
            if text == root || text.starts_with(&format!("{}\\", root.trim_end_matches('\\'))) {
                return Some(format!("it is inside your OneDrive folder ({var})"));
            }
        }
    }
    for ancestor in abs.ancestors() {
        if ancestor.parent().is_none() {
            break;
        }
        if let Ok(md) = std::fs::symlink_metadata(ancestor) {
            if md.file_attributes() & REPARSE_POINT != 0 {
                return Some(format!(
                    "it is under a sync folder, symlink or junction ({})",
                    ancestor.display()
                ));
            }
        }
    }
    None
}

#[cfg(not(windows))]
pub fn sync_managed(_path: &Path) -> Option<String> {
    None
}

/// Moves a path to the Recycle Bin / Trash. This is the default removal action.
pub fn move_to_trash(path: &Path) -> Result<()> {
    if let Some(reason) = sync_managed(path) {
        return Err(Error::Trash {
            path: path.to_path_buf(),
            reason,
        });
    }
    trash::delete(long_path(path)).map_err(|e| Error::Trash {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })
}

/// Deletes a path for good. The GUI only calls this after a second, explicit confirmation.
pub fn delete_permanently(path: &Path) -> Result<()> {
    let long = long_path(path);
    let meta = std::fs::symlink_metadata(&long).map_err(|e| Error::io(path, e))?;
    let result = if meta.is_dir() {
        std::fs::remove_dir_all(&long)
    } else {
        std::fs::remove_file(&long)
    };
    result.map_err(|e| Error::io(path, e))
}
