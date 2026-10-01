//! Compaction gate: merging tables never changes logical state, reclaims
//! space, bounds fan-out, and survives a SIGKILL at each publish window.
//!
//! The oracle is `std::collections::HashMap` — no shared code with figDB.

use fig_core::Config;
use fig_sstable::{SstableReader, SstableWriter};
use fig_storage::{Database, Manifest};
use fig_wal::FsyncPolicy;
use std::collections::{BTreeMap, HashMap};

const THRESHOLD: usize = 256;

fn open(dir: &std::path::Path) -> Database {
    Database::open(dir, Config::default(), FsyncPolicy::Always, THRESHOLD).unwrap()
}

fn table_dir(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("tables")
}

fn db_scan(db: &Database) -> Vec<(Vec<u8>, Vec<u8>)> {
    db.scan(&[0x00], &[0xff, 0xff]).unwrap()
}

fn oracle_scan(oracle: &HashMap<Vec<u8>, Vec<u8>>) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut v: Vec<_> = oracle.iter().map(|(k, x)| (k.clone(), x.clone())).collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

fn next_rand(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

#[test]
fn compact_interleaved_with_writes_matches_oracle() {
    for seed in [11u64, 22, 33] {
        let dir = tempfile::tempdir().unwrap();
        let mut db = open(dir.path());
        let mut oracle: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).max(1);
        for step in 0..800 {
            let r = next_rand(&mut state) % 100;
            let k = vec![
                (next_rand(&mut state) % 64) as u8,
                (next_rand(&mut state) % 64) as u8,
            ];
            if r < 50 {
                let v = vec![(next_rand(&mut state) % 251) as u8; 1 + (step % 8)];
                db.put(k.clone(), v.clone()).unwrap();
                oracle.insert(k, v);
            } else if r < 65 {
                db.delete(&k).unwrap();
                oracle.remove(&k);
            } else if r < 80 {
                assert_eq!(
                    db.get(&k).unwrap(),
                    oracle.get(&k).cloned(),
                    "seed {seed} step {step}: get diverged"
                );
            } else if r < 90 {
                // Manual compact at random points, incl. nearly-empty stacks.
                db.compact().unwrap();
                assert_eq!(
                    db_scan(&db),
                    oracle_scan(&oracle),
                    "seed {seed} step {step}: state changed by compact"
                );
            } else {
                // Clean restart: everything is Always-synced, so all of it
                // must survive.
                drop(db);
                db = open(dir.path());
                assert_eq!(
                    db_scan(&db),
                    oracle_scan(&oracle),
                    "seed {seed} step {step}: state lost across reopen"
                );
            }
        }
        db.compact().unwrap();
        assert_eq!(db_scan(&db), oracle_scan(&oracle));
    }
}

#[test]
fn full_compact_gc_drops_tombstones_and_reclaims_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = open(dir.path());
    for i in 0..100u8 {
        db.put(vec![i], vec![i, i, i]).unwrap();
    }
    db.flush().unwrap();
    for i in 0..100u8 {
        db.delete(&[i]).unwrap();
    }
    db.flush().unwrap();
    let tables_before = db.table_count();
    assert!(tables_before >= 2);
    let bytes_before: u64 = std::fs::read_dir(table_dir(dir.path()))
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    assert!(
        db.compact().unwrap(),
        "merging {tables_before} tables must run"
    );
    // Everything deleted and merged: no live keys, no tables left at all.
    assert_eq!(db_scan(&db), Vec::new());
    assert_eq!(db.table_count(), 0, "GC must leave zero tables behind");
    let bytes_after: u64 = std::fs::read_dir(table_dir(dir.path()))
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    assert!(
        bytes_after < bytes_before,
        "compaction must reclaim bytes ({bytes_after} >= {bytes_before})"
    );
    // And it holds across reopen (manifest + empty stack round-trip).
    drop(db);
    let db = open(dir.path());
    assert_eq!(db_scan(&db), Vec::new());
    assert_eq!(db.table_count(), 0);
    assert_eq!(db.get(&[7]).unwrap(), None);
}

#[test]
fn auto_compact_bounds_table_count() {
    // Without compaction this load leaves dozens of tables; the policy must
    // fold them as they accumulate while staying oracle-correct.
    let dir = tempfile::tempdir().unwrap();
    let mut db = open(dir.path());
    let mut oracle: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();
    for i in 0..600usize {
        let k = vec![(i % 256) as u8, (i / 256) as u8];
        let v = vec![(i % 251) as u8; 4];
        db.put(k.clone(), v.clone()).unwrap();
        oracle.insert(k, v);
    }
    assert!(
        db.table_count() <= 8,
        "auto-compaction must bound fan-out, got {} tables",
        db.table_count()
    );
    assert!(
        db.metrics().compactions >= 1,
        "expected auto-compactions to fire"
    );
    assert_eq!(db_scan(&db), oracle_scan(&oracle));
    drop(db);
    let db = open(dir.path());
    assert!(db.table_count() <= 8);
    assert_eq!(db_scan(&db), oracle_scan(&oracle));
}

#[test]
fn disabled_auto_compact_grows_until_background_step_runs() {
    // Server mode contract: with inline compaction off, the write path only
    // appends; explicit background steps fold the stack. Same oracle rules.
    use std::collections::HashMap;
    let dir = tempfile::tempdir().unwrap();
    let mut db = open(dir.path());
    db.set_auto_compact(false);
    let mut oracle: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();
    for i in 0..400usize {
        let k = vec![(i % 128) as u8, (i / 128) as u8];
        let v = vec![(i % 251) as u8; 4];
        db.put(k.clone(), v.clone()).unwrap();
        oracle.insert(k, v);
    }
    assert!(
        db.table_count() > 8,
        "inline compaction off: stack must grow unbounded, got {}",
        db.table_count()
    );
    assert_eq!(db.metrics().compactions, 0);
    while db.background_compact().unwrap() {}
    assert!(
        db.table_count() <= 8,
        "background steps must fold the stack, got {}",
        db.table_count()
    );
    let mut want: Vec<_> = oracle.into_iter().collect();
    want.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(db_scan(&db), want);
}

#[test]
fn crash_after_output_publish_but_before_manifest_keeps_inputs() {
    // Crash window A: merged output renamed to final, manifest never swapped.
    // Open must reap the orphan and serve the inputs untouched.
    let dir = tempfile::tempdir().unwrap();
    let mut db = open(dir.path());
    for i in 0..20u8 {
        db.put(vec![i], vec![i]).unwrap();
    }
    db.flush().unwrap();
    for i in 20..40u8 {
        db.put(vec![i], vec![i]).unwrap();
    }
    db.flush().unwrap();
    assert_eq!(db.table_count(), 2);
    drop(db);
    // Forge the orphan output (id 2, unlisted).
    let tables = table_dir(dir.path());
    let mut w = SstableWriter::create(&tables.join("sst-000002.sst"), 1024).unwrap();
    w.put(b"zz-evil", b"x").unwrap();
    w.finish().unwrap();
    let db = open(dir.path());
    assert_eq!(db.table_count(), 2, "orphan output must not join the stack");
    assert_eq!(db.get(b"zz-evil").unwrap(), None);
    for i in 0..40u8 {
        assert_eq!(db.get(&[i]).unwrap(), Some(vec![i]));
    }
    assert!(!tables.join("sst-000002.sst").exists(), "orphan reaped");
}

#[test]
fn crash_after_manifest_swap_but_before_input_deletion_completes() {
    // Crash window B: manifest lists the merged output, input files still on
    // disk. Open must finish the swap: serve the output, delete the inputs.
    let dir = tempfile::tempdir().unwrap();
    let mut db = open(dir.path());
    for i in 0..20u8 {
        db.put(vec![i], vec![i]).unwrap();
    }
    db.flush().unwrap(); // table 0
    for i in 20..40u8 {
        db.put(vec![i], vec![i]).unwrap();
    }
    db.flush().unwrap(); // table 1
    drop(db);
    // Reproduce exactly what merge_tables does: merge inputs oldest→newest,
    // publish output id 2, then swap the manifest — but stop before deleting
    // inputs 0 and 1.
    let tables = table_dir(dir.path());
    let mut merged: BTreeMap<Vec<u8>, Option<Vec<u8>>> = BTreeMap::new();
    for id in [0u64, 1] {
        let path = tables.join(format!("sst-{id:06}.sst"));
        for rec in SstableReader::open(&path).unwrap().iter().unwrap() {
            merged.insert(rec.key, rec.value);
        }
    }
    let out_tmp = tables.join("sst-000002.sst.tmp");
    let out_final = tables.join("sst-000002.sst");
    let mut w = SstableWriter::create(&out_tmp, 1024).unwrap();
    for (k, v) in &merged {
        match v {
            Some(x) => w.put(k, x).unwrap(),
            None => w.delete(k).unwrap(),
        }
    }
    w.finish().unwrap();
    std::fs::rename(&out_tmp, &out_final).unwrap();
    let bytes = serde_json::to_vec(&Manifest::new(3, vec![2])).unwrap();
    std::fs::write(tables.join("MANIFEST.tmp"), &bytes).unwrap();
    std::fs::rename(tables.join("MANIFEST.tmp"), tables.join("MANIFEST")).unwrap();
    // Crash lands here: inputs still present, manifest already swapped.
    let db = open(dir.path());
    assert_eq!(db.table_count(), 1, "inputs must be reaped, output kept");
    for i in 0..40u8 {
        assert_eq!(db.get(&[i]).unwrap(), Some(vec![i]), "merged data intact");
    }
    assert!(!tables.join("sst-000000.sst").exists());
    assert!(!tables.join("sst-000001.sst").exists());
    assert!(out_final.exists());
}
