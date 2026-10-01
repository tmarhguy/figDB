//! Crash/restart gate for Commit 03:
//! random crash/restart testing preserves acknowledged records.
//!
//! Model: `sync()` = acknowledgement. Everything before the last `sync` must
//! survive any torn-tail crash; anything after may be lost but must never come
//! back half-written or reordered.

use fig_wal::{FsyncPolicy, Wal, WalEntry, WalOptions};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::fs::OpenOptions;
use std::io::Write;

fn test_opts(dir: &std::path::Path) -> WalOptions {
    WalOptions {
        dir: dir.to_path_buf(),
        max_segment_bytes: 16 * 1024 * 1024,
        fsync: FsyncPolicy::Never, // manual sync = explicit ack points
    }
}

fn active_segment(dir: &std::path::Path) -> std::path::PathBuf {
    let mut segs: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().map(|x| x == "log").unwrap_or(false))
        .collect();
    segs.sort();
    segs.pop().expect("wal segment must exist")
}

/// Simulate a crash: drop the WAL (losing unsynced page-cache data is modeled by
/// truncating to a random point in [synced_len, file_len]), then reopen.
fn crash_and_reopen(
    dir: &std::path::Path,
    synced_len: u64,
    rng: &mut StdRng,
) -> (Wal, Vec<WalEntry>) {
    let seg = active_segment(dir);
    let len = std::fs::metadata(&seg).unwrap().len();
    assert!(synced_len <= len);
    // Random torn point: synced_len (clean loss of unsynced) .. len (full torn frame).
    let cut = if len == synced_len {
        len
    } else {
        rng.gen_range(synced_len..=len)
    };
    // Model page-cache loss + tear: truncate file to `cut`.
    let f = OpenOptions::new().write(true).open(&seg).unwrap();
    f.set_len(cut).unwrap();
    drop(f);
    let opts = test_opts(dir);
    let (w, rec, _) = Wal::open(&opts).unwrap();
    (w, rec)
}

#[test]
fn acknowledged_prefix_survives_random_crashes() {
    for seed in 0..20u64 {
        let mut rng = StdRng::seed_from_u64(seed);
        let dir = tempfile::tempdir().unwrap();
        let opts = test_opts(dir.path());
        let (mut w, _, _) = Wal::open(&opts).unwrap();

        let mut acked: Vec<WalEntry> = Vec::new();
        let mut staged: Vec<WalEntry> = Vec::new();
        let mut seq = 0u64;

        for i in 0..200 {
            let key = vec![rng.gen_range(0..8u8)];
            let val = vec![rng.gen_range(0..8u8); 3];
            let s = w.append_put(key.clone(), val.clone()).unwrap();
            assert_eq!(s, seq);
            staged.push(WalEntry {
                seq,
                op: fig_wal::WalOp::Put(key, val),
            });
            seq += 1;

            // Sync (acknowledge) every ~7 ops.
            if i % 7 == 6 {
                w.sync().unwrap();
                acked.append(&mut staged);
            }

            // Random crash at ~5% of steps.
            if rng.gen_range(0..100) < 5 {
                let synced_len = w.synced_len();
                drop(w);
                let (w2, rec) = crash_and_reopen(dir.path(), synced_len, &mut rng);
                // Correct prefix property: acked ⊆ recovered ⊆ acked+staged.
                // A crash may preserve a clean prefix of the unsynced suffix
                // (if the tear point falls after whole frames); it must never
                // lose acked data, reorder, or return torn frames.
                let full: Vec<WalEntry> = acked
                    .iter()
                    .cloned()
                    .chain(staged.iter().cloned())
                    .collect();
                assert!(
                    rec.len() >= acked.len() && rec.len() <= full.len(),
                    "seed {seed} step {i}: recovered {} outside [{}, {}]",
                    rec.len(),
                    acked.len(),
                    full.len()
                );
                assert_eq!(
                    &rec[..acked.len()],
                    &acked[..],
                    "seed {seed} step {i}: acknowledged prefix lost"
                );
                assert_eq!(
                    &rec[..],
                    &full[..rec.len()],
                    "seed {seed} step {i}: recovered not a prefix of written"
                );
                for (i, e) in rec.iter().enumerate() {
                    assert_eq!(e.seq, i as u64, "seed {seed} step {i}: seq gap");
                }
                w = w2;
                // Whatever survived is now the acknowledged baseline; the rest
                // of the staged suffix was never acknowledged and is forgotten.
                staged.clear();
                seq = w.next_seq();
                acked = rec;
            }
        }
        // Final sync + clean reopen: everything acked must be present in order.
        w.sync().unwrap();
        acked.append(&mut staged);
        let synced_len = w.synced_len();
        drop(w);
        let (_, rec) = crash_and_reopen(dir.path(), synced_len, &mut rng);
        assert_eq!(rec, acked);
        for (i, e) in rec.iter().enumerate() {
            assert_eq!(e.seq, i as u64);
        }
    }
}

#[test]
fn corrupt_middle_frame_truncates_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let opts = test_opts(dir.path());
    let (mut w, _, _) = Wal::open(&opts).unwrap();
    for i in 0u8..10 {
        w.append_put(vec![i], vec![i; 8]).unwrap();
    }
    w.sync().unwrap();
    drop(w);

    // Corrupt a byte inside the 5th frame (not the 1st, so a prefix survives).
    // Frame layout is variable; parse frame_len prefixes to find frame 5.
    let seg = active_segment(dir.path());
    let mut bytes = std::fs::read(&seg).unwrap();
    let mut off = 8usize; // skip file header
    for _ in 0..4 {
        let fl = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        off += 4 + fl;
    }
    // Now `off` is the start of the 5th frame; flip a body byte (skip its len word).
    let flip_at = off + 4 + 9; // frame_len + seq(8) + op(1)
    assert!(flip_at < bytes.len(), "frame 5 must exist");
    bytes[flip_at] ^= 0xFF;
    std::fs::write(&seg, &bytes).unwrap();

    let (_, rec, report) = Wal::open(&test_opts(dir.path())).unwrap();
    // Prefix before corruption survives; suffix is truncated; seqs stay dense.
    assert!(report.corrupt_truncated);
    assert!(rec.len() < 10);
    assert!(!rec.is_empty());
    for (i, e) in rec.iter().enumerate() {
        assert_eq!(e.seq, i as u64);
    }
    // Further appends continue the sequence (no reuse → no phantom duplicates).
    let (mut w2, _, _) = Wal::open(&test_opts(dir.path())).unwrap();
    let s = w2.append_put(b"new".to_vec(), b"val".to_vec()).unwrap();
    assert_eq!(s, rec.len() as u64);
}

#[test]
fn torn_tail_is_truncated_not_replayed() {
    let dir = tempfile::tempdir().unwrap();
    let opts = test_opts(dir.path());
    let (mut w, _, _) = Wal::open(&opts).unwrap();
    w.append_put(b"a".to_vec(), b"1".to_vec()).unwrap();
    w.sync().unwrap();
    // Stage an unsynced record, then tear it mid-frame.
    w.append_put(b"b".to_vec(), b"2".to_vec()).unwrap();
    let seg = active_segment(dir.path());
    let len = std::fs::metadata(&seg).unwrap().len();
    drop(w);
    // Cut 3 bytes off → torn tail.
    let f = OpenOptions::new().write(true).open(&seg).unwrap();
    f.set_len(len - 3).unwrap();
    drop(f);
    // Also append garbage partial bytes to simulate a torn write that left junk.
    {
        let mut f = OpenOptions::new().append(true).open(&seg).unwrap();
        f.write_all(&[0x99, 0x88]).unwrap();
    }
    let (_, rec, report) = Wal::open(&test_opts(dir.path())).unwrap();
    assert_eq!(rec.len(), 1, "only acked record must replay");
    assert!(report.torn_truncated_bytes > 0);
}
