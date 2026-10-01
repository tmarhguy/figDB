# Consistency & Isolation

## What holds today

Ordered single-key semantics from `MemoryKv`: `PUT` overwrites, `DELETE`
removes, `GET` returns the latest value, `SCAN` returns an ascending range.
The naive `ReferenceKv` agrees on every randomized operation stream.

## What is explicitly NOT claimed

No versions, no snapshots, no transactions. The target is serializable
isolation with MVCC, validated by anomaly tests (dirty reads, lost updates,
write skew, phantoms) — none of that exists yet, and this doc will say when
it does.
