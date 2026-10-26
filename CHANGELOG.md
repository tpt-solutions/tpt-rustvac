# Changelog

All notable changes are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- `tpt-rustvac-core`: flat-arena data model, streaming scan engine, Rust target discovery
  with `CARGO_TARGET_DIR` and `.cargo/config.toml` resolution, general folder measurement,
  hardlink dedup, `.tar.zst` archives with verification, restore, and config loading.
- `rustvac` CLI: `scan`, `list`, `measure`, `compress`, `restore`, `archive list|rm`, `gui`.
- `rustvac-gui`: tree view in a tpt-shell window, with Rust Targets / Any Folder / Mixed modes,
  filters, keyboard shortcuts, and a two-step confirmation dialog. Windows only so far.
- Release packaging through cargo-dist (Windows, macOS and Linux).
