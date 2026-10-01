//! Naive reference KV oracle.
//!
//! `ReferenceKv` is intentionally *not* optimized: a linear `Vec` of pairs with
//! O(n) operations and an explicit sort on scan. `MemoryKv` (BTreeMap) must agree
//! with it on every operation stream — if the two ever diverge, `MemoryKv` is wrong.
//!
//! Later storage engines (WAL → memtable → SSTable → LSM) reuse the same harness:
//! they must also agree with this oracle.

use crate::kv::{Op, OpResult};

/// Deliberately naive map: insertion-ordered vec, linear lookup.
#[derive(Debug, Default, Clone)]
pub struct ReferenceKv {
    pairs: Vec<(Vec<u8>, Vec<u8>)>,
}

impl ReferenceKv {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.pairs
            .iter()
            .rev()
            .find(|(k, _)| k.as_slice() == key)
            .map(|(_, v)| v.clone())
    }

    pub fn put(&mut self, key: Vec<u8>, value: Vec<u8>) -> Option<Vec<u8>> {
        let prev = self.get(&key);
        if let Some(slot) = self.pairs.iter_mut().find(|(k, _)| *k == key) {
            slot.1 = value;
        } else {
            self.pairs.push((key, value));
        }
        prev
    }

    pub fn delete(&mut self, key: &[u8]) -> bool {
        let before = self.pairs.len();
        self.pairs.retain(|(k, _)| k.as_slice() != key);
        self.pairs.len() != before
    }

    pub fn scan(&self, start: &[u8], end: &[u8]) -> crate::Result<Vec<(Vec<u8>, Vec<u8>)>> {
        if start >= end {
            return Err(crate::Error::InvalidArgument(
                "scan: start must be < end".to_string(),
            ));
        }
        let mut out: Vec<(Vec<u8>, Vec<u8>)> = self
            .pairs
            .iter()
            .filter(|(k, _)| k.as_slice() >= start && k.as_slice() < end)
            .cloned()
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    /// Apply an [`Op`], mirroring [`crate::kv::MemoryKv::apply`].
    pub fn apply(&mut self, op: &Op) -> OpResult {
        match op {
            Op::Put { key, value } => OpResult::Put(self.put(key.clone(), value.clone())),
            Op::Delete { key } => OpResult::Delete(self.delete(key)),
            Op::Get { key } => OpResult::Get(self.get(key)),
            Op::Scan { start, end } => match self.scan(start, end) {
                Ok(pairs) => OpResult::Scan(pairs),
                Err(e) => OpResult::ScanErr(e.to_string()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_agrees_on_basics() {
        let mut r = ReferenceKv::new();
        assert_eq!(r.get(b"k"), None);
        r.put(b"k".to_vec(), b"v".to_vec());
        assert_eq!(r.get(b"k"), Some(b"v".to_vec()));
        assert!(r.delete(b"k"));
        assert_eq!(r.get(b"k"), None);
    }
}
