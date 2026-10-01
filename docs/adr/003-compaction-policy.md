# ADR-003: Size-tiered compaction (oldest-batch merge, full-merge tombstone GC)

- **Status:** accepted
- **Date:** 2026-10-01
- **Deciders:** FigDB maintainers

## Context

Every flush adds one SSTable and nothing ever removes one: a 20k-key load at
a 4KB threshold left ~250 tables. `get()` fans out newest→oldest and `scan()`
reads every table fully per call. Table count (hence worst-case read cost) is
unbounded. The Phase-1 manifest gives us atomic multi-file swaps, which is the
primitive compaction needs.

## Decision

- **Trigger:** after each flush, while `tables.len() >= 8`, merge the oldest 8
  into one new sorted run (newest input wins per key). Foreground, not a
  background thread: no write concurrency exists yet to hide behind, and a
  stalled write is observable while a lost write is not.
- **Manual `compact()`:** flush the memtable, then merge ALL tables into one.
- **Tombstone GC:** partial merges keep every tombstone (older tables may sit
  below the inputs). A full merge with an empty memtable drops tombstones with
  no live value — the key is deleted everywhere, nothing left to shadow. A
  fully-deleted merge publishes no output table at all.
- **Crash safety:** same publish protocol as flush (output tmp → sync →
  rename → dir fsync → manifest swap → delete inputs → dir fsync). Crash
  before swap: orphan output reaped, inputs intact. Crash after swap: unlisted
  inputs reaped at open. Covered by `tests/compaction.rs` windows A/B.

## Alternatives

- **Leveled compaction (L0 + non-overlapping L1+):** better read bounds, but
  needs level bookkeeping in the manifest and overlap-aware placement — for a
  single-node milestone, size-tiered bounds fan-out to ≤8 with far less code.
- **Background compaction thread:** hides merge latency but introduces
  concurrent flush/compact/open races against the manifest; premature while
  all writes are single-threaded.

## Tradeoffs

Gain: 20k-key load went 250 → 5 tables, bytes/key 62.2 → 59.7, tombstones of
fully-deleted keys disappear entirely. Cost: foreground merges stall the
writing call (20k load 5.6k→3.5k puts/s at the tiny test threshold; each
merge rewrites ~8 small tables). Metrics expose `compactions` /
`compacted_records` so the stall is measurable, not mysterious.

## Consequences

`Database::compact()`, auto-policy in `flush()`, `tests/compaction.rs` (5
tests). Revisit: background compaction when a server introduces concurrent
writers; leveled layout if size-tiered write amplification shows up in
production-sized benchmarks.
