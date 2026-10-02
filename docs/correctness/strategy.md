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
| Deterministic simulation, chaos, soak | simulation live (`fig-raft` sim + `raft_sim.rs` gate); chaos/soak with networking (R3) |
| Raft safety table (below) | live: sim gate + `fig-raft` persistence gate |

## Raft safety table

Each property names the test that proves it. Sim tests are deterministic on
their seeds; persistence tests use real files + reopen.

| Property | Proved by |
|---|---|
| At most one leader per term | `elects_exactly_one_leader_on_many_seeds` (10 seeds) + `minority_partition_elects_no_rival_and_heals_without_loss` |
| Election safety across partitions | `terms_only_move_forward` (repeated partition/heal cycles) |
| Log matching (truncate-then-append, no holes) | `replicates_and_applies_in_order` + `uncommitted_suffix_may_be_overwritten_after_restart` |
| Leader completeness (committed prefix survives failover) | `leader_partition_fails_over_and_old_entries_survive` |
| Commit only current-term entries by counting; older entries commit implicitly | `advance_commit` unit path via `replicates_and_applies_in_order` + `committed_on` agreement |
| Apply exactly once, in order, never uncommitted | `check_apply_contract` on every sim test |
| Vote durable before its reply is sent | `roundtrip_vote_and_entries` (reopen sees the vote) via `Store::save_if_dirty` |
| Entry durable before ack/apply; heartbeats cost no write | `roundtrip_vote_and_entries` + `heartbeat_writes_nothing` |
| Crash before rename leaves only litter; corrupt state fails to start | `tmp_litter_reaped_and_corrupt_is_corruption` |
| Restart replays term/vote/log; volatile state re-elects | `Node::restore` path in `roundtrip_vote_and_entries` |

No numbers are published until actually measured.
