# ADRs — Architecture Decision Records

Location: `docs/adr/` per `internal.md` §41.

Each ADR covers: **Context · Decision · Alternatives · Tradeoffs · Consequences**.

Planned:

```text
001-memtable-structure.md      (Commit 02/04: BTreeMap vs skip list)
002-sstable-layout.md          (Commit 05)
003-compaction-policy.md       (Commit 07: leveled vs size-tiered)
004-raft-persistence.md        (Commit 10)
005-mvcc-timestamps.md         (Commit 14)
006-transaction-protocol.md    (Commit 15: optimistic vs locking)
007-sharding-strategy.md       (Commit 16: range partitioning)
```

Template: `000-template.md`. Numbering is sequential; never reuse numbers.
