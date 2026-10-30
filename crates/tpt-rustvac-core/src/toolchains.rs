//! Installed Rust toolchains. Listing reads the rustup home folder. Removal goes through
//! `rustup toolchain uninstall`, never by deleting folders.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};
use crate::target::{parse_rustc_vv, RustcInfo};

/// A toolchain folder under `<rustup home>/toolchains`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolchain {
    pub name: String,
    pub path: PathBuf,
}

/// One line of `rustup toolchain list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub name: String,
    pub default: bool,
    pub active: bool,
}

/// Toolchain folders in `<rustup_home>/toolchains`, sorted by name. A missing folder is an
/// empty list.
pub fn installed(rustup_home: &Path) -> Vec<Toolchain> {
    let Ok(entries) = std::fs::read_dir(rustup_home.join("toolchains")) else {
        return Vec::new();
    };
    let mut out: Vec<Toolchain> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| Toolchain {
            name: e.file_name().to_string_lossy().into_owned(),
            path: e.path(),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// When rustup last touched a toolchain folder (its own mtime: installs, updates and
/// component changes). It is not a record of the compiler being run, so treat it as a hint
/// that an old toolchain is a candidate for removal.
pub fn last_used(toolchain: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(toolchain).and_then(|m| m.modified()).ok()
}

/// Runs each toolchain's `rustc -vV`. Toolchains that cannot run (a half-installed folder)
/// are left out.
pub fn installed_rustc_versions(rustup_home: &Path) -> Vec<RustcInfo> {
    installed(rustup_home)
        .iter()
        .filter_map(|t| rustc_version(&t.path))
        .collect()
}

/// `rustc -vV` of one toolchain folder.
pub fn rustc_version(toolchain: &Path) -> Option<RustcInfo> {
    let rustc = toolchain
        .join("bin")
        .join(format!("rustc{}", std::env::consts::EXE_SUFFIX));
    let out = Command::new(rustc).arg("-vV").output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_rustc_vv(&String::from_utf8_lossy(&out.stdout))
}

/// Parses `rustup toolchain list`. Lines look like `stable-x86_64-pc-windows-msvc (active, default)`.
/// Older rustups print `(default)` only. The "no installed toolchains" notice gives an empty list.
pub fn parse_list(output: &str) -> Vec<Listing> {
    output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.to_ascii_lowercase().starts_with("no installed"))
        .map(|line| {
            let (name, rest) = match line.split_once(char::is_whitespace) {
                Some((n, r)) => (n, r),
                None => (line, ""),
            };
            let markers = rest.trim().trim_start_matches('(').trim_end_matches(')');
            let has = |m: &str| markers.split(',').any(|p| p.trim() == m);
            Listing {
                name: name.to_string(),
                default: has("default"),
                active: has("active"),
            }
        })
        .collect()
}

/// `rustup toolchain list`, or an error when rustup is not on PATH.
pub fn list_with_rustup() -> Result<Vec<Listing>> {
    let out = rustup(&["toolchain", "list"])?;
    Ok(parse_list(&out))
}

/// Arguments for removing a toolchain.
pub fn uninstall_args(name: &str) -> Vec<String> {
    vec!["toolchain".into(), "uninstall".into(), name.into()]
}

/// Removes a toolchain with rustup. The default toolchain is refused.
pub fn uninstall(name: &str) -> Result<String> {
    let listing = list_with_rustup()?;
    let found = listing
        .iter()
        .find(|l| l.name == name)
        .ok_or_else(|| Error::NotFound(format!("toolchain '{name}'")))?;
    if found.default {
        return Err(Error::Unsupported(format!(
            "'{name}' is the default toolchain. Set another default with `rustup default` first."
        )));
    }
    let args = uninstall_args(name);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    rustup(&refs)
}

fn rustup(args: &[&str]) -> Result<String> {
    let out = Command::new("rustup")
        .args(args)
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                Error::Unsupported("rustup was not found on PATH".to_string())
            }
            _ => Error::io(Path::new("rustup"), e),
        })?;
    // rustup prints progress on stderr, so keep both.
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Ok(text)
    } else {
        Err(Error::Unsupported(format!(
            "rustup {} failed: {}",
            args.join(" "),
            text.trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_markers() {
        let out = "stable-x86_64-pc-windows-msvc (active, default)\nnightly-2026-01-01-x86_64-pc-windows-msvc\n1.80.0-x86_64-pc-windows-msvc (override)\n";
        let list = parse_list(out);
        assert_eq!(list.len(), 3);
        assert!(list[0].default && list[0].active);
        assert!(!list[1].default && !list[1].active);
        assert!(!list[2].default && !list[2].active);
        assert_eq!(list[1].name, "nightly-2026-01-01-x86_64-pc-windows-msvc");
    }

    #[test]
    fn old_style_default_marker() {
        let list = parse_list("stable-x86_64-unknown-linux-gnu (default)\n");
        assert!(list[0].default);
        assert!(!list[0].active);
    }

    #[test]
    fn no_toolchains() {
        assert!(parse_list("no installed toolchains\n").is_empty());
        assert!(parse_list("").is_empty());
    }

    #[test]
    fn uninstall_command_line() {
        assert_eq!(
            uninstall_args("nightly"),
            vec!["toolchain", "uninstall", "nightly"]
        );
    }

    #[test]
    fn lists_folders() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("toolchains/b")).unwrap();
        std::fs::create_dir_all(dir.path().join("toolchains/a")).unwrap();
        std::fs::write(dir.path().join("toolchains/stray.txt"), b"x").unwrap();
        let names: Vec<_> = installed(dir.path()).into_iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["a", "b"]);
        assert!(installed(&dir.path().join("missing")).is_empty());
    }
}
