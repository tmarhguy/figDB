# Testing

- **Unit + integration** on every commit: `cargo fmt --check`, `clippy -D warnings`,
  `cargo test --workspace --all-targets` (see `scripts/check.sh`, mirrored in CI).
- **WAL crash gate** in CI: `cargo test -p fig-wal --test crash`.
- **Coming with the memtable:** randomized kill/restart testing
  (`PUT → WAL → memtable`, `restart → WAL replay`), WAL decoder fuzzing.
- **Coming with networking:** deterministic simulation with a seeded virtual
  clock/network/disk, chaos runner, soak tests.
