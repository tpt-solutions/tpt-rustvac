//! Fixture generator shared by the integration tests and `examples/make_fixtures.rs`.
//!
//! Builds a small tree of fake Cargo projects with known sizes, so tests can assert exact numbers.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use tpt_rustvac_core::identity::disk_bytes;

/// Writes `size` bytes to `path`, and returns the bytes the file takes on disk. The scanner
/// reports that number, so tests compare against it. On Windows it can be smaller than `size`
/// (cloud placeholders, sparse files) or larger (cluster rounding for small files).
pub fn write_file(path: &Path, size: usize) -> u64 {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, vec![b'x'; size]).unwrap();
    disk_of(path)
}

/// The bytes an existing file takes on disk, as the scanner counts them.
pub fn disk_of(path: &Path) -> u64 {
    disk_bytes(path, &fs::metadata(path).unwrap())
}

/// Sizes the fixture tree was built with, so tests can compare exact totals.
pub struct RustFixture {
    pub root: PathBuf,
    /// (project dir, target dir, expected bytes, expected files)
    pub projects: Vec<(PathBuf, PathBuf, u64, u64)>,
    pub total_bytes: u64,
    pub total_files: u64,
}

/// Builds the Rust-target fixture under `root`:
/// - `api-server`: target/ with 4000 + 1000 bytes
/// - `web-app`: target/ with 2000 bytes
/// - `workspace`: root has target/ (700 bytes); member crate has none, so it is not reported
/// - `custom-dir`: `.cargo/config.toml` sets `build.target-dir = "out"` (300 bytes)
/// - `no-target`: Cargo.toml with no target dir, so it is not reported
/// - `notes`: not a project, holds a 5000-byte file that must never count
pub fn build_rust_fixture(root: &Path) -> RustFixture {
    let mut projects = Vec::new();

    let api = root.join("api-server");
    write_file(&api.join("Cargo.toml"), 40);
    let api_bytes = write_file(&api.join("target/debug/deps/lib.rlib"), 4000)
        + write_file(&api.join("target/release/app"), 1000);
    projects.push((api.clone(), api.join("target"), api_bytes, 2));

    let web = root.join("web-app");
    write_file(&web.join("Cargo.toml"), 40);
    let web_bytes = write_file(&web.join("target/debug/app"), 2000);
    projects.push((web.clone(), web.join("target"), web_bytes, 1));

    let ws = root.join("workspace");
    write_file(&ws.join("Cargo.toml"), 60);
    let ws_bytes = write_file(&ws.join("target/debug/ws.bin"), 700);
    write_file(&ws.join("crates/member/Cargo.toml"), 40);
    projects.push((ws.clone(), ws.join("target"), ws_bytes, 1));

    let custom = root.join("custom-dir");
    write_file(&custom.join("Cargo.toml"), 40);
    write_file(&custom.join(".cargo/config.toml"), 0);
    fs::write(
        custom.join(".cargo/config.toml"),
        "[build]\ntarget-dir = \"out\"\n",
    )
    .unwrap();
    let custom_bytes = write_file(&custom.join("out/x.bin"), 300);
    projects.push((custom.clone(), custom.join("out"), custom_bytes, 1));

    write_file(&root.join("no-target/Cargo.toml"), 40);

    write_file(&root.join("notes/readme.txt"), 5000);

    let total_bytes = projects.iter().map(|p| p.2).sum();
    let total_files = projects.iter().map(|p| p.3).sum();
    RustFixture {
        root: root.to_path_buf(),
        projects,
        total_bytes,
        total_files,
    }
}
