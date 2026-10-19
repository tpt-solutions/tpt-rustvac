//! Writes the fixture tree to a directory for manual testing:
//! `cargo run -p tpt-rustvac-core --example make_fixtures -- fixtures/generated`

#[path = "../tests/support/mod.rs"]
mod support;

fn main() {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "fixtures/generated".into());
    let fixture = support::build_rust_fixture(std::path::Path::new(&dir));
    println!(
        "wrote {} projects ({} bytes, {} files) under {}",
        fixture.projects.len(),
        fixture.total_bytes,
        fixture.total_files,
        fixture.root.display()
    );
}
