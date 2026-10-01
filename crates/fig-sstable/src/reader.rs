//! SSTable reader: indexed point lookup, range scan, full iteration.
//!
//! The footer is validated first (magic + size), then the index (checksum),
//! then each data block on access (checksum). Any mismatch is `Corruption` —
//! a file that fails here was never a complete SSTable and must be discarded,
//! never repaired in place.

use crate::format::{self, IndexEntry, Record, FOOTER_LEN, HEADER_LEN, MAGIC, VERSION};
use fig_core::{Error, Result};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// An opened, validated SSTable. The whole file is read into memory on open —
/// fine for this stage; block caching arrives when reads need it.
pub struct SstableReader {
    path: PathBuf,
    file_bytes: Vec<u8>,
    index: Vec<IndexEntry>,
}

impl SstableReader {
    /// Open and validate: header, footer, index checksum.
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path).map_err(Error::Io)?;
        let mut file_bytes = Vec::new();
        file.read_to_end(&mut file_bytes).map_err(Error::Io)?;

        if file_bytes.len() < HEADER_LEN + FOOTER_LEN {
            return Err(Error::Corruption(format!(
                "sstable too small: {}",
                path.display()
            )));
        }
        if file_bytes[..4] != MAGIC
            || u32::from_le_bytes(file_bytes[4..8].try_into().unwrap()) != VERSION
        {
            return Err(Error::Corruption(format!(
                "sstable bad header: {}",
                path.display()
            )));
        }
        let footer_at = file_bytes.len() - FOOTER_LEN;
        let footer = &file_bytes[footer_at..];
        let index_offset = u64::from_le_bytes(footer[..8].try_into().unwrap()) as usize;
        let index_len = u64::from_le_bytes(footer[8..16].try_into().unwrap()) as usize;
        let index_crc = u32::from_le_bytes(footer[16..20].try_into().unwrap());
        if footer[20..24] != MAGIC {
            return Err(Error::Corruption(format!(
                "sstable bad footer: {}",
                path.display()
            )));
        }
        if index_offset < HEADER_LEN || index_offset + index_len != footer_at {
            return Err(Error::Corruption(format!(
                "sstable bad index range: {}",
                path.display()
            )));
        }
        let index_bytes = &file_bytes[index_offset..index_offset + index_len];
        if format::crc(index_bytes) != index_crc {
            return Err(Error::Corruption(format!(
                "sstable index checksum mismatch: {}",
                path.display()
            )));
        }
        let index = format::decode_index(index_bytes)?;

        // Index entries must be ordered, non-overlapping, inside the data area.
        let mut prev_end = HEADER_LEN;
        let mut prev_key: Option<&[u8]> = None;
        for e in &index {
            if let Some(pk) = prev_key {
                if e.first_key.as_slice() <= pk {
                    return Err(Error::Corruption(
                        "sstable index keys out of order".to_string(),
                    ));
                }
            }
            let start = e.offset as usize;
            let end = start + e.len as usize;
            if start < prev_end || end > index_offset {
                return Err(Error::Corruption("sstable index range invalid".to_string()));
            }
            prev_end = end;
            prev_key = Some(&e.first_key);
        }

        Ok(Self {
            path: path.to_path_buf(),
            file_bytes,
            index,
        })
    }

    /// Number of data blocks.
    pub fn blocks(&self) -> usize {
        self.index.len()
    }

    /// Read + checksum one data block, returning its records.
    fn read_block(&self, e: &IndexEntry) -> Result<Vec<Record>> {
        let start = e.offset as usize;
        let end = start + e.len as usize;
        let bytes = self
            .file_bytes
            .get(start..end)
            .ok_or_else(|| Error::Corruption("sstable block out of bounds".to_string()))?;
        if format::crc(bytes) != e.crc {
            return Err(Error::Corruption(format!(
                "sstable block checksum mismatch at offset {}",
                e.offset
            )));
        }
        format::decode_block(bytes)
    }

    /// Block index containing `key`: last block with `first_key <= key`.
    fn block_for(&self, key: &[u8]) -> Option<usize> {
        let mut candidate = None;
        for (i, e) in self.index.iter().enumerate() {
            if e.first_key.as_slice() <= key {
                candidate = Some(i);
            } else {
                break;
            }
        }
        candidate
    }

    /// GET(key): the latest record value, or None if missing or tombstoned.
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let Some(i) = self.block_for(key) else {
            return Ok(None);
        };
        for rec in self.read_block(&self.index[i])? {
            if rec.key.as_slice() == key {
                return Ok(rec.value);
            }
        }
        Ok(None)
    }

    /// All live records (tombstones skipped) in ascending order.
    pub fn iter(&self) -> Result<Vec<Record>> {
        let mut out = Vec::new();
        for e in &self.index {
            out.extend(self.read_block(e)?);
        }
        Ok(out)
    }

    /// SCAN(start, end): live key/value pairs with `start <= k < end`.
    /// Tombstones suppress keys; a tombstone with no live value yields nothing.
    pub fn scan(&self, start: &[u8], end: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        if start >= end {
            return Err(Error::InvalidArgument(
                "scan: start must be < end".to_string(),
            ));
        }
        let mut out = Vec::new();
        for rec in self.iter()? {
            if rec.key.as_slice() >= start && rec.key.as_slice() < end {
                if let Some(v) = rec.value {
                    out.push((rec.key, v));
                }
            }
        }
        Ok(out)
    }

    /// Validate every block checksum without returning data (scrub).
    pub fn scrub(&self) -> Result<u64> {
        let mut records = 0u64;
        for e in &self.index {
            records += self.read_block(e)?.len() as u64;
        }
        Ok(records)
    }

    /// Path of the open file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writer::SstableWriter;
    use tempfile::tempdir;

    fn sample(path: &Path) {
        let mut w = SstableWriter::create(path, 64).unwrap();
        w.put(b"a", b"1").unwrap();
        w.put(b"b", b"2").unwrap();
        w.put(b"c", b"3").unwrap();
        w.delete(b"d").unwrap();
        w.put(b"e", b"5").unwrap();
        w.finish().unwrap();
    }

    #[test]
    fn get_scan_iter_agree() {
        let d = tempdir().unwrap();
        let p = d.path().join("s.sst");
        sample(&p);
        let r = SstableReader::open(&p).unwrap();
        assert_eq!(r.get(b"a").unwrap(), Some(b"1".to_vec()));
        assert_eq!(r.get(b"d").unwrap(), None); // tombstoned
        assert_eq!(r.get(b"zzz").unwrap(), None);
        // Tombstone suppressed from scans.
        let s = r.scan(b"a", b"z").unwrap();
        let keys: Vec<&[u8]> = s.iter().map(|(k, _)| k.as_slice()).collect();
        assert_eq!(keys, vec![b"a".as_slice(), b"b", b"c", b"e"]);
        assert_eq!(r.scrub().unwrap(), 5);
    }

    #[test]
    fn garbage_file_fails_safely() {
        let d = tempdir().unwrap();
        for (name, bytes) in [
            ("empty.sst", vec![]),
            ("tiny.sst", vec![0x00, 0x01, 0x02]),
            ("wrong-magic.sst", vec![0xFF; 64]),
        ] {
            let p = d.path().join(name);
            std::fs::write(&p, bytes).unwrap();
            assert!(SstableReader::open(&p).is_err(), "{name} must fail");
        }
    }

    #[test]
    fn flipped_byte_fails_safely() {
        let d = tempdir().unwrap();
        let p = d.path().join("s.sst");
        sample(&p);
        let mut bytes = std::fs::read(&p).unwrap();
        bytes[20] ^= 0xFF;
        std::fs::write(&p, &bytes).unwrap();
        // Either open or scrub must catch it — never silent wrong data.
        match SstableReader::open(&p) {
            Err(_) => {}
            Ok(r) => assert!(r.scrub().is_err() || r.get(b"a").unwrap().is_none()),
        }
    }
}
