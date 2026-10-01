# FigDB Architecture

This document is the public architecture entry point; deep dives live in
`durability.md`, `consistency.md`, and `../adr/`.

## Topology (target)

```text
CLIENTS → Client Library → RPC/API → Router → Txn Coordinator → Shard Directory
  → SHARD A/B/C → Raft Group (N1 N2 N3) → Storage Engine (WAL → Memtable → SSTables)
```

Only the bottom of the storage engine exists today, but it now runs as a
process: the WAL (`fig-wal`), the ordered in-memory model it replays into
(`fig-core::MemoryKv`), flushed SSTables with manifest publishing and
size-tiered compaction (`fig-storage`, `fig-sstable`), and a single-node TCP
server + CLI over it (`fig-server`). Shards, Raft groups, and transactions
are the target, not the current code.

## Write lifecycle vocabulary (never conflated)

```text
received → replicated → committed → persisted → applied → acknowledged
```

Today there is no replication, so the only acknowledgement point is
`Wal::sync()`: everything before the last sync survives a crash. The exact
rule lives in `durability.md`.

## Public API

`GET / PUT / DELETE / SCAN` over TCP (newline-delimited JSON, base64 values;
see ADR-004), plus `SYNC / FLUSH / COMPACT / STATS` for durability and ops.
Later `BATCH_* / CAS / PREFIX_SCAN`, transactions, and admin endpoints. Keys
and values are arbitrary bytes subject to `fig-core::Config::Limits`.

## Correctness strategy

Reference models + differential testing on randomized operation streams — live
since the KV oracle. WAL crash tests prove the acknowledged prefix survives
torn tails and corruption. Each landed piece updates this doc, its ADRs, and
its tests in the same commit.

## Observability

Structured `tracing` via `fig-core::tracing_util`; per-crate log targets.
Metrics and distributed traces arrive with the networked pieces.
