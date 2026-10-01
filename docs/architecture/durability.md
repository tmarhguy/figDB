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

## What is explicitly NOT claimed

- No memtable yet: replay currently yields entries, nothing applies them.
- No replication: a disk loss is not survived, only a process crash.
- No checksums above the WAL frame level.
