# Correctness Strategy (Commit 01)

Per `internal.md` §§23–28. Implemented incrementally; this file tracks what exists.

| Technique | Status (Commit 01) | Lands in |
|---|---|---|
| ReferenceKV oracle + differential tests | planned | Commit 02 |
| Property tests (PUT/GET, DELETE, compaction-invariance, restart-durability, snapshot-restore, follower-convergence, txn atomicity) | planned | Commits 02–07, 14–15 |
| History collection + linearizability checker | planned | Commits 15, 19 |
| Loom concurrency tests | planned | Commits 06, 15 |
| WAL/SSTable/RPC/snapshot fuzzing (no panic/corruption, bounded resources) | planned | Commit 19 |
| Deterministic simulation (`fig-sim --seed N`) | planned | Commit 12 |
| Chaos runner + soak | planned | Commit 19 |

No correctness numbers are published until actually measured (§32).
