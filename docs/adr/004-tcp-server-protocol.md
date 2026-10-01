# ADR-004: Single-node TCP server with JSON-lines protocol

- **Status:** accepted
- **Date:** 2026-10-01
- **Deciders:** FigDB maintainers

## Context

The database was library-only: usable from Rust tests and examples, but no
running process to talk to. The topology doc already names a client →
RPC/API → storage path; the first live hop is a single-node server with no
replication, transactions, or auth.

## Decision

- New `fig-server` crate: `fig-server` binary (TCP listener, one
  manifest-backed `Database`) + `fig-cli` (human frontend).
- Protocol: newline-delimited JSON, one request → one response. Ops
  `put|delete|get|scan|sync|flush|compact|stats`; keys/values base64 (arbitrary
  bytes through a JSON envelope; new `base64` infra dependency). Every
  response carries `seq` + `ok`, failures carry the stable `Error::code()`.
- Concurrency: one `Database` behind a Tokio mutex, one task per connection.
  Requests serialize — matches the single-threaded engine (foreground
  flush/compact); no lock-striping to get wrong.
- Durability: server runs `FsyncPolicy::Never`; `sync` is the ack point, same
  rule as the library. `scan` defaults to 1000 pairs, capped at 10k.
- Garbage input (bad JSON, unknown op, bad base64, inverted range) returns
  `INVALID_ARGUMENT` and the connection stays up.

## Alternatives

- **Binary length-prefixed protocol:** more efficient, but undebuggable with
  `nc`/shell and harder to extend; JSON wins while the op set is small.
- **HTTP:** heavier framing + dependency surface for identical semantics;
  reconsider when browsers or load balancers are clients.

## Tradeoffs

Gain: the DB runs as a process; CLI-driven smoke + kill-9 tests work over the
real wire path. Cost: JSON + base64 overhead (~33% on values), one-at-a-time
request execution bounds throughput (measured next, with benchmarks).

## Consequences

`crates/fig-server/` + `tests/tcp.rs` (4 tests) + `real_test.sh` phase 4
(CLI writes → SIGKILL → restart → verify). Revisit: binary framing if
protocol overhead dominates profiles; auth/TLS before any untrusted network.
