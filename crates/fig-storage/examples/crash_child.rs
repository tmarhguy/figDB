//! Crash-loop writer for the REAL kill -9 test.
//!
//! Writes sequential keys with `FsyncPolicy::Never` (nothing is durable unless
//! `sync()` says so), syncing every 1000 puts and recording the synced count
//! in `<dir>/SYNCED` (fsynced). The parent script `kill -9`s this process
//! mid-write; `verify_crash` then checks the acked prefix survived.
//!
//! Run via scripts/real_test.sh, or manually:
//! ```bash
//! rm -rf /tmp/fig-crash && cargo run -p fig-storage --example crash_child -- /tmp/fig-crash &
//! sleep 1 && kill -9 $! && cargo run -p fig-storage --example verify_crash -- /tmp/fig-crash
//! ```

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
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("wipe");
    }
    // Huge threshold: stay WAL-only so the test isolates WAL durability.
    let mut db =
        Database::open(&dir, Config::default(), FsyncPolicy::Never, 100_000_000).expect("open");
    let mut i = 0usize;
    loop {
        let k = format!("c{i:010}").into_bytes();
        let v = format!("v{i:010}").into_bytes();
        db.put(k, v).expect("put");
        i += 1;
        if i.is_multiple_of(1000) {
            db.sync().expect("sync");
            // Sidecar: the acked prefix length. fsync it so the parent can trust it.
            let p = dir.join("SYNCED");
            std::fs::write(&p, i.to_string()).expect("write SYNCED");
            let f = std::fs::File::open(&p).expect("open SYNCED");
            f.sync_all().expect("fsync SYNCED");
            if i.is_multiple_of(10_000) {
                println!("crash_child: {i} puts, {i} synced");
            }
        }
    }
}
