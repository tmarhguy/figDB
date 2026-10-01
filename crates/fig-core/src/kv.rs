//! Ordered in-memory KV model.
//!
//! This is the correctness oracle for all later storage work (Commits 03–08):
//! every persistent engine must remain logically equivalent to this map.
//!
//! Semantics:
//! - Keys and values are arbitrary bytes (`Vec<u8>`), ordered lexicographically
//!   (`memcmp` order, i.e. Rust `Vec<u8>` `Ord`).
//! - `PUT` inserts or overwrites. `DELETE` removes; deleting a missing key is a no-op.
//! - `GET` returns the latest value or `None`.
//! - `SCAN(start, end)` returns all pairs with `start <= k < end` in ascending order.
//!   An empty range yields an empty vector; `start >= end` is invalid.
//! - Limits from [`crate::Config`] are enforced by the *validated* entry points
//!   (`put_validated`, `delete_validated`); the raw methods assume pre-checked input
//!   so differential tests can focus on ordering semantics.

use std::collections::BTreeMap;
use std::ops::Bound;

/// Ordered in-memory key-value store.
#[derive(Debug, Default, Clone)]
pub struct MemoryKv {
    map: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl MemoryKv {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of live keys.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Every live pair in ascending order. The only complete read;
    /// range queries go through [`MemoryKv::scan`].
    pub fn iter(&self) -> impl Iterator<Item = (Vec<u8>, Vec<u8>)> + '_ {
        self.map.iter().map(|(k, v)| (k.clone(), v.clone()))
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// GET(key) → Option<value>.
    pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.map.get(key).cloned()
    }

    /// PUT(key, value), overwriting any prior value. Returns previous value, if any.
    pub fn put(&mut self, key: Vec<u8>, value: Vec<u8>) -> Option<Vec<u8>> {
        self.map.insert(key, value)
    }

    /// DELETE(key). Returns true if a key was removed.
    pub fn delete(&mut self, key: &[u8]) -> bool {
        self.map.remove(key).is_some()
    }

    /// SCAN(start, end) → ascending pairs with `start <= k < end`.
    ///
    /// Returns [`crate::Error::InvalidArgument`] when `start >= end`.
    pub fn scan(&self, start: &[u8], end: &[u8]) -> crate::Result<Vec<(Vec<u8>, Vec<u8>)>> {
        if start >= end {
            return Err(crate::Error::InvalidArgument(
                "scan: start must be < end".to_string(),
            ));
        }
        Ok(self
            .map
            .range::<[u8], _>((Bound::Included(start), Bound::Excluded(end)))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
    }

    /// Validated PUT enforcing [`crate::Config`] limits.
    pub fn put_validated(
        &mut self,
        cfg: &crate::Config,
        key: Vec<u8>,
        value: Vec<u8>,
    ) -> crate::Result<Option<Vec<u8>>> {
        cfg.check_key(&key)?;
        cfg.check_value(&value)?;
        Ok(self.put(key, value))
    }

    /// Validated DELETE enforcing key limits.
    pub fn delete_validated(&mut self, cfg: &crate::Config, key: &[u8]) -> crate::Result<bool> {
        cfg.check_key(key)?;
        Ok(self.delete(key))
    }

    /// Apply an [`Op`] and return the observable result (used by differential tests).
    pub fn apply(&mut self, op: &Op) -> OpResult {
        match op {
            Op::Put { key, value } => {
                let prev = self.put(key.clone(), value.clone());
                OpResult::Put(prev)
            }
            Op::Delete { key } => OpResult::Delete(self.delete(key)),
            Op::Get { key } => OpResult::Get(self.get(key)),
            Op::Scan { start, end } => match self.scan(start, end) {
                Ok(pairs) => OpResult::Scan(pairs),
                Err(e) => OpResult::ScanErr(e.to_string()),
            },
        }
    }
}

/// A generated KV operation (used by randomized differential tests).
#[derive(Debug, Clone)]
pub enum Op {
    Put { key: Vec<u8>, value: Vec<u8> },
    Delete { key: Vec<u8> },
    Get { key: Vec<u8> },
    Scan { start: Vec<u8>, end: Vec<u8> },
}

/// Observable result of an [`Op`], comparable across implementations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpResult {
    Put(Option<Vec<u8>>),
    Delete(bool),
    Get(Option<Vec<u8>>),
    Scan(Vec<(Vec<u8>, Vec<u8>)>),
    ScanErr(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    #[test]
    fn put_get_delete_roundtrip() {
        let mut kv = MemoryKv::new();
        assert_eq!(kv.get(b"a"), None);
        kv.put(b"a".to_vec(), b"1".to_vec());
        assert_eq!(kv.get(b"a"), Some(b"1".to_vec()));
        kv.put(b"a".to_vec(), b"2".to_vec());
        assert_eq!(kv.get(b"a"), Some(b"2".to_vec()));
        assert!(kv.delete(b"a"));
        assert_eq!(kv.get(b"a"), None);
        assert!(!kv.delete(b"a"));
    }

    #[test]
    fn byte_order_is_memcmp() {
        let mut kv = MemoryKv::new();
        kv.put(vec![0xff], b"x".to_vec());
        kv.put(vec![0x00], b"y".to_vec());
        kv.put(b"a".to_vec(), b"z".to_vec());
        let all = kv.scan(&[0x00], &[0xff, 0xff]).unwrap();
        let keys: Vec<Vec<u8>> = all.into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec![vec![0x00], b"a".to_vec(), vec![0xff]]);
    }

    #[test]
    fn scan_bounds() {
        let mut kv = MemoryKv::new();
        for k in [b"a", b"b", b"c", b"d"] {
            kv.put(k.to_vec(), k.to_vec());
        }
        let r = kv.scan(b"a", b"d").unwrap();
        assert_eq!(r.len(), 3);
        assert!(kv.scan(b"d", b"a").is_err());
        assert!(kv.scan(b"a", b"a").is_err());
        assert!(kv.scan(b"z", &[0xff]).unwrap().is_empty());
    }

    #[test]
    fn validated_entry_points_enforce_limits() {
        let cfg = Config::default();
        let mut kv = MemoryKv::new();
        assert!(kv.put_validated(&cfg, vec![], b"v".to_vec()).is_err());
        assert!(kv
            .put_validated(&cfg, vec![0u8; cfg.limits.max_key_bytes + 1], b"v".to_vec())
            .is_err());
        assert!(kv
            .put_validated(
                &cfg,
                b"k".to_vec(),
                vec![0u8; cfg.limits.max_value_bytes + 1]
            )
            .is_err());
        kv.put_validated(&cfg, b"k".to_vec(), b"v".to_vec())
            .unwrap();
    }
}
