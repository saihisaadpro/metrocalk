# Metrocalk prompts — run order & parallel-safety

One prompt = one focused Opus session (system prompt: `00-orchestrator.md` v2). This is the run map: what's done, what's next, what can run at the same time, and **how to run two at once without the git/doc collisions we've already hit.**

## M1 run graph

```
M1.2 ✓  (/ecs — World trait + Flecs backend)
  │
  ├── M1.3  (07 · /core: metadata registry)        ⏳ in flight ─┐
  └── M1.4  (08 · /tools/scene-gen: stress scene)  ───────────────┤   PAIR A:  07 ∥ 08
                                                                  │
        ── once Pair A lands ──                                   │
  ├── M1.5  (09 · .github + query bench)  [needs 08's fixture] ──┐
  └── M1→2  (10 · /core: commit pipeline) [needs 07's registry] ─┘   PAIR B:  09 ∥ 10
                                                                  │
        ── once Pair B lands ──  M1 complete  →  M2 (authored from M1 evidence; not pre-written)
```

## Parallel-safe pairs (code-disjoint AND dependencies satisfied)

- **Pair A — M1.3 (07) ∥ M1.4 (08).** 07 owns `/core` (registry); 08 owns a **new `/tools/scene-gen` crate** and drives the `/ecs` `World` trait via raw pairs/tags — it does **not** need the registry. Disjoint crates, both unblocked by M1.2. **07 is already running → launch 08 alongside.**
- **Pair B — M1.5 (09) ∥ M1→2 (10).** 09 owns `.github/` + a criterion bench (kept **outside `/core`**); 10 owns the `/core` commit pipeline. Disjoint. Start once Pair A lands (09 needs M1.4's fixture; 10 needs M1.3's registry).

**Not parallel:** 07 and 10 both own `/core` → 10 runs strictly after 07. 08→09 is a dependency chain (09 reuses 08's 5k fixture).

## How to run a pair safely (the discipline that prevents the collisions)

The collisions we've hit — `index.lock` races, `progress.md` churn — come from **two sessions sharing one git index and one working copy.** Two ways to avoid them:

**Recommended — git worktrees (full isolation):**

```
# give the second session its own working copy + branch + index
git worktree add ../metrocalk-m1.4 -b m1.4     # session 08 runs in here
#   (session 07 keeps running in the main checkout, on its own branch)
# when both pass:
git checkout main && git merge m1.3 && git merge m1.4
git worktree remove ../metrocalk-m1.4
```

Separate indexes → no lock race; separate working copies → no file collision. Expect at most one trivial merge nit: both sessions prepend to `progress/M1.md` (newest-first), so the top of the log can conflict — resolve by keeping both entries.

**Zero-overhead fallback — interleave.** The pair is independent, so just run the two sessions back-to-back (07 then 08). You give up wall-clock but get the planning clarity with zero git risk.

**Never** run two sessions in the **same** working copy at the same time — that is exactly the `index.lock` race.

## Lane rule

Each prompt's header declares the crate/paths it **Owns**. A parallel pair must own disjoint paths (Pairs A and B do). Each session updates `progress.md` / `architecture.md` normally **on its own branch**; the merge reconciles them.

## Status

`05` M1.1 ✓ · `06` M1.2 ✓ · `07` M1.3 ⏳ · `08` M1.4 · `09` M1.5 · `10` M1→2 · **M2** = not yet authored (write from M1 evidence — Tauri WebView2 IPC + real-scene render are the M2 gates).
