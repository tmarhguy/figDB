# Status: what's real

This is the checked-in inventory. Everything above the storage engine —
server replication wiring, transactions — is next, not here.

| Piece | Where | State |
|---|---|---|
| Ordered KV model (`MemoryKv`: GET/PUT/DELETE/SCAN over bytes) | [`crates/fig-core/src/kv.rs`](../../crates/fig-core/src/kv.rs) | done, tested |
| Naive oracle (`ReferenceKv`) + seeded operation streams | [`crates/fig-core/src/reference.rs`](../../crates/fig-core/src/reference.rs), [`ops.rs`](../../crates/fig-core/src/ops.rs) | done, tested |
| Checksummed WAL: framed records, seqnos, segments, replay, torn-tail vs corruption recovery | [`crates/fig-wal/src/`](../../crates/fig-wal/src/) | done, tested |
| WAL-backed memtable (`Engine`): `PUT → WAL → memtable`, `restart → replay` | [`crates/fig-storage/src/lib.rs`](../../crates/fig-storage/src/lib.rs) | done, tested |
| Immutable SSTables: checksummed blocks, index, footer, tombstones | [`crates/fig-sstable/src/`](../../crates/fig-sstable/src/) | done, tested |
| LSM database: threshold flushing, Bloom filters, merged reads, metrics | [`crates/fig-storage/src/lsm.rs`](../../crates/fig-storage/src/lsm.rs) | done, tested |
| Crash-safe publishing: `MANIFEST` (tmp→rename→fsync) defines the live table set; orphans/litter reaped at open | [`crates/fig-storage/src/manifest.rs`](../../crates/fig-storage/src/manifest.rs) | done, tested |
| Size-tiered compaction: oldest-8 auto-merge, manual full merge, tombstone GC | [`crates/fig-storage/src/lsm.rs`](../../crates/fig-storage/src/lsm.rs) (`compact`) | done, tested |
| TCP server + CLI: JSON-lines `put/get/delete/scan/sync/flush/compact/stats`, base64 values; snapshot reads, RwLock, background compaction timer | [`crates/fig-server/src/`](../../crates/fig-server/src/) | done, tested |
| Load generator + soak: `fig-bench` (ops/s, p50/p99 over TCP), `scripts/soak.sh` (kill-9 every cycle) | [`crates/fig-server/src/bench.rs`](../../crates/fig-server/src/bench.rs), [`scripts/soak.sh`](../../scripts/soak.sh) | done, measured |
| Deterministic Raft core (`Node`): elections, log matching, current-term commit, exactly-once in-order apply — no I/O, no clock | [`crates/fig-raft/src/core.rs`](../../crates/fig-raft/src/core.rs) | done, tested |
| Seeded simulation harness: virtual ticks, partitions, drops; proves single-leader, convergence, failover | [`crates/fig-raft/src/sim.rs`](../../crates/fig-raft/src/sim.rs), [`tests/raft_sim.rs`](../../crates/fig-raft/tests/raft_sim.rs) | done, tested |
| Raft durability: `current_term`/`voted_for`/log via `Dirty` tracking + crash-safe `Store` (tmp→rename→fsync); restart replays, heartbeats write nothing | [`crates/fig-raft/src/persist.rs`](../../crates/fig-raft/src/persist.rs) | done, tested |
| Shared errors, config limits, tracing bootstrap | [`crates/fig-core/src/`](../../crates/fig-core/src/error.rs) | done, tested |

Next up: R3 server integration — client writes replicate to a majority
before ack, followers answer `NOT_LEADER` naming the known leader, leaders
serve reads after a commit barrier. The write ceiling is still the WAL, so
redundancy (not local speed) remains the bottleneck worth attacking.
