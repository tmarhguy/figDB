# 🦞 FigDB

**A distributed transactional database built from first principles in Rust.**

> **Status: foundation (Commit 01).** Workspace, CI, tracing, config, error conventions,
> architecture docs, and ADR process are in place. Storage (WAL → memtable → SSTable →
> LSM), Raft, MVCC/transactions, sharding, simulation, chaos, and dashboard land in
> Commits 02–20 per `internal.md` §50. No performance or correctness numbers are claimed
> until measured (§32, §54).

**Core areas:** Raft consensus · LSM storage · WAL · MVCC · serializable transactions ·
sharding · replication · snapshots · crash recovery · deterministic fault simulation ·
chaos testing · observability · benchmarking

## Quickstart (local, single node placeholder)

```bash
rustup show            # stable required (1.75+)
./scripts/check.sh     # fmt + clippy + test
cargo test --workspace
```

## Layout

```text
crates/
  fig-core/      shared errors, config/limits, tracing bootstrap
  fig-wal/       Commit 03: checksummed WAL
  fig-storage/   Commits 04,06,07,08: memtable/LSM/flush/compact/cache/bench
  fig-sstable/   Commit 05: immutable SSTables
  fig-raft/      Commits 10,11,13: election/replication/snapshots
  fig-mvcc/      Commit 14: versioning
  fig-txn/       Commits 15,17: single-shard + cross-shard atomic commit
  fig-sharding/  Commits 16,18: routing + split/rebalance
  fig-rpc/       Commit 09: tonic/gRPC protocol
  fig-client/    client library (leader discovery, retries, request IDs)
  fig-server/    node binary
  fig-sim/       Commit 12: deterministic simulator (`--seed N`)
  fig-chaos/     Commit 19: chaos runner
  fig-bench/     Commit 08+: benchmarks
proto/               gRPC IDL (Commit 09)
docs/
  architecture/      system overview + durability/consistency notes
  adr/               architecture decision records
  correctness/       testing strategy
  benchmarks/        measured results only (no placeholders)
  testing/           sim/chaos/soak plans
dashboard/           React/TS console (Commit 20)
deployment/          Docker/local-cluster (Commit 20)
```

## Guarantees (honest, current)

- **Durability:** not yet implemented — acknowledged-write safety arrives with WAL (Commit 03)
  + crash-safe memtable (Commit 04) + replication (Commit 11). See `docs/architecture/durability.md`.
- **Isolation:** not yet implemented — target is serializable (Commit 15); snapshot isolation
  may appear as an intermediate milestone. We never claim stronger semantics than implemented.
- **Consistency:** single-node KV first (Commit 02 oracle), then Raft safety (Commits 10–11)
  validated by deterministic simulation (Commit 12) + linearizability checks (Commit 19).

See `docs/architecture/overview.md` and `docs/adr/` for decisions.

## 20-commit roadmap

Tracked in `internal.md` §50 (private spec, git-ignored). Public story (§51):

```text
01 Foundation → 02 In-memory model → 03 WAL → 04 Crash-safe persistence →
05 SSTables → 06 LSM → 07 Compaction → 08 Perf baseline → 09 Network DB →
10 Election → 11 Replication → 12 Simulator → 13 Snapshots → 14 MVCC →
15 Serializable txn → 16 Sharding → 17 Distributed txn → 18 Rebalance →
19 Chaos/correctness → 20 Console + v1.0.0
```

Every milestone: implementation + unit + integration + failure tests + metrics + docs (§42).

## Contributing / CI

- Commit style: `storage: ...`, `raft: ...`, `txn: ...`, `chore: ...` (§49). No `update/fix/stuff`.
- CI runs `cargo fmt --check`, `clippy -D warnings`, unit + integration tests; nightly runs
  sim campaigns, fuzzing, chaos, soak (§43).
