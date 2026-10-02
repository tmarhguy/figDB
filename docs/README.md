# Docs index

Canonical guidance first, dated records nowhere near it.

| Path | What it is |
|---|---|
| [`architecture/overview.md`](architecture/overview.md) | System topology, write lifecycle, API, correctness strategy |
| [`architecture/status.md`](architecture/status.md) | What's actually landed, crate by crate |
| [`architecture/durability.md`](architecture/durability.md) | When a write is acknowledged; crash points tested |
| [`architecture/consistency.md`](architecture/consistency.md) | Isolation target (serializable) and how it's validated |
| [`adr/`](adr/) | Architecture decision records — context, decision, alternatives, tradeoffs |
| [`correctness/strategy.md`](correctness/strategy.md) | Reference models → property tests → simulation → chaos |
| [`benchmarks/`](benchmarks/) | Measured results only; no placeholders become numbers until run |
| [`testing/strategy.md`](testing/strategy.md) | Simulator, chaos runner, soak, CI/nightly split |
| [`testing/inventory.md`](testing/inventory.md) | What each test file proves (76 passing) |

Rule: when prose and code disagree, the code plus its tests win until the docs
are updated in the same milestone.
