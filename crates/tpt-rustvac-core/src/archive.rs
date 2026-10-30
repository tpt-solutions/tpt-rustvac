//! Compress, verify, restore and list. The archive lives in a central "vacuum"
//! directory as `<slug>-<yyyymmdd>.tar.zst`.
//!
//! Safety rules:
//! - Any unreadable entry aborts the pack. A partial archive is never kept as if complete.
//! - The original is removed only after the archive has been re-read and verified.
//! - Removal goes to the Recycle Bin / Trash, so it stays recoverable.

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Local, Utc};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::identity::hardlink_identity;
use crate::safety::{absolute, long_path};
use crate::walk::is_busy;

/// Name of the manifest entry inside every archive. It is the last entry, so it can
/// record the counts it describes.
pub const MANIFEST: &str = "rustvac-manifest.json";
const ARCHIVE_SUFFIX: &str = ".tar.zst";
const FORMAT_VERSION: u32 = 1;
const ZSTD_LEVEL: i32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub format_version: u32,
    pub slug: String,
    /// The project directory the target belonged to. Default restore destination.
    pub project: PathBuf,
    pub created: String,
    /// Regular files in `target/`. Hardlinked duplicates are stored once by tar anyway.
    pub files: u64,
    /// Uncompressed bytes of those files.
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub struct PackReport {
    pub archive: PathBuf,
    pub manifest: Manifest,
    pub compressed_bytes: u64,
    /// Newest mtime seen while packing. Used for the busy check before removal.
    pub newest: Option<SystemTime>,
}

#[derive(Debug, Clone)]
pub struct ArchiveEntry {
    pub path: PathBuf,
    pub slug: String,
    pub date: String,
    pub bytes_on_disk: u64,
}

/// Stable, filesystem-safe name for a project: a readable prefix plus a hash of the full path,
/// so two projects with the same folder name never collide.
pub fn slug_for(project: &Path) -> String {
    let text = absolute(project).to_string_lossy().into_owned();
    let mut readable = String::new();
    let mut last_dash = false;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            readable.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !readable.is_empty() {
            readable.push('-');
            last_dash = true;
        }
    }
    let trimmed: String = readable.trim_end_matches('-').chars().take(60).collect();
    let prefix = if trimmed.is_empty() {
        "project"
    } else {
        trimmed.as_str()
    };
    format!(
        "{}-{:08x}",
        prefix.trim_end_matches('-'),
        fnv1a32(text.as_bytes())
    )
}

fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for b in bytes {
        hash ^= *b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Splits `<slug>-<yyyymmdd>[_n].tar.zst` into `(slug, yyyymmdd)`.
fn parse_archive_name(file_name: &str) -> Option<(String, String)> {
    let stem = file_name.strip_suffix(ARCHIVE_SUFFIX)?;
    let dash = stem.rfind('-')?;
    let slug = &stem[..dash];
    let rest = &stem[dash + 1..];
    let date = rest.get(0..8)?;
    if !date.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if rest.len() != 8 && !rest[8..].starts_with('_') {
        return None;
    }
    if slug.is_empty() {
        return None;
    }
    Some((slug.to_string(), date.to_string()))
}

/// Lists archives in the vacuum, sorted by slug, then newest first.
pub fn list(archive_dir: &Path) -> Result<Vec<ArchiveEntry>> {
    let dir = long_path(archive_dir);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| Error::io(archive_dir, e))? {
        let entry = entry.map_err(|e| Error::io(archive_dir, e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some((slug, date)) = parse_archive_name(&name) {
            let bytes_on_disk = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push(ArchiveEntry {
                path: archive_dir.join(&name),
                slug,
                date,
                bytes_on_disk,
            });
        }
    }
    out.sort_by(|a, b| a.slug.cmp(&b.slug).then(b.date.cmp(&a.date)));
    Ok(out)
}

/// Finds the newest archive for a slug.
pub fn latest_for_slug(archive_dir: &Path, slug: &str) -> Result<ArchiveEntry> {
    list(archive_dir)?
        .into_iter()
        .filter(|e| e.slug == slug)
        .max_by(|a, b| a.date.cmp(&b.date))
        .ok_or_else(|| Error::NotFound(slug.to_string()))
}

/// Deletes every archive for a slug. Archives are often the only copy left, so the
/// caller must confirm first.
pub fn remove_slug(archive_dir: &Path, slug: &str) -> Result<Vec<PathBuf>> {
    let matches: Vec<PathBuf> = list(archive_dir)?
        .into_iter()
        .filter(|e| e.slug == slug)
        .map(|e| e.path)
        .collect();
    if matches.is_empty() {
        return Err(Error::NotFound(slug.to_string()));
    }
    for path in &matches {
        fs::remove_file(long_path(path)).map_err(|e| Error::io(path, e))?;
    }
    Ok(matches)
}

/// Packs `target` into the vacuum, verifies the result, and returns the report.
/// Does not remove the original. See [`compress_and_vacuum`] for that.
pub fn pack(target: &Path, project: &Path, archive_dir: &Path) -> Result<PackReport> {
    pack_checked(target, project, archive_dir, verify)
}

/// `pack` with the check that runs on the written archive passed in. Tests use it to make the
/// check fail. The archive is removed when the check fails, and `pack` returns an error.
fn pack_checked(
    target: &Path,
    project: &Path,
    archive_dir: &Path,
    check: impl FnOnce(&Path) -> Result<Manifest>,
) -> Result<PackReport> {
    let target_abs = absolute(target);
    let archive_abs = absolute(archive_dir);
    if archive_abs.starts_with(&target_abs) {
        return Err(Error::Archive {
            path: target.to_path_buf(),
            reason: "the archive directory is inside the target".into(),
        });
    }
    fs::create_dir_all(long_path(archive_dir)).map_err(|e| Error::io(archive_dir, e))?;

    let slug = slug_for(project);
    let date = Local::now().format("%Y%m%d").to_string();
    let final_path = unique_archive_path(archive_dir, &slug, &date);
    let partial = final_path.with_file_name(format!(
        ".{}.partial",
        final_path.file_name().unwrap_or_default().to_string_lossy()
    ));

    let written = write_archive(target, project, &slug, &partial);
    let (manifest, newest) = match written {
        Ok(v) => v,
        Err(e) => {
            let _ = fs::remove_file(long_path(&partial));
            return Err(e);
        }
    };

    fs::rename(long_path(&partial), long_path(&final_path))
        .map_err(|e| Error::io(&final_path, e))?;
    let verified = match check(&final_path) {
        Ok(m) => m,
        Err(e) => {
            let _ = fs::remove_file(long_path(&final_path));
            return Err(e);
        }
    };
    if verified != manifest {
        let _ = fs::remove_file(long_path(&final_path));
        return Err(Error::Verify {
            path: final_path,
            reason: "manifest changed during verification".into(),
        });
    }
    let compressed_bytes = fs::metadata(long_path(&final_path))
        .map(|m| m.len())
        .unwrap_or(0);
    Ok(PackReport {
        archive: final_path,
        manifest,
        compressed_bytes,
        newest,
    })
}

fn unique_archive_path(archive_dir: &Path, slug: &str, date: &str) -> PathBuf {
    let first = archive_dir.join(format!("{slug}-{date}{ARCHIVE_SUFFIX}"));
    if !long_path(&first).exists() {
        return first;
    }
    (2..)
        .map(|n| archive_dir.join(format!("{slug}-{date}_{n}{ARCHIVE_SUFFIX}")))
        .find(|p| !long_path(p).exists())
        .expect("unbounded iterator always yields")
}

fn write_archive(
    target: &Path,
    project: &Path,
    slug: &str,
    partial: &Path,
) -> Result<(Manifest, Option<SystemTime>)> {
    let file = File::create(long_path(partial)).map_err(|e| Error::io(partial, e))?;
    let mut encoder = zstd::stream::write::Encoder::new(BufWriter::new(file), ZSTD_LEVEL)
        .map_err(|e| Error::io(partial, e))?;
    encoder
        .include_checksum(true)
        .map_err(|e| Error::io(partial, e))?;
    let workers = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1);
    if workers > 1 {
        encoder
            .multithread(workers)
            .map_err(|e| Error::io(partial, e))?;
    }

    let mut tar = tar::Builder::new(encoder);
    tar.follow_symlinks(false);
    let (files, bytes, newest) = append_tree(&mut tar, target)?;

    let manifest = Manifest {
        format_version: FORMAT_VERSION,
        slug: slug.to_string(),
        project: absolute(project),
        created: Utc::now().to_rfc3339(),
        files,
        bytes,
    };
    let data = serde_json::to_vec_pretty(&manifest).map_err(|e| Error::Archive {
        path: target.to_path_buf(),
        reason: e.to_string(),
    })?;
    let mut header = tar::Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(Utc::now().timestamp().max(0) as u64);
    tar.append_data(&mut header, MANIFEST, data.as_slice())
        .map_err(|e| Error::io(partial, e))?;

    let encoder = tar.into_inner().map_err(|e| Error::io(partial, e))?;
    let buffered = encoder.finish().map_err(|e| Error::io(partial, e))?;
    let file = buffered
        .into_inner()
        .map_err(|e| Error::io(partial, e.into_error()))?;
    file.sync_all().map_err(|e| Error::io(partial, e))?;
    Ok((manifest, newest))
}

/// Adds everything under `target` to the tar as `target/...`. Returns the file count,
/// the byte total and the newest mtime. Any unreadable or special entry is an error.
fn append_tree<W: Write>(
    tar: &mut tar::Builder<W>,
    target: &Path,
) -> Result<(u64, u64, Option<SystemTime>)> {
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut newest: Option<SystemTime> = None;
    let mut first_names: std::collections::HashMap<(u64, u64), PathBuf> =
        std::collections::HashMap::new();

    let walker = WalkBuilder::new(target)
        .standard_filters(false)
        .hidden(false)
        .follow_links(false)
        .build();
    for result in walker {
        let entry = result.map_err(|e| Error::Archive {
            path: target.to_path_buf(),
            reason: e.to_string(),
        })?;
        let path = entry.path();
        let rel = path.strip_prefix(target).unwrap_or(Path::new(""));
        let name = Path::new("target").join(rel);
        let Some(ft) = entry.file_type() else {
            continue;
        };
        let archive_err = |e: io::Error| Error::Archive {
            path: path.to_path_buf(),
            reason: e.to_string(),
        };

        if ft.is_dir() {
            tar.append_dir(&name, path).map_err(archive_err)?;
        } else if ft.is_file() {
            let md = entry.metadata().map_err(|e| Error::Archive {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?;
            newest = max_opt(newest, md.modified().ok());
            let identity = hardlink_identity(path, &md);
            match identity.and_then(|id| first_names.get(&id).cloned()) {
                Some(first_name) => {
                    // Later links to an already-stored file become tar hard-link entries,
                    // so the bytes are stored once and restore as links.
                    let mut header = tar::Header::new_gnu();
                    header.set_entry_type(tar::EntryType::hard_link());
                    header.set_size(0);
                    header.set_mode(0o644);
                    tar.append_link(&mut header, &name, &first_name)
                        .map_err(archive_err)?;
                }
                None => {
                    tar.append_path_with_name(path, &name)
                        .map_err(archive_err)?;
                    if let Some(id) = identity {
                        first_names.insert(id, name.clone());
                    }
                    files += 1;
                    bytes += md.len();
                }
            }
        } else if ft.is_symlink() {
            tar.append_path_with_name(path, &name)
                .map_err(archive_err)?;
        } else {
            return Err(Error::Archive {
                path: path.to_path_buf(),
                reason: "special file (socket, FIFO or device) cannot be archived".into(),
            });
        }
    }
    Ok((files, bytes, newest))
}

fn max_opt(a: Option<SystemTime>, b: Option<SystemTime>) -> Option<SystemTime> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

/// Reads the whole archive, checks the zstd checksum and tar structure, and checks the
/// file count and byte total against the manifest. Returns the manifest on success.
pub fn verify(archive: &Path) -> Result<Manifest> {
    let fail = |reason: String| Error::Verify {
        path: archive.to_path_buf(),
        reason,
    };
    let file = File::open(long_path(archive)).map_err(|e| Error::io(archive, e))?;
    let decoder = zstd::stream::read::Decoder::new(BufReader::new(file))
        .map_err(|e| Error::io(archive, e))?;
    let mut ar = tar::Archive::new(decoder);

    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut manifest: Option<Manifest> = None;
    let entries = ar.entries().map_err(|e| fail(e.to_string()))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| fail(e.to_string()))?;
        let path = entry.path().map_err(|e| fail(e.to_string()))?.into_owned();
        if path == Path::new(MANIFEST) {
            let mut text = String::new();
            entry
                .read_to_string(&mut text)
                .map_err(|e| fail(e.to_string()))?;
            manifest =
                Some(serde_json::from_str(&text).map_err(|e| fail(format!("bad manifest: {e}")))?);
            continue;
        }
        let is_file = entry.header().entry_type().is_file();
        // Reading the whole entry is what catches truncation and zstd checksum errors.
        let n = io::copy(&mut entry, &mut io::sink()).map_err(|e| fail(e.to_string()))?;
        if is_file {
            files += 1;
            bytes += n;
        }
    }

    let manifest = manifest.ok_or_else(|| fail("manifest entry is missing".into()))?;
    if manifest.format_version != FORMAT_VERSION {
        return Err(fail(format!(
            "unsupported format version {}",
            manifest.format_version
        )));
    }
    if manifest.files != files || manifest.bytes != bytes {
        return Err(fail(format!(
            "expected {} files / {} bytes, found {} files / {} bytes",
            manifest.files, manifest.bytes, files, bytes
        )));
    }
    Ok(manifest)
}

/// Unpacks an archive back into `<into>/target`. Refuses if that already exists.
/// Cargo sees changed mtimes and rebuilds incrementally, which is expected.
pub fn restore(archive: &Path, into: &Path) -> Result<PathBuf> {
    verify(archive)?;
    let dest = into.join("target");
    if long_path(&dest).exists() {
        return Err(Error::Exists(dest));
    }
    let base = long_path(into);
    fs::create_dir_all(&base).map_err(|e| Error::io(into, e))?;

    let file = File::open(long_path(archive)).map_err(|e| Error::io(archive, e))?;
    let decoder = zstd::stream::read::Decoder::new(BufReader::new(file))
        .map_err(|e| Error::io(archive, e))?;
    let mut ar = tar::Archive::new(decoder);
    for entry in ar.entries().map_err(|e| Error::io(archive, e))? {
        let mut entry = entry.map_err(|e| Error::io(archive, e))?;
        let path = entry
            .path()
            .map_err(|e| Error::io(archive, e))?
            .into_owned();
        if path == Path::new(MANIFEST) {
            continue;
        }
        // unpack_in refuses entries that escape the destination (e.g. "..").
        entry.unpack_in(&base).map_err(|e| Error::io(archive, e))?;
    }
    Ok(dest)
}

/// Runs `compact /c /s /q /f <target>`. Windows only. This is the transparent,
/// in-place option. It is not an archive.
pub fn compact_transparent(target: &Path) -> Result<()> {
    if !cfg!(windows) {
        return Err(Error::Unsupported(
            "transparent compression is only available on Windows".into(),
        ));
    }
    let status = std::process::Command::new("compact")
        .args(["/c", "/s", "/q", "/f"])
        // compact.exe rejects the \\?\ long-path prefix, so pass the plain absolute path.
        .arg(absolute(target))
        .status()
        .map_err(|e| Error::io(target, e))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Archive {
            path: target.to_path_buf(),
            reason: format!("compact exited with {status}"),
        })
    }
}

/// Full compress flow: pack, verify, then move the original to the trash.
/// If the target turns out to be busy, the archive is kept and the original is left alone.
pub fn compress_and_vacuum(
    target: &Path,
    project: &Path,
    archive_dir: &Path,
    busy_grace: Duration,
) -> Result<PackReport> {
    if let Some(reason) = crate::safety::sync_managed(target) {
        return Err(Error::Archive {
            path: target.to_path_buf(),
            reason,
        });
    }
    let report = pack(target, project, archive_dir)?;
    if is_busy(report.newest, SystemTime::now(), busy_grace) {
        return Err(Error::Busy {
            path: target.to_path_buf(),
            grace_secs: busy_grace.as_secs(),
        });
    }
    trash::delete(long_path(target)).map_err(|e| Error::Trash {
        path: target.to_path_buf(),
        reason: e.to_string(),
    })?;
    Ok(report)
}

/// Formats a `DateTime` the way the CLI and GUI show archive timestamps.
pub fn format_created(created: &str) -> String {
    DateTime::parse_from_rfc3339(created)
        .map(|d| d.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|_| created.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_archive_names() {
        assert_eq!(
            parse_archive_name("api-server-1a2b3c4d-20261019.tar.zst"),
            Some(("api-server-1a2b3c4d".into(), "20261019".into()))
        );
        assert_eq!(
            parse_archive_name("api-server-1a2b3c4d-20261019_2.tar.zst"),
            Some(("api-server-1a2b3c4d".into(), "20261019".into()))
        );
        assert_eq!(parse_archive_name("notes.txt"), None);
        assert_eq!(parse_archive_name("slug-2026.tar.zst"), None);
    }

    /// `compress_and_vacuum` trashes the original only after `pack` returns Ok, so a failed
    /// check must give an error, no archive, and an untouched target.
    #[test]
    fn a_failed_check_keeps_the_target_and_leaves_no_archive() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let target = project.join("target");
        fs::create_dir_all(target.join("debug")).unwrap();
        fs::write(target.join("debug/a.bin"), vec![1u8; 1000]).unwrap();
        let archive_dir = dir.path().join("archive");

        let result = pack_checked(&target, &project, &archive_dir, |path| {
            Err(Error::Verify {
                path: path.to_path_buf(),
                reason: "injected for the test".into(),
            })
        });

        assert!(result.is_err());
        assert!(target.join("debug/a.bin").is_file());
        assert_eq!(fs::read_dir(&archive_dir).unwrap().count(), 0);
    }

    #[test]
    fn slug_is_stable_and_distinct() {
        let a = slug_for(Path::new("/code/api"));
        assert_eq!(a, slug_for(Path::new("/code/api")));
        assert_ne!(a, slug_for(Path::new("/other/api")));
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }
}
