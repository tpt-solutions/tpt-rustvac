//! Paths inside a sync folder, or behind a symlink or junction, must not be trashed or archived.
//! The OneDrive check depends on this machine's environment, so this test covers the reparse
//! point rule only. Windows only: other systems have no sync-folder rule.

#![cfg(windows)]

use std::fs;
use std::path::Path;
use std::process::Command;

use tpt_rustvac_core::safety::{move_to_trash, sync_managed};

#[test]
fn a_junction_is_refused_and_a_plain_folder_is_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real").join("target");
    fs::create_dir_all(&real).unwrap();
    fs::write(real.join("a.bin"), b"x").unwrap();
    let link = dir.path().join("link");

    let made = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&real)
        .status()
        .unwrap();
    if !made.success() {
        eprintln!("cannot create a junction here; skipping");
        return;
    }

    assert!(
        sync_managed(&real).is_none(),
        "a plain folder may be trashed"
    );
    assert!(sync_managed(&link).is_some(), "a junction is refused");
    assert!(
        sync_managed(&link.join("a.bin")).is_some(),
        "a file under a junction is refused"
    );

    // The guard runs before the trash call, so a refused junction is not touched.
    let refused = move_to_trash(&link);
    assert!(refused.is_err());
    assert!(Path::new(&link).exists(), "the junction is still there");
    assert!(real.join("a.bin").exists(), "the real folder is untouched");

    fs::remove_dir(&link).unwrap();
}
