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

## Run C — 60k keys, background compaction + concurrent clients (release)

Same dataset/settings as Run B, plus `--compact-interval-ms 200`, after
ADR-005 (snapshot reads, RwLock, merges off the write path).
`fig-bench --clients` partitions keys by stride; throughput is wall-clock
(`n/wall` — the previous `n/Σlat` formula understated concurrency).

| Clients | Op | Throughput (wall) | avg | p50 | p99 | max |
|---|---|---|---|---|---|---|
| 1 | PUT | 26,987/s | 0.04 ms | 0.03 ms | 0.12 ms | 21.41 ms |
| 1 | GET | 28,471/s | 0.03 ms | 0.03 ms | 0.11 ms | 2.28 ms |
| 8 | PUT | 59,315/s | 0.13 ms | 0.08 ms | 0.52 ms | 27.18 ms |
| 8 | GET | 96,013/s | 0.08 ms | 0.07 ms | 0.29 ms | 10.22 ms |

End state (both): 5 flushes, 1 auto-compaction, final compact 0.06 s →
1 table, 5.0 MB. Against Run B (inline compaction): single-client PUT
8.0k → 27.0k/s, p99 1.74 → 0.12 ms — moving merges off the write path is
the dominant win; concurrency adds the rest (reads 3.4x, writes 2.2x —
WAL append serialization is the remaining write ceiling).

## Soak — SIGKILL per cycle (debug)

`scripts/soak.sh 3 2000`: 3 cycles × 2,000 keys, server `kill -9`d after
each load, restarted, then *every* earlier cycle's prefix re-verified
(2k random gets each). All green; final `compact` → 1 table.
GET throughput during soak stayed 8–13k/s across restarts — no degradation
as tables accumulate, because auto-compaction bounds the stack.

## What is NOT claimed

- Concurrency numbers are loopback-only, 1–8 clients; no multi-host latency.
- No fsync-per-write numbers (all runs batch sync every 500).
- No power-loss testing — kill -9 exercises page-cache-survives crashes;
  unsynced-tail loss is proven by truncation simulation (`fig-wal`
  crash gate), not by pulling the plug.
