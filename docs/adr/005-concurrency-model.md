# ADR-005: Snapshot reads, serialized writes, background compaction

- **Status:** accepted
- **Date:** 2026-10-01
- **Deciders:** FigDB maintainers

## Context

Every request took one Tokio mutex and flush compacted inline: the 60k-key
release run showed PUT p99 1.74 ms / max 66 ms (foreground merges stalling
writers) and concurrent clients made it worse. The WAL (`File`, `!Sync`)
cannot sit behind a Tokio `RwLock`, and file I/O on async workers stalls the
runtime.

## Decision

- **Snapshot table set:** `Database.tables` is `Arc<Vec<Arc<Table>>>`,
  swapped wholesale on flush/compact. `get`/`scan` need only `&self` and
  iterate a cloned snapshot — a concurrent merge is invisible to them
  (old-Arcs keep serving from memory). The manifest is the same idea made
  durable.
- **Locking:** `std::sync::RwLock<Database>` (not Tokio's — `Database`
  is `Send` but not `Sync`). Reads take it shared, writes exclusive.
- **No guard crosses an `.await`:** each request is dispatched on
  `spawn_blocking`, which also moves WAL appends/fsyncs/merges off async
  workers. Poisoned lock → `INTERNAL`, never a panic.
- **Background compaction:** `Database::set_auto_compact(false)` takes
  merges out of the server write path; a timer task (`--compact-interval-ms`,
  default 1000, `0` disables) calls the new `background_compact()` step
  (oldest-8 merge, tombstones kept). The library default keeps inline
  auto-compaction — `flush()` behavior is unchanged for non-server users.
- **Throughput honesty:** `fig-bench` reports wall-clock throughput
  (`n/wall`), not `n/Σlat` — the old formula reads as per-op latency
  reciprocal and *falls* under concurrency. `--clients C` partitions keys
  by stride so writers never share a key.

## Alternatives

- **Tokio RwLock + `Mutex<File>` inside Wal:** would make `Database: Sync`
  but pushes lock management into the crash-critical WAL path; rejected —
  keep the WAL's locking exactly as crash-tested.
- **Lock-free memtable (skip list) + versioned reads:** real eventual
  design, but a second concurrency mechanism in one milestone doubles the
  proof burden. The RwLock write lock is the honest single-writer bottleneck.

## Tradeoffs

Measured (release, 60k keys, M3 loopback): 1-client PUT 8.0k → **27.0k/s**,
p99 1.74 → **0.12 ms** (merges off the write path); 8 clients PUT
**59.3k/s**, GET **96.0k/s** (reads 3.4x, writes 2.2x — WAL serialization
is the remaining write ceiling). Cost: background merges still take the
write lock (PUT max 27 ms while a merge lands); readers never block on
merges but queue behind an active writer.

## Consequences

`lsm.rs` snapshot + `set_auto_compact`/`background_compact`; server
RwLock + timer + `--compact-interval-ms`; bench `--clients` + wall metric;
`tcp.rs` concurrency + bg-fold tests, `compaction.rs` flag-contract test.
Revisit: skip-list memtable when the write lock (not the WAL) tops profiles;
binary protocol if JSON/base64 dominates (not yet: loopback RTT dominates).
