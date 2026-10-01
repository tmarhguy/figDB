# Benchmarks (Commit 01)

> Per `internal.md` §32: every published result must state hardware, CPU, RAM, storage,
> OS, compiler, commit, cluster size, replication factor, dataset, payload, concurrency,
> durability settings, workload. **No placeholders become numbers until measured (§54).**

Matrix (§31): datasets 1/10/50+ GB · payloads 128B/1KB/4KB/64KB · concurrency
1/8/32/128/512/1000+ · clusters 1/3/5 nodes · workloads 100% read, 100% write, 95/5,
50/50, RMW, scans, high-contention, Zipfian, uniform.

First reproducible baseline lands in Commit 08 (`fig-bench` + block cache).
