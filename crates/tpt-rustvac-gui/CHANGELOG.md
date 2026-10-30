# Changelog

All notable changes to this crate are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- A Disk health panel (`disk.refresh`): rustup, toolchain, cargo and WSL/Docker disk sizes with next steps. WSL disks have Reveal and the manual compact steps, and no delete button. Measured on a background thread.
- Tree view in a tpt-shell window with Rust Targets / Any Folder / Mixed modes, filters,
  keyboard shortcuts, and a two-step confirmation dialog. Windows only so far.
