//! The read-only drive check. It reads free space, sums what a sweep could reclaim, and saves
//! a recommendation that `rustvac status` shows later. It never removes anything.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::sweep::{SweepKind, SweepPlan};
use crate::wsl::WslDisk;

pub const JSON_NAME: &str = "recommendation.json";
pub const TEXT_NAME: &str = "recommendation.txt";

/// Free and total bytes on the drive that holds `path`.
#[cfg(windows)]
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let (mut free, mut total, mut total_free) = (0u64, 0u64, 0u64);
    // SAFETY: `wide` is a valid NUL-terminated UTF-16 string, and the three out-pointers point
    // at live u64 values.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut total_free) };
    (ok != 0).then_some((free, total))
}

// The casts are no-ops on some Unixes (Linux reports u64) and needed on others (macOS reports
// narrower block counts), so they stay.
#[cfg(unix)]
#[allow(clippy::unnecessary_cast)]
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;

    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: `c` is a valid NUL-terminated string and `st` is a valid out-pointer.
    if unsafe { libc::statvfs(c.as_ptr(), st.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: statvfs returned 0, so it filled the struct.
    let st = unsafe { st.assume_init() };
    let unit = st.f_frsize as u64;
    Some((st.f_bavail as u64 * unit, st.f_blocks as u64 * unit))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KindTotal {
    pub kind: SweepKind,
    pub bytes: u64,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TopItem {
    pub kind: SweepKind,
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Recommendation {
    pub created_unix: u64,
    /// The folder whose drive was measured.
    pub drive: String,
    pub free_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    /// What `rustvac sweep` would remove right now.
    pub reclaimable_bytes: u64,
    pub by_kind: Vec<KindTotal>,
    pub top: Vec<TopItem>,
    /// Old targets that `rustvac compress` could archive. Not part of `reclaimable_bytes`.
    pub compressible_bytes: u64,
    pub warn_below: Option<u64>,
    /// True when free space is known and under `warn_below`.
    pub low_space: bool,
    /// WSL and Docker virtual disks. Report only. Absent in recommendations saved by older
    /// versions.
    #[serde(default)]
    pub wsl: Vec<WslDisk>,
}

pub struct CheckInput<'a> {
    pub plan: &'a SweepPlan,
    pub drive: &'a Path,
    /// Free and total bytes. Injected, so tests do not depend on the real drive.
    pub space: Option<(u64, u64)>,
    pub compressible_bytes: u64,
    pub wsl: &'a [WslDisk],
    pub warn_below: Option<u64>,
    pub now: SystemTime,
    pub top_n: usize,
}

pub fn build(input: &CheckInput) -> Recommendation {
    let mut top: Vec<TopItem> = input
        .plan
        .items
        .iter()
        .map(|i| TopItem {
            kind: i.kind,
            path: i.path.display().to_string(),
            bytes: i.size,
        })
        .collect();
    top.sort_by_key(|t| std::cmp::Reverse(t.bytes));
    top.truncate(input.top_n);
    let free = input.space.map(|s| s.0);
    Recommendation {
        created_unix: unix_secs(input.now),
        drive: input.drive.display().to_string(),
        free_bytes: free,
        total_bytes: input.space.map(|s| s.1),
        reclaimable_bytes: input.plan.total_size(),
        by_kind: input
            .plan
            .by_kind()
            .into_iter()
            .map(|(kind, bytes, count)| KindTotal { kind, bytes, count })
            .collect(),
        top,
        compressible_bytes: input.compressible_bytes,
        warn_below: input.warn_below,
        low_space: matches!((free, input.warn_below), (Some(f), Some(w)) if f < w),
        wsl: input.wsl.to_vec(),
    }
}

fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Recommendation {
    pub fn age(&self, now: SystemTime) -> Duration {
        Duration::from_secs(unix_secs(now).saturating_sub(self.created_unix))
    }
}

/// Local date and time for a unix timestamp, like `2026-10-30 14:05`.
pub fn local_time(unix: u64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(unix as i64, 0)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "unknown time".into())
}

/// Where the report files live: `<home>/.rustvac`.
pub fn report_dir(home: &Path) -> PathBuf {
    home.join(".rustvac")
}

/// Writes the JSON and the readable text next to each other.
pub fn save(dir: &Path, rec: &Recommendation, text: &str) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let json = serde_json::to_string_pretty(rec)
        .map_err(|e| Error::Config(format!("could not encode the recommendation: {e}")))?;
    let json_path = dir.join(JSON_NAME);
    std::fs::write(&json_path, json).map_err(|e| Error::io(&json_path, e))?;
    let text_path = dir.join(TEXT_NAME);
    std::fs::write(&text_path, text).map_err(|e| Error::io(&text_path, e))
}

/// The last saved recommendation, or `None` if there is none or it cannot be read.
pub fn load(dir: &Path) -> Option<Recommendation> {
    let text = std::fs::read_to_string(dir.join(JSON_NAME)).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sweep::SweepItem;

    fn plan() -> SweepPlan {
        let item = |kind, path: &str, size| SweepItem {
            kind,
            path: PathBuf::from(path),
            members: vec![PathBuf::from(path)],
            size,
            newest: None,
        };
        SweepPlan {
            items: vec![
                item(SweepKind::Incremental, "a", 300),
                item(SweepKind::Incremental, "b", 100),
                item(SweepKind::RustupTmp, "c", 900),
            ],
            skipped: Vec::new(),
        }
    }

    fn input<'a>(
        plan: &'a SweepPlan,
        space: Option<(u64, u64)>,
        warn: Option<u64>,
    ) -> CheckInput<'a> {
        CheckInput {
            plan,
            drive: Path::new("C:\\Users\\x"),
            space,
            compressible_bytes: 7,
            wsl: &[],
            warn_below: warn,
            now: SystemTime::UNIX_EPOCH + Duration::from_secs(1000),
            top_n: 2,
        }
    }

    #[test]
    fn totals_and_top_items() {
        let plan = plan();
        let rec = build(&input(&plan, Some((5000, 10000)), None));
        assert_eq!(rec.reclaimable_bytes, 1300);
        assert_eq!(rec.compressible_bytes, 7);
        assert_eq!(rec.by_kind.len(), 2);
        assert_eq!(rec.by_kind[0].kind, SweepKind::Incremental);
        assert_eq!(rec.by_kind[0].bytes, 400);
        assert_eq!(rec.by_kind[0].count, 2);
        assert_eq!(rec.top.len(), 2);
        assert_eq!(rec.top[0].bytes, 900);
        assert_eq!(rec.created_unix, 1000);
    }

    #[test]
    fn low_space_needs_known_free_and_a_threshold() {
        let plan = plan();
        assert!(build(&input(&plan, Some((50, 100)), Some(100))).low_space);
        assert!(!build(&input(&plan, Some((150, 200)), Some(100))).low_space);
        assert!(!build(&input(&plan, None, Some(100))).low_space);
        assert!(!build(&input(&plan, Some((50, 100)), None)).low_space);
    }

    #[test]
    fn saves_and_loads() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan();
        let rec = build(&input(&plan, Some((1, 2)), Some(3)));
        save(dir.path(), &rec, "text").unwrap();
        assert_eq!(load(dir.path()).unwrap(), rec);
        assert!(dir.path().join(TEXT_NAME).is_file());
        assert!(load(&dir.path().join("nowhere")).is_none());
    }

    #[test]
    fn an_old_recommendation_without_wsl_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan();
        let rec = build(&input(&plan, Some((1, 2)), None));
        let mut value = serde_json::to_value(&rec).unwrap();
        value.as_object_mut().unwrap().remove("wsl");
        std::fs::write(dir.path().join(JSON_NAME), value.to_string()).unwrap();
        let loaded = load(dir.path()).unwrap();
        assert!(loaded.wsl.is_empty());
    }

    #[test]
    fn reads_the_real_drive() {
        let dir = tempfile::tempdir().unwrap();
        let (free, total) = disk_space(dir.path()).unwrap();
        assert!(total > 0 && free <= total);
    }
}
