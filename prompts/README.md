# Metrocalk prompts — run order

One prompt = one focused Opus session (system prompt: `00-orchestrator.md` v2).

> **Execution mode: SERIAL** — one session at a time (decided 2026-06-14, to keep things clean). No worktrees, no lane-staging: run a prompt, commit on `master`, run the next. The parallel/worktree discipline is **paused** and preserved at the bottom for if/when you revisit it.

## Status

**M1 ✓ complete + hardened** — `05`–`11` (foundation, commit pipeline, M1.6 audit fixes).
**M2 gates:** `12` M2.1 ✓ (IPC **pass**; single-window compositing now looks **viable** after the live visual pass — see `14`) · `13` M2.2 ✓ (render **pass**).
**M2 build:** `15` M2.4 (transport) ⏳ running · then `14` · `16` · `17`.

## Serial run order (next → last)

1. **`15` M2.4 — transport + delta protocol** *(running)*.
2. **`14` M2.3 — shell-composition gate → ADR-008.** Do this next: it's the riskiest remaining unknown (it decides what `17` builds), and the live visual pass already unblocked it. (Re-scoped: confirm single-window first — it now looks viable — then DComp, then CEF.)
3. **`16` M2.5 — editor UI.** Needs `15`'s transport contract.
4. **`17` M2.6 — shell + viewport integration.** Needs `14` (ADR-008) + `15` + `16` + `13`'s render verdict. The M2 milestone close → then M3 (binding UX, authored from this evidence).

Serially you just go top-to-bottom; the dependencies above are *why* that's the order.

## One-time cleanup before continuing serially

The earlier parallel runs left branches + worktrees (`m1.6-pipeline-hardening`, `m2.1`, `m2.2-render-gate`, and `15`'s). Going serial doesn't auto-undo them — do **one consolidation pass** when `15` finishes: merge each into `master` in order, then `git worktree remove <path>` + `git branch -d <branch>` each. (Settle the stray `@`-subjects before pushing if you want them clean.) After that, every serial session commits straight on `master` (or a quick branch + fast-forward if you like a per-session safety net) — and no new worktrees appear.

## Doc/report protocol (every prompt enforces it)

Each prompt `14`–`17` carries a `<reporting_and_documentation>` section: **a task isn't done until it's documented and reported** — `progress/M2.md` + `architecture.md` updated (state-only, prune stale), the relevant **ADR written from measured evidence**, and a report in the fixed structure: **Outcome · Numbers (quoted, ≥2 runs, never invented) · Decisions · Honest boundaries (what's NOT tested — mandatory) · Risks · Next.**

## Lane rule (still useful serially)

Each prompt header declares the crate/paths it **Owns** — serially this no longer needs isolating, but it still tells you a session's blast radius. (The prompts' "commit on your own branch if running parallel" clauses are now no-ops — just commit on `master`.)

## Audit note (M1 → M2 handoff)

M1 was audited against expectations before M2-build authoring: foundation sound; two doc-drifts for the build to fix — the `architecture.md` status line still says "M1 — foundation build" (we're in M2), and the system-shape diagram still implies the naive webview-over-wgpu overlay (refresh to the ADR-008 shell in M2.6).

---

## Parallel mode (PAUSED — preserved for later)

If you ever re-enable concurrent sessions: give each its own git **worktree grouped under one sibling folder** (`git worktree add ../metrocalk-worktrees/<branch> -b <branch>`) so the project dir stays clean and CI/cargo never traverses a nested duplicate checkout; run disjoint-path lanes only; merge each branch then `git worktree remove` + `git branch -d`. The parallel-safe pairs were {`14` ∥ `15`} and (earlier) {`12` ∥ `13`}. **Never** run two sessions in the same working copy — that was the `index.lock` race.
