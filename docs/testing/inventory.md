# Test inventory (76 passing)

Tests live next to the code — unit tests in `src/` files, crash tests in
`tests/`. CI runs all of this on every push
([workflow](../../.github/workflows/ci.yml)).

| File | What it proves |
|---|---|
| [`crates/fig-core/src/kv.rs`](../../crates/fig-core/src/kv.rs) (4 tests) | PUT/GET/DELETE roundtrip, byte ordering, scan bounds, limit enforcement |
| [`crates/fig-core/src/ops.rs`](../../crates/fig-core/src/ops.rs) (3 tests) | `MemoryKv` agrees with `ReferenceKv` on seeded streams up to 10k ops |
| [`crates/fig-core/src/config.rs`](../../crates/fig-core/src/config.rs) (2 tests) + [`reference.rs`](../../crates/fig-core/src/reference.rs) (1) + [`error.rs`](../../crates/fig-core/src/error.rs) (1) + [`tracing_util.rs`](../../crates/fig-core/src/tracing_util.rs) (1) | Config validation, oracle basics, stable error codes, idempotent tracing init |
| [`crates/fig-wal/src/record.rs`](../../crates/fig-wal/src/record.rs) (3 tests) | Frame roundtrip, torn prefix reads as torn (not corrupt), bit flips detected |
| [`crates/fig-wal/src/segment.rs`](../../crates/fig-wal/src/segment.rs) (1 test) | Header + single-frame write/replay |
| [`crates/fig-wal/src/wal.rs`](../../crates/fig-wal/src/wal.rs) (2 tests) | Dense seqnos across reopen, order preserved across rotation |
| [`crates/fig-wal/tests/crash.rs`](../../crates/fig-wal/tests/crash.rs) (3 tests) | Acked prefix survives 20 seeded crash campaigns; mid-file corruption truncates the suffix; torn tails never replay |
| [`crates/fig-storage/src/lib.rs`](../../crates/fig-storage/src/lib.rs) (3 tests) | Roundtrip, reopen replays the log, rejected writes leave no trace |
| [`crates/fig-storage/tests/recovery.rs`](../../crates/fig-storage/tests/recovery.rs) (2 tests) | 15 seeded kill/restart campaigns preserve acked writes; torn tails never half-apply |
| [`crates/fig-sstable/src/format.rs`](../../crates/fig-sstable/src/format.rs) (3 tests) | Record/index roundtrips with tombstones; truncation reads as corruption |
| [`crates/fig-sstable/src/writer.rs`](../../crates/fig-sstable/src/writer.rs) (3 tests) | Out-of-order keys rejected, no overwrite, empty table finishes |
| [`crates/fig-sstable/src/reader.rs`](../../crates/fig-sstable/src/reader.rs) (3 tests) | Get/scan/iter agree with tombstone suppression; garbage and bit flips fail safely |
| [`crates/fig-sstable/tests/sstable.rs`](../../crates/fig-sstable/tests/sstable.rs) (2 tests) | 20 seeded streams match the oracle on gets, scans, tombstones |
| [`crates/fig-storage/src/bloom.rs`](../../crates/fig-storage/src/bloom.rs) (3 tests) | No false negatives; false-positive rate under target |
| [`crates/fig-storage/src/lsm.rs`](../../crates/fig-storage/src/lsm.rs) (3 tests) | Flush moves reads to tables; deletes shadow older tables; empty flush is a noop |
| [`crates/fig-storage/src/manifest.rs`](../../crates/fig-storage/src/manifest.rs) (5 tests) | Manifest roundtrips; staging litter reaped; unlisted tables removed; corrupt manifest is `Corruption` |
| [`crates/fig-storage/tests/lsm.rs`](../../crates/fig-storage/tests/lsm.rs) (2 tests) | Wide-key workloads match the oracle across flushes, kills, restarts |
| [`crates/fig-storage/tests/crash_mid_flush.rs`](../../crates/fig-storage/tests/crash_mid_flush.rs) (4 tests) | SIGKILL litter at each flush window reopens clean; unlisted tables never leak; legacy dirs adopted |
| [`crates/fig-storage/tests/compaction.rs`](../../crates/fig-storage/tests/compaction.rs) (6 tests) | Random compact/write/kill streams match the oracle; full-merge GC reclaims files; auto-policy bounds tables at 8; both compact crash windows reopen clean; inline-off + background-step contract |
| [`crates/fig-server/tests/tcp.rs`](../../crates/fig-server/tests/tcp.rs) (6 tests) | Wire roundtrip incl. binary keys; garbage input rejected, connection survives; acked data survives full restart; 8 concurrent clients stay correct; background timer folds tables unaided |
| [`crates/fig-raft/tests/raft_sim.rs`](../../crates/fig-raft/tests/raft_sim.rs) (6 tests) | One leader on 10 seeds; ordered replication + apply contract; solo self-election; minority isolation with no rival; leader failover preserving the committed prefix; monotonic terms |
| [`crates/fig-raft/src/persist.rs`](../../crates/fig-raft/src/persist.rs) (4 tests) | Vote + entries survive reopen; heartbeats write nothing; tmp litter reaped, corrupt state is `Corruption`; uncommitted suffix may be overwritten, never applied pre-commit |

Run everything:

```bash
./scripts/check.sh   # fmt --check + clippy -D warnings + full test suite
```

Or scoped:

```bash
cargo test -p fig-core            # oracle + differential tests
cargo test -p fig-wal            # unit tests
cargo test -p fig-wal --test crash   # crash/restart gate (also its own CI job)
cargo test -p fig-storage        # engine + kill/restart gate
cargo test -p fig-sstable        # format, writer, reader, differential gate
cargo test -p fig-server         # TCP wire gate (roundtrip, garbage, restart)
cargo test -p fig-raft           # sim gate (election, replication, partitions) + persistence gate
```
