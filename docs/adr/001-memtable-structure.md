# ADR-001: Memtable / In-Memory Ordered Model Structure

- **Status:** accepted
- **Date:** 2026-10-01
- **Deciders:** FigDB maintainers
- **Applies to:** Commit 02 (`core: implement ordered in-memory KV model`), Commit 04 (WAL-backed memtable)

## Context

Commit 02 needs an ordered in-memory KV model that doubles as the correctness oracle
for all later storage work (WAL replay, SSTables, LSM, compaction must all remain
logically equivalent to it). Requirements: lexicographic byte ordering, GET/PUT/DELETE/SCAN
with clear range semantics, deterministic behavior under randomized operation streams,
and a naive independent reference for differential testing.

Candidates: `BTreeMap<Vec<u8>, Vec<u8>>`, custom skip list, `HashMap` + sort-on-read,
red-black/AVL hand-rolled tree.

## Decision

Use `std::collections::BTreeMap<Vec<u8>, Vec<u8>>` for `MemoryKv` (Commit 02) and for
the memtable in Commit 04.

## Alternatives

- **Skip list (e.g. `crossbeam-skiplist`):** lock-free concurrent access, classic LSM
  memtable choice. Rejected for now: concurrency is handled at a higher layer in early
  milestones; adds unsafe-adjacent complexity before crash-consistency is proven.
- **HashMap + sort on scan:** O(1) point ops but O(n log n) scans; breaks the ordered
  invariant the LSM relies on and complicates range correctness proofs.

## Tradeoffs

- Gain: std-guaranteed ordering correctness, simple code, easy differential testing
  against a deliberately naive `ReferenceKv` (linear vec), fast enough for oracle duty.
- Give up: single-threaded writes (requires external synchronization for concurrent
  writers), no lock-free snapshot handles yet.
- Cost: clones on read (`get` returns owned `Vec<u8>`); acceptable until profiling
  (Commit 08) says otherwise.

## Consequences

- `MemoryKv` + `ReferenceKv` + seeded `gen_ops` harness become the differential-test
  template reused by WAL replay, SSTable readers, flush, and compaction.
- Revisit when: concurrent write throughput or memtable memory overhead shows up in
  Commit 08 benchmarks; then evaluate skip list with an ADR superseding this one.
