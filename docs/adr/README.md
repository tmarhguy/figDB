# ADRs — Architecture Decision Records

Each ADR covers: **Context · Decision · Alternatives · Tradeoffs · Consequences**.

Decisions so far:

- `001-memtable-structure.md` — BTreeMap for the ordered in-memory model.
- `002-manifest-publishing.md` — MANIFEST defines the live table set; tmp+rename publishing.
- `003-compaction-policy.md` — size-tiered oldest-batch merge; tombstone GC on full merges.

Template: `000-template.md`. Numbering is sequential; never reuse numbers.
