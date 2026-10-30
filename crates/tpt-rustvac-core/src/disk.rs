//! One read-only report of where Rust-related disk space sits: rustup, cargo and WSL virtual
//! disks. Each group comes with a suggested next step. Nothing here removes or compacts anything.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::Serialize;

use crate::caches::{CacheItem, CacheKind, Tier};
use crate::wsl::WslDisk;

#[derive(Debug, Clone, Serialize)]
pub struct DiskItem {
    pub kind: CacheKind,
    pub label: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub tier: Tier,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolchainInfo {
    pub name: String,
    pub path: PathBuf,
    pub bytes: u64,
    /// Seconds since the unix epoch of the folder's last change.
    pub last_used_unix: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskReport {
    /// Rustup's scratch folders (tmp, downloads, update hashes).
    pub rustup: Vec<DiskItem>,
    pub toolchains: Vec<ToolchainInfo>,
    pub cargo: Vec<DiskItem>,
    pub wsl: Vec<WslDisk>,
    pub free_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
}

/// Builds the report from already-discovered parts, so tests need no real machine.
pub fn build(items: &[CacheItem], wsl: Vec<WslDisk>, space: Option<(u64, u64)>) -> DiskReport {
    let mut report = DiskReport {
        rustup: Vec::new(),
        toolchains: Vec::new(),
        cargo: Vec::new(),
        wsl,
        free_bytes: space.map(|s| s.0),
        total_bytes: space.map(|s| s.1),
    };
    for i in items {
        let item = DiskItem {
            kind: i.kind,
            label: i.kind.label().to_string(),
            path: i.path.clone(),
            bytes: i.size,
            tier: i.tier(),
        };
        match i.kind {
            CacheKind::Toolchain => report.toolchains.push(ToolchainInfo {
                name: i.name.clone(),
                path: i.path.clone(),
                bytes: i.size,
                last_used_unix: crate::toolchains::last_used(&i.path).map(unix_secs),
            }),
            CacheKind::RustupTmp | CacheKind::RustupDownloads | CacheKind::RustupUpdateHashes => {
                report.rustup.push(item)
            }
            _ => report.cargo.push(item),
        }
    }
    report
        .toolchains
        .sort_by_key(|t| std::cmp::Reverse(t.bytes));
    report.wsl.sort_by_key(|d| std::cmp::Reverse(d.size_bytes));
    report
}

fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl DiskReport {
    pub fn rustup_bytes(&self) -> u64 {
        self.rustup.iter().map(|i| i.bytes).sum()
    }

    pub fn toolchain_bytes(&self) -> u64 {
        self.toolchains.iter().map(|t| t.bytes).sum()
    }

    pub fn cargo_bytes(&self) -> u64 {
        self.cargo.iter().map(|i| i.bytes).sum()
    }

    pub fn wsl_bytes(&self) -> u64 {
        self.wsl.iter().map(|d| d.size_bytes).sum()
    }

    /// Regenerable bytes in the rustup and cargo groups, which `rustvac caches clean` frees.
    pub fn regenerable_bytes(&self) -> u64 {
        self.rustup
            .iter()
            .chain(&self.cargo)
            .filter(|i| i.tier == Tier::Regenerable)
            .map(|i| i.bytes)
            .sum()
    }
}

/// The suggested next step for the rustup, toolchain, cargo and WSL groups. `None` when
/// there is nothing to do for that group.
pub fn next_steps(report: &DiskReport) -> Steps {
    let regenerable = |items: &[DiskItem]| {
        items
            .iter()
            .any(|i| i.tier == Tier::Regenerable && i.bytes > 0)
    };
    Steps {
        rustup: regenerable(&report.rustup)
            .then(|| "run `rustvac caches clean --rustup-tmp --rustup-downloads`".to_string()),
        toolchains: (report.toolchains.len() > 1).then(|| {
            "list them with `rustvac toolchains`, then `rustvac toolchains rm <name>` for old ones"
                .to_string()
        }),
        cargo: regenerable(&report.cargo)
            .then(|| "run `rustvac caches clean --cargo-registry --cargo-git`".to_string()),
        wsl: (!report.wsl.is_empty()).then(|| {
            "WSL disks only grow. Clear space inside the distro, then compact the disk by hand \
             (rustvac never does this). Use `rustvac disk --json` for the exact steps."
                .to_string()
        }),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Steps {
    pub rustup: Option<String>,
    pub toolchains: Option<String>,
    pub cargo: Option<String>,
    pub wsl: Option<String>,
}

/// The largest WSL disk, for the one-line mention in `check` and `status`.
pub fn largest_wsl(wsl: &[WslDisk]) -> Option<&WslDisk> {
    wsl.iter().max_by_key(|d| d.size_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caches::discover;
    use crate::identity::disk_bytes;
    use crate::wsl::WslKind;
    use std::fs;
    use std::path::Path;

    /// Writes `bytes` and returns what the file takes on disk. On Windows a small file takes a
    /// whole cluster, so the expected totals come from the same function the scanner uses.
    fn write(path: &Path, bytes: usize) -> u64 {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![b'x'; bytes]).unwrap();
        disk_bytes(path, &fs::metadata(path).unwrap())
    }

    #[test]
    fn groups_items_and_sums() {
        let dir = tempfile::tempdir().unwrap();
        let rustup = dir.path().join("rustup");
        let cargo = dir.path().join("cargo");
        let tmp = write(&rustup.join("tmp/a"), 100);
        let hashes = write(&rustup.join("update-hashes/stable"), 10);
        let stable = write(&rustup.join("toolchains/stable/bin/rustc"), 500);
        let old = write(&rustup.join("toolchains/old/bin/rustc"), 900);
        let registry = write(&cargo.join("registry/cache/i/x.crate"), 70);
        let db = write(&cargo.join("git/db/r/pack"), 30);
        let items = discover(Some(&rustup), Some(&cargo));
        let wsl = vec![WslDisk {
            name: "Ubuntu".into(),
            path: dir.path().join("ext4.vhdx"),
            size_bytes: 4000,
            kind: WslKind::Distro,
        }];
        let report = build(&items, wsl, Some((10, 100)));
        assert_eq!(report.rustup_bytes(), tmp + hashes);
        assert_eq!(report.toolchain_bytes(), stable + old);
        assert_eq!(report.cargo_bytes(), registry + db);
        assert_eq!(report.regenerable_bytes(), tmp + hashes + registry);
        assert_eq!(report.wsl_bytes(), 4000);
        // Largest toolchain first.
        assert_eq!(report.toolchains[0].name, "old");
        assert_eq!(report.free_bytes, Some(10));

        let steps = next_steps(&report);
        assert!(steps.rustup.is_some() && steps.cargo.is_some());
        assert!(steps.toolchains.is_some());
        assert!(steps.wsl.unwrap().contains("never"));
    }

    #[test]
    fn empty_machine_has_no_steps() {
        let report = build(&[], Vec::new(), None);
        let steps = next_steps(&report);
        assert!(steps.rustup.is_none() && steps.cargo.is_none());
        assert!(steps.toolchains.is_none() && steps.wsl.is_none());
        assert!(report.free_bytes.is_none());
    }
}
