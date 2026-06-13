# Prompt 10 — M1→2: ECS↔Loro commit pipeline + merge-validation + engine-side undo stack

> Use with `00-orchestrator.md` (v2) as system prompt. **Effort `max`** — load-bearing: this makes invariant 3 (the single transactional pipeline) real and carries three *measured* loro-spike constraints (F1/F2/F3).
> **Lane:** Owns `/core` commit-pipeline + undo + merge-validation modules (depends on `/ecs`) · **Blocked by M1.3** (07 — same `/core` crate) · **Parallel-safe with M1.5 (09)** · see `prompts/README.md` for the worktree discipline.
> Read first: ADR-002 + `spikes/loro/README.md` (F1/F2/F3 + the 8 invalid-state classes + F2's *measured* undo cost), the `metrocalk-ecs` `World` trait shipped in M1.2 (`/ecs`: `Entity`/`Target`/`Clause`/`defer`), and invariants 1/2/3.

---

<task>
Build the single transactional commit pipeline that *is* invariant 3: every mutation — editor, plugin, AI — becomes a transaction applied to the authoritative ECS (the `/ecs` `World`) and mirrored as deltas into the Loro document. On top of it, the engine-side undo/redo stack and the post-merge validation layer. After this session, "everything is a transaction you can undo, and a merge can't corrupt the scene" is real code, not a promise.
</task>

<scope>
In scope (all in `/core`, depending on `/ecs`): the commit pipeline (ECS mutation ↔ Loro mirror, deltas only); transaction grouping; the engine-side in-memory inverse-op undo/redo stack; peer-namespaced entity IDs; the merge-validation layer (the 8 invalid-state classes); property + merge tests. Use the `World` trait's mutations + `defer` for atomic batches, and **regular Loro containers** (not `ensure_mergeable_*`, per F1).
Out of scope: real-time collaboration networking (Phase 2 — the pipeline must be *transport-ready* but ship no transport impl here); the browser pure-Rust backend (ADR-006, Phase 2); the Rules layer; UI; rendering. Build the transactional core + undo + merge-validation only.
</scope>

<deliverables>
1. **Commit pipeline (invariant 3).** One entry point all mutations flow through as transactions. The `/ecs` `World` stays authoritative at runtime; each committed transaction mirrors to the Loro document as **deltas** (invariant 2 — never a full-state snapshot). A transaction = one `defer`'d `World` batch ↔ one Loro commit. The *same* path serves human / plugin / AI callers — no side doors.
2. **Engine-side undo/redo stack (F2).** An in-memory inverse-op stack — **not** Loro `checkout` (the spike measured 50–62 ms for bulk/deep undo). One user action = one small transaction group; latest-op undo/redo is O(change), target **< 5 ms p99**. Loro `checkout` stays reserved for deep history / time-travel, never per-Ctrl-Z.
3. **Peer-namespaced entity IDs (F3).** Entity IDs embed the peer/replica id (or derive from Loro's `(peer, counter)` `TreeID`) so concurrent creation on two replicas never collides — the spike saw 23 duplicate-eid violations with a peer-local counter.
4. **Merge-validation layer (F1 + the 8 classes).** After importing/merging Loro updates, run a validator that **detects and repairs** the eight invalid-state classes catalogued in `spikes/loro` (dangling edge endpoint · orphan component record · entity-missing-record · duplicate eid · tree cycle · alive-under-deleted-ancestor · corrupt asset-ref · malformed edge). This is invariant 3's second sentence — re-check ECS invariants after every CRDT merge. Regular containers per F1 (the mergeable helper breaks under undo/redo).
5. **`flecs_ecs` M1 integration go/no-go.** Record whether driving Flecs through the wrapper + pipeline holds up under transactional load at scene scale (the integration gate ADR-001 deferred to M1). Quote numbers.
</deliverables>

<success_criteria>
Undo/redo property tests pass over randomized transaction sequences **including entity resurrection** (delete → undo restores the entity + its components + its edges); latest-op undo **< 5 ms p99** (measured, twice); a two-fork merge **converges** (byte-identical canonical deep-value) AND the validator **repairs every injected instance** of all 8 classes; **deltas only** across the mirror (no full-state snapshot — inspect/grep); the pipeline is the **sole** mutation path (a direct-`World`-mutation bypass is prevented by visibility or caught by a test); `flecs_ecs` M1 go/no-go recorded with numbers; no `flecs_ecs` / `loro` types leak past their owning crates (CI grep).
</success_criteria>

<verification>
Property-test the undo stack with seeded randomized sequences incl. delete/resurrect and grouped multi-op actions. Re-run the loro-spike two-fork merge stress through the **real** pipeline, injecting all 8 invalid-state classes and asserting convergence + repair. Bench undo twice per the orchestrator's discipline (median + p99). Write one adversarial paragraph: "the case the in-memory inverse-op stack diverges from Loro's own op history — e.g., a merge lands between an op and its undo" — if persuasive, reconcile the design before finishing. Confirm invariant 2 by inspecting the mirror path (no `send_state`/snapshot).
</verification>

<definition_of_done>
☐ commit pipeline is the sole mutation path; `/ecs` `World` authoritative, Loro mirrored via deltas · ☐ undo/redo property tests green incl. resurrection; latest-op undo < 5 ms p99 (twice) · ☐ peer-namespaced IDs; zero dup-eid under concurrent create · ☐ merge-validation repairs all 8 classes; two-fork merge converges · ☐ `flecs_ecs` M1 go/no-go recorded with numbers · ☐ no Flecs/Loro type leaks (CI grep) · ☐ progress.md → **M1 complete / M2 next** + dated log; architecture.md commit-pipeline + undo now real (prune the "later") · ☐ working tree clean, committed (on your branch if running parallel — see `prompts/README.md`).
</definition_of_done>
