# Correctness Strategy

What runs, in order of arrival:

| Technique | Status |
|---|---|
| ReferenceKV oracle + differential tests on seeded streams | live (`fig-core`) |
| WAL crash/restart gate: acked prefix survives torn tails + corruption | live (`fig-wal/tests/crash.rs`) |
| Memtable kill/restart gate: engine matches the oracle across kills | live (`fig-storage/tests/recovery.rs`) |
| Property tests per engine (PUT/GET, restart durability, ... ) | next, with the memtable |
| History collection + linearizability checker | with transactions |
| Fuzzing (WAL decoder first — untrusted bytes must not panic) | with the memtable |
| Deterministic simulation, chaos, soak | with networking |

No numbers are published until actually measured.
