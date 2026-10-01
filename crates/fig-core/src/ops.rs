//! Randomized operation streams for differential testing.
//!
//! ```text
//! Generated Operation Stream
//!        │
//!        ├───────────────┐
//!        ↓               ↓
//!    MemoryKv       ReferenceKv
//!        ↓               ↓
//!     result A         result B → compare
//! ```
//!
//! Small key/value alphabets keep collisions likely so overwrites, deletes of
//! missing keys, and scans over sparse ranges are all exercised.

use crate::kv::Op;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// Generate `n` operations from `seed` over a small key space.
pub fn gen_ops(seed: u64, n: usize) -> Vec<Op> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut ops = Vec::with_capacity(n);
    for _ in 0..n {
        let kind: u8 = rng.gen_range(0..100);
        // Tiny alphabet (8 keys, small values) → high collision rate.
        let key = vec![rng.gen_range(0..8u8)];
        let value = vec![rng.gen_range(0..8u8); rng.gen_range(0..4)];
        if kind < 45 {
            ops.push(Op::Put {
                key,
                value: if value.is_empty() { vec![0] } else { value },
            });
        } else if kind < 65 {
            ops.push(Op::Delete { key });
        } else if kind < 85 {
            ops.push(Op::Get { key });
        } else {
            let mut a = vec![rng.gen_range(0..8u8)];
            let mut b = vec![rng.gen_range(0..8u8)];
            if a == b {
                b = vec![8u8]; // force non-empty range sometimes
            }
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            ops.push(Op::Scan { start: a, end: b });
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kv::MemoryKv;
    use crate::reference::ReferenceKv;

    fn run_stream(seed: u64, n: usize) {
        let ops = gen_ops(seed, n);
        let mut a = MemoryKv::new();
        let mut b = ReferenceKv::new();
        for op in &ops {
            assert_eq!(
                a.apply(op),
                b.apply(op),
                "divergence on {op:?} (seed {seed})"
            );
        }
        // Final full-range scan must also agree.
        let full_a = a.scan(&[0x00], &[0xff]).unwrap();
        let full_b = b.scan(&[0x00], &[0xff]).unwrap();
        assert_eq!(full_a, full_b, "final state divergence (seed {seed})");
    }

    #[test]
    fn differential_small_seeds() {
        for seed in 0..25 {
            run_stream(seed, 500);
        }
    }

    #[test]
    fn differential_large_stream() {
        run_stream(0x00C1_0BE8, 10_000);
    }

    #[test]
    fn put_get_property() {
        // PUT(k,v); GET(k) == v · DELETE(k); GET(k) == None
        for seed in 0..10 {
            let ops = gen_ops(seed, 1000);
            let mut kv = MemoryKv::new();
            for op in &ops {
                match op {
                    Op::Put { key, value } => {
                        kv.put(key.clone(), value.clone());
                        assert_eq!(kv.get(key), Some(value.clone()));
                    }
                    Op::Delete { key } => {
                        kv.delete(key);
                        assert_eq!(kv.get(key), None);
                    }
                    _ => {
                        kv.apply(op);
                    }
                }
            }
        }
    }
}
