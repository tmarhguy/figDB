//! Differential gate for SSTables: whatever goes in sorted must come back
//! identical, matching `ReferenceKv` on gets, scans, and tombstones.
//!
//! Each seed builds a random write stream, folds it to the last write per
//! key, sorts ascending (the writer's contract), and compares every read
//! path against the oracle.

use fig_core::ops::gen_ops;
use fig_core::{Op, ReferenceKv};
use fig_sstable::{SstableReader, SstableWriter};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::BTreeMap;

/// Last-write-wins fold of the stream's Put/Delete ops, ascending by key.
fn fold_writes(ops: &[Op]) -> Vec<(Vec<u8>, Option<Vec<u8>>)> {
    let mut map: BTreeMap<Vec<u8>, Option<Vec<u8>>> = BTreeMap::new();
    for op in ops {
        match op {
            Op::Put { key, value } => {
                map.insert(key.clone(), Some(value.clone()));
            }
            Op::Delete { key } => {
                map.insert(key.clone(), None);
            }
            Op::Get { .. } | Op::Scan { .. } => {}
        }
    }
    map.into_iter().collect()
}

fn write_table(path: &std::path::Path, folded: &[(Vec<u8>, Option<Vec<u8>>)], block_target: usize) {
    let mut w = SstableWriter::create(path, block_target).unwrap();
    for (k, v) in folded {
        match v {
            Some(value) => w.put(k, value).unwrap(),
            None => w.delete(k).unwrap(),
        }
    }
    let meta = w.finish().unwrap();
    assert_eq!(meta.records, folded.len() as u64);
}

#[test]
fn sstable_matches_oracle_on_random_streams() {
    for seed in 0..20u64 {
        let ops = gen_ops(seed, 300);
        let folded = fold_writes(&ops);

        // Oracle state: all write ops in stream order.
        let mut oracle = ReferenceKv::new();
        for op in &ops {
            oracle.apply(op);
        }

        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.sst");
        // Tiny blocks force multi-block files with many index entries.
        write_table(&p, &folded, 64);
        let r = SstableReader::open(&p).unwrap();
        assert!(r.blocks() >= 1, "seed {seed}: expected blocks");

        // Full iteration: records in order, tombstones present.
        let records = r.iter().unwrap();
        assert_eq!(records.len(), folded.len(), "seed {seed}: record count");
        for (rec, (k, v)) in records.iter().zip(folded.iter()) {
            assert_eq!(&rec.key, k, "seed {seed}: key order");
            assert_eq!(&rec.value, v, "seed {seed}: value/tombstone");
        }

        // Live scan matches the oracle's live set.
        let want = oracle.scan(&[0x00], &[0xff]).unwrap();
        assert_eq!(r.scan(&[0x00], &[0xff]).unwrap(), want, "seed {seed}: scan");

        // Random point gets + range scans against the oracle.
        let mut rng = StdRng::seed_from_u64(seed ^ 0x9E37);
        for _ in 0..20 {
            let key = vec![rng.gen_range(0..10u8)];
            assert_eq!(
                r.get(&key).unwrap(),
                oracle.get(&key),
                "seed {seed}: get {key:?}"
            );
            let mut a = vec![rng.gen_range(0..10u8)];
            let mut b = vec![rng.gen_range(0..10u8)];
            if a == b {
                continue;
            }
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            assert_eq!(
                r.scan(&a, &b).unwrap(),
                oracle.scan(&a, &b).unwrap(),
                "seed {seed}: scan {a:?}..{b:?}"
            );
        }
    }
}

#[test]
fn empty_and_single_key_tables() {
    let dir = tempfile::tempdir().unwrap();

    let p = dir.path().join("empty.sst");
    write_table(&p, &[], 64);
    let r = SstableReader::open(&p).unwrap();
    assert_eq!(r.blocks(), 0);
    assert_eq!(r.get(b"x").unwrap(), None);
    assert!(r.scan(&[0x00], &[0xff]).unwrap().is_empty());

    let p = dir.path().join("one.sst");
    write_table(&p, &[(b"k".to_vec(), Some(b"v".to_vec()))], 64);
    let r = SstableReader::open(&p).unwrap();
    assert_eq!(r.get(b"k").unwrap(), Some(b"v".to_vec()));
}
