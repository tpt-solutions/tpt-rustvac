# tpt-rustvac-core

The engine behind [rustvac](https://github.com/tpt-solutions/tpt-rustvac). No UI code.

- Streaming scan engine over a flat-arena data model
- Rust target discovery, honouring `CARGO_TARGET_DIR` and `.cargo/config.toml`
- General folder measurement, with hardlink dedup and size on disk on Windows
- `.tar.zst` archives with verification, and restore
- Safety checks: busy targets, protected roots, sync-managed paths, `keep` lists
- Config loading

This crate is used by `tpt-rustvac-cli` and `tpt-rustvac-gui`. Its API is not yet stable.

Licensed under MIT or Apache-2.0, at your option.
