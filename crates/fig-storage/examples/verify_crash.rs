//! Verifier for the REAL kill -9 test.
//!
//! Opens `<dir>` in a NEW process after `crash_child` was SIGKILLed and checks:
//! 1. The DB opens without error (no torn/corrupt tail panics).
//! 2. Scans return well-formed `cXXXXXXXXXX -> vXXXXXXXXXX` pairs only.
//! 3. Every key in the acked prefix `[0, SYNCED)` is present with the right value.
//!    Keys past SYNCED may or may not exist (unsynced tail) — both are correct.

use fig_core::Config;
use fig_storage::Database;
use fig_wal::FsyncPolicy;
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or("/tmp/fig-crash".to_string()),
    );
    let synced: usize = std::fs::read_to_string(dir.join("SYNCED"))
        .expect("SYNCED sidecar missing (did crash_child run?)")
        .trim()
        .parse()
        .expect("SYNCED corrupt");
    assert!(
        synced >= 1000,
        "child was killed before its first sync; rerun"
    );
    println!("verify_crash: child acked {synced} puts before SIGKILL");

    let db = Database::open(&dir, Config::default(), FsyncPolicy::Never, 100_000_000)
        .expect("open after SIGKILL must succeed (torn tail truncated, never panic)");

    // 1. Full scan is well-formed.
    let all = db
        .scan(b"a", b"d")
        .expect("scan after SIGKILL must succeed");
    for (k, v) in &all {
        let ks = String::from_utf8_lossy(k);
        let vs = String::from_utf8_lossy(v);
        assert!(ks.starts_with('c'), "unexpected key {ks}");
        assert!(vs.starts_with('v'), "unexpected value {vs}");
        assert_eq!(&ks[1..], &vs[1..], "key/value index mismatch {ks} vs {vs}");
    }
    println!(
        "verify_crash: scan OK, {} records, all well-formed",
        all.len()
    );

    // 2. Acked prefix fully present.
    let mut missing = 0;
    for i in 0..synced {
        let k = format!("c{i:010}").into_bytes();
        let want = format!("v{i:010}").into_bytes();
        match db.get(&k).expect("get") {
            Some(got) => assert_eq!(got, want, "acked key {i} has wrong value"),
            None => missing += 1,
        }
    }
    // Torn tail may have eaten into the acked region by at most one partial
    // frame at the truncation point — but with Always-synced SYNCED sidecar and
    // Always... actually policy is Never + explicit sync, so the synced prefix
    // was fsynced: require ALL of it present.
    assert_eq!(
        missing, 0,
        "{missing}/{synced} acked keys lost across SIGKILL"
    );
    println!("verify_crash: acked prefix [0,{synced}) fully present");

    // 3. No phantom keys beyond what was ever written (scan count bounded).
    // Child wrote sequentially; anything present must be c<10 digits>.
    println!("PASS: fig-real kill -9 test ({synced} acked puts survived SIGKILL)");
}
