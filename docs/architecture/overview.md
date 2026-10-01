# FigDB Architecture (Commit 01)

Source: `internal.md` §§2–5, §§40–43. This document is the public architecture entry point;
deep dives live in `durability.md`, `consistency.md`, and `docs/adr/`.

## Topology

```text
CLIENTS → Client Library → RPC/API → Router → Txn Coordinator → Shard Directory
  → SHARD A/B/C → Raft Group (N1 N2 N3) → Storage Engine (WAL → Memtable → SSTables)
```

- **Shards** partition the byte keyspace by ranges (`[a,g) → shard 1`, ...). Routing +
  stale-route recovery land in Commit 16.
- **Raft groups** (one per shard + control group for metadata) provide replicated,
  committed log entries deterministically applied to storage (Commits 10–11, 13).
- **Storage** is an LSM tree built from first principles (no RocksDB): WAL, memtable,
  immutable memtables, SSTables with index/Bloom, block cache, background flush +
  compaction (Commits 03–08).

## Write lifecycle vocabulary (§5 — never conflated)

```text
received → replicated → committed → persisted → applied → acknowledged
```

- Acknowledgement happens only after the documented durability mode is satisfied
  (exact point defined in `durability.md` once WAL + replication land).
- `Raft log index`, `database sequence/version`, and `transaction timestamp` are
  separate namespaces unless an ADR explicitly unifies them (§10).

## Public API (§3)

`GET / PUT / DELETE / SCAN`, `BATCH_GET / BATCH_WRITE / CAS / PREFIX_SCAN`,
transactions `BEGIN / TXN_GET / TXN_PUT / TXN_DELETE / COMMIT / ABORT`,
admin `CLUSTER_STATUS / NODE_STATUS / SHARD_STATUS / METRICS`.
Keys/values are arbitrary bytes subject to `fig-core::Config::Limits`.

## Correctness strategy (§§23–28)

Reference models + differential testing → property tests → history/linearizability
checks → `loom` concurrency tests → WAL/SSTable/RPC fuzzing → deterministic
simulation (`fig-sim --seed N`, Commit 12) → chaos runner + soak (Commit 19).

## Observability (§§34–35)

Structured `tracing` spans propagate
`client → router → shard → Raft → WAL → state machine → replication → response`,
with Raft/storage/txn/API metrics exposed Prometheus-style. See `docs/testing/` later.

## Milestones

Commits 02–20 implement the above in order (§50). Each milestone updates this doc +
ADRs + metrics + tests per Definition of Feature Completion (§42).
