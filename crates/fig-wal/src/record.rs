//! WAL record framing.
//!
//! ```text
//! frame := frame_len:u32 | seq:u64 | op:u8 | key_len:u32 | key | [value_len:u32 | value] | crc:u32
//! ```
//!
//! - `frame_len` = bytes following it up to and including `crc` (so a torn tail is
//!   detectable as `remaining < 4` or `remaining < frame_len`).
//! - `op`: `1 = Put`, `2 = Delete`.
//! - `crc` = CRC-32 (IEEE) over `seq .. value` (everything after `frame_len`, before `crc`).
//!
//! Segment files start with an 8-byte header: magic `LBW1` + version `u32 LE 1`.
//! A file shorter than 8 bytes is treated as a torn/empty tail.

use crc32fast::Hasher;

pub const OP_PUT: u8 = 1;
pub const OP_DELETE: u8 = 2;

pub const FILE_MAGIC: [u8; 4] = [0x4C, 0x42, 0x57, 0x31]; // "LBW1"
pub const FILE_VERSION: u32 = 1;
pub const FILE_HEADER_LEN: usize = 8;

/// A logical WAL entry (sequence number assigned by [`crate::Wal`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalEntry {
    pub seq: u64,
    pub op: WalOp,
}

/// Put or Delete operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalOp {
    Put(Vec<u8>, Vec<u8>),
    Delete(Vec<u8>),
}

/// Encode one frame (without file header) into `out`.
pub fn encode(seq: u64, op: &WalOp, out: &mut Vec<u8>) {
    let mut body = Vec::with_capacity(64);
    body.extend_from_slice(&seq.to_le_bytes());
    match op {
        WalOp::Put(k, v) => {
            body.push(OP_PUT);
            body.extend_from_slice(&(k.len() as u32).to_le_bytes());
            body.extend_from_slice(k);
            body.extend_from_slice(&(v.len() as u32).to_le_bytes());
            body.extend_from_slice(v);
        }
        WalOp::Delete(k) => {
            body.push(OP_DELETE);
            body.extend_from_slice(&(k.len() as u32).to_le_bytes());
            body.extend_from_slice(k);
        }
    }
    let mut h = Hasher::new();
    h.update(&body);
    let crc = h.finalize();

    out.extend_from_slice(&(body.len() as u32 + 4).to_le_bytes());
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc.to_le_bytes());
}

/// Decode error kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeErr {
    /// Not enough bytes for a full frame (torn tail). `needed` may exceed `available`.
    Torn { available: usize, needed: usize },
    /// Checksum mismatch (corruption, not a clean tear).
    Corrupt { seq: Option<u64>, offset: u64 },
    /// Unknown op code (corruption / version skew).
    BadOp(u8),
}

/// Try to decode one frame at `data`. Returns `(entry, frame_total_len)` on success.
/// `base_offset` is the file offset of `data[0]` (for error reporting).
pub fn decode(data: &[u8], base_offset: u64) -> Result<(WalEntry, usize), DecodeErr> {
    if data.len() < 4 {
        return Err(DecodeErr::Torn {
            available: data.len(),
            needed: 4,
        });
    }
    let frame_len = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    if frame_len < 8 + 1 + 4 + 4 {
        // Minimum: seq(8) + op(1) + key_len(4) + crc(4)
        return Err(DecodeErr::Corrupt {
            seq: None,
            offset: base_offset,
        });
    }
    if data.len() < 4 + frame_len {
        return Err(DecodeErr::Torn {
            available: data.len(),
            needed: 4 + frame_len,
        });
    }
    let body = &data[4..4 + frame_len - 4];
    let stored_crc = u32::from_le_bytes(data[4 + frame_len - 4..4 + frame_len].try_into().unwrap());
    let mut h = Hasher::new();
    h.update(body);
    if h.finalize() != stored_crc {
        // Best-effort seq extraction for diagnostics.
        let seq = body
            .get(0..8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()));
        return Err(DecodeErr::Corrupt {
            seq,
            offset: base_offset,
        });
    }
    let seq = u64::from_le_bytes(body[0..8].try_into().unwrap());
    let op = body[8];
    let mut pos = 9;
    let read_u32 = |pos: &mut usize| -> Option<u32> {
        if body.len() < *pos + 4 {
            return None;
        }
        let v = u32::from_le_bytes(body[*pos..*pos + 4].try_into().unwrap());
        *pos += 4;
        Some(v)
    };
    match op {
        OP_PUT => {
            let kl = read_u32(&mut pos).ok_or(DecodeErr::Corrupt {
                seq: Some(seq),
                offset: base_offset,
            })? as usize;
            if body.len() < pos + kl + 4 {
                return Err(DecodeErr::Corrupt {
                    seq: Some(seq),
                    offset: base_offset,
                });
            }
            let key = body[pos..pos + kl].to_vec();
            pos += kl;
            let vl = read_u32(&mut pos).ok_or(DecodeErr::Corrupt {
                seq: Some(seq),
                offset: base_offset,
            })? as usize;
            if body.len() != pos + vl {
                return Err(DecodeErr::Corrupt {
                    seq: Some(seq),
                    offset: base_offset,
                });
            }
            let value = body[pos..pos + vl].to_vec();
            Ok((
                WalEntry {
                    seq,
                    op: WalOp::Put(key, value),
                },
                4 + frame_len,
            ))
        }
        OP_DELETE => {
            let kl = read_u32(&mut pos).ok_or(DecodeErr::Corrupt {
                seq: Some(seq),
                offset: base_offset,
            })? as usize;
            if body.len() != pos + kl {
                return Err(DecodeErr::Corrupt {
                    seq: Some(seq),
                    offset: base_offset,
                });
            }
            let key = body[pos..pos + kl].to_vec();
            Ok((
                WalEntry {
                    seq,
                    op: WalOp::Delete(key),
                },
                4 + frame_len,
            ))
        }
        other => Err(DecodeErr::BadOp(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_put_delete() {
        for (seq, op) in [
            (1u64, WalOp::Put(b"k".to_vec(), b"v".to_vec())),
            (2u64, WalOp::Delete(b"k".to_vec())),
            (u64::MAX, WalOp::Put(vec![], vec![0; 100])),
        ] {
            let mut buf = Vec::new();
            encode(seq, &op, &mut buf);
            let (e, n) = decode(&buf, 0).unwrap();
            assert_eq!(n, buf.len());
            assert_eq!(e.seq, seq);
            assert_eq!(e.op, op);
        }
    }

    #[test]
    fn torn_prefix_is_torn_not_corrupt() {
        let mut buf = Vec::new();
        encode(7, &WalOp::Put(b"k".to_vec(), b"v".to_vec()), &mut buf);
        for cut in [0, 1, 3, 4, 5, buf.len() - 1] {
            match decode(&buf[..cut], 0) {
                Err(DecodeErr::Torn { .. }) => {}
                other => panic!("cut {cut}: expected Torn, got {other:?}"),
            }
        }
    }

    #[test]
    fn single_bit_flip_is_corrupt() {
        let mut buf = Vec::new();
        encode(9, &WalOp::Put(b"key".to_vec(), b"value".to_vec()), &mut buf);
        buf[10] ^= 0x01;
        assert!(matches!(decode(&buf, 0), Err(DecodeErr::Corrupt { .. })));
    }
}
