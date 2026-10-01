//! LSM database: one WAL-backed memtable plus a stack of flushed SSTables.
//!
//! Write path: `PUT → WAL → memtable → (threshold) → SSTable`.
//! Reads merge newest-first: memtable, then tables, with tombstones shadowing
//! older layers. A flush writes the memtable (plus pending tombstones) to a
//! new table and restarts the WAL empty — the table, already synced, is the
//! durable copy from that point on.
//!
//! Flushing is foreground on a size threshold: it is correct and
//! crash-tested. Compaction is either inline in `flush()` (library default)
//! or on a background timer via `background_compact()` (server mode, which
//! disables the inline path so merges never stall writers). Reads use
//! atomically-swapped snapshots, so neither path is visible to them.

use crate::bloom::Bloom;
use crate::manifest::{self, Manifest};
use crate::Engine;
use fig_core::{Config, Error, Result};
use fig_sstable::{SstableReader, SstableWriter, DEFAULT_BLOCK_TARGET};
use fig_wal::FsyncPolicy;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Counters for observability. Plain struct today; Prometheus exposition later.
#[derive(Debug, Clone, Default)]
pub struct LsmMetrics {
    /// Completed flushes.
    pub flushes: u64,
    /// Records written to tables across all flushes.
    pub flushed_records: u64,
    /// Table bytes written across all flushes.
    pub flushed_bytes: u64,
    /// Completed compactions (each merged ≥2 tables into ≤1).
    pub compactions: u64,
    /// Input records merged across all compactions.
    pub compacted_records: u64,
    /// Tables currently open.
    pub tables: usize,
}

/// Auto-compaction policy: when the stack reaches this many tables, merge the
/// oldest batch. Bounds read fan-out without a background thread; see ADR-003.
const COMPACT_THRESHOLD_TABLES: usize = 8;
const COMPACT_BATCH: usize = 8;

/// One flushed table with its in-memory helpers.
struct Table {
    id: u64,
    reader: SstableReader,
    bloom: Bloom,
    tombstones: HashSet<Vec<u8>>,
}

fn open_table(id: u64, path: &Path) -> Result<Table> {
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
        id,
        reader,
        bloom,
        tombstones,
    })
}

/// The database: memtable engine in front, SSTables behind.
///
/// The table stack is an atomically-swapped snapshot (`Arc`): readers clone
/// the pointer and iterate without holding any lock, so a concurrent
/// flush/compact can replace the stack underneath them — holders of the old
/// snapshot keep serving it (their bytes are in memory) while new readers
/// see the new one. The manifest on disk is the same idea made durable.
pub struct Database {
    wal_dir: PathBuf,
    table_dir: PathBuf,
    engine: Option<Engine>,
    /// Oldest → newest. Swapped wholesale on flush/compact, never mutated.
    tables: Arc<Vec<Arc<Table>>>,
    /// Deletes since the last flush (newer than every table).
    tombstones: BTreeSet<Vec<u8>>,
    /// Estimated memtable bytes since the last flush.
    mem_bytes: usize,
    flush_threshold: usize,
    /// Whether `flush()` folds the stack inline when the threshold is hit.
    /// The server disables this and lets a background task own compaction so
    /// merges never stall the write path; the library default stays `true`.
    auto_compact: bool,
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
        // The live table set is defined by MANIFEST, not the directory
        // listing (see `manifest.rs`). Staging litter and unpublished orphans
        // from a crashed flush/compact are cleaned here, before any reads.
        manifest::remove_staging_litter(&table_dir)?;
        let (table_ids, next_table_id) = match manifest::read_manifest(&table_dir)? {
            Some(m) => {
                manifest::remove_unlisted_tables(&table_dir, &m.tables)?;
                (m.tables, m.next_table_id)
            }
            None => {
                // Pre-manifest directory: adopt existing tables once, then
                // publish the manifest so later opens take the fast path.
                let ids = manifest::scan_legacy_tables(&table_dir)?;
                let next = ids.iter().max().map(|max| max + 1).unwrap_or(0);
                manifest::write_manifest(&table_dir, &Manifest::new(next, ids.clone()))?;
                (ids, next)
            }
        };
        let mut tables = Vec::new();
        for id in &table_ids {
            tables.push(Arc::new(open_table(
                *id,
                &manifest::table_path(&table_dir, *id),
            )?));
        }
        let mut db = Self {
            wal_dir,
            table_dir,
            engine: Some(engine),
            tables: Arc::new(tables),
            tombstones,
            mem_bytes,
            flush_threshold: flush_threshold.max(1),
            auto_compact: true,
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

    /// GET(key): memtable first, then newest table to oldest. Operates on a
    /// snapshot: safe to call concurrently with flush/compact on another
    /// thread (needs only `&self`).
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        if let Some(v) = self.eng().get(key) {
            return Ok(Some(v));
        }
        let snapshot = Arc::clone(&self.tables);
        for table in snapshot.iter().rev() {
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

    /// SCAN(start, end): merged ascending live pairs. Snapshot read like `get`.
    pub fn scan(&self, start: &[u8], end: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        if start >= end {
            return Err(Error::InvalidArgument(
                "scan: start must be < end".to_string(),
            ));
        }
        let mut merged: BTreeMap<Vec<u8>, Option<Vec<u8>>> = BTreeMap::new();
        let snapshot = Arc::clone(&self.tables);
        for table in snapshot.iter() {
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
        // Crash-safe publish: write to a staging path readers never open,
        // then rename (atomic) + dir fsync. A crash before the rename leaves
        // only `.tmp` litter (cleaned at open); a crash before the manifest
        // update below leaves an unlisted orphan (also cleaned at open). The
        // WAL still holds the data in both windows, so nothing is lost.
        let tmp_path = manifest::table_tmp_path(&self.table_dir, id);
        let final_path = manifest::table_path(&self.table_dir, id);
        if final_path.exists() {
            return Err(Error::Internal(format!(
                "flush: table {id} already published"
            )));
        }
        if tmp_path.exists() {
            std::fs::remove_file(&tmp_path).map_err(Error::Io)?;
        }
        let mut w = SstableWriter::create(&tmp_path, DEFAULT_BLOCK_TARGET)?;
        for (k, v) in &merged {
            match v {
                Some(value) => w.put(k, value)?,
                None => w.delete(k)?,
            }
        }
        let meta = w.finish()?;
        std::fs::rename(&tmp_path, &final_path).map_err(Error::Io)?;
        manifest::fsync_dir(&self.table_dir)?;
        let table = match open_table(id, &final_path) {
            Ok(t) => t,
            Err(e) => {
                let _ = std::fs::remove_file(&final_path);
                return Err(e);
            }
        };

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
        // Swap the snapshot: concurrent readers keep the old Arc (their bytes
        // are in memory), new readers see the appended stack.
        let mut next: Vec<Arc<Table>> = self.tables.iter().cloned().collect();
        next.push(Arc::new(table));
        self.tables = Arc::new(next);
        self.tombstones.clear();
        // Publish the new table set before dropping the WAL: until this
        // manifest lands, the WAL is the only durable copy; after it lands,
        // the table is. Either side of a crash has the data.
        let live_ids: Vec<u64> = self.tables.iter().map(|t| t.id).collect();
        manifest::write_manifest(
            &self.table_dir,
            &Manifest::new(self.next_table_id, live_ids),
        )?;
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
        // A flush grows the stack by one; fold the oldest batch down when the
        // fan-out bound is reached — unless a background task owns compaction
        // (server mode), in which case the write path only ever appends.
        if self.auto_compact {
            self.maybe_compact()?;
        }
        Ok(true)
    }

    /// Whether `flush()` compacts inline when the stack hits the threshold.
    /// The server disables this (`false`) and runs [`Database::background_compact`]
    /// on a timer instead, so merges never stall writers.
    pub fn set_auto_compact(&mut self, on: bool) {
        self.auto_compact = on;
    }

    /// One background step: merge the oldest batch if the stack reached the
    /// threshold. Returns true if a merge ran. Safe to call on a timer while
    /// readers use snapshots — same crash protocol as `flush()`.
    pub fn background_compact(&mut self) -> Result<bool> {
        if self.tables.len() < COMPACT_THRESHOLD_TABLES {
            return Ok(false);
        }
        // Partial merge: older tables may still sit below the inputs, so
        // every tombstone is kept — it may be shadowing them.
        self.merge_tables(COMPACT_BATCH, false)
    }

    /// Compact: flush the memtable, then merge ALL tables into one sorted run,
    /// garbage-collecting tombstones that shadow nothing. Returns true if a
    /// merge ran. Crash-safe by the same publish protocol as flush (output
    /// tmp → rename → manifest swap → delete inputs); see `manifest.rs`.
    pub fn compact(&mut self) -> Result<bool> {
        if !self.eng().is_empty() || !self.tombstones.is_empty() {
            self.flush()?;
        }
        if self.tables.len() < 2 {
            return Ok(false);
        }
        // Full merge with an empty memtable: no older layer remains below, so
        // tombstones whose key has no live value can be dropped entirely.
        self.merge_tables(self.tables.len(), true)
    }

    fn maybe_compact(&mut self) -> Result<()> {
        while self.tables.len() >= COMPACT_THRESHOLD_TABLES {
            // Partial merge: older tables may still sit below the inputs, so
            // every tombstone is kept — it may be shadowing them.
            self.merge_tables(COMPACT_BATCH, false)?;
        }
        Ok(())
    }

    /// Merge the oldest `n` tables into one new sorted run (newest input wins
    /// per key). Returns true if a merge ran. On any error before the manifest
    /// swap, in-memory state is untouched and the call is retryable.
    fn merge_tables(&mut self, n: usize, gc_tombstones: bool) -> Result<bool> {
        let n = n.min(self.tables.len());
        if n < 2 {
            return Ok(false);
        }
        let mut merged: BTreeMap<Vec<u8>, Option<Vec<u8>>> = BTreeMap::new();
        let mut input_records = 0u64;
        for table in self.tables.iter().take(n) {
            for rec in table.reader.iter()? {
                input_records += 1;
                merged.insert(rec.key, rec.value);
            }
        }
        if gc_tombstones {
            merged.retain(|_, v| v.is_some());
        }
        let input_ids: Vec<u64> = self.tables.iter().take(n).map(|t| t.id).collect();

        // Publish the output before touching the inputs: until the manifest
        // swap, the output is an unlisted orphan (reaped at open) and the
        // inputs are the durable copy.
        let output = if merged.is_empty() {
            None
        } else {
            let id = self.next_table_id;
            let tmp_path = manifest::table_tmp_path(&self.table_dir, id);
            let final_path = manifest::table_path(&self.table_dir, id);
            if tmp_path.exists() {
                std::fs::remove_file(&tmp_path).map_err(Error::Io)?;
            }
            let mut w = SstableWriter::create(&tmp_path, DEFAULT_BLOCK_TARGET)?;
            for (k, v) in &merged {
                match v {
                    Some(value) => w.put(k, value)?,
                    None => w.delete(k)?,
                }
            }
            let meta = w.finish()?;
            std::fs::rename(&tmp_path, &final_path).map_err(Error::Io)?;
            manifest::fsync_dir(&self.table_dir)?;
            match open_table(id, &final_path) {
                Ok(t) => {
                    self.next_table_id += 1;
                    self.metrics.flushed_bytes += meta.bytes;
                    Some(t)
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&final_path);
                    return Err(e);
                }
            }
        };

        // Swap the snapshot first (output holds the oldest data, remaining
        // tables are newer), then the manifest, then reap the input files. A
        // crash after the manifest leaves unlisted inputs that open reaps;
        // the data is already in the output. Readers on the old Arc are
        // unaffected either way.
        let mut next: Vec<Arc<Table>> = self.tables.iter().skip(n).cloned().collect();
        if let Some(t) = output {
            next.insert(0, Arc::new(t));
        }
        self.tables = Arc::new(next);
        let live_ids: Vec<u64> = self.tables.iter().map(|t| t.id).collect();
        manifest::write_manifest(
            &self.table_dir,
            &Manifest::new(self.next_table_id, live_ids),
        )?;
        for id in &input_ids {
            let p = manifest::table_path(&self.table_dir, *id);
            if let Err(e) = std::fs::remove_file(&p) {
                // Already durable via the output; open reaps stragglers.
                tracing::warn!(table = id, error = %e, "compact: input deletion failed");
            }
        }
        self.metrics.compactions += 1;
        self.metrics.compacted_records += input_records;
        self.metrics.tables = self.tables.len();
        tracing::info!(
            inputs = n,
            records = input_records,
            tables = self.tables.len(),
            "compacted tables"
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
