# ADR-013: Live editor persistence — deterministic-seed + replay-log, not Loro-merge-on-start

**Date:** 2026-06-16 · **Status:** Accepted — shipped in M2 (`editor-shell/src/persist.rs`); recorded
now (the deferred M2 persistence ADR) · **Builds on:** [ADR-002](002-loro-over-custom-wal.md) (Loro is
the durable mirror), the commit pipeline + `Engine::clear_history`, the M1.6 merge-drops-capabilities
carry-forward.

> **Status update (2026-06-19) — live-verified; the gap was *surfacing*, not this strategy.** A user
> report ("binds don't survive close→reopen") was diagnosed by **measurement**, not assumption: the live
> release shell prints `restored 14 edits (0 skipped)` and `project_full` carries the restored binding
> edges on connect — i.e. this replay-log restores correctly through the real exe at `SCENE_N=5000`. The
> real defect was downstream: the UI didn't **surface** the restored state on reload (no auto-selection,
> the bound HealthBar's high id fell past the requirers cap, and the viewport drew no binding lines). The
> persistence design here is **unchanged**; the fix was UI surfacing (panel tracking badge + auto-focus,
> 3D viewport tracking lines) + window-position restore. The previously-noted test gap (the headless
> `persistence.rs` exercised the `Log` only at n=500 against a temp file) is **closed**:
> `editor-shell/tests/reload_surfacing.rs` now drives the live record stream (`Bind`/`Edit`/`Describe`/
> `Undo`) through a real on-disk log at the shell's real `SCENE_N`, asserts the net state **and** that
> `project_full` carries the surviving edge (the data→UI seam); a live reload E2E covers the surfacing.

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
