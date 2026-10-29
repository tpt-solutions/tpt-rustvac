# Changelog

All notable changes to this crate are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- `rustvac` commands: `scan`, `list`, `measure`, `compress`, `restore`, `archive list|rm`, `gui`.
- `rustvac prune --stale-deps` removes build output for dependency versions the project's
  `Cargo.lock` no longer names, and leaves the live build in place.
