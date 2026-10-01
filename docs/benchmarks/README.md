# Benchmarks

First measured run: 2026-10-01, commit `04f79c8` (server + compaction).
No earlier numbers exist; nothing here is projected or interpolated.

## Setup (applies to all runs below)

- Hardware: Apple M3 (MacBook Air), 16 GiB RAM, internal SSD (APFS)
- OS: Darwin 25.6.0 (arm64)
- Compiler: rustc 1.98.1
- Dataset: `bench-{i:08}` keys (14 B), deterministic 64 B values, sequential
  inserts + uniform-random reads, single TCP connection, sequential requests
- Durability: server `FsyncPolicy::Never`, explicit `sync` every 500 puts
  (a `sync` covers all puts before it — every measured put is acknowledged)
- Tool: `fig-bench` (per-op latency in µs, p50/p99 over the full run)

## Run A — 10k keys, no flush pressure (debug build)

`fig-server --flush-threshold 4194304`, `fig-bench --n 10000`
(`target/debug`; kept as the small-load shape reference — Run B is the
release record.)

| Op | Throughput | avg | p50 | p99 | max |
|---|---|---|---|---|---|
| PUT | 17,585/s | 0.06 ms | 0.04 ms | 0.23 ms | 3.75 ms |
| GET (random) | 19,596/s | 0.05 ms | 0.04 ms | 0.20 ms | 2.40 ms |

Storage after run: 1 flush, 10,000 records, 877,978 table bytes
(~88 B/key on disk for 14 B key + 64 B value — framing + index + bloom).

## Run B — 60k keys through flush + auto-compact (release)

`fig-server --flush-threshold 1048576`, `fig-bench --n 60000 --compact`

| Op | Throughput | avg | p50 | p99 | max |
|---|---|---|---|---|---|
| PUT | 7,960/s | 0.13 ms | 0.05 ms | 1.74 ms | 65.88 ms |
| GET (random) | 10,023/s | 0.10 ms | 0.05 ms | 1.33 ms | 31.59 ms |

Same workload, debug build: PUT 4,890/s (p99 2.18 ms, max 251 ms),
GET 10,694/s. GET is RTT-bound, so release barely moves it; PUT is
execution-bound (flush/compact rewrites), so release helps ~1.6x.

End state: 5 flushes, 1 auto-compaction (60,000 records merged), final
manual compact 0.12 s → **1 table, 5.0 MB** (~87 B/key). Flush bytes
totaled 10,535,368 vs 5.0 MB final: write amplification ≈ 2.1x at this
size (each key written ~twice: once by flush, once by merge).

The p99/max column is the foreground-compaction tradeoff made visible:
writes stall during merges (max 66 ms release). No background thread hides
it yet — see ADR-003.

## Soak — SIGKILL per cycle (debug)

`scripts/soak.sh 3 2000`: 3 cycles × 2,000 keys, server `kill -9`d after
each load, restarted, then *every* earlier cycle's prefix re-verified
(2k random gets each). All green; final `compact` → 1 table.
GET throughput during soak stayed 8–13k/s across restarts — no degradation
as tables accumulate, because auto-compaction bounds the stack.

## What is NOT claimed

- No multi-client concurrency numbers (requests serialize; §ADR-004).
- No fsync-per-write numbers (all runs batch sync every 500).
- No power-loss testing — kill -9 exercises page-cache-survives crashes;
  unsynced-tail loss is proven by truncation simulation (`fig-wal`
  crash gate), not by pulling the plug.
