//! SSTable file layout (version 1).
//!
//! ```text
//! file := header | data_block+ | index_block | footer
//! header := magic "LBST" | version:u32
//! record := op:u8 (1=Put, 2=Delete) | key_len:u32 | key | [value_len:u32 | value]
//! data_block := record+                     (cut at ~target size by the writer)
//! index_entry := key_len:u32 | first_key | offset:u64 | len:u64 | crc:u32
//! index_block := count:u32 | index_entry+
//! footer := index_offset:u64 | index_len:u64 | index_crc:u32 | magic "LBST"
//! ```
//!
//! - Keys inside a file are strictly ascending; the writer enforces it.
//! - `crc` on an index entry covers its data block's bytes; `index_crc` covers
//!   the index block. A mismatch means corruption, never a torn tail —
//!   SSTables are written fully and synced before they are ever read.
//! - `Delete` records are tombstones (value absent); merging layers resolve them.

use fig_core::{Error, Result};

pub const MAGIC: [u8; 4] = [0x4C, 0x42, 0x53, 0x54]; // "LBST"
pub const VERSION: u32 = 1;
pub const HEADER_LEN: usize = 8;
/// index_offset(8) + index_len(8) + index_crc(4) + magic(4).
pub const FOOTER_LEN: usize = 24;

pub const OP_PUT: u8 = 1;
pub const OP_DELETE: u8 = 2;

/// One sorted record. `value: None` is a tombstone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub key: Vec<u8>,
    pub value: Option<Vec<u8>>,
}

impl Record {
    pub fn put(key: Vec<u8>, value: Vec<u8>) -> Self {
        Self {
            key,
            value: Some(value),
        }
    }

    pub fn delete(key: Vec<u8>) -> Self {
        Self { key, value: None }
    }
}

/// One index entry: the first key of a data block and where to find it.
#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub first_key: Vec<u8>,
    pub offset: u64,
    pub len: u64,
    pub crc: u32,
}

/// CRC-32 (IEEE) over `bytes`.
pub fn crc(bytes: &[u8]) -> u32 {
    let mut h = crc32fast::Hasher::new();
    h.update(bytes);
    h.finalize()
}

fn corrupt(msg: impl Into<String>) -> Error {
    Error::Corruption(msg.into())
}

/// Append one record to `out`.
pub fn encode_record(rec: &Record, out: &mut Vec<u8>) {
    match &rec.value {
        Some(v) => {
            out.push(OP_PUT);
            out.extend_from_slice(&(rec.key.len() as u32).to_le_bytes());
            out.extend_from_slice(&rec.key);
            out.extend_from_slice(&(v.len() as u32).to_le_bytes());
            out.extend_from_slice(v);
        }
        None => {
            out.push(OP_DELETE);
            out.extend_from_slice(&(rec.key.len() as u32).to_le_bytes());
            out.extend_from_slice(&rec.key);
        }
    }
}

/// Decode all records in one data block. Any truncation is corruption —
/// SSTables are never read torn.
pub fn decode_block(mut bytes: &[u8]) -> Result<Vec<Record>> {
    let mut out = Vec::new();
    while !bytes.is_empty() {
        let op = bytes[0];
        bytes = &bytes[1..];
        if bytes.len() < 4 {
            return Err(corrupt("sstable: truncated key len"));
        }
        let kl = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        bytes = &bytes[4..];
        if bytes.len() < kl {
            return Err(corrupt("sstable: truncated key"));
        }
        let key = bytes[..kl].to_vec();
        bytes = &bytes[kl..];
        match op {
            OP_PUT => {
                if bytes.len() < 4 {
                    return Err(corrupt("sstable: truncated value len"));
                }
                let vl = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
                bytes = &bytes[4..];
                if bytes.len() < vl {
                    return Err(corrupt("sstable: truncated value"));
                }
                let value = bytes[..vl].to_vec();
                bytes = &bytes[vl..];
                out.push(Record::put(key, value));
            }
            OP_DELETE => out.push(Record::delete(key)),
            other => return Err(corrupt(format!("sstable: bad op {other}"))),
        }
    }
    Ok(out)
}

/// Append one index entry to `out`.
pub fn encode_index_entry(e: &IndexEntry, out: &mut Vec<u8>) {
    out.extend_from_slice(&(e.first_key.len() as u32).to_le_bytes());
    out.extend_from_slice(&e.first_key);
    out.extend_from_slice(&e.offset.to_le_bytes());
    out.extend_from_slice(&e.len.to_le_bytes());
    out.extend_from_slice(&e.crc.to_le_bytes());
}

/// Decode the index block (without footer).
pub fn decode_index(mut bytes: &[u8]) -> Result<Vec<IndexEntry>> {
    if bytes.len() < 4 {
        return Err(corrupt("sstable: truncated index count"));
    }
    let count = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    bytes = &bytes[4..];
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if bytes.len() < 4 {
            return Err(corrupt("sstable: truncated index key len"));
        }
        let kl = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        bytes = &bytes[4..];
        if bytes.len() < kl + 8 + 8 + 4 {
            return Err(corrupt("sstable: truncated index entry"));
        }
        let first_key = bytes[..kl].to_vec();
        bytes = &bytes[kl..];
        let offset = u64::from_le_bytes(bytes[..8].try_into().unwrap());
        let len = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        let crc = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
        bytes = &bytes[20..];
        out.push(IndexEntry {
            first_key,
            offset,
            len,
            crc,
        });
    }
    if !bytes.is_empty() {
        return Err(corrupt("sstable: trailing bytes in index"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_roundtrip_with_tombstone() {
        let recs = vec![
            Record::put(b"a".to_vec(), b"1".to_vec()),
            Record::delete(b"b".to_vec()),
            Record::put(vec![], vec![0; 64]),
        ];
        let mut buf = Vec::new();
        for r in &recs {
            encode_record(r, &mut buf);
        }
        assert_eq!(decode_block(&buf).unwrap(), recs);
    }

    #[test]
    fn truncated_block_is_corruption() {
        let mut buf = Vec::new();
        encode_record(&Record::put(b"key".to_vec(), b"value".to_vec()), &mut buf);
        for cut in [1, 5, buf.len() - 1] {
            assert!(decode_block(&buf[..cut]).is_err(), "cut {cut} must fail");
        }
    }

    #[test]
    fn index_roundtrip() {
        let entries = vec![
            IndexEntry {
                first_key: b"a".to_vec(),
                offset: 8,
                len: 100,
                crc: 12345,
            },
            IndexEntry {
                first_key: b"m".to_vec(),
                offset: 108,
                len: 200,
                crc: 67890,
            },
        ];
        let mut buf = Vec::new();
        buf.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for e in &entries {
            encode_index_entry(e, &mut buf);
        }
        let back = decode_index(&buf).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].first_key, b"a");
        assert_eq!(back[1].offset, 108);
    }
}
