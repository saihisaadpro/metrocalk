# Metrocalk prompts — run order & parallel-safety

One prompt = one focused Opus session (system prompt: `00-orchestrator.md` v2). This is the run map: what's done, what's next, what can run at the same time, and how to run several at once **without** the git/doc collisions we've hit.

## Status

`05` M1.1 ✓ · `06` M1.2 ✓ · `07` M1.3 ✓ · `08` M1.4 ✓ · `09` M1.5 ✓ · `10` M1→2 (commit pipeline) ⏳ *landing (committing)*
→ **M1 foundation complete** once `10` is committed.
Audit-driven: `11` M1.6 (pipeline hardening). Risk-first M2 exit gates: `12` M2.1 (Tauri shell), `13` M2.2 (render).
**M2 build** (real shell + transport impls): NOT yet authored — write it from the `12`/`13` gate evidence (the shell's transport choice depends on the Tauri-go/CEF verdict).

## Run graph

```
M1.1–M1.5 ✓   →   M1→2 (10) ⏳ commit
                        │
        ── once 10 is committed: a 3-way parallel set (disjoint paths) ──
        ├── 11  M1.6  (/core: pipeline hardening — audit fixes)   [blocked by 10 committed]
        ├── 12  M2.1  (/spikes/tauri-shell — IPC + compositing gate, Windows)   [independent]
        └── 13  M2.2  (/spikes/render-scene — wgpu native+wasm gate)            [independent]
                        │
        ── once 12 + 13 pass ──  M2 BUILD (shell + transport 3 impls + binary delta protocol),
                                 authored from the gate evidence (Tauri-go or CEF)
```

## Parallel-safe trio (once M1→2 is committed)

- **`11` M1.6** — owns `/core` (`pipeline.rs`, `undo.rs`). Blocked by M1→2 being committed (it edits the same files).
- **`12` M2.1** — owns a throwaway `/spikes/tauri-shell`; the ADR-003 exit gate (IPC + compositing). **Windows-only.**
- **`13` M2.2** — owns a throwaway `/spikes/render-scene`; the real-scene wgpu gate (native + wasm).

All three own **disjoint paths**, so they're a genuine 3-way parallel set. `12` and `13` are the M2 *exit gates* (risk-first — the M0 playbook); `11` fixes the audit findings before the editor builds on the pipeline. **Not parallel:** the M2 *build* waits on `12` + `13`'s verdicts.

## The discipline (prevents the collisions we've hit) — worktrees, or interleave

Two sessions sharing one git index + working copy is what caused the `index.lock` races and `progress.md` churn. Avoid it:

**Recommended — git worktrees (full isolation):**
```
git worktree add ../metrocalk-m2.1 -b m2.1     # session 12 here
git worktree add ../metrocalk-m2.2 -b m2.2     # session 13 here
# (11 in the main checkout on its own branch, AFTER 10 is committed)
# when each passes: git checkout main && git merge <branch>   (resolve the trivial progress/M1.md top-of-log nit)
```
Separate indexes → no lock race; separate working copies → no file collision.

**Zero-overhead fallback — interleave:** run them back-to-back; you lose wall-clock, gain zero git risk.

**Never** run two sessions in the **same** working copy at once.

## Why `11` (M1.6) exists

An orchestrator audit of the M1→2 code found real gaps that green CI missed (the failing cases are untested): (1) undoing an *added* field to a component that already held other fields **over-removed the whole component** (data loss on undo); (2) `commit` wasn't actually atomic on a mid-transaction failure (partial, un-revertable mutation — the plugin/AI path); plus a perf O(n) reverse lookup and pervasive `.unwrap()` on the mutation path; and a Phase-2 hole (capabilities vanish after a merge). `11` fixes 1–4 before the editor (M3+) relies on the pipeline; the merge hole is recorded for Phase-2. Details in `11-m1.6-pipeline-hardening.md`.

## Lane rule

Each prompt's header declares the crate/paths it **Owns**; a parallel set must own disjoint paths (this trio does). Each session updates `progress.md` / `architecture.md` on its own branch; the merge reconciles them.
