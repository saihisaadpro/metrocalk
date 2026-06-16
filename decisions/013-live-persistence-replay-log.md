# ADR-013: Live editor persistence — deterministic-seed + replay-log, not Loro-merge-on-start

**Date:** 2026-06-16 · **Status:** Accepted — shipped in M2 (`editor-shell/src/persist.rs`); recorded
now (the deferred M2 persistence ADR) · **Builds on:** [ADR-002](002-loro-over-custom-wal.md) (Loro is
the durable mirror), the commit pipeline + `Engine::clear_history`, the M1.6 merge-drops-capabilities
carry-forward.

## Context

The live `/editor-shell` needs the scene to **survive close→reopen** (north-star test #1, box 5). The
obvious route — export the Loro document on edit and `merge` it on launch — is wrong *today*:
`Engine::merge` rebuilds scene entities from Loro but **not their ECS tags/pairs**, so the capability
pairs the M3.1 reveal's `without(BindsTo, *)` query depends on are dropped after a merge (the documented
M1.6 carry-forward). Merge-on-start would silently break binding-by-intent after the first reload. Plus
merging a 5k snapshot is a ~350 ms one-shot cliff (measured, M2).

## Decision

**Reconstruct the scene by deterministic re-seed + replay of an append-only edit log — not Loro-merge.**
On launch: (1) re-seed deterministically (fixed `SEED` + counter id allocation → byte-identical
`EntityId`s every run, so saved `(from,to)` keys still refer to the same entities); (2) replay an
append-only log of the user's committed mutations (`Edit` / `Bind` / `Undo`) **back through the commit
pipeline** (invariant 3); (3) `clear_history` so the restored scene is non-undoable (Ctrl-Z can't
delete a restored world — same guard as the seed). The log carries a `#mtk <fingerprint>` header
(seed + scene size + algo version); replay **discards** a log from an incompatible build rather than
binding saved ids against a divergent id space. Replay is tolerant — a malformed/rejected/divergent
record is skipped, never fatal.

## Consequences

- **Correct + capability-preserving:** replay goes through `commit`, which re-adds the ECS capability
  pairs (unlike `merge`), so the reveal exclusion survives reload. A bind survives close→reopen
  (headless-proven, `editor-shell/tests/persistence.rs`).
- **Cheap on the happy path:** no 350 ms snapshot-merge; re-seed is deterministic + fast, replay is the
  small user-edit stream.
- **Build-coupled, fingerprint-guarded:** the determinism is tied to the seed/algo; the header detects
  an incompatible log and discards it instead of mis-binding.
- **Not a general persistence layer:** the log grows with session lifetime (compaction is a follow-up),
  and this is editor-session persistence, *not* the collab/durable-doc story — Loro remains the durable
  mirror (ADR-002) and the future merge path is the real cross-peer persistence, once
  `rebuild_ecs_from_loro` re-derives ECS pairs from the document.

## Revisit when

The capability-rebuild carry-forward lands (`rebuild_ecs_from_loro` restores tags/pairs) — at which point
Loro-merge becomes a viable load path and this replay-log can become a compaction/snapshot detail; or
collab arrives and durable persistence must be the merged Loro document, not a local replay log.
