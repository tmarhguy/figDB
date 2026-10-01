//! LSM database: one WAL-backed memtable plus a stack of flushed SSTables.
//!
//! Write path: `PUT → WAL → memtable → (threshold) → SSTable`.
//! Reads merge newest-first: memtable, then tables, with tombstones shadowing
//! older layers. A flush writes the memtable (plus pending tombstones) to a
//! new table and restarts the WAL empty — the table, already synced, is the
//! durable copy from that point on.
//!
//! Flushing is foreground on a size threshold for now: it is correct and
//! crash-tested. A background flush thread arrives with compaction, when
//! there is real write concurrency to hide.

use crate::bloom::Bloom;
use crate::Engine;
use fig_core::{Config, Error, Result};
use fig_sstable::{SstableReader, SstableWriter, DEFAULT_BLOCK_TARGET};
use fig_wal::FsyncPolicy;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

/// Counters for observability. Plain struct today; Prometheus exposition later.
#[derive(Debug, Clone, Default)]
pub struct LsmMetrics {
    /// Completed flushes.
    pub flushes: u64,
    /// Records written to tables across all flushes.
    pub flushed_records: u64,
    /// Table bytes written across all flushes.
    pub flushed_bytes: u64,
    /// Tables currently open.
    pub tables: usize,
}

/// One flushed table with its in-memory helpers.
struct Table {
    reader: SstableReader,
    bloom: Bloom,
    tombstones: HashSet<Vec<u8>>,
}

fn table_path(table_dir: &Path, id: u64) -> PathBuf {
    table_dir.join(format!("sst-{id:06}.sst"))
}

fn list_tables(table_dir: &Path) -> Result<Vec<(u64, PathBuf)>> {
    let mut out = Vec::new();
    if !table_dir.exists() {
        return Ok(out);
    }
    let entries = std::fs::read_dir(table_dir).map_err(Error::Io)?;
    for ent in entries {
        let ent = ent.map_err(Error::Io)?;
        let name = ent.file_name().to_string_lossy().to_string();
        if let Some(id) = name
            .strip_prefix("sst-")
            .and_then(|s| s.strip_suffix(".sst"))
            .and_then(|s| s.parse::<u64>().ok())
        {
            out.push((id, ent.path()));
        }
    }
    out.sort_by_key(|(id, _)| *id);
    Ok(out)
}

fn open_table(path: &Path) -> Result<Table> {
    let reader = SstableReader::open(path)?;
    let records = reader.iter()?;
    let mut bloom = Bloom::new(records.len(), 0.01);
    let mut tombstones = HashSet::new();
    for rec in &records {
        bloom.insert(&rec.key);
        if rec.value.is_none() {
            tombstones.insert(rec.key.clone());
        }
    }
    Ok(Table {
        reader,
        bloom,
        tombstones,
    })
}

/// The database: memtable engine in front, SSTables behind.
pub struct Database {
    wal_dir: PathBuf,
    table_dir: PathBuf,
    engine: Option<Engine>,
    /// Oldest → newest.
    tables: Vec<Table>,
    /// Deletes since the last flush (newer than every table).
    tombstones: BTreeSet<Vec<u8>>,
    /// Estimated memtable bytes since the last flush.
    mem_bytes: usize,
    flush_threshold: usize,
    metrics: LsmMetrics,
    next_table_id: u64,
    cfg: Config,
    fsync: FsyncPolicy,
}

impl Database {
    /// Open (creating `dir`), replaying the WAL tail and all tables.
    pub fn open(
        dir: &Path,
        cfg: Config,
        fsync: FsyncPolicy,
        flush_threshold: usize,
    ) -> Result<Self> {
        cfg.validate()?;
        let wal_dir = dir.join("wal");
        let table_dir = dir.join("tables");
        std::fs::create_dir_all(&wal_dir).map_err(Error::Io)?;
        std::fs::create_dir_all(&table_dir).map_err(Error::Io)?;

        let mut engine = Engine::open(&wal_dir, cfg.clone(), fsync)?;
        // The WAL tail refills the memtable on open, so the flush accounting
        // must restart from what replay restored — not from zero. Otherwise
        // frequent restarts would starve flushing forever, and replayed
        // deletes would lose the tombstones that shadow older tables.
        let tombstones = engine.take_replay_tombstones();
        // Same units as the write path: live pairs plus one tombstone
        // record (key + op byte) per pending delete.
        let mem_bytes: usize = engine
            .scan_all()
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>()
            + tombstones.iter().map(|k| k.len() + 1).sum::<usize>();
        let mut tables = Vec::new();
        let mut next_table_id = 0u64;
        for (id, path) in list_tables(&table_dir)? {
            tables.push(open_table(&path)?);
            next_table_id = next_table_id.max(id + 1);
        }
        let mut db = Self {
            wal_dir,
            table_dir,
            engine: Some(engine),
            tables,
            tombstones,
            mem_bytes,
            flush_threshold: flush_threshold.max(1),
            metrics: LsmMetrics::default(),
            next_table_id,
            cfg,
            fsync,
        };
        db.metrics.tables = db.tables.len();
        Ok(db)
    }

    fn eng(&self) -> &Engine {
        self.engine.as_ref().expect("engine present outside flush")
    }

    fn eng_mut(&mut self) -> &mut Engine {
        self.engine.as_mut().expect("engine present outside flush")
    }

    /// PUT(key, value). Live memtable bytes are accounted net: overwrites
    /// replace, so only the delta counts toward the flush threshold.
    pub fn put(&mut self, key: Vec<u8>, value: Vec<u8>) -> Result<()> {
        let prev = self.eng().get(&key);
        self.eng_mut().put(key.clone(), value.clone())?;
        match prev {
            Some(old) => {
                self.mem_bytes = self
                    .mem_bytes
                    .saturating_add(value.len())
                    .saturating_sub(old.len());
            }
            None => {
                self.mem_bytes = self.mem_bytes.saturating_add(key.len() + value.len());
                if self.tombstones.remove(&key) {
                    self.mem_bytes = self.mem_bytes.saturating_sub(key.len() + 1);
                }
            }
        }
        self.maybe_flush()
    }

    /// DELETE(key). Live bytes drop by the removed pair; the tombstone itself
    /// costs key bytes until the next flush carries it to a table.
    /// Returns true if a live key was removed from the memtable
    /// (false does NOT mean absent — older tables may still hold it).
    pub fn delete(&mut self, key: &[u8]) -> Result<bool> {
        let old = self.eng().get(key);
        let removed = self.eng_mut().delete(key)?;
        match old {
            Some(v) => {
                self.mem_bytes = self
                    .mem_bytes
                    .saturating_sub(key.len() + v.len())
                    .saturating_add(key.len() + 1);
            }
            None => {
                self.mem_bytes = self.mem_bytes.saturating_add(key.len() + 1);
            }
        }
        self.tombstones.insert(key.to_vec());
        self.maybe_flush()?;
        Ok(removed)
    }

    /// GET(key): memtable first, then newest table to oldest.
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        if let Some(v) = self.eng().get(key) {
            return Ok(Some(v));
        }
        for table in self.tables.iter().rev() {
            if table.tombstones.contains(key) {
                return Ok(None);
            }
            if !table.bloom.contains(key) {
                continue;
            }
            if let Some(v) = table.reader.get(key)? {
                return Ok(Some(v));
            }
        }
        Ok(None)
    }

    /// SCAN(start, end): merged ascending live pairs.
    pub fn scan(&self, start: &[u8], end: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        if start >= end {
            return Err(Error::InvalidArgument(
                "scan: start must be < end".to_string(),
            ));
        }
        let mut merged: BTreeMap<Vec<u8>, Option<Vec<u8>>> = BTreeMap::new();
        for table in &self.tables {
            for rec in table.reader.iter()? {
                merged.insert(rec.key, rec.value);
            }
        }
        for (k, v) in self.eng().scan_all() {
            merged.insert(k, Some(v));
        }
        for k in &self.tombstones {
            merged.insert(k.clone(), None);
        }
        Ok(merged
            .into_iter()
            .filter(|(k, _)| k.as_slice() >= start && k.as_slice() < end)
            .filter_map(|(k, v)| v.map(|v| (k, v)))
            .collect())
    }

    /// fsync the WAL tail.
    pub fn sync(&mut self) -> Result<()> {
        self.eng_mut().sync()
    }

    /// Flush the memtable (and pending tombstones) to a new SSTable.
    /// Returns true if a table was written.
    pub fn flush(&mut self) -> Result<bool> {
        if self.eng().is_empty() && self.tombstones.is_empty() {
            return Ok(false);
        }
        let mut merged: BTreeMap<Vec<u8>, Option<Vec<u8>>> = BTreeMap::new();
        for (k, v) in self.eng().scan_all() {
            merged.insert(k, Some(v));
        }
        for k in &self.tombstones {
            merged.insert(k.clone(), None);
        }

        let id = self.next_table_id;
        let path = table_path(&self.table_dir, id);
        let mut w = SstableWriter::create(&path, DEFAULT_BLOCK_TARGET)?;
        for (k, v) in &merged {
            match v {
                Some(value) => w.put(k, value)?,
                None => w.delete(k)?,
            }
        }
        let meta = w.finish()?;
        let table = open_table(&path)?;

        // The table is synced and readable: the WAL's copy is redundant.
        // Drop the engine (closing its files), delete its segments, reopen empty.
        drop(self.engine.take());
        let entries = std::fs::read_dir(&self.wal_dir).map_err(Error::Io)?;
        for ent in entries {
            let ent = ent.map_err(Error::Io)?;
            let p = ent.path();
            if p.extension().map(|x| x == "log").unwrap_or(false) {
                std::fs::remove_file(&p).map_err(Error::Io)?;
            }
        }
        self.engine = Some(Engine::open(&self.wal_dir, self.cfg.clone(), self.fsync)?);

        self.next_table_id += 1;
        self.tables.push(table);
        self.tombstones.clear();
        self.mem_bytes = 0;
        self.metrics.flushes += 1;
        self.metrics.flushed_records += meta.records;
        self.metrics.flushed_bytes += meta.bytes;
        self.metrics.tables = self.tables.len();
        tracing::info!(
            table = id,
            records = meta.records,
            bytes = meta.bytes,
            "flushed memtable to sstable"
        );
        Ok(true)
    }

    fn maybe_flush(&mut self) -> Result<()> {
        if self.mem_bytes >= self.flush_threshold {
            self.flush()?;
        }
        Ok(())
    }

    /// Current metrics snapshot.
    pub fn metrics(&self) -> &LsmMetrics {
        &self.metrics
    }

    /// Test hook: durable WAL bytes (crash simulations cut at/after this).
    pub fn synced_len(&self) -> u64 {
        self.eng().synced_len()
    }

    /// Test hook: WAL directory for crash simulations.
    pub fn wal_dir(&self) -> &Path {
        &self.wal_dir
    }

    /// Test hook: tables currently open.
    pub fn table_count(&self) -> usize {
        self.tables.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn db(dir: &Path, threshold: usize) -> Database {
        Database::open(dir, Config::default(), FsyncPolicy::Always, threshold).unwrap()
    }

    #[test]
    fn flush_moves_reads_to_tables() {
        let d = tempdir().unwrap();
        let mut db = db(d.path(), 16);
        for i in 0u8..20 {
            db.put(vec![i], vec![i, i]).unwrap();
        }
        assert!(db.table_count() >= 1, "threshold writes must flush");
        assert_eq!(db.metrics().flushes as usize, db.table_count());
        for i in 0u8..20 {
            assert_eq!(db.get(&[i]).unwrap(), Some(vec![i, i]));
        }
        // Reopen: tables + empty WAL tail agree.
        drop(db);
        let db = Database::open(d.path(), Config::default(), FsyncPolicy::Always, 64).unwrap();
        for i in 0u8..20 {
            assert_eq!(db.get(&[i]).unwrap(), Some(vec![i, i]));
        }
    }

    #[test]
    fn deletes_shadow_older_tables() {
        let d = tempdir().unwrap();
        let mut db = db(d.path(), 64);
        db.put(b"k".to_vec(), b"v1".to_vec()).unwrap();
        db.flush().unwrap();
        assert_eq!(db.get(b"k").unwrap(), Some(b"v1".to_vec()));
        db.delete(b"k").unwrap();
        db.flush().unwrap();
        // Tombstone-only table hides the older value, even after reopen.
        assert_eq!(db.get(b"k").unwrap(), None);
        drop(db);
        let db = Database::open(d.path(), Config::default(), FsyncPolicy::Always, 64).unwrap();
        assert_eq!(db.get(b"k").unwrap(), None);
        // And resurrection works.
        let mut db = db;
        db.put(b"k".to_vec(), b"v2".to_vec()).unwrap();
        assert_eq!(db.get(b"k").unwrap(), Some(b"v2".to_vec()));
    }

    #[test]
    fn empty_flush_is_a_noop() {
        let d = tempdir().unwrap();
        let mut db = db(d.path(), 64);
        assert!(!db.flush().unwrap());
        assert_eq!(db.metrics().flushes, 0);
    }
}
