# Prompt 19 — Close-out: finish M2 (live + measured) → land north-star #1 (M3.1)

> Use with `00-orchestrator.md` (v2). **Effort `max`.** Serial; commit on `master` in small green increments.
> **This is a close-out spec, not a one-shot.** It spans M2's live close **and** M3.1, and it MAY take more than one session. The discipline that makes that safe: **land verifiable green increments; if context runs low, STOP at a clean *committed* checkpoint and record exactly what remains in `progress/M2.md` (or `progress/M3.md`) — never cram a broken half-build, never fabricate a measurement or a human verdict.** (This is the exact checkpoint discipline the M2.6 spine session modeled — keep it; it's why the project has never had a broken `master`.)
>
> **The finish line:** Metrocalk is a clickable desktop app — the real 5k scene renders, you select an entity, **ranked** compatible targets reveal with **every "no" explained**, one click binds (undoable, persists on reload), and a human has judged whether it *feels* like the categorical win over Unity/Unreal. M2 has a **measured** go/no-go; `north-star-tests.md` test 1 passes its **live** acceptance; the docs are flipped to M3. This prompt combines the M2.6 live remainder (prompt 17 part 2) + M3.1 (prompt 18) into one self-contained finish.
>
> Read first: `00-orchestrator`; `metrocalk.md` (north-star #1) + `north-star-tests.md` (**test 1 = the literal acceptance script**); ADR-008 (single-window shell — proven), ADR-009 (transport), ADR-010 (editor); the M2.6 spine already on `main`; the confirmed M3.1 machinery in `<state>`.

<state>
All on `master`/`origin/main`, tree clean (`799867c`+). **M2 integrated, headless:** the `editor-shell` `bridge` (editor `EditTx` → `/core` `Op` → `Engine::commit` → echoed `ProjectionDelta`; **real `Engine<FlecsWorld>`, no MockCore**; undoable; reject-with-the-pipeline's-real-reason) + the `/core` projection read-API (`entity_ids` · `parent_of` · `components_of` · `bindings` + `project_full`) are built + green (6 real-engine tests). **M3.1 machinery confirmed real:** the M1.3 registry carries `provides`/`requires`/`observes` interned as capability pairs (`providers_of`/`requirers_of`); the M1.2 `World` has `targets(e, rel)`, `build_query`/`for_each_match`, `has_pair`. The ADR-008 composite, M2.2 renderer, M2.4 transport, M2.5 editor are all proven pieces. **Also done since (on `main`):** M3.1's reveal→rank→explain engine + latency (Part B deliverables 1–5) — `editor-shell/src/reveal.rs`, ADR-011, **p99 1.1 ms** on a 5k capability scene, green (two budget-blowing drafts fixed by an indexed query + O(1)-per-target `why_not`). **Still open — start here:** **Part A** (the live Tauri editor + the 6 residuals + the measured M2 gate), then **Part B deliverable 6** (surface the done reveal engine live + the human dogfood verdict — gated on Part A's live editor existing). Do NOT rebuild Part B 1–5; wire it.
</state>

---

## Part A — close M2 (live + measured)

Build the live Tauri `/editor-shell` by **wiring the proven spine** — do not rebuild it. ADR-008 single-window composite (transparent WebView2 over the wgpu viewport); the M2.2 instanced renderer wired to the live `/core` scene via `project_full`; Rust pixel-picking + camera; the real Tauri Channel (M2.4) replacing any stub; the M2.5 editor pointed at the real core (no MockCore). Then:

1. **Live round-trip** — click an entity in the viewport → inspector shows it → edit a field / bind a target → commit → echo → viewport + UI update; Ctrl-Z undoes; reload preserves (Loro).
2. **Zero-per-frame-IPC drag** — instrument and verify a 60 Hz viewport drag stays entirely in Rust (invariant 4).
3. **wasm32 parity** — the scene renders in the browser build (ADR-006).
4. **The 6 deferred residuals — each measured + recorded** (pass, or honest flag): DPI 100%↔200% monitor move · min-spec machine · sustained ≥60 s flicker watch · real Tauri Channel e2e on Windows (re-confirm ~3.4 ms) · Windows snapshot-load cliff · real-browser store-apply.
5. **The measured M2 go/no-go** — does the integrated editor hold the frame budget on the real scene? Quote end-to-end numbers (render + commit + delta + UI update), ≥2 runs.

## Part B — land north-star #1 (M3.1)

The deterministic reveal engine is **landable green independent of the live loop** — build + test + latency-measure it first (it's the actual differentiator), then surface it live. Grounded in the confirmed API:

1. **Reveal** — for selected entity `e`: `world.targets(e, requires_rel)` → required caps; per cap `C`, `world.build_query([Clause::with(Pair{provides_rel, Exact(C)}), Clause::without(Pair{binds_to, Any})])` + `for_each_match` → compatible candidates (providers not yet bound). The exact M1.5 query (~12 µs).
2. **Rank** — proximity (Transform distance via `engine.components_of`), then recency, then affinity — **deterministic**; document + write an ADR (+ its `decisions/README` row).
3. **Explain every "no"** — for each non-candidate, the registry-derived reason (`!provides C` → "doesn't provide Health"; `has_pair(e, binds_to, that)` → "already bound"; type/dim mismatch). Genuinely helpful, not generic.
4. **One-click bind** — wire the candidate click to the built `apply_edit` (Bind → `Op::AddBinding` → commit → undoable; already tested).
5. **Latency proof** — compat-query + reveal latency on a 5k scene **that carries capability pairs** (`Provides`/`Requires`). **Prereq:** M2.6's Transform-only seed doesn't register capabilities — add a stdlib-capability scene using the `scene.rs build_scene` pattern that does `add_pair(Provides, Health)`.
6. **Live surfacing + dogfood** (needs Part A's live loop) — highlight ranked candidates in the viewport + React Flow; run `north-star-tests.md` **test 1** in the running window; **record the honest "does it feel like the categorical win" verdict — the human judgment IS the finding, never a rubber-stamp.**

## Human / hardware-in-the-loop gates (instrument + flag + hand off; NEVER fabricate)

Three items need the user's eyes/hardware on the running app: the **sustained ≥60 s flicker watch**, the **min-spec machine**, and the **M3.1 dogfood verdict**. Build each so the user can run it with one documented command; instrument what you can (swapchain-aware capture, IPC/frame counters); record honest pass-or-flag results. If you can't run one solo, that's a clean hand-off to the user — not a fail, and never a fabricated pass.

## Documentation & report close (the heaviest doc duty — this flips the milestone)

- `progress.md` → **M2 complete** (with the measured gate) → **M3 in progress**; shard M3 into `progress/M3.md` (archive M2 per the sharding pattern).
- `architecture.md` → validate the transparent-overlay diagram with ADR-008 (remove any "self-composite/DComp/CEF" hedge — the evidence retired it); mark the shell/transport/viewport/editor/binding-UX rows real; status line → M2 complete (then M3); **strike the resolved open-questions** (the prune rule).
- `decisions/` → the ranking-function ADR (+ any other) and its `decisions/README.md` index row.
- `north-star-tests.md` → tick **test 1**'s boxes with the live results + the dogfood verdict.
- Report in the `00` structure (Outcome · Numbers · Decisions · **Honest boundaries** · Risks · Next), with **every residual + the gate + the dogfood verdict recorded** (pass or honest flag — never invented).

## Definition of done (the whole arc — when all green, the thesis is proven *live*)

☐ live single-window editor: real scene renders, click→inspect→edit/bind→commit→echo, undoable, persists · ☐ zero-per-frame-IPC drag verified · ☐ wasm32 parity · ☐ all 6 residuals measured or honestly flagged · ☐ measured M2 go/no-go quoted · ☐ M3.1 reveal+rank+explain engine green + <16 ms on a capability-bearing 5k scene · ☐ one-click bind wired (undoable, persists) · ☐ `north-star-tests.md` test 1 passes **live** + the honest dogfood verdict recorded · ☐ diff reviewed against all 5 invariants · ☐ docs flipped (M2 complete → M3; diagram validated; ADRs + index; test-1 ticks) · ☐ committed green on `master`, tree clean.

> When this DoD is green: **M2 is complete and north-star #1 is real** — Metrocalk is a clickable app whose signature interaction works and has been judged by a human. Next is M3.2 (describe-to-create / type-to-create — the second half of the north-star), authored from this evidence.
