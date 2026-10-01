//! SSTable writer: ascending records → checksummed blocks + index + footer.
//!
//! The writer enforces strictly ascending keys; out-of-order input is rejected
//! before touching the file. The file is synced on `finish()` — an SSTable is
//! either complete and valid or it is never published to readers.

use crate::format::{self, IndexEntry, Record, FOOTER_LEN, HEADER_LEN, MAGIC, VERSION};
use fig_core::{Error, Result};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Default data-block target size: 4 KiB of encoded records.
pub const DEFAULT_BLOCK_TARGET: usize = 4 * 1024;

/// Summary returned by [`SstableWriter::finish`].
#[derive(Debug, Clone)]
pub struct SstableMeta {
    pub path: PathBuf,
    pub records: u64,
    pub blocks: u64,
    pub bytes: u64,
}

/// Writes one SSTable file. Keys must arrive in strictly ascending order.
pub struct SstableWriter {
    file: File,
    path: PathBuf,
    block_target: usize,
    /// Bytes written so far (== next write offset).
    pos: u64,
    /// Current open block: encoded bytes, its first key, last key seen.
    block: Vec<u8>,
    block_first_key: Option<Vec<u8>>,
    last_key: Option<Vec<u8>>,
    index: Vec<IndexEntry>,
    records: u64,
}

impl SstableWriter {
    /// Create a new table file, writing the header. Fails if `path` exists —
    /// SSTables are immutable and never overwritten in place.
    pub fn create(path: &Path, block_target: usize) -> Result<Self> {
        if path.exists() {
            return Err(Error::InvalidArgument(format!(
                "sstable already exists: {}",
                path.display()
            )));
        }
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(Error::Io)?;
            }
        }
        let mut file = File::create(path).map_err(Error::Io)?;
        let mut header = Vec::with_capacity(HEADER_LEN);
        header.extend_from_slice(&MAGIC);
        header.extend_from_slice(&VERSION.to_le_bytes());
        file.write_all(&header).map_err(Error::Io)?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
            block_target: block_target.max(64),
            pos: HEADER_LEN as u64,
            block: Vec::new(),
            block_first_key: None,
            last_key: None,
            index: Vec::new(),
            records: 0,
        })
    }

    fn check_order(&self, key: &[u8]) -> Result<()> {
        if let Some(last) = &self.last_key {
            if key <= last.as_slice() {
                return Err(Error::InvalidArgument(
                    "sstable: keys must arrive in strictly ascending order".to_string(),
                ));
            }
        }
        Ok(())
    }

    fn append_record(&mut self, rec: &Record) -> Result<()> {
        self.check_order(&rec.key)?;
        if self.block_first_key.is_none() {
            self.block_first_key = Some(rec.key.clone());
        }
        format::encode_record(rec, &mut self.block);
        self.last_key = Some(rec.key.clone());
        self.records += 1;
        if self.block.len() >= self.block_target {
            self.flush_block()?;
        }
        Ok(())
    }

    /// PUT a record. Key must exceed every previous key in this file.
    pub fn put(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        self.append_record(&Record::put(key.to_vec(), value.to_vec()))
    }

    /// Write a tombstone. Key must exceed every previous key in this file.
    pub fn delete(&mut self, key: &[u8]) -> Result<()> {
        self.append_record(&Record::delete(key.to_vec()))
    }

    /// Flush the open block (if non-empty) as one indexed, checksummed unit.
    fn flush_block(&mut self) -> Result<()> {
        if self.block.is_empty() {
            return Ok(());
        }
        let bytes = std::mem::take(&mut self.block);
        let first_key = self.block_first_key.take().expect("block has a first key");
        let crc = format::crc(&bytes);
        self.file.write_all(&bytes).map_err(Error::Io)?;
        self.index.push(IndexEntry {
            first_key,
            offset: self.pos,
            len: bytes.len() as u64,
            crc,
        });
        self.pos += bytes.len() as u64;
        Ok(())
    }

    /// Finish the table: flush the last block, write index + footer, sync.
    /// Returns file metadata. On any error the file is left incomplete and
    /// must be discarded by the caller (readers reject it via the footer).
    pub fn finish(mut self) -> Result<SstableMeta> {
        self.flush_block()?;

        let mut index_bytes = Vec::new();
        index_bytes.extend_from_slice(&(self.index.len() as u32).to_le_bytes());
        for e in &self.index {
            format::encode_index_entry(e, &mut index_bytes);
        }
        let index_crc = format::crc(&index_bytes);
        let index_offset = self.pos;
        self.file.write_all(&index_bytes).map_err(Error::Io)?;
        self.pos += index_bytes.len() as u64;

        let mut footer = Vec::with_capacity(FOOTER_LEN);
        footer.extend_from_slice(&index_offset.to_le_bytes());
        footer.extend_from_slice(&(index_bytes.len() as u64).to_le_bytes());
        footer.extend_from_slice(&index_crc.to_le_bytes());
        footer.extend_from_slice(&MAGIC);
        debug_assert_eq!(footer.len(), FOOTER_LEN);
        self.file.write_all(&footer).map_err(Error::Io)?;
        self.file.sync_all().map_err(Error::Io)?;
        self.pos += footer.len() as u64;

        Ok(SstableMeta {
            path: self.path.clone(),
            records: self.records,
            blocks: self.index.len() as u64,
            bytes: self.pos,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn rejects_out_of_order_keys() {
        let d = tempdir().unwrap();
        let p = d.path().join("t.sst");
        let mut w = SstableWriter::create(&p, 1024).unwrap();
        w.put(b"b", b"1").unwrap();
        assert!(w.put(b"a", b"2").is_err());
        assert!(w.put(b"b", b"3").is_err());
    }

    #[test]
    fn refuses_to_overwrite() {
        let d = tempdir().unwrap();
        let p = d.path().join("t.sst");
        std::fs::write(&p, b"junk").unwrap();
        assert!(SstableWriter::create(&p, 1024).is_err());
    }

    #[test]
    fn empty_table_finishes() {
        let d = tempdir().unwrap();
        let p = d.path().join("empty.sst");
        let meta = SstableWriter::create(&p, 1024).unwrap().finish().unwrap();
        assert_eq!(meta.records, 0);
        assert_eq!(meta.blocks, 0);
        assert!(meta.bytes > 0);
    }
}
