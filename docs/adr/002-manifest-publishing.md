# ADR-002: Manifest-defined table set with tmp+rename publishing

- **Status:** accepted
- **Date:** 2026-10-01
- **Deciders:** FigDB maintainers

## Context

Flush wrote each SSTable directly to its final `sst-{id}.sst` path. A SIGKILL
mid-flush left a partial file that `Database::open` rejected with
`Corruption` — the database would not open at all. Compaction (next) needs
atomic multi-file swaps, which directory listings cannot provide.

## Decision

The live table set is defined by `tables/MANIFEST`
(`{version, next_table_id, tables[]}`, JSON), updated atomically
(tmp → `sync_all` → `rename` → dir fsync). Table files publish the same way:
write `sst-{id}.sst.tmp`, sync, rename, dir fsync, then update the manifest.
Open reaps `*.tmp` litter and deletes published-but-unlisted `.sst` orphans
(the WAL still holds their data). Pre-manifest directories are adopted once.

## Alternatives

- **Dir listing + footer validation:** no atomic swap; a crashed compaction
  could leave inputs + output mutually inconsistent with no record of intent.
- **Write-ahead manifest log (LevelDB-style CURRENT + log):** more machinery
  (log replay, snapshots) than a single-level table stack needs yet.

## Tradeoffs

Gain: every flush/compact crash window reopens cleanly; one manifest write per
flush (~extra file sync; measured 20k-key load 11k→5k puts/s at a 4KB
threshold forcing 244 flushes — compaction will cut flush count, not manifest
cost). Give up: cross-version manifest compat beyond a version check;
`next_table_id` resets are impossible (ids only advance).

## Consequences

`fig-storage/src/manifest.rs` + `tests/crash_mid_flush.rs`; durability doc
records the window table. Revisit when adding levels (manifest will need
level membership) or a manifest log (if single-JSON rewrite cost matters).
