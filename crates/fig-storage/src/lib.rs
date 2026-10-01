//! `fig-storage`: WAL-backed memtable engine.
//!
//! Write path: `PUT → WAL → Memtable`. Recovery: `restart → WAL replay`.
//! A write is acknowledged iff it sits at or before the last `sync()` —
//! same rule as the WAL, lifted one layer up: the memtable is just the
//! replayed prefix made queryable.

use fig_core::{Config, Error, MemoryKv, Result};
use fig_wal::{FsyncPolicy, Wal, WalOp, WalOptions};
use std::path::Path;

/// A persistent single-node map: every mutation is WAL-appended before it
/// touches the memtable, so a crash loses at most the unsynced tail.
pub struct Engine {
    wal: Wal,
    mem: MemoryKv,
    cfg: Config,
}

impl Engine {
    /// Open (creating `dir`), replaying the WAL into a fresh memtable.
    pub fn open(dir: &Path, cfg: Config, fsync: FsyncPolicy) -> Result<Self> {
        cfg.validate()?;
        let opts = WalOptions {
            dir: dir.to_path_buf(),
            max_segment_bytes: 64 * 1024 * 1024,
            fsync,
        };
        let (wal, entries, _) = Wal::open(&opts).map_err(Error::Io)?;
        let mut mem = MemoryKv::new();
        let mut expect = 0u64;
        for e in &entries {
            // The log is the source of truth; a gap here is a bug, not data.
            debug_assert_eq!(e.seq, expect, "wal replay gap at seq {expect}");
            expect += 1;
            match &e.op {
                WalOp::Put(k, v) => {
                    mem.put(k.clone(), v.clone());
                }
                WalOp::Delete(k) => {
                    mem.delete(k);
                }
            }
        }
        tracing::info!(
            replayed = entries.len(),
            "storage engine opened"
        );
        Ok(Self { wal, mem, cfg })
    }

    /// PUT(key, value): WAL first, memtable second. Durable on return iff the
    /// fsync policy syncs (or a later `sync()` covers it).
    pub fn put(&mut self, key: Vec<u8>, value: Vec<u8>) -> Result<()> {
        self.cfg.check_key(&key)?;
        self.cfg.check_value(&value)?;
        self.wal
            .append_put(key.clone(), value.clone())
            .map_err(Error::Io)?;
        self.mem.put(key, value);
        Ok(())
    }

    /// DELETE(key): recorded in the WAL, then removed from the memtable.
    /// Returns true if a live key was removed.
    pub fn delete(&mut self, key: &[u8]) -> Result<bool> {
        self.cfg.check_key(key)?;
        self.wal
            .append_delete(key.to_vec())
            .map_err(Error::Io)?;
        Ok(self.mem.delete(key))
    }

    /// GET(key).
    pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.mem.get(key)
    }

    /// SCAN(start, end): ascending pairs with `start <= k < end`.
    pub fn scan(&self, start: &[u8], end: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        self.mem.scan(start, end)
    }

    /// fsync the WAL: everything written so far becomes acknowledged.
    pub fn sync(&mut self) -> Result<()> {
        self.wal.sync().map_err(Error::Io)
    }

    /// Number of live keys.
    pub fn len(&self) -> usize {
        self.mem.len()
    }

    /// Whether the map is empty.
    pub fn is_empty(&self) -> bool {
        self.mem.is_empty()
    }

    /// Next WAL sequence number (count of appended records).
    pub fn next_seq(&self) -> u64 {
        self.wal.next_seq()
    }

    /// Test hook: durable bytes in the active segment (crash simulations cut
    /// the file at or after this offset).
    pub fn synced_len(&self) -> u64 {
        self.wal.synced_len()
    }

    /// Test hook: WAL directory (crash simulations truncate the file here).
    pub fn wal_dir(&self) -> &Path {
        self.wal.dir()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn engine(dir: &Path) -> Engine {
        Engine::open(dir, Config::default(), FsyncPolicy::Always).unwrap()
    }

    #[test]
    fn put_get_delete_scan_roundtrip() {
        let d = tempdir().unwrap();
        let mut e = engine(d.path());
        assert_eq!(e.get(b"a"), None);
        e.put(b"a".to_vec(), b"1".to_vec()).unwrap();
        e.put(b"b".to_vec(), b"2".to_vec()).unwrap();
        assert_eq!(e.get(b"a"), Some(b"1".to_vec()));
        assert!(e.delete(b"a").unwrap());
        assert_eq!(e.get(b"a"), None);
        let r = e.scan(b"a", &[0xff]).unwrap();
        assert_eq!(r, vec![(b"b".to_vec(), b"2".to_vec())]);
    }

    #[test]
    fn reopen_replays_the_log() {
        let d = tempdir().unwrap();
        {
            let mut e = engine(d.path());
            e.put(b"k1".to_vec(), b"v1".to_vec()).unwrap();
            e.put(b"k2".to_vec(), b"v2".to_vec()).unwrap();
            e.delete(b"k1").unwrap();
        }
        let e = engine(d.path());
        assert_eq!(e.get(b"k1"), None);
        assert_eq!(e.get(b"k2"), Some(b"v2".to_vec()));
        assert_eq!(e.next_seq(), 3);
    }

    #[test]
    fn limits_are_enforced_before_the_wal() {
        let d = tempdir().unwrap();
        let mut e = engine(d.path());
        assert!(e.put(vec![], b"v".to_vec()).is_err());
        // Rejected write must leave no trace: seq and state unchanged.
        assert_eq!(e.next_seq(), 0);
        assert!(e.is_empty());
    }
}
