//! Stale-dependency pruning end to end. Only build output for versions the lockfile no longer
//! names is removed, and the live build is left in place.

use std::fs;
use std::path::Path;

use tpt_rustvac_core::config::{RemoveMode, RemovePolicy};
use tpt_rustvac_core::prune;
use tpt_rustvac_core::safety::remove_group_with_policy;

const REGISTRY: &str = "/home/me/.cargo/registry/src/index.crates.io-6f17";

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn only_old_versions_are_removed_from_a_project() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("app");
    let target = project.join("target");
    write(
        &project.join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.2\"\n",
    );

    let deps = target.join("debug/deps");
    // serde 1.0.1 is no longer in the lockfile: its unit and fingerprint should go.
    write(
        &deps.join("serde-aaaa.d"),
        &format!("x : {REGISTRY}/serde-1.0.1/src/lib.rs\n"),
    );
    write(&deps.join("libserde-aaaa.rlib"), "old");
    write(&target.join("debug/.fingerprint/serde-aaaa/json"), "{}");
    // serde 1.0.2 is live: its unit must stay.
    write(
        &deps.join("serde-bbbb.d"),
        &format!("x : {REGISTRY}/serde-1.0.2/src/lib.rs\n"),
    );
    write(&deps.join("libserde-bbbb.rlib"), "new");
    // A workspace crate has no registry source, so it is never stale.
    write(&deps.join("app-cccc.d"), "x : /home/me/app/src/main.rs\n");

    let live = prune::live_crate_dirs(&project, &target).unwrap();
    let items = prune::plan_stale_deps(&target, &live);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].members.len(), 3);

    let policy = RemovePolicy {
        mode: RemoveMode::Permanent,
        trash_max: 0,
    };
    for item in &items {
        remove_group_with_policy(&item.members, item.size, &policy).unwrap();
    }

    assert!(!deps.join("serde-aaaa.d").exists());
    assert!(!deps.join("libserde-aaaa.rlib").exists());
    assert!(!target.join("debug/.fingerprint/serde-aaaa").exists());
    assert!(deps.join("serde-bbbb.d").exists());
    assert!(deps.join("libserde-bbbb.rlib").exists());
    assert!(target.join("debug/.fingerprint").exists());
    assert!(deps.join("app-cccc.d").exists());

    // Nothing is left to find on a second run.
    assert!(prune::plan_stale_deps(&target, &live).is_empty());
}

#[test]
fn a_target_outside_its_project_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("app");
    write(&project.join("Cargo.lock"), "version = 4\n");
    let shared = dir.path().join("shared-target");
    fs::create_dir_all(&shared).unwrap();
    assert!(prune::live_crate_dirs(&project, &shared).is_err());
}
