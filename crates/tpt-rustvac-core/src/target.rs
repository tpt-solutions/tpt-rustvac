//! Resolves where a Cargo project's `target/` directory actually lives.
//!
//! Order: `CARGO_TARGET_DIR`, then `build.target-dir` in `<project>/.cargo/config.toml`,
//! then `~/.cargo/config.toml`, then `<project>/target`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Inputs that come from the process environment. Split out so tests can supply them.
#[derive(Debug, Clone, Default)]
pub struct TargetEnv {
    pub cargo_target_dir: Option<PathBuf>,
    pub home: Option<PathBuf>,
}

impl TargetEnv {
    pub fn from_process() -> Self {
        TargetEnv {
            cargo_target_dir: std::env::var_os("CARGO_TARGET_DIR")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            home: crate::config::home_dir(),
        }
    }
}

#[derive(Deserialize, Default)]
struct CargoConfig {
    #[serde(default)]
    build: BuildSection,
}

#[derive(Deserialize, Default)]
struct BuildSection {
    #[serde(rename = "target-dir")]
    target_dir: Option<String>,
}

pub fn resolve_target_dir(project: &Path, env: &TargetEnv) -> PathBuf {
    if let Some(dir) = &env.cargo_target_dir {
        return join_if_relative(project, dir);
    }
    let project_cfg = project.join(".cargo").join("config.toml");
    if let Some(dir) = config_target_dir(&project_cfg, project) {
        return dir;
    }
    if let Some(home) = &env.home {
        let home_cfg = home.join(".cargo").join("config.toml");
        if let Some(dir) = config_target_dir(&home_cfg, home) {
            return dir;
        }
    }
    project.join("target")
}

/// Reads `build.target-dir` from a cargo config file. Relative values are
/// resolved against the directory that contains `.cargo`, as cargo does.
fn config_target_dir(config_file: &Path, base: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(config_file).ok()?;
    let parsed: CargoConfig = toml::from_str(&text).ok()?;
    let value = parsed.build.target_dir?;
    Some(join_if_relative(base, Path::new(&value)))
}

fn join_if_relative(base: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// The compiler a target was built with, from `rustc -vV`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RustcInfo {
    /// For example `1.99.1` or `1.100.0-nightly`.
    pub release: String,
    pub host: Option<String>,
    pub commit: Option<String>,
}

impl RustcInfo {
    /// Same compiler: equal release, and equal commit when both sides know it. Nightlies share
    /// a release string across days, so the commit is what tells them apart.
    pub fn same_compiler(&self, other: &RustcInfo) -> bool {
        if self.release != other.release {
            return false;
        }
        match (&self.commit, &other.commit) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        }
    }

    /// Short text for tables: `1.99.1`.
    pub fn label(&self) -> &str {
        &self.release
    }
}

/// Parses the text of `rustc -vV`. Needs a `release:` line.
pub fn parse_rustc_vv(text: &str) -> Option<RustcInfo> {
    let mut release = None;
    let mut host = None;
    let mut commit = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("release:") {
            release = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("host:") {
            host = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("commit-hash:") {
            // Builds from source print "unknown".
            let v = v.trim();
            if !v.is_empty() && v != "unknown" {
                commit = Some(v.to_string());
            }
        }
    }
    release.map(|release| RustcInfo {
        release,
        host,
        commit,
    })
}

/// Parses the text of `.rustc_info.json`. The `outputs` map holds the stdout of several rustc
/// probes. One of them is `rustc -vV`, which is the one with a `release:` line.
pub fn parse_rustc_info(json: &str) -> Option<RustcInfo> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    value
        .get("outputs")?
        .as_object()?
        .values()
        .filter_map(|o| o.get("stdout")?.as_str())
        .find_map(parse_rustc_vv)
}

/// The compiler that last built `target_path`, read from `<target>/.rustc_info.json`.
/// `None` when the file is missing or unreadable, which callers must not treat as stale.
pub fn built_with(target_path: &Path) -> Option<RustcInfo> {
    let text = std::fs::read_to_string(target_path.join(".rustc_info.json")).ok()?;
    parse_rustc_info(&text)
}

/// True when the target has a known compiler and no installed one matches it.
pub fn is_stale(built: Option<&RustcInfo>, installed: &[RustcInfo]) -> bool {
    match built {
        Some(b) => !installed.iter().any(|i| i.same_compiler(b)),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Layout copied from a real file, with the cfg dump shortened.
    const SAMPLE: &str = r#"{"rustc_fingerprint":8815208633441915749,"outputs":{"16156137155129734780":{"success":true,"status":"","code":0,"stdout":"rustc 1.99.1 (58cf8f9c3 2026-10-13)\nbinary: rustc\ncommit-hash: 58cf8f9c35b355199727627d07ae34a4e2846d67\ncommit-date: 2026-10-13\nhost: x86_64-pc-windows-msvc\nrelease: 1.99.1\nLLVM version: 23.1.1\n","stderr":""},"7971740275564407648":{"success":true,"status":"","code":0,"stdout":"___.exe\nC:\\Users\\Phillip\\.rustup\\toolchains\\stable-x86_64-pc-windows-msvc\npacked\n","stderr":""}},"successes":{}}"#;

    #[test]
    fn reads_rustc_info() {
        let info = parse_rustc_info(SAMPLE).unwrap();
        assert_eq!(info.release, "1.99.1");
        assert_eq!(info.host.as_deref(), Some("x86_64-pc-windows-msvc"));
        assert_eq!(
            info.commit.as_deref(),
            Some("58cf8f9c35b355199727627d07ae34a4e2846d67")
        );
    }

    #[test]
    fn bad_input_is_none() {
        assert!(parse_rustc_info("not json").is_none());
        assert!(parse_rustc_info("{}").is_none());
        assert!(parse_rustc_info(r#"{"outputs":{"1":{"stdout":"nothing here"}}}"#).is_none());
    }

    #[test]
    fn unknown_commit_is_dropped() {
        let info = parse_rustc_vv("release: 1.80.0\ncommit-hash: unknown\n").unwrap();
        assert_eq!(info.commit, None);
    }

    #[test]
    fn compiler_matching() {
        let a = parse_rustc_vv("release: 1.99.0-nightly\ncommit-hash: aaa\n").unwrap();
        let b = parse_rustc_vv("release: 1.99.0-nightly\ncommit-hash: bbb\n").unwrap();
        let no_commit = parse_rustc_vv("release: 1.99.0-nightly\n").unwrap();
        assert!(a.same_compiler(&a));
        assert!(!a.same_compiler(&b));
        assert!(a.same_compiler(&no_commit));
    }

    #[test]
    fn staleness() {
        let built = parse_rustc_vv("release: 1.90.0\n").unwrap();
        let current = parse_rustc_vv("release: 1.99.1\n").unwrap();
        assert!(is_stale(Some(&built), std::slice::from_ref(&current)));
        assert!(!is_stale(Some(&built), &[current, built.clone()]));
        // Unknown compiler is never stale.
        assert!(!is_stale(None, &[]));
    }
}
