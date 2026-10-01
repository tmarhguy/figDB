
<h1 align="center">FigDB</h1>
<p align="center"><strong>A distributed transactional database built from first principles in Rust.</strong></p>
<p align="center">
  <a href="https://github.com/tmarhguy/fig/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/tmarhguy/fig/actions/workflows/ci.yml/badge.svg"></a>
  <a href="docs/README.md"><img alt="Status: active development" src="https://img.shields.io/badge/status-active%20development-2ea043"></a>
  <a href="#tests-21-passing"><img alt="Tests: 21 passing" src="https://img.shields.io/badge/tests-21%20passing-2ea043"></a>
  <a href="#license-and-author"><img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-990000"></a>
</p>

FigDB is a database built the hard way: its storage, consensus, and
transaction mechanisms are implemented directly, not delegated to an embedded
engine. No RocksDB, no etcd, no Raft library.

Honest status: two crates exist. An ordered in-memory KV model with a
differential-test oracle, and a checksummed write-ahead log with crash tests.
Everything above that — memtable, SSTables, Raft, transactions — is next, not
here. This README describes only what is checked in.

**Explore:** [architecture](docs/architecture/overview.md) ·
[durability](docs/architecture/durability.md) ·
[consistency](docs/architecture/consistency.md) ·
[correctness](docs/correctness/strategy.md) ·
[testing](docs/testing/strategy.md) ·
[ADRs](docs/adr/README.md)

## What's real

| Piece | Where | State |
|---|---|---|
| Ordered KV model (`MemoryKv`: GET/PUT/DELETE/SCAN over bytes) | [`crates/fig-core/src/kv.rs`](crates/fig-core/src/kv.rs) | done, tested |
| Naive oracle (`ReferenceKv`) + seeded operation streams | [`crates/fig-core/src/reference.rs`](crates/fig-core/src/reference.rs), [`ops.rs`](crates/fig-core/src/ops.rs) | done, tested |
| Checksummed WAL: framed records, seqnos, segments, replay, torn-tail vs corruption recovery | [`crates/fig-wal/src/`](crates/fig-wal/src/) | done, tested |
| Shared errors, config limits, tracing bootstrap | [`crates/fig-core/src/`](crates/fig-core/src/error.rs) | done, tested |

Next up: a WAL-backed memtable, so `restart → WAL replay` yields a live map.

## Tests (21 passing)

Tests live next to the code — unit tests in `src/` files, crash tests in
`tests/`. CI runs all of this on every push
([workflow](.github/workflows/ci.yml)).

| File | What it proves |
|---|---|
| [`crates/fig-core/src/kv.rs`](crates/fig-core/src/kv.rs) (4 tests) | PUT/GET/DELETE roundtrip, byte ordering, scan bounds, limit enforcement |
| [`crates/fig-core/src/ops.rs`](crates/fig-core/src/ops.rs) (3 tests) | `MemoryKv` agrees with `ReferenceKv` on seeded streams up to 10k ops |
| [`crates/fig-wal/src/record.rs`](crates/fig-wal/src/record.rs) (3 tests) | Frame roundtrip, torn prefix reads as torn (not corrupt), bit flips detected |
| [`crates/fig-wal/src/segment.rs`](crates/fig-wal/src/segment.rs) (1 test) | Header + single-frame write/replay |
| [`crates/fig-wal/src/wal.rs`](crates/fig-wal/src/wal.rs) (2 tests) | Dense seqnos across reopen, order preserved across rotation |
| [`crates/fig-wal/tests/crash.rs`](crates/fig-wal/tests/crash.rs) (3 tests) | Acked prefix survives 20 seeded crash campaigns; mid-file corruption truncates the suffix; torn tails never replay |

Run everything:

```bash
./scripts/check.sh   # fmt --check + clippy -D warnings + full test suite
```

Or scoped:

```bash
cargo test -p fig-core            # oracle + differential tests
cargo test -p fig-wal            # unit tests
cargo test -p fig-wal --test crash   # crash/restart gate (also its own CI job)
```

## Durability in one paragraph

A write is acknowledged if and only if it sits at or before the last
`Wal::sync()`. Recovery replays exactly that prefix — torn tails truncated,
corrupt frames stop and truncate replay, sequence numbers stay dense. Details:
[durability](docs/architecture/durability.md).

## Repository map

```text
crates/fig-core/    errors, config/limits, KV oracle, tracing
crates/fig-wal/     the log (record / segment / wal) + crash tests
docs/                   architecture, ADRs, correctness, benchmarks, testing
scripts/check.sh        local gate: fmt + clippy + tests
.github/workflows/     ci.yml mirrors check.sh, plus the WAL crash gate
```

## Documentation and history

Start with [`docs/README.md`](docs/README.md). Important decisions get an ADR
in [`docs/adr/`](docs/adr/). Commits look like `wal: ...`, `core: ...`,
`docs: ...`, `chore: ...` — one piece of work each, so the history reads like
the build went.

## License and author

Intended license: **MIT OR Apache-2.0** (matching `Cargo.toml`); license texts
land before any public release. Infrastructure libraries only (Tokio, serde,
tracing) — no embedded database engine.

Database architecture and project by **Tyrone Marhguy**.
