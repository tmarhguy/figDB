# Durability (Commit 01 placeholder)

> Full durability argument lands with WAL (Commit 03), crash-safe memtable (Commit 04),
> and replication (Commit 11). This file reserves the contract so later commits only
> fill in measured behavior — never weaken it (§52).

## Vocabulary (`internal.md` §5)

`received ≠ replicated ≠ committed ≠ persisted ≠ applied ≠ acknowledged.`

## Acknowledgement rule (to be finalized in Commit 11)

- **Single-node (Commits 03–04):** a write is acknowledged only after WAL append + `fsync`
  per the configured durability mode and memtable apply. Torn/corrupt tails are detected
  by CRC and truncated; acknowledged prefix survives restart (property-tested).
- **Replicated (Commit 11+):** a write is acknowledged only after Raft quorum commit +
  state-machine apply + persistence per durability mode. Leader commit without quorum
  is never acknowledged.

## Crash points tested (WAL, §6)

`before append / during append / after append / before fsync / after fsync /
before memtable apply / after memtable apply`, plus `FAIL_WRITE / FAIL_FSYNC /
CORRUPT_BLOCK / truncated file / disk full` via fault injection (Commit 19).

## What is explicitly NOT claimed yet

No durable acknowledged writes exist in Commit 01 (in-memory skeletons only).
