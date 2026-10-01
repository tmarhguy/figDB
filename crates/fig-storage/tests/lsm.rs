//! Equivalence gate for the LSM: large randomized workloads with flushes,
//! kills, and restarts always match the oracle.
//!
//! The workload uses a wide key space (2-byte keys) so the live set actually
//! grows and the flush threshold fires repeatedly — an 8-key workload would
//! fit in memory forever and never test flushing at all.
//!
//! Accounting is exact. The harness keeps the true op history `all` and a
//! durable prefix length `durable_len` (an op is durable iff synced or
//! flushed — a flush syncs its table file, so everything before a flush is
//! durable too). A clean crash recovers exactly `all[..durable_len]`; a torn
//! crash recovers `all[..n]` for some `n >= durable_len`. After a kill the
//! history truncates to what survived, and the run continues.

use fig_core::{Config, Op, OpResult, ReferenceKv};
use fig_storage::Database;
use fig_wal::FsyncPolicy;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::fs::OpenOptions;

const THRESHOLD: usize = 1024;

fn gen_key(rng: &mut StdRng) -> Vec<u8> {
    vec![rng.gen::<u8>(), rng.gen::<u8>()]
}

/// Wide-key operation stream: same mix as the core oracle streams, but over
/// a 65k key space with 1–8 byte values so flushes actually trigger.
fn gen_lsm_ops(seed: u64, n: usize) -> Vec<Op> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut ops = Vec::with_capacity(n);
    for _ in 0..n {
        let kind: u8 = rng.gen_range(0..100);
        if kind < 45 {
            let len = rng.gen_range(1..=8);
            ops.push(Op::Put {
                key: gen_key(&mut rng),
                value: (0..len).map(|_| rng.gen::<u8>()).collect(),
            });
        } else if kind < 65 {
            ops.push(Op::Delete {
                key: gen_key(&mut rng),
            });
        } else if kind < 85 {
            ops.push(Op::Get {
                key: gen_key(&mut rng),
            });
        } else {
            let mut a = gen_key(&mut rng);
            let mut b = gen_key(&mut rng);
            if a == b {
                continue;
            }
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            ops.push(Op::Scan { start: a, end: b });
        }
    }
    ops
}

fn active_segment(dir: &std::path::Path) -> std::path::PathBuf {
    let wal = dir.join("wal");
    let mut segs: Vec<_> = std::fs::read_dir(&wal)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().map(|x| x == "log").unwrap_or(false))
        .collect();
    segs.sort();
    segs.pop().expect("wal segment must exist")
}

fn replay(ops: &[Op]) -> ReferenceKv {
    let mut r = ReferenceKv::new();
    for op in ops {
        r.apply(op);
    }
    r
}

fn full_scan(r: &ReferenceKv) -> Vec<(Vec<u8>, Vec<u8>)> {
    r.scan(&[0x00, 0x00], &[0xff, 0xff]).unwrap()
}

fn db_scan(db: &Database) -> Vec<(Vec<u8>, Vec<u8>)> {
    db.scan(&[0x00, 0x00], &[0xff, 0xff]).unwrap()
}

/// Kill the database: drop it (memory lost), cut the WAL file, reopen.
fn kill(
    dir: &std::path::Path,
    db: Database,
    policy: FsyncPolicy,
    torn: bool,
    rng: &mut StdRng,
) -> Database {
    let synced = db.synced_len();
    drop(db);
    let seg = active_segment(dir);
    let len = std::fs::metadata(&seg).unwrap().len();
    let cut = if torn && len > synced {
        rng.gen_range(synced..=len)
    } else {
        synced
    };
    OpenOptions::new()
        .write(true)
        .open(&seg)
        .unwrap()
        .set_len(cut)
        .unwrap();
    Database::open(dir, Config::default(), policy, THRESHOLD).unwrap()
}

fn read_db(db: &Database, op: &Op) -> OpResult {
    match op {
        Op::Get { key } => OpResult::Get(db.get(key).unwrap()),
        Op::Scan { start, end } => match db.scan(start, end) {
            Ok(pairs) => OpResult::Scan(pairs),
            Err(e) => OpResult::ScanErr(e.to_string()),
        },
        _ => panic!("read_db called on write op"),
    }
}

fn apply_write(db: &mut Database, op: &Op) {
    match op {
        Op::Put { key, value } => db.put(key.clone(), value.clone()).unwrap(),
        Op::Delete { key } => {
            db.delete(key).unwrap();
        }
        _ => panic!("apply_write called on read op"),
    }
}

#[test]
fn always_policy_recovers_everything() {
    for seed in 0..6u64 {
        let dir = tempfile::tempdir().unwrap();
        let mut rng = StdRng::seed_from_u64(seed);
        let mut db = Database::open(
            dir.path(),
            Config::default(),
            FsyncPolicy::Always,
            THRESHOLD,
        )
        .unwrap();
        let mut all: Vec<Op> = Vec::new();

        for op in gen_lsm_ops(seed, 600) {
            match &op {
                Op::Put { .. } | Op::Delete { .. } => apply_write(&mut db, &op),
                Op::Get { .. } | Op::Scan { .. } => {
                    assert_eq!(read_db(&db, &op), replay(&all).apply(&op));
                }
            }
            all.push(op);

            if rng.gen_range(0..100) < 6 {
                db = kill(dir.path(), db, FsyncPolicy::Always, false, &mut rng);
                assert_eq!(
                    db_scan(&db),
                    full_scan(&replay(&all)),
                    "seed {seed}: state lost across kill"
                );
            }
        }
        assert_eq!(db_scan(&db), full_scan(&replay(&all)));
        assert!(
            db.table_count() >= 1,
            "seed {seed}: expected flushed tables"
        );
    }
}

#[test]
fn manual_sync_recovers_the_durable_prefix() {
    for seed in 0..5u64 {
        let dir = tempfile::tempdir().unwrap();
        let mut rng = StdRng::seed_from_u64(seed ^ 0x5EED);
        let mut db =
            Database::open(dir.path(), Config::default(), FsyncPolicy::Never, THRESHOLD).unwrap();

        let mut all: Vec<Op> = Vec::new();
        let mut durable_len = 0usize;
        let mut seen_flushes = 0u64;

        let ops = gen_lsm_ops(seed, 600);
        for (i, op) in ops.iter().enumerate() {
            match op {
                Op::Put { .. } | Op::Delete { .. } => {
                    apply_write(&mut db, op);
                    all.push(op.clone());
                }
                Op::Get { .. } | Op::Scan { .. } => {
                    assert_eq!(
                        read_db(&db, op),
                        replay(&all).apply(op),
                        "seed {seed} step {i}: live read diverged"
                    );
                }
            }

            if i % 7 == 6 {
                db.sync().unwrap();
                durable_len = all.len();
            }
            // A flush syncs its table: everything so far is durable.
            if db.metrics().flushes > seen_flushes {
                seen_flushes = db.metrics().flushes;
                durable_len = all.len();
            }

            if rng.gen_range(0..100) < 6 {
                let torn = rng.gen_bool(0.3);
                db = kill(dir.path(), db, FsyncPolicy::Never, torn, &mut rng);
                seen_flushes = db.metrics().flushes;
                let got = db_scan(&db);
                // Find the surviving history length: some prefix >= durable.
                let mut survived = None;
                let mut r = ReferenceKv::new();
                for (n, op) in all.iter().enumerate() {
                    r.apply(op);
                    if n + 1 >= durable_len && full_scan(&r) == got {
                        survived = Some(n + 1);
                        break;
                    }
                }
                let n = survived.unwrap_or_else(|| {
                    panic!("seed {seed} step {i}: recovered state is not an op prefix")
                });
                all.truncate(n);
                durable_len = durable_len.min(n);
            }
        }

        db.sync().unwrap();
        assert_eq!(db_scan(&db), full_scan(&replay(&all)));
        assert!(
            db.table_count() >= 1,
            "seed {seed}: expected flushed tables"
        );
    }
}
