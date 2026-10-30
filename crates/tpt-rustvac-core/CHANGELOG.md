# Changelog

All notable changes to this crate are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed
- On Windows a plain file's size comes from its length, rounded up to the cluster size, with no extra open. Only compressed, sparse, reparse, offline and recall-flagged files ask `GetCompressedFileSizeW`. Cluster size is read once per volume.
- Project discovery walks in parallel, not serially. On a 124-target tree of 306 GB and 861,000 files, a scan fell from 1,877 s to 18 s. Projects are sorted by path before their ids are given out, so ids are the same each run.
- Rust-target scans measure each target on its own worker, with no nested parallel walk. Each target's child folders are sent as soon as it is measured, and a project's rollup once its last target is done, so the tree fills in during the scan.
- `Arena::remove_subtree` takes a node out of the tree and fixes its ancestors' totals, without a rescan.

### Added
- `tests/invariants.rs`: cache planner enumerated over every kind, age and busy case; proptest checks for the sweep planner (roots, kept, busy, stale), the auto removal threshold, the disk report's regenerable total, the low-space alert, and pack/verify/restore round trips.
- `config.refresh_in_background` (default `false`), for the GUI's Rescan.
- `wsl` (find and size `ext4.vhdx` disks from the registry and `%LOCALAPPDATA%`, plus manual compact steps as text) and `disk` (one report over caches, toolchains and WSL). `CacheKind::RustupUpdateHashes`, `toolchains::last_used`, and a `wsl` field on `check::Recommendation` that older saved reports load without.
- Stale-dependency pruning (`prune::plan_stale_deps`): finds build units whose `.d` file names
  a registry crate version that is not in `Cargo.lock`. Removal takes every output of the unit.
  Included in `sweep` and `check`.
- Flat-arena data model, streaming scan engine, and Rust target discovery with
  `CARGO_TARGET_DIR` and `.cargo/config.toml` resolution.
- General folder measurement with hardlink dedup.
- `.tar.zst` archives with verification, restore, and config loading.
