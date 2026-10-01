
<h1 align="center">FigDB</h1>
<p align="center"><strong>A distributed transactional database built from first principles in Rust.</strong></p>
<p align="center">
  <a href="https://github.com/tmarhguy/fig/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/tmarhguy/fig/actions/workflows/ci.yml/badge.svg"></a>
  <a href="docs/README.md"><img alt="Status: active development" src="https://img.shields.io/badge/status-active%20development-2ea043"></a>
  <a href="docs/architecture/overview.md"><img alt="Consensus: custom Raft" src="https://img.shields.io/badge/consensus-custom%20Raft-011F5B"></a>
  <a href="docs/architecture/overview.md"><img alt="Storage: LSM from scratch" src="https://img.shields.io/badge/storage-LSM%20from%20scratch-011F5B"></a>
  <a href="#license-and-author"><img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-990000"></a>
</p>

FigDB is one database expressed at several layers:

- a crash-consistent **LSM storage engine from first principles** — WAL,
  memtables, SSTables, Bloom filters, block cache, background compaction;
- **custom Raft consensus** with replicated state machines, leader election,
  snapshots, and online replica recovery — no etcd, no Raft library;
- **serializable MVCC transactions**, single-shard first, then atomic
  cross-shard coordination;
- range-partitioned **sharding** with routing, splitting, and rebalancing;
- a **deterministic distributed simulator** (`fig-sim --seed N`) that
  reproduces crashes, partitions, and storage faults exactly;
- chaos, fuzzing, linearizability checks, benchmarks, and a React/TypeScript
  operations console with interactive failure demos.

The ordered in-memory KV model and its differential-test oracle are real and
checked in. The WAL is in progress. Everything after that is roadmap with
reserved crate boundaries — not running code. This README never claims numbers
or guarantees FigDB has not yet measured and implemented.

**Explore:** [architecture](docs/architecture/overview.md) ·
[durability](docs/architecture/durability.md) ·
[consistency](docs/architecture/consistency.md) ·
[correctness strategy](docs/correctness/strategy.md) ·
[benchmarks](docs/benchmarks/README.md) ·
[testing](docs/testing/strategy.md) ·
[ADRs](docs/adr/README.md) ·
[RPC contract](proto/fig.proto)

## Architecture at a glance

```text
CLIENTS → Client Library → RPC/API → Router → Txn Coordinator → Shard Directory
  → SHARD A/B/C → Raft Group (N1 N2 N3) → Storage Engine (WAL → Memtable → SSTables)
```

### One write, six distinct stages

```text
received → replicated → committed → persisted → applied → acknowledged
```

These are never conflated. A write is acknowledged only after the documented
durability mode is satisfied — WAL-synced single-node first, Raft-quorum
committed once replication lands. See
[durability](docs/architecture/durability.md).

### Public API

`GET / PUT / DELETE / SCAN`, `BATCH_GET / BATCH_WRITE / CAS / PREFIX_SCAN`,
transactions `BEGIN / TXN_GET / TXN_PUT / TXN_DELETE / COMMIT / ABORT`,
admin `CLUSTER_STATUS / NODE_STATUS / SHARD_STATUS / METRICS`.
Keys and values are arbitrary bytes subject to enforced limits
(`fig-core::Config::Limits`).

## What runs now

| Layer | Current, repository-backed statement |
|---|---|
| Workspace | 14-crate Rust workspace, CI (fmt/clippy/unit/integration), structured tracing, enforced config limits |
| KV model | Ordered `MemoryKv` (BTreeMap) + naive `ReferenceKv` oracle + seeded differential streams; 12 tests green |
| WAL | Framed CRC-32 records, sequence numbers, segment rotation, torn-tail vs corruption recovery — **in progress, uncommitted** |
| Memtable/LSM/SSTables | Reserved crates only; land in Commits 04–08 |
| Raft/simulator | Reserved crates + `fig.proto` placeholder; land in Commits 09–13 |
| MVCC/transactions/sharding | Reserved crates; land in Commits 14–18 |
| Chaos/console | Reserved crates; land in Commits 19–20 |

For the full milestone sequence and the definition of feature completion, see
the 20-commit roadmap below and [`docs/`](docs/).

## Roadmap: twenty milestone commits

```text
01 Foundation → 02 In-memory model → 03 WAL → 04 Crash-safe persistence →
05 SSTables → 06 LSM → 07 Compaction → 08 Perf baseline → 09 Network DB →
10 Election → 11 Replication → 12 Simulator → 13 Snapshots → 14 MVCC →
15 Serializable txn → 16 Sharding → 17 Distributed txn → 18 Rebalance →
19 Chaos/correctness → 20 Console + v1.0.0
```

Each milestone ships implementation + unit + integration + failure tests +
metrics + docs. History uses meaningful prefixes (`storage:`, `raft:`,
`txn:`, `chore:`, `docs:`) — never `update` / `fix` / `stuff`.

## See it, run it, inspect it

### Prerequisites

Stable Rust (1.75+) via rustup. On macOS with the MacOSX27 SDK,
`scripts/check.sh` pins a known-good SDK automatically.

### Verify the foundation

```bash
./scripts/check.sh   # cargo fmt --check + clippy -D warnings + cargo test
```

### Exercise the KV oracle

```bash
cargo test -p fig-core          # unit + differential tests
cargo test -p fig-wal           # WAL + crash/restart gate (once landed)
```

### Inspect a crate boundary

```bash
cargo doc -p fig-core --open   # errors, limits, tracing conventions
cat proto/fig.proto            # gRPC surface reserved for Commit 09
```

These commands prove workspace behavior, not a running database. There is no
cluster to start yet — that arrives with the network database (Commit 09).

## Proof across the stack

Correctness is built incrementally, never inferred from integration tests
passing:

- **Reference models + differential testing** — every engine must agree with
  the oracle on randomized operation streams (live since Commit 02).
- **Property tests** — PUT/GET roundtrips, compaction invariance, restart
  durability, snapshot restore, follower convergence, txn atomicity.
- **Deterministic simulation** — seeded virtual clock/network/disk reproduce
  drops, delays, reorders, crashes, partitions, and disk faults (Commit 12).
- **Fuzzing + chaos + soak** — parser/SSTable/WAL fuzzers, chaos runner,
  hours-long soak with no unbounded growth (Commit 19).

See [`docs/correctness/strategy.md`](docs/correctness/strategy.md) and
[`docs/testing/strategy.md`](docs/testing/strategy.md).

## Repository map

| Path | Purpose |
|---|---|
| [`crates/fig-core/`](crates/fig-core/) | Shared errors, config/limits, KV oracle, tracing bootstrap |
| [`crates/fig-wal/`](crates/fig-wal/) | Checksummed WAL (Commit 03) |
| [`crates/fig-storage/`](crates/fig-storage/) | Memtable/LSM/flush/compact/cache (Commits 04, 06–08) |
| [`crates/fig-sstable/`](crates/fig-sstable/) | Immutable SSTables (Commit 05) |
| [`crates/fig-raft/`](crates/fig-raft/) | Election/replication/snapshots (Commits 10–11, 13) |
| [`crates/fig-mvcc/`](crates/fig-mvcc/) | Versioning (Commit 14) |
| [`crates/fig-txn/`](crates/fig-txn/) | Single-shard + cross-shard atomic commit (Commits 15, 17) |
| [`crates/fig-sharding/`](crates/fig-sharding/) | Routing + split/rebalance (Commits 16, 18) |
| [`crates/fig-rpc/`](crates/fig-rpc/) | tonic/gRPC protocol (Commit 09) |
| [`crates/fig-client/`](crates/fig-client/) | Leader discovery, retries, request IDs |
| [`crates/fig-server/`](crates/fig-server/) | Node binary |
| [`crates/fig-sim/`](crates/fig-sim/) | Deterministic simulator (Commit 12) |
| [`crates/fig-chaos/`](crates/fig-chaos/) | Chaos runner (Commit 19) |
| [`crates/fig-bench/`](crates/fig-bench/) | Benchmarks (Commit 08+) |
| [`proto/`](proto/) | gRPC IDL |
| [`docs/`](docs/) | Architecture, ADRs, correctness, benchmarks, testing |
| [`dashboard/`](dashboard/) | React/TS console (Commit 20) |
| [`deployment/`](deployment/) | Docker/local cluster (Commit 20) |

## Documentation and history

Start with [`docs/README.md`](docs/README.md). It separates current canonical
guidance from dated records. Each important decision gets an ADR in
[`docs/adr/`](docs/adr/) covering context, decision, alternatives, tradeoffs,
and consequences — when prose and code disagree, the code plus its tests win
until the docs are updated in the same milestone.

## License and author

Intended license: **MIT OR Apache-2.0** (matching `Cargo.toml`); license texts
land before any public release. No code here is published under another
license, and no third-party database engine is embedded — infrastructure
libraries only (Tokio, tonic, serde, tracing).

Database architecture and project by **Tyrone Marhguy**.
