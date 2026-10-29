# Changelog

All notable changes to this crate are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Stale-dependency pruning (`prune::plan_stale_deps`): finds build units whose `.d` file names
  a registry crate version that is not in `Cargo.lock`. Removal takes every output of the unit.
  Included in `sweep` and `check`.
- Flat-arena data model, streaming scan engine, and Rust target discovery with
  `CARGO_TARGET_DIR` and `.cargo/config.toml` resolution.
- General folder measurement with hardlink dedup.
- `.tar.zst` archives with verification, restore, and config loading.
