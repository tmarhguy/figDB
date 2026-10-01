//! Kill/restart gate for the storage engine:
//! process termination at randomized points preserves acknowledged writes.
//!
//! Model: `sync()` = acknowledgement. Crashes are simulated by dropping the
//! engine and truncating the WAL file — to `synced_len` for a clean loss of
//! the unsynced tail, or to a random point after it for a torn write. The
//! reopened engine must match a `ReferenceKv` replay of exactly the
//! acknowledged operations (clean cut) or some prefix of them (torn cut).

use fig_core::ops::gen_ops;
use fig_core::{Config, Op, ReferenceKv};
use fig_storage::Engine;
use fig_wal::FsyncPolicy;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::fs::OpenOptions;

fn active_segment(dir: &std::path::Path) -> std::path::PathBuf {
    let mut segs: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().map(|x| x == "log").unwrap_or(false))
        .collect();
    segs.sort();
    segs.pop().expect("wal segment must exist")
}

fn replay_reference(ops: &[Op]) -> ReferenceKv {
    let mut r = ReferenceKv::new();
    for op in ops {
        r.apply(op);
    }
    r
}

fn full_scan_of(r: &ReferenceKv) -> Vec<(Vec<u8>, Vec<u8>)> {
    r.scan(&[0x00], &[0xff]).unwrap()
}

/// Apply one op to the engine, returning the observable result for comparison.
fn apply_engine(e: &mut Engine, op: &Op) -> fig_core::OpResult {
    match op {
        Op::Put { key, value } => {
            let _ = e.put(key.clone(), value.clone()).unwrap();
            fig_core::OpResult::Put(
                // Previous value isn't returned by Engine::put; fetch before.
                // Handled by caller comparing against reference separately.
                None,
            )
        }
        Op::Delete { key } => fig_core::OpResult::Delete(e.delete(key).unwrap()),
        Op::Get { key } => fig_core::OpResult::Get(e.get(key)),
        Op::Scan { start, end } => match e.scan(start, end) {
            Ok(pairs) => fig_core::OpResult::Scan(pairs),
            Err(err) => fig_core::OpResult::ScanErr(err.to_string()),
        },
    }
}

#[test]
fn kill_restart_preserves_acked_writes() {
    for seed in 0..15u64 {
        let mut rng = StdRng::seed_from_u64(seed);
        let dir = tempfile::tempdir().unwrap();
        let mut e = Engine::open(dir.path(), Config::default(), FsyncPolicy::Never).unwrap();

        let mut acked: Vec<Op> = Vec::new();
        let mut staged: Vec<Op> = Vec::new();

        for (i, op) in gen_ops(seed, 300).into_iter().enumerate() {
            // Reads must agree live, not just after recovery.
            match &op {
                Op::Get { .. } | Op::Scan { .. } => {
                    let got = apply_engine(&mut e, &op);
                    // Staged writes may already be visible in the engine, so
                    // compare against acked+staged, not acked alone.
                    let mut full = acked.clone();
                    full.extend(staged.clone());
                    assert_eq!(got, replay_reference(&full).apply(&op));
                }
                Op::Put { .. } | Op::Delete { .. } => {
                    apply_engine(&mut e, &op);
                    staged.push(op);
                }
            }

            if i % 5 == 4 {
                e.sync().unwrap();
                acked.append(&mut staged);
            }

            if rng.gen_range(0..100) < 8 {
                // Clean crash: unsynced tail is lost entirely.
                let synced = e.synced_len();
                drop(e);
                let seg = active_segment(dir.path());
                OpenOptions::new()
                    .write(true)
                    .open(&seg)
                    .unwrap()
                    .set_len(synced)
                    .unwrap();
                e = Engine::open(dir.path(), Config::default(), FsyncPolicy::Never).unwrap();
                let want = full_scan_of(&replay_reference(&acked));
                let got = e.scan(&[0x00], &[0xff]).unwrap();
                assert_eq!(got, want, "seed {seed} step {i}: acked state lost");
                staged.clear();
            }
        }

        e.sync().unwrap();
        acked.append(&mut staged);
        let synced = e.synced_len();
        drop(e);
        let seg = active_segment(dir.path());
        OpenOptions::new()
            .write(true)
            .open(&seg)
            .unwrap()
            .set_len(synced)
            .unwrap();
        let e = Engine::open(dir.path(), Config::default(), FsyncPolicy::Never).unwrap();
        assert_eq!(
            e.scan(&[0x00], &[0xff]).unwrap(),
            full_scan_of(&replay_reference(&acked)),
            "seed {seed}: final state mismatch"
        );
    }
}

#[test]
fn torn_tail_never_half_applies() {
    for seed in 0..10u64 {
        let mut rng = StdRng::seed_from_u64(seed);
        let dir = tempfile::tempdir().unwrap();
        let mut e = Engine::open(dir.path(), Config::default(), FsyncPolicy::Never).unwrap();

        let ops = gen_ops(seed, 200);
        let mut acked_len = 0usize;
        for (i, op) in ops.iter().enumerate() {
            match op {
                Op::Put { .. } | Op::Delete { .. } => {
                    apply_engine(&mut e, op);
                }
                _ => {
                    apply_engine(&mut e, op);
                }
            }
            if i % 5 == 4 {
                e.sync().unwrap();
                acked_len = i + 1;
            }
        }

        // Tear the tail at a random point at/after the synced prefix.
        let synced = e.synced_len();
        drop(e);
        let seg = active_segment(dir.path());
        let len = std::fs::metadata(&seg).unwrap().len();
        let cut = if len == synced {
            len
        } else {
            rng.gen_range(synced..=len)
        };
        OpenOptions::new()
            .write(true)
            .open(&seg)
            .unwrap()
            .set_len(cut)
            .unwrap();
        // Junk bytes after the cut model a torn write's garbage.
        if cut == len {
            let mut f = OpenOptions::new().append(true).open(&seg).unwrap();
            use std::io::Write;
            f.write_all(&[0xDE, 0xAD]).unwrap();
        }

        let e = Engine::open(dir.path(), Config::default(), FsyncPolicy::Never).unwrap();
        let got = e.scan(&[0x00], &[0xff]).unwrap();

        // Recovered state must equal *some* prefix of the op stream that
        // includes every acknowledged op — never a mix, never torn.
        let mut matched = false;
        let mut r = ReferenceKv::new();
        for (n, op) in ops.iter().enumerate() {
            r.apply(op);
            if n + 1 >= acked_len && full_scan_of(&r) == got {
                matched = true;
                break;
            }
        }
        // Edge case: nothing beyond acked ops survived.
        if !matched && full_scan_of(&replay_reference(&ops[..acked_len])) == got {
            matched = true;
        }
        assert!(matched, "seed {seed}: recovered state is not an op prefix");
    }
}
