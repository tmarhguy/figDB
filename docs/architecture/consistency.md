# Consistency & Isolation (Commit 01 placeholder)

> Targets from `internal.md` §§13–15. Implemented in Commits 14–15 (MVCC + serializable
> single-shard txn) and Commit 17 (atomic cross-shard commit). Until then, only the
> ordered single-key semantics of the Commit 02 reference model hold.

## Goals

- **Final target: serializable.** Snapshot isolation may exist as an intermediate
  milestone; docs never claim stronger semantics than implemented (§14).
- **Anomaly tests required:** dirty reads, non-repeatable reads, lost updates,
  write skew, phantoms where relevant.
- **Cross-shard atomicity:** no transaction may permanently commit on only a subset
  of participating shards; coordinator/participant crash matrix in §16 is fault-tested.

## Method

Reference MVCC model + differential tests → optimistic validation or locking
(decided in ADR-006) → history collection + serialization checker → deterministic
simulation of coordinator failures → linearizability/history checks (Commit 19).
