# Release checklist

1. Update the version in the workspace `Cargo.toml` and add a dated section to `CHANGELOG.md`.
2. `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets`, and `cargo test --workspace`.
3. `cargo deny check`. Licence rule: no Apache-2.0-only dependencies (see `deny.toml`).
4. Run the 5 GiB round trip: `cargo test --release -p tpt-rustvac-core --test large_roundtrip -- --ignored`.
5. `dist plan`, then check that LICENSE-MIT, LICENSE-APACHE and README.md are in every archive.
6. Tag `vX.Y.Z` and push. The `release.yml` workflow builds and uploads the artifacts.
7. Smoke-test one downloaded artifact per platform: `rustvac --version`, `rustvac scan --path <a folder>`.
