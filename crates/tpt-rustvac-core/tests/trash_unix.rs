//! A target moved to the trash must be restorable. Checked on Linux only: Windows was checked
//! by hand against the Recycle Bin, and this test would fill a developer's Recycle Bin, and
//! macOS has no listing API in the `trash` crate to check against.

#![cfg(all(unix, not(target_os = "macos")))]

use std::fs;

use tpt_rustvac_core::safety::move_to_trash;

#[test]
fn a_target_moved_to_the_trash_can_be_restored() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    fs::create_dir_all(target.join("debug")).unwrap();
    fs::write(target.join("debug").join("app.bin"), vec![7u8; 4096]).unwrap();

    move_to_trash(&target).unwrap();
    assert!(!target.exists(), "the original is gone after the move");

    let mine: Vec<_> = trash::os_limited::list()
        .unwrap()
        .into_iter()
        .filter(|item| item.original_path() == target)
        .collect();
    assert_eq!(mine.len(), 1, "the trash lists the moved target once");

    trash::os_limited::restore_all(mine).unwrap();
    assert_eq!(
        fs::read(target.join("debug").join("app.bin")).unwrap(),
        vec![7u8; 4096],
        "the restored file has the same bytes"
    );
}
