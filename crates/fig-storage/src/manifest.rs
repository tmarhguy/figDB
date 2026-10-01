//! Crash-safe table manifest: the atomic publish point for SSTables.
//!
//! Invariant: the set of live tables is defined by `MANIFEST`, not by the
//! directory listing. A table file is *published* only when these three are
//! true in order:
//!
//! 1. `sst-{id}.sst` exists, fully written and synced (the writer syncs on
//!    `finish()`, and we `rename` from a `.tmp` path so readers never see a
//!    partial file at its final name).
//! 2. `MANIFEST` lists `id`, written atomically (`MANIFEST.tmp` → sync →
//!    `rename` → dir fsync).
//! 3. Reader successfully opened the file (in-memory state).
//!
//! Crash windows and why each is safe:
//! - Crash before rename: only `*.tmp` litter remains → deleted at open.
//! - Crash after table rename, before manifest: the `.sst` file exists but is
//!   unlisted → treated as an unpublished orphan and deleted at open. The
//!   memtable/WAL still holds the data (WAL segments are only deleted *after*
//!   the manifest update), so nothing is lost.
//! - Crash after manifest, before WAL deletion: table is live *and* the WAL
//!   still replays the same keys. Reads merge memtable over tables, so
//!   duplicates are harmless; the next flush rewrites them and clears the WAL.
//! - Crash after WAL deletion: steady state, table is the durable copy.
//!
//! Legacy directories (written before the manifest existed) have `.sst` files
//! and no `MANIFEST`: open adopts them in id order and writes the manifest.

use fig_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MANIFEST_NAME: &str = "MANIFEST";
const MANIFEST_TMP_NAME: &str = "MANIFEST.tmp";
const MANIFEST_VERSION: u32 = 1;

/// Durable table-set state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub next_table_id: u64,
    /// Live table ids, oldest → newest.
    pub tables: Vec<u64>,
}

impl Manifest {
    pub fn empty() -> Self {
        Self {
            version: MANIFEST_VERSION,
            next_table_id: 0,
            tables: Vec::new(),
        }
    }

    pub fn new(next_table_id: u64, tables: Vec<u64>) -> Self {
        Self {
            version: MANIFEST_VERSION,
            next_table_id,
            tables,
        }
    }
}

pub fn manifest_path(table_dir: &Path) -> PathBuf {
    table_dir.join(MANIFEST_NAME)
}

/// fsync a directory so renames inside it survive a crash.
pub fn fsync_dir(path: &Path) -> Result<()> {
    let f = std::fs::File::open(path).map_err(Error::Io)?;
    f.sync_all().map_err(Error::Io)
}

/// Final path for a published table.
pub fn table_path(table_dir: &Path, id: u64) -> PathBuf {
    table_dir.join(format!("sst-{id:06}.sst"))
}

/// Staging path used while a table is being written (never read).
pub fn table_tmp_path(table_dir: &Path, id: u64) -> PathBuf {
    table_dir.join(format!("sst-{id:06}.sst.tmp"))
}

/// Parse a live-table filename: `sst-000123.sst` → `123`. Returns `None` for
/// staging files, the manifest, and anything else.
pub fn table_id_from_name(name: &str) -> Option<u64> {
    name.strip_prefix("sst-")
        .and_then(|s| s.strip_suffix(".sst"))
        .and_then(|s| s.parse::<u64>().ok())
}

/// Read the manifest, or `Ok(None)` when this directory predates manifests.
pub fn read_manifest(table_dir: &Path) -> Result<Option<Manifest>> {
    let path = manifest_path(table_dir);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path).map_err(Error::Io)?;
    let m: Manifest = serde_json::from_slice(&bytes)
        .map_err(|e| Error::Corruption(format!("manifest decode failed: {e}")))?;
    if m.version != MANIFEST_VERSION {
        return Err(Error::Corruption(format!(
            "manifest version {} unsupported",
            m.version
        )));
    }
    Ok(Some(m))
}

/// Atomically replace the manifest (tmp → sync → rename → dir fsync).
pub fn write_manifest(table_dir: &Path, manifest: &Manifest) -> Result<()> {
    let tmp = table_dir.join(MANIFEST_TMP_NAME);
    let bytes = serde_json::to_vec(manifest).map_err(|e| Error::Codec(e.to_string()))?;
    {
        let mut f = std::fs::File::create(&tmp).map_err(Error::Io)?;
        use std::io::Write as _;
        f.write_all(&bytes).map_err(Error::Io)?;
        f.sync_all().map_err(Error::Io)?;
    }
    std::fs::rename(&tmp, manifest_path(table_dir)).map_err(Error::Io)?;
    fsync_dir(table_dir)
}

/// Delete staging litter (`*.tmp`, `MANIFEST.tmp`) left by a crashed writer.
/// Never touches published files.
pub fn remove_staging_litter(table_dir: &Path) -> Result<()> {
    if !table_dir.exists() {
        return Ok(());
    }
    let entries = std::fs::read_dir(table_dir).map_err(Error::Io)?;
    for ent in entries {
        let ent = ent.map_err(Error::Io)?;
        let name = ent.file_name().to_string_lossy().to_string();
        if name.ends_with(".tmp") {
            std::fs::remove_file(ent.path()).map_err(Error::Io)?;
        }
    }
    Ok(())
}

/// Adopt pre-manifest `.sst` files (id order). Used once per legacy directory.
pub fn scan_legacy_tables(table_dir: &Path) -> Result<Vec<u64>> {
    let mut out = Vec::new();
    if !table_dir.exists() {
        return Ok(out);
    }
    let entries = std::fs::read_dir(table_dir).map_err(Error::Io)?;
    for ent in entries {
        let ent = ent.map_err(Error::Io)?;
        let name = ent.file_name().to_string_lossy().to_string();
        if let Some(id) = table_id_from_name(&name) {
            out.push(id);
        }
    }
    out.sort_unstable();
    Ok(out)
}

/// Delete published `.sst` files NOT in `live`: unpublished orphans (crashed
/// between table rename and manifest write) and compacted-away inputs whose
/// removal never finished.
pub fn remove_unlisted_tables(table_dir: &Path, live: &[u64]) -> Result<()> {
    use std::collections::HashSet;
    let keep: HashSet<u64> = live.iter().copied().collect();
    let entries = std::fs::read_dir(table_dir).map_err(Error::Io)?;
    for ent in entries {
        let ent = ent.map_err(Error::Io)?;
        let name = ent.file_name().to_string_lossy().to_string();
        if let Some(id) = table_id_from_name(&name) {
            if !keep.contains(&id) {
                std::fs::remove_file(ent.path()).map_err(Error::Io)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn roundtrip_empty() {
        let d = tempdir().unwrap();
        assert!(read_manifest(d.path()).unwrap().is_none());
        let m = Manifest::empty();
        write_manifest(d.path(), &m).unwrap();
        let back = read_manifest(d.path()).unwrap().unwrap();
        assert_eq!(back.next_table_id, 0);
        assert!(back.tables.is_empty());
        // Atomicity litter: no .tmp left behind.
        assert!(!d.path().join(MANIFEST_TMP_NAME).exists());
    }

    #[test]
    fn roundtrip_with_tables() {
        let d = tempdir().unwrap();
        let m = Manifest {
            version: MANIFEST_VERSION,
            next_table_id: 4,
            tables: vec![0, 2, 3],
        };
        write_manifest(d.path(), &m).unwrap();
        let back = read_manifest(d.path()).unwrap().unwrap();
        assert_eq!(back.tables, vec![0, 2, 3]);
        assert_eq!(back.next_table_id, 4);
    }

    #[test]
    fn staging_litter_removed_published_kept() {
        let d = tempdir().unwrap();
        std::fs::write(d.path().join("sst-000001.sst.tmp"), b"partial").unwrap();
        std::fs::write(d.path().join("MANIFEST.tmp"), b"partial").unwrap();
        std::fs::write(d.path().join("sst-000000.sst"), b"real").unwrap();
        remove_staging_litter(d.path()).unwrap();
        assert!(!d.path().join("sst-000001.sst.tmp").exists());
        assert!(!d.path().join("MANIFEST.tmp").exists());
        assert!(d.path().join("sst-000000.sst").exists());
    }

    #[test]
    fn unlisted_tables_removed() {
        let d = tempdir().unwrap();
        for id in [0u64, 1, 2] {
            std::fs::write(table_path(d.path(), id), b"t").unwrap();
        }
        remove_unlisted_tables(d.path(), &[0, 2]).unwrap();
        assert!(table_path(d.path(), 0).exists());
        assert!(!table_path(d.path(), 1).exists());
        assert!(table_path(d.path(), 2).exists());
    }

    #[test]
    fn corrupt_manifest_is_corruption_not_io() {
        let d = tempdir().unwrap();
        std::fs::write(d.path().join(MANIFEST_NAME), b"{not json").unwrap();
        let err = read_manifest(d.path()).unwrap_err();
        assert!(matches!(err, Error::Corruption(_)));
    }
}
