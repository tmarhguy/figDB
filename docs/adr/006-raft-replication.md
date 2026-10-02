# ADR-006: Raft replication (static cluster, deterministic core)

- **Status:** accepted
- **Date:** 2026-10-01
- **Deciders:** FigDB maintainers
- **Progress:** R1 landed (deterministic core + sim, `36bb090`); R2 landed
  (`Dirty` tracking + crash-safe `Store`, 10 `fig-raft` tests green).
  R3 (server integration) is next.

## Context

FigDB is single-node: disk loss is not survived, only process crashes
(§durability). The headline promises a distributed database. The first
replication step must be correct before it is fast, and testable before it
is networked.

## Decision

- New `fig-raft` crate in three layers, landed in order:
  - **R1 — deterministic core:** pure state machine, no I/O, no threads, no
    clock. Inputs are messages + timer events; outputs are outbound messages
    + committed entries. Election, RequestVote, AppendEntries with the log
    matching property, leader commit advancement, follower application.
  - **R2 — persistence:** `currentTerm`/`votedFor`/log to disk, crash-safe
    via the manifest protocol (tmp → sync → rename → dir fsync). Restart
    replays into the core; uncommitted suffix entries may be overwritten by a
    new leader — never applied before commit.
  - **R3 — server integration:** client writes replicate to a majority
    before ack; followers answer with `NOT_LEADER` (the `Error` variant
    already exists) naming the known leader; leader serves reads only after
    a commit barrier in its term (no leases — simpler, one extra round trip
    on leadership change, documented).
- **Simulation first:** a seeded virtual network (FIFO + partitions + drops,
  virtual ticks) proves single-leader election, log convergence, commit
  safety, and partition healing deterministically — the same technique the
  testing strategy slated for "with networking".
- Static 3-node membership (ids in config). No joint consensus yet.

## Alternatives

- **Embed an existing Raft library:** violates the project's first-principles
  rule (no etcd/Raft library) and would still need the same integration
  proof; rejected on sight.
- **Network-first with real sockets:** nondeterministic tests hide timing
  bugs; simulation must come first, sockets after.
- **Leader leases for reads:** faster reads, but lease correctness depends on
  clock bounds we have not built; commit-barrier reads are one RTT slower on
  failover and obviously correct.

## Tradeoffs

Gain: replicated dati with majority survival; every safety property
simulated on seeds before any packet is sent. Cost: log grows unbounded
until snapshots land (follow-up milestone; entries are small command bytes,
acceptable for now); static membership means no elastic resize yet.

## Consequences

`crates/fig-raft/` + sim tests; `docs/correctness` gains the Raft safety
table when R1 lands (done: see `correctness/strategy.md`). R2 adds the
`persist::Store` gate (vote/log survival, heartbeat-writes-nothing,
suffix-overwrite). Revisit: log snapshots + membership change as the next
replication milestone; transactions only after reads/writes replicate.
