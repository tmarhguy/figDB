# Durability

## Vocabulary

`received ≠ replicated ≠ committed ≠ persisted ≠ applied ≠ acknowledged.`

There is no replication yet, so only three of these are meaningful today:
received, persisted, acknowledged.

## Acknowledgement rule

A write is acknowledged if and only if it sits at or before the last
`Wal::sync()`. The WAL replays that prefix after a crash: torn tails are
truncated, corrupt frames stop replay and are truncated, sequence numbers stay
dense so no write is ever replayed twice or skipped.

Crash behavior is proven by `crates/fig-wal/tests/crash.rs`: randomized
streams with crashes at random points always recover a prefix that contains
every acknowledged write and nothing torn.

## Table publishing (manifest rule)

The live SSTable set is defined by `tables/MANIFEST`, not the directory
listing. Flush publishes in order: write `sst-{id}.sst.tmp` → sync →
`rename` → dir fsync → update `MANIFEST` (tmp → sync → rename → dir fsync)
→ delete WAL segments. So:

- Crash before rename: only `*.tmp` litter → deleted at open.
- Crash after rename, before manifest: unlisted orphan `.sst` → deleted at
  open (the WAL still holds the data; never merged into reads).
- Crash after manifest, before WAL deletion: table and WAL overlap —
  harmless duplicates, next flush clears the WAL.

Proven by `crates/fig-storage/tests/crash_mid_flush.rs`, which plants the
litter of a SIGKILL at each window and asserts open succeeds with exactly
the acknowledged state. Pre-manifest directories are adopted once (tables
taken in id order, manifest written).

## Compaction crash windows

Compaction publishes by the same protocol (output tmp → sync → rename →
manifest swap → delete inputs), so its windows reduce to the flush cases:
crash before the swap leaves an unlisted orphan output (inputs intact, WAL
unaffected); crash after the swap leaves unlisted inputs that open reaps —
the output already holds their data. Tombstone GC only runs on full merges
with an empty memtable, where no older layer can exist below, so a dropped
tombstone can never resurrect a shadowed key. Proven by
`crates/fig-storage/tests/compaction.rs` (windows A/B forged on disk).

## What is explicitly NOT claimed

- The memtable is the replayed log made queryable — queries above it merge
  flushed SSTables, which are now manifest-published (but never compacted yet).
- No replication: a disk loss is not survived, only a process crash.
- No checksums above the WAL frame level.
