# Testing: Simulation, Chaos, Soak (Commit 01 plan)

Per `internal.md` §§12, 28–29, 43–44.

- **Deterministic simulator (Commit 12):** virtual clock/network/disk/scheduler;
  `DROP/DELAY/DUPLICATE/REORDER/CRASH/RESTART/PARTITION/HEAL/FAIL_WRITE/FAIL_FSYNC/CORRUPT_BLOCK`;
  `fig-sim --seed N` reproduces execution + event trace for replay.
- **Chaos runner (Commit 19):** `fig chaos run --nodes 5 --duration 30m ...`;
  tracks acknowledged-write losses, txn violations, availability, election/catch-up
  durations, p99.
- **Soak (Commit 19):** hours-long load; monitors memory, FDs, tasks, WAL/SSTable
  growth, compaction backlog, latency, errors. No unbounded growth.
- **CI:** per-commit fmt/clippy/unit/integration; nightly: large sim runs, fuzzing,
  chaos, soak, perf regression.
