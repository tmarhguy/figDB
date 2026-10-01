//! Real proof the LSM is a database, not just passing unit tests.
//!
//! What this does that `cargo test` does not:
//! 1. Uses an INDEPENDENT oracle: `std::collections::HashMap`, not `ReferenceKv`.
//!    Same author wrote both sides of the differential tests, so agreement there
//!    could hide a shared misunderstanding. HashMap has no shared code.
//! 2. Works in a visible on-disk directory you can `ls`/`du` (default
//!    `./target/fig-real`). Data must survive `drop + reopen` in the same run
//!    AND a second process invocation (`verify` mode) after the first exits.
//! 3. Prints real numbers: puts/sec, gets/sec, flushes, tables, bytes on disk.
//!
//! Run:
//! ```bash
//! cargo run -p fig-storage --example real -- /tmp/fig-real 20000
//! ls -R /tmp/fig-real && du -sh /tmp/fig-real
//! cargo run -p fig-storage --example real -- /tmp/fig-real 20000 verify
//! ```

use fig_core::Config;
use fig_storage::Database;
use fig_wal::FsyncPolicy;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

const THRESHOLD: usize = 4096;

fn key(i: usize) -> Vec<u8> {
    format!("k{i:08}").into_bytes()
}

fn value(i: usize) -> Vec<u8> {
    // 16..48 bytes, deterministic from i so any process can recompute it.
    let pad = "x".repeat(16 + (i % 32));
    format!("v{i:08}-{pad}").into_bytes()
}

fn dir_bytes(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(p) = stack.pop() {
        let entries = match std::fs::read_dir(&p) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for ent in entries.flatten() {
            let path = ent.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(m) = ent.metadata() {
                total += m.len();
            }
        }
    }
    total
}

/// Simple deterministic PRNG (xorshift) so reads are reproducible without rand.
fn next_rand(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn fresh_run(dir: &Path, n: usize) {
    if dir.exists() {
        std::fs::remove_dir_all(dir).expect("wipe old dir");
    }
    let mut db = Database::open(dir, Config::default(), FsyncPolicy::Never, THRESHOLD)
        .expect("open fresh db");
    let mut oracle: HashMap<Vec<u8>, Vec<u8>> = HashMap::with_capacity(n);

    // Phase 1: bulk puts, batched sync every 500 (realistic batch-load policy).
    let t0 = Instant::now();
    for i in 0..n {
        let k = key(i);
        let v = value(i);
        db.put(k.clone(), v.clone()).expect("put");
        oracle.insert(k, v);
        if i % 500 == 499 {
            db.sync().expect("sync");
        }
    }
    db.sync().expect("final sync");
    let put_secs = t0.elapsed().as_secs_f64();
    println!(
        "PUT  {n} keys in {put_secs:.2}s = {:.0}/s (sync every 500, threshold {THRESHOLD})",
        n as f64 / put_secs
    );
    println!(
        "     flushes={} tables={}",
        db.metrics().flushes,
        db.table_count()
    );
    assert!(
        db.table_count() >= 1,
        "threshold {THRESHOLD} should have forced at least one flush over {n} keys"
    );

    // Phase 2: random gets vs HashMap.
    let t0 = Instant::now();
    let mut state = 0x0123_4567_89ab_cdefu64;
    let gets = n;
    for _ in 0..gets {
        let i = (next_rand(&mut state) % n as u64) as usize;
        let k = key(i);
        let got = db.get(&k).expect("get").expect("key must exist");
        assert_eq!(got, oracle[&k], "get mismatch at {i}");
    }
    let get_secs = t0.elapsed().as_secs_f64();
    println!(
        "GET  {gets} random reads in {get_secs:.2}s = {:.0}/s",
        gets as f64 / get_secs
    );

    // Phase 3: full scan vs sorted oracle.
    let t0 = Instant::now();
    let got = db.scan(b"k", b"l").expect("scan");
    let mut want: Vec<(Vec<u8>, Vec<u8>)> =
        oracle.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    want.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(got.len(), want.len(), "scan length mismatch");
    assert_eq!(got, want, "scan content mismatch");
    println!(
        "SCAN full table ({} pairs) in {:.2}s",
        got.len(),
        t0.elapsed().as_secs_f64()
    );

    // Phase 4: same-process restart — drop everything, reopen, re-verify.
    let flushes_before = db.metrics().flushes;
    drop(db);
    let db = Database::open(dir, Config::default(), FsyncPolicy::Never, THRESHOLD).expect("reopen");
    assert_eq!(
        db.metrics().flushes,
        0,
        "metrics restart per open (tables re-listed, counters fresh)"
    );
    let _ = flushes_before;
    for i in [0, 1, n / 2, n - 1] {
        let k = key(i);
        assert_eq!(
            db.get(&k).expect("get").as_deref(),
            Some(value(i).as_slice()),
            "lost key {i} across reopen"
        );
    }
    let got2 = db.scan(b"k", b"l").expect("scan after reopen");
    assert_eq!(got2, want, "data changed across reopen");
    println!("REOPEN same-process restart: all {n} keys + full scan intact");

    // Phase 5: deletes survive flush + reopen.
    drop(db);
    let mut db =
        Database::open(dir, Config::default(), FsyncPolicy::Never, THRESHOLD).expect("reopen 2");
    let mut deleted = 0;
    for i in (0..n).step_by(10) {
        db.delete(&key(i)).expect("delete");
        oracle.remove(&key(i));
        deleted += 1;
    }
    db.sync().expect("sync deletes");
    db.flush().expect("flush tombstones");
    drop(db);
    let db =
        Database::open(dir, Config::default(), FsyncPolicy::Never, THRESHOLD).expect("reopen 3");
    for i in (0..n).step_by(10) {
        assert_eq!(
            db.get(&key(i)).expect("get"),
            None,
            "deleted key {i} resurrected"
        );
    }
    for i in [1, 3, n / 2 + 1, n - 1] {
        assert_eq!(
            db.get(&key(i)).expect("get").as_deref(),
            Some(value(i).as_slice()),
            "live key {i} lost"
        );
    }
    let got3 = db.scan(b"k", b"l").expect("scan after deletes");
    let mut want3: Vec<(Vec<u8>, Vec<u8>)> =
        oracle.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    want3.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(got3, want3, "scan mismatch after deletes");
    println!("DELETE {deleted} keys + flush + reopen: tombstones hold, survivors intact");

    let bytes = dir_bytes(dir);
    println!(
        "DISK {} bytes on disk for {n} keys ({} deleted) = {:.1} bytes/key, tables={}",
        bytes,
        deleted,
        bytes as f64 / n as f64,
        db.table_count()
    );
    println!("PASS: fig-real fresh run ({n} keys) vs HashMap oracle");
}

fn verify_only(dir: &Path, n: usize) {
    // Second-process proof: this runs in a NEW process after the first exited.
    // Recomputes keys/values deterministically (no shared memory with fresh_run).
    let db = Database::open(dir, Config::default(), FsyncPolicy::Never, THRESHOLD)
        .expect("open existing db (did fresh run happen?)");
    let mut live = 0;
    for i in 0..n {
        let k = key(i);
        match db.get(&k).expect("get") {
            Some(v) => {
                assert_eq!(v, value(i), "value corruption at {i}");
                assert!(i % 10 != 0, "deleted key {i} came back after process exit");
                live += 1;
            }
            None => assert!(i % 10 == 0, "live key {i} missing after process exit"),
        }
    }
    println!(
        "VERIFY new process: {live}/{} live keys intact after process exit",
        n
    );
    println!("PASS: fig-real verify (cross-process durability)");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(
        args.get(1)
            .cloned()
            .unwrap_or("./target/fig-real".to_string()),
    );
    let n: usize = args
        .get(2)
        .map(|s| s.parse().expect("n must be usize"))
        .unwrap_or(20_000);
    let mode = args.get(3).cloned().unwrap_or("fresh".to_string());
    match mode.as_str() {
        "verify" => verify_only(&dir, n),
        _ => fresh_run(&dir, n),
    }
}
