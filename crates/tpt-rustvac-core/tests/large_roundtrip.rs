//! Round-trip a 5 GiB target through pack, verify and restore, then compare every byte.
//!
//! Ignored by default because it writes about 15 GiB of temporary data. Run with:
//! `cargo test --release -p tpt-rustvac-core --test large_roundtrip -- --ignored --nocapture`

use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::Path;

use tpt_rustvac_core::archive;

const SIZE: u64 = 5 * 1024 * 1024 * 1024;
const CHUNK: usize = 8 * 1024 * 1024;

/// Writes `size` bytes of xorshift output. It is effectively incompressible, so the
/// archive stays large and compression is actually exercised.
fn write_random(path: &Path, size: u64) {
    let mut out = File::create(path).unwrap();
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut buf = vec![0u8; CHUNK];
    let mut written = 0u64;
    while written < size {
        for chunk in buf.chunks_exact_mut(8) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            chunk.copy_from_slice(&state.to_le_bytes());
        }
        let n = CHUNK.min((size - written) as usize);
        out.write_all(&buf[..n]).unwrap();
        written += n as u64;
    }
    out.sync_all().unwrap();
}

/// True if both files have identical bytes. Compares in chunks, so memory stays flat.
fn files_identical(a: &Path, b: &Path) -> bool {
    let mut ra = BufReader::new(File::open(a).unwrap());
    let mut rb = BufReader::new(File::open(b).unwrap());
    let mut ba = vec![0u8; CHUNK];
    let mut bb = vec![0u8; CHUNK];
    loop {
        let na = ra.read(&mut ba).unwrap();
        if na == 0 {
            return rb.read(&mut bb).unwrap() == 0;
        }
        if rb.read_exact(&mut bb[..na]).is_err() || ba[..na] != bb[..na] {
            return false;
        }
    }
}

#[test]
#[ignore = "writes ~15 GiB of temporary data"]
fn round_trip_5gib() {
    let work = tempfile::tempdir().unwrap();
    let project = work.path().join("big-project");
    let target = project.join("target");
    fs::create_dir_all(target.join("debug")).unwrap();
    fs::write(project.join("Cargo.toml"), "[package]\nname = \"big\"\n").unwrap();
    let original = target.join("debug").join("big.rlib");

    let t0 = std::time::Instant::now();
    write_random(&original, SIZE);
    println!("wrote 5 GiB fixture in {:.1}s", t0.elapsed().as_secs_f64());

    let vacuum = work.path().join("vacuum");
    let t1 = std::time::Instant::now();
    let report = archive::pack(&target, &project, &vacuum).unwrap();
    println!(
        "packed in {:.1}s: {} bytes -> {} bytes",
        t1.elapsed().as_secs_f64(),
        report.manifest.bytes,
        report.compressed_bytes
    );
    assert_eq!(report.manifest.files, 1);
    assert_eq!(report.manifest.bytes, SIZE);

    let t2 = std::time::Instant::now();
    let verified = archive::verify(&report.archive).unwrap();
    println!("verified in {:.1}s", t2.elapsed().as_secs_f64());
    assert_eq!(verified, report.manifest);

    // Pretend the original is gone, as after a trash step, and restore into a fresh folder.
    fs::remove_file(&original).unwrap();
    let restore_into = work.path().join("restored");
    let t3 = std::time::Instant::now();
    let restored = archive::restore(&report.archive, &restore_into).unwrap();
    println!("restored in {:.1}s", t3.elapsed().as_secs_f64());

    let restored_file = restored.join("debug").join("big.rlib");
    assert_eq!(fs::metadata(&restored_file).unwrap().len(), SIZE);
    // Compare against the original's content, which we just deleted, by regenerating it.
    let reference = work.path().join("reference.bin");
    write_random(&reference, SIZE);
    assert!(
        files_identical(&reference, &restored_file),
        "restored bytes differ from the original"
    );
    println!("round trip OK: 5 GiB identical after pack, verify and restore");
}
