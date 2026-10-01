//! Single WAL segment file.
//!
//! Handles header validation, appends, and full-file replay with torn-tail vs
//! corruption distinction.

use crate::record::{self, DecodeErr, WalEntry, FILE_HEADER_LEN, FILE_MAGIC, FILE_VERSION};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Outcome of replaying one segment.
#[derive(Debug)]
pub struct ReplayOut {
    pub entries: Vec<WalEntry>,
    /// Bytes truncated as torn tail (clean tear at EOF).
    pub torn_truncated_bytes: u64,
    /// Offset of first corrupt frame, if any (not a clean tear).
    pub corrupt_offset: Option<u64>,
}

/// A segment file handle (append side).
pub struct Segment {
    pub path: PathBuf,
    pub file: File,
    /// Logical size (bytes on disk).
    pub size: u64,
    /// Bytes durable via fsync.
    pub synced_size: u64,
}

pub fn segment_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("wal-{id:06}.log"))
}

pub fn list_segments(dir: &Path) -> std::io::Result<Vec<(u64, PathBuf)>> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for ent in std::fs::read_dir(dir)? {
        let ent = ent?;
        let name = ent.file_name().to_string_lossy().to_string();
        if let Some(id) = name
            .strip_prefix("wal-")
            .and_then(|s| s.strip_suffix(".log"))
            .and_then(|s| s.parse::<u64>().ok())
        {
            out.push((id, ent.path()));
        }
    }
    out.sort_by_key(|(id, _)| *id);
    Ok(out)
}

/// Open (creating if needed) a segment for append. Validates or writes the header.
pub fn open_for_append(path: &Path) -> std::io::Result<Segment> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    let size = file.seek(SeekFrom::End(0))?;
    let mut synced_size = size;
    if size == 0 {
        let mut hdr = Vec::with_capacity(FILE_HEADER_LEN);
        hdr.extend_from_slice(&FILE_MAGIC);
        hdr.extend_from_slice(&FILE_VERSION.to_le_bytes());
        file.write_all(&hdr)?;
        file.sync_data()?;
        synced_size = FILE_HEADER_LEN as u64;
    } else if size < FILE_HEADER_LEN as u64 {
        // Torn header from a crash before the first header fsync — handled at
        // replay time; keep size as-is here.
    }
    Ok(Segment {
        path: path.to_path_buf(),
        file,
        size: std::cmp::max(size, 0),
        synced_size,
    })
}

/// Replay an entire segment file. Returns entries + tear/corruption info.
/// On torn tail, the caller should truncate the file to `valid_up_to`.
pub fn replay(path: &Path) -> std::io::Result<(ReplayOut, u64)> {
    let mut file = File::open(path)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;

    // Header check.
    if buf.len() < FILE_HEADER_LEN {
        // Empty or torn header → whole file is torn tail.
        return Ok((
            ReplayOut {
                entries: Vec::new(),
                torn_truncated_bytes: buf.len() as u64,
                corrupt_offset: None,
            },
            0,
        ));
    }
    if buf[0..4] != FILE_MAGIC || u32::from_le_bytes(buf[4..8].try_into().unwrap()) != FILE_VERSION
    {
        return Ok((
            ReplayOut {
                entries: Vec::new(),
                torn_truncated_bytes: 0,
                corrupt_offset: Some(0),
            },
            0,
        ));
    }

    let mut entries = Vec::new();
    let mut off = FILE_HEADER_LEN as u64;
    while (off as usize) < buf.len() {
        let slice = &buf[off as usize..];
        match record::decode(slice, off) {
            Ok((e, n)) => {
                entries.push(e);
                off += n as u64;
            }
            Err(DecodeErr::Torn { .. }) => {
                let torn = buf.len() as u64 - off;
                return Ok((
                    ReplayOut {
                        entries,
                        torn_truncated_bytes: torn,
                        corrupt_offset: None,
                    },
                    off,
                ));
            }
            Err(DecodeErr::Corrupt { .. } | DecodeErr::BadOp(_)) => {
                return Ok((
                    ReplayOut {
                        entries,
                        torn_truncated_bytes: 0,
                        corrupt_offset: Some(off),
                    },
                    off,
                ));
            }
        }
    }
    Ok((
        ReplayOut {
            entries,
            torn_truncated_bytes: 0,
            corrupt_offset: None,
        },
        off,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{encode, WalOp};
    use tempfile::tempdir;

    #[test]
    fn header_roundtrip() {
        let d = tempdir().unwrap();
        let p = segment_path(d.path(), 1);
        {
            let mut seg = open_for_append(&p).unwrap();
            let mut buf = Vec::new();
            encode(1, &WalOp::Put(b"k".to_vec(), b"v".to_vec()), &mut buf);
            seg.file.write_all(&buf).unwrap();
            seg.file.sync_data().unwrap();
        }
        let (out, valid) = replay(&p).unwrap();
        assert_eq!(out.entries.len(), 1);
        assert_eq!(out.corrupt_offset, None);
        assert_eq!(
            valid as usize,
            FILE_HEADER_LEN + {
                let mut b = Vec::new();
                encode(1, &WalOp::Put(b"k".to_vec(), b"v".to_vec()), &mut b);
                b.len()
            }
        );
    }
}
