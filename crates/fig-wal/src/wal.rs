//! Multi-segment WAL manager.
//!
//! Crash model (`internal.md` §6):
//!
//! - `append` stages bytes in the OS page cache (visible to the process, not durable).
//! - `sync` (`fsync`) makes everything appended so far durable = acknowledged.
//! - A crash may lose the unsynced suffix and may tear the last unsynced frame.
//! - Recovery replays all segments, truncates a clean torn tail, and stops at the
//!   first corrupt frame (truncating it — records after corruption are not trusted).
//!
//! Invariants:
//! - Sequence numbers are dense and monotonic across segments (`next_seq` resumes at
//!   `max_replayed + 1`).
//! - Only the synced prefix is acknowledged; `synced_seq` tracks the last synced seq.
//! - Rotation never splits a frame: the current segment rolls *before* an append that
//!   would exceed `max_segment_bytes` (unless the segment is empty apart from header).

use crate::record::{encode, WalEntry, WalOp, FILE_HEADER_LEN};
use crate::segment::{self, Segment};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

/// When to fsync automatically on append.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsyncPolicy {
    /// fsync after every append (slowest, safest; default for tests).
    Always,
    /// Never fsync automatically; caller drives [`Wal::sync`] (benchmarks / batch loads).
    Never,
}

/// Options for [`Wal::open`].
#[derive(Debug, Clone)]
pub struct WalOptions {
    pub dir: PathBuf,
    pub max_segment_bytes: u64,
    pub fsync: FsyncPolicy,
}

impl WalOptions {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            max_segment_bytes: 64 * 1024 * 1024,
            fsync: FsyncPolicy::Always,
        }
    }
}

/// Recovery report for observability (§35: `wal_bytes`).
#[derive(Debug, Clone, Default)]
pub struct RecoveryReport {
    pub segments: usize,
    pub entries_replayed: usize,
    pub torn_truncated_bytes: u64,
    pub corrupt_truncated: bool,
    pub next_seq: u64,
}

/// Write-ahead log.
pub struct Wal {
    dir: PathBuf,
    max_segment_bytes: u64,
    fsync: FsyncPolicy,
    cur_id: u64,
    cur: Segment,
    next_seq: u64,
    synced_seq: u64,
}

impl Wal {
    /// Open (creating `dir`), replay all segments, truncate torn tails, and return
    /// `(wal, recovered_entries_in_order, report)`.
    pub fn open(opts: &WalOptions) -> std::io::Result<(Self, Vec<WalEntry>, RecoveryReport)> {
        std::fs::create_dir_all(&opts.dir)?;
        let segs = segment::list_segments(&opts.dir)?;
        let mut all = Vec::new();
        let mut torn_total = 0u64;
        let mut corrupt = false;

        for (id, path) in &segs {
            let (out, valid_up_to) = segment::replay(path)?;
            torn_total += out.torn_truncated_bytes;
            if out.corrupt_offset.is_some() {
                corrupt = true;
            }
            // Truncate torn tail / corrupt suffix so the log is clean for appends.
            let len = std::fs::metadata(path)?.len();
            if valid_up_to < len {
                let f = OpenOptions::new().write(true).open(path)?;
                f.set_len(valid_up_to)?;
            }
            all.extend(out.entries);
            let _ = id;
        }

        // Sequence continuity: replay order is segment order + file order, which is seq order.
        let mut next_seq = 0u64;
        for e in &all {
            next_seq = next_seq.max(e.seq + 1);
        }

        let cur_id = segs.last().map(|(id, _)| *id).unwrap_or(1);
        if segs.is_empty() {
            let p = segment::segment_path(&opts.dir, cur_id);
            let seg = segment::open_for_append(&p)?;
            let mut w = Self {
                dir: opts.dir.clone(),
                max_segment_bytes: opts.max_segment_bytes,
                fsync: opts.fsync,
                cur_id,
                cur: seg,
                next_seq,
                synced_seq: next_seq.saturating_sub(1),
            };
            w.cur.size = std::fs::metadata(&w.cur.path)?.len();
            w.cur.synced_size = w.cur.size;
            let report = RecoveryReport {
                segments: 0,
                entries_replayed: 0,
                torn_truncated_bytes: 0,
                corrupt_truncated: false,
                next_seq,
            };
            return Ok((w, all, report));
        }

        let cur_path = segment::segment_path(&opts.dir, cur_id);
        let mut cur = segment::open_for_append(&cur_path)?;
        cur.size = std::fs::metadata(&cur_path)?.len();
        cur.synced_size = cur.size;

        let report = RecoveryReport {
            segments: segs.len(),
            entries_replayed: all.len(),
            torn_truncated_bytes: torn_total,
            corrupt_truncated: corrupt,
            next_seq,
        };
        Ok((
            Self {
                dir: opts.dir.clone(),
                max_segment_bytes: opts.max_segment_bytes,
                fsync: opts.fsync,
                cur_id,
                cur,
                next_seq,
                synced_seq: next_seq.saturating_sub(1),
            },
            all,
            report,
        ))
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Last synced (acknowledged) sequence. `u64::MAX` sentinel never returned;
    /// when empty, `synced_seq == u64::MAX` would underflow, so we track via next_seq.
    pub fn synced_seq(&self) -> Option<u64> {
        if self.next_seq == 0 {
            None
        } else {
            // `synced_seq` only advances on sync; between append and sync it lags.
            Some(self.synced_seq_value())
        }
    }

    fn synced_seq_value(&self) -> u64 {
        self.synced_seq
    }

    fn maybe_rotate(&mut self, upcoming: usize) -> std::io::Result<()> {
        if self.cur.size > FILE_HEADER_LEN as u64
            && self.cur.size + upcoming as u64 > self.max_segment_bytes
        {
            self.cur.file.sync_data()?;
            self.cur_id += 1;
            let p = segment::segment_path(&self.dir, self.cur_id);
            self.cur = segment::open_for_append(&p)?;
            self.cur.size = std::fs::metadata(&p)?.len();
            self.cur.synced_size = self.cur.size;
        }
        Ok(())
    }

    fn append_op(&mut self, op: WalOp) -> std::io::Result<u64> {
        let seq = self.next_seq;
        let mut buf = Vec::new();
        encode(seq, &op, &mut buf);
        self.maybe_rotate(buf.len())?;
        // Re-encode after rotation is unnecessary (seq unchanged); just write.
        self.cur.file.write_all(&buf)?;
        self.cur.size += buf.len() as u64;
        self.next_seq += 1;
        if self.fsync == FsyncPolicy::Always {
            self.sync()?;
        }
        Ok(seq)
    }

    pub fn append_put(&mut self, key: Vec<u8>, value: Vec<u8>) -> std::io::Result<u64> {
        self.append_op(WalOp::Put(key, value))
    }

    pub fn append_delete(&mut self, key: Vec<u8>) -> std::io::Result<u64> {
        self.append_op(WalOp::Delete(key))
    }

    /// fsync current segment; everything appended so far becomes acknowledged.
    pub fn sync(&mut self) -> std::io::Result<()> {
        self.cur.file.sync_data()?;
        self.cur.synced_size = self.cur.size;
        self.synced_seq = self.next_seq.saturating_sub(1);
        Ok(())
    }

    /// Test hook: bytes currently durable (synced prefix length of active segment).
    pub fn synced_len(&self) -> u64 {
        self.cur.synced_size
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn opts(dir: &Path) -> WalOptions {
        WalOptions {
            dir: dir.to_path_buf(),
            max_segment_bytes: 1024 * 1024,
            fsync: FsyncPolicy::Always,
        }
    }

    #[test]
    fn seqs_are_dense_across_reopen() {
        let d = tempdir().unwrap();
        let o = opts(d.path());
        let (mut w, rec, _) = Wal::open(&o).unwrap();
        assert!(rec.is_empty());
        w.append_put(b"a".to_vec(), b"1".to_vec()).unwrap();
        w.append_delete(b"a".to_vec()).unwrap();
        drop(w);
        let (_, rec2, rep) = Wal::open(&o).unwrap();
        assert_eq!(rec2.len(), 2);
        assert_eq!(rec2[0].seq, 0);
        assert_eq!(rec2[1].seq, 1);
        assert_eq!(rep.next_seq, 2);
    }

    #[test]
    fn rotation_keeps_order() {
        let d = tempdir().unwrap();
        let o = WalOptions {
            dir: d.path().to_path_buf(),
            max_segment_bytes: 200,
            fsync: FsyncPolicy::Always,
        };
        let (mut w, _, _) = Wal::open(&o).unwrap();
        for i in 0u8..20 {
            w.append_put(vec![i], vec![i; 10]).unwrap();
        }
        drop(w);
        let (_, rec, rep) = Wal::open(&o).unwrap();
        assert_eq!(rec.len(), 20);
        assert!(rep.segments >= 2);
        for (i, e) in rec.iter().enumerate() {
            assert_eq!(e.seq, i as u64);
        }
    }
}
