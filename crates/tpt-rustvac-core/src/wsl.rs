//! WSL and Docker virtual disks (`ext4.vhdx`). They only grow: deleting files inside a distro
//! does not shrink the file on the host. This module finds and sizes them and describes the
//! manual compact steps. It never runs anything and never removes a disk.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const DISK_NAME: &str = "ext4.vhdx";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WslKind {
    Distro,
    Docker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WslDisk {
    pub name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub kind: WslKind,
}

/// A distro registered with `wsl --import` or installed from the store, as the registry
/// lists it under `HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registered {
    pub name: String,
    pub base_path: PathBuf,
}

/// Finds the virtual disks under a `%LOCALAPPDATA%` folder and under the registered base
/// paths. A disk found both ways is listed once, under its registered name.
pub fn discover(local_app_data: Option<&Path>, registered: &[Registered]) -> Vec<WslDisk> {
    let mut disks: Vec<WslDisk> = Vec::new();
    for r in registered {
        let kind = if r.name.starts_with("docker-desktop") || has_docker_component(&r.base_path) {
            WslKind::Docker
        } else {
            WslKind::Distro
        };
        push(
            &mut disks,
            r.name.clone(),
            r.base_path.join(DISK_NAME),
            kind,
        );
    }
    if let Some(local) = local_app_data {
        if let Ok(packages) = std::fs::read_dir(local.join("Packages")) {
            for p in packages.filter_map(|e| e.ok()) {
                let disk = p.path().join("LocalState").join(DISK_NAME);
                push(
                    &mut disks,
                    p.file_name().to_string_lossy().into_owned(),
                    disk,
                    WslKind::Distro,
                );
            }
        }
        let docker = local.join("Docker").join("wsl");
        let mut found = Vec::new();
        find_disks(&docker, 4, &mut found);
        for disk in found {
            let name = disk
                .parent()
                .and_then(Path::file_name)
                .map(|n| format!("Docker {}", n.to_string_lossy()))
                .unwrap_or_else(|| "Docker".to_string());
            push(&mut disks, name, disk, WslKind::Docker);
        }
    }
    disks.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes).then(a.name.cmp(&b.name)));
    disks
}

fn has_docker_component(path: &Path) -> bool {
    path.components()
        .any(|c| c.as_os_str().eq_ignore_ascii_case("Docker"))
}

fn push(disks: &mut Vec<WslDisk>, name: String, path: PathBuf, kind: WslKind) {
    let Ok(md) = std::fs::metadata(&path) else {
        return;
    };
    if !md.is_file() || disks.iter().any(|d| same_file(&d.path, &path)) {
        return;
    }
    disks.push(WslDisk {
        name,
        path,
        size_bytes: md.len(),
        kind,
    });
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// Every `ext4.vhdx` under `dir`, down to `depth` levels.
fn find_disks(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for e in read.filter_map(|e| e.ok()) {
        let path = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() && depth > 0 {
            find_disks(&path, depth - 1, out);
        } else if ft.is_file() && e.file_name() == DISK_NAME {
            out.push(path);
        }
    }
}

/// The manual steps that return unused space to the host. Text only: rustvac never runs them.
pub fn compact_hint(disk: &WslDisk) -> String {
    let path = disk.path.display();
    match disk.kind {
        WslKind::Distro => format!(
            "1. In the distro, clear what you no longer need (for example `rustvac sweep` there).\n\
             2. In PowerShell: wsl --shutdown\n\
             3. Then, as administrator: Optimize-VHD -Path \"{path}\" -Mode Full\n\
             \x20  (without Hyper-V, use diskpart: select vdisk file=\"{path}\", attach vdisk readonly, compact vdisk, detach vdisk)"
        ),
        WslKind::Docker => format!(
            "1. In Docker, run `docker system prune` to free image and volume space.\n\
             2. Quit Docker Desktop and run: wsl --shutdown\n\
             3. Then, as administrator: Optimize-VHD -Path \"{path}\" -Mode Full"
        ),
    }
}

/// Distros from the registry. Other platforms have none.
#[cfg(windows)]
pub fn registered_distros() -> Vec<Registered> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
        RRF_RT_REG_SZ,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn read_string(key: HKEY, name: &str) -> Option<String> {
        let name = wide(name);
        let mut size = 0u32;
        // SAFETY: `name` is NUL-terminated, and a null data pointer asks only for the size.
        let rc = unsafe {
            RegGetValueW(
                key,
                std::ptr::null(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if rc != ERROR_SUCCESS || size < 2 {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2)];
        // SAFETY: `buf` holds at least `size` bytes.
        let rc = unsafe {
            RegGetValueW(
                key,
                std::ptr::null(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }

    let mut out = Vec::new();
    let path = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Lxss");
    let mut root: HKEY = std::ptr::null_mut();
    // SAFETY: `path` is NUL-terminated and `root` is a valid out-pointer.
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_READ, &mut root) }
        != ERROR_SUCCESS
    {
        return out;
    }
    for index in 0.. {
        let mut name = [0u16; 256];
        let mut len = name.len() as u32;
        // SAFETY: `name` has room for `len` UTF-16 units, and the optional outputs are null.
        let rc = unsafe {
            RegEnumKeyExW(
                root,
                index,
                name.as_mut_ptr(),
                &mut len,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if rc != ERROR_SUCCESS {
            break;
        }
        let sub = &name[..len as usize];
        let mut sub_z = sub.to_vec();
        sub_z.push(0);
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: `root` is open, `sub_z` is NUL-terminated, `key` is a valid out-pointer.
        if unsafe { RegOpenKeyExW(root, sub_z.as_ptr(), 0, KEY_READ, &mut key) } != ERROR_SUCCESS {
            continue;
        }
        if let (Some(distro), Some(base)) = (
            read_string(key, "DistributionName"),
            read_string(key, "BasePath"),
        ) {
            out.push(Registered {
                name: distro,
                base_path: PathBuf::from(base.trim_start_matches(r"\\?\")),
            });
        }
        // SAFETY: `key` was opened above.
        unsafe { RegCloseKey(key) };
    }
    // SAFETY: `root` was opened above.
    unsafe { RegCloseKey(root) };
    out
}

#[cfg(not(windows))]
pub fn registered_distros() -> Vec<Registered> {
    Vec::new()
}

/// The disks on this machine: the registry first, then `%LOCALAPPDATA%`. Empty off Windows.
pub fn discover_here() -> Vec<WslDisk> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    discover(local.as_deref(), &registered_distros())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn disk(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![0u8; bytes]).unwrap();
    }

    #[test]
    fn finds_store_docker_and_imported_disks() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("local");
        disk(&local.join("Packages/Ubuntu_abc/LocalState/ext4.vhdx"), 300);
        disk(&local.join("Packages/Other/LocalState/readme.txt"), 5);
        disk(&local.join("Docker/wsl/disk/ext4.vhdx"), 200);
        disk(&local.join("Docker/wsl/main/ext4.vhdx"), 50);
        let imported = dir.path().join("imported");
        disk(&imported.join("ext4.vhdx"), 400);
        let registered = [Registered {
            name: "Arch".into(),
            base_path: imported,
        }];

        let found = discover(Some(&local), &registered);
        let names: Vec<_> = found.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Arch", "Ubuntu_abc", "Docker disk", "Docker main"]);
        assert_eq!(found[0].size_bytes, 400);
        assert_eq!(found[2].kind, WslKind::Docker);
    }

    #[test]
    fn registered_name_wins_over_the_package_folder() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("local");
        let base = local.join("Packages/Ubuntu_abc/LocalState");
        disk(&base.join("ext4.vhdx"), 10);
        let registered = [Registered {
            name: "Ubuntu".into(),
            base_path: base,
        }];
        let found = discover(Some(&local), &registered);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Ubuntu");
    }

    #[test]
    fn docker_desktop_in_the_registry_is_a_docker_disk() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("Docker/wsl/main");
        disk(&base.join("ext4.vhdx"), 10);
        let registered = [Registered {
            name: "docker-desktop".into(),
            base_path: base,
        }];
        let found = discover(None, &registered);
        assert_eq!(found[0].kind, WslKind::Docker);
    }

    #[test]
    fn nothing_found_is_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        assert!(discover(Some(dir.path()), &[]).is_empty());
        assert!(discover(None, &[]).is_empty());
    }

    #[test]
    fn hint_names_the_disk_and_never_runs_anything() {
        let d = WslDisk {
            name: "Ubuntu".into(),
            path: PathBuf::from("C:\\x\\ext4.vhdx"),
            size_bytes: 1,
            kind: WslKind::Distro,
        };
        let hint = compact_hint(&d);
        assert!(hint.contains("wsl --shutdown"));
        assert!(hint.contains("C:\\x\\ext4.vhdx"));
    }
}
