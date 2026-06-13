# M0 Gate Review — 2026-06-13

Consolidates the three M0 spikes (`spikes/loro`, `spikes/flecs`, `spikes/wasm`) into settled gate
decisions and the M1 work breakdown. Verdicts are on measured numbers, quoted below. M1 starts on
settled ground: no "decide later" carried forward.

## 1. Gate verdicts (measured vs the prompt-01/02/03 criteria)

### Spike ① — Loro document layer → **ADOPT** (confirms [ADR-002](decisions/002-loro-over-custom-wal.md))

| criterion (prompt 01) | threshold | measured | verdict |
|---|---|---|---|
| single-op undo p99 @10k history | < 5 ms | **0.13 ms** (undo of latest op) | ✅ |
| 10k-mutation run | < 10 s | **0.56 s** | ✅ |
| full snapshot of 5k-entity scene | < 20 MB | **4.51 MB** | ✅ |
| every post-merge invalid state detectable + repairable | all | 8 classes catalogued, all detectable + repairable | ✅ |

All four pass. Merge converged bidirectionally; MovableTree produced **no cycles** under conflicting
reparents; asset-ref strings survived merges intact. Carry-forward constraints (do not change the
decision): **F1** `ensure_mergeable_*` corrupts under undo/redo → use regular containers + the
merge-validation layer; **F2** undo computes its inverse via full-document checkouts, so *latest-op*
undo is 70 µs but **consecutive/50-op-group undo is 50–62 ms** (> the 16 ms budget) → M1 needs an
engine-side inverse-op undo stack, Loro `checkout` reserved for deep history; **F3** entity IDs must
be peer-namespaced (concurrent create collided → 23 dup-eid violations).

### Spike ② — Flecs query engine → **ADOPT** (confirms [ADR-001](decisions/001-flecs-over-bevy-ecs.md))

| criterion (prompt 02) | threshold | measured (safety locks ON, shipping config) | verdict |
|---|---|---|---|
| cached compat query p99 @5k | < 16 ms | **12.2 µs** (~1300×) | ✅ |
| cached compat query p99 @20k | < 16 ms | **58.3 µs** (~275×) | ✅ |
| query p99 under mutation | < 16 ms | **41.1 µs** (~390×) | ✅ |
| zero stale results under churn | required | PASS (211→1211→211) | ✅ |
| no soundness landmine the wrapper can't contain | required | contained by `flecs_safety_locks` (0–10% cost) | ✅ |

All pass. Criterion cross-check agrees (~14.9 µs mean). Safety-lock ON vs OFF delta is 0–10% (noise)
→ ship ON. Carry-forward: **F1** pair-induced archetype fragmentation = **14.8 KB/entity at 20k**
(query latency unaffected); mitigate with `DontFragment`/sparse — validate in M1.

### Spike ③ — wasm32 + WebGPU → **PASS (browser-render leg of [ADR-003](decisions/003-desktop-first-tauri-exit-gate.md))**

| criterion (prompt 03) | measured | verdict |
|---|---|---|
| same crate renders native + ≥2 browsers | native (Vulkan) + Chrome 149 + Edge 149, 512×512 verified by pixel readback + screenshot | ✅ (literal); ⚠️ both browsers are Dawn — 2nd *engine* (Firefox/Safari) not verified on this machine |
| CI green < 5 min, verified by a real run | **green 54 s cold / 19 s warm**, and **verified to fail** on a deliberate wasm32 break, then reverted | ✅ |
| CONSTRAINTS.md complete with real numbers | sizes, frame times, adapter diff, flecs/loro wasm32 check, browser matrix | ✅ |

Pass. Honest gap: cross-engine coverage (Firefox/Safari) — low risk (Firefox WebGPU is built on
`wgpu`), fast follow-up. Funnel baseline: **~130 KB brotli transfer** (118 KB wasm + 12 KB JS) for a
minimal wgpu app.

## 2. Cross-spike reconciliation

### 2a. wasm32 compile results → ADR-001/002/004 (handled, not buried)

- `loro` 1.13.1 **builds for wasm32** ✓ (needs `getrandom` `js` at runtime). ADR-002 unaffected.
- `flecs_ecs` 0.2.2 **does not build for wasm32** ✗ (C core, no wasm libc/sysroot). This trips
  ADR-001's revisit clause on a *new* axis (web target) and threatened ADR-004's browser funnel.
- **Resolved by [ADR-006](decisions/006-browser-query-backend.md):** the browser does not run Flecs;
  the query-API wrapper gets a second, pure-Rust backend over the Loro projection (Phase 2). The
  ADR-004 funnel is **preserved** — the browser lite-editor runs Loro + pure-Rust queries + wgpu,
  all wasm-proven, fully offline. ADR-004 is not relitigated; only internal architecture refined +
  Phase-2 scope added (build/benchmark the pure-Rust backend).

### 2b. Frame-budget arithmetic — does the edit path compose under 16 ms?

Worst-case single user edit (mutate → mirror → re-query), stacking measured p99s pessimistically:

| step | source | p99 cost |
|---|---|---|
| ECS mutation (Flecs add/remove/set pair) | spike ② (≤ the 100-mutation churn, ~sub-µs each) | ~5 µs |
| Loro commit (mirror the mutation) | spike ① per-op p99 192 µs (prepare+execute+commit) | 192 µs |
| cache invalidation + cached query re-run | spike ② B2 p99 (after *100* mutations; upper bound for one) | 41 µs |
| **Rust-side subtotal** | | **≈ 0.24 ms** |
| UI projection + React update (generous; off the Rust hot path, deltas only) | not measured — budgeted | ~1 ms |
| **end-to-end worst case** | | **≈ 1.2 ms** |

**≈ 1.2 ms vs the 16 ms budget — ~7%.** Even at 5× pessimism for unmeasured real-scene effects
(larger components, ranking heuristics, explainability, merge-validation re-check), ~6 ms — still
inside 16 ms. The edit path composes comfortably. **Two things are explicitly NOT in this budget:**

1. **Interactive undo must not use Loro `checkout`.** Latest-op undo is 70 µs (fine), but
   consecutive/group undo is **50–62 ms** (spike ① F2) — 3–4× over budget. M1 builds an engine-side
   in-memory inverse-op stack; Loro checkout is for deep history/time-travel only.
2. **Real-scene render cost is unmeasured.** Spike ③ rendered a *triangle* (render work ≈ 0; its
   "8.3 ms frame" is 120 Hz vsync cadence, not work). The 5k-entity scene's render cost is an
   M2 measurement (the "60 Hz drag on Windows WebView2" gate), not proven here.

### 2c. Memory coexistence at 5k entities (desktop budget)

- Loro 5k working set: **56 MB** (spike ① bench-1 peak, gen + 10.5k mutations).
- Flecs 5k: **~74 MB** (extrapolated from spike ② 296 MB @ 20k = 14.8 KB/entity, *worst-case*
  fragmentation; DontFragment cuts this).
- **Coexist ≈ 130 MB at 5k** + tens of MB wgpu/app → ~0.4–0.8 % of a 16–32 GB dev desktop.
  Comfortable. At 20k (~Loro + ~296 MB Flecs ≈ ~0.5 GB) still fine for desktop; apply DontFragment.
  Browser never runs Flecs (ADR-006), so no double-footprint under wasm's 4 GB ceiling.
  (Compare at equal scale — do not add Loro's 222 MB whole-process figure, which was a 100k-op doc +
  two merge forks, to the Flecs 20k number; different scenarios.)

## 3. ADR statuses after this review (no "pending")

| ADR | status |
|---|---|
| 001 Flecs | **Accepted — confirmed at M0 gate review.** Native-only for wasm (→ ADR-006). M1 *integration* go/no-go is a normal roadmap milestone, not an open decision. |
| 002 Loro | **Accepted — M0 gate PASSED.** Confirmed; frame-budget + wasm-build reconciled. |
| 003 Tauri/desktop-first | **Accepted — browser-render leg PROVEN.** Browser-ECS dimension resolved by ADR-006. The M2 Tauri WebView2 IPC gate genuinely remains (not in M0 scope). |
| 006 Browser query backend | **Accepted (new).** Pure-Rust index over Loro on wasm; Flecs native-only. |

## 4. The strongest case this review is wrong

The spikes are isolated microbenchmarks; the real M1 system composes them through glue that **none
of them built or measured**. Specifically: the ECS↔Loro commit pipeline (invariant #3) was never
wired — spike ① measured Loro commits on a Loro-only model and spike ② measured Flecs queries on a
Flecs-only model, but nobody measured a mutation flowing ECS→delta→Loro-op→merge-validation→cache-
invalidation→query as one transaction; the per-edit cost could be dominated by that unmeasured glue
(delta serialization, the invariant re-check after every commit, revision-counter bookkeeping).
The "8.3 ms native frame" is a zero-work vsync-locked triangle, so the editor's real-scene render
budget is entirely unproven; the worst-case Flecs fragmentation (14.8 KB/entity) is a real number a
diverse real scene could approach; and the browser leg "passed" on two Dawn browsers + a triangle,
with the entire browser query path now resting on an **unbuilt, unmeasured** pure-Rust backend
(ADR-006). **Assessment:** this is a fair critique of *sequencing*, not of the verdicts. Every
measured gate passed by 100–1300×, leaving 2–3 orders of magnitude of headroom to absorb unmeasured
glue — the open question is whether headroom is 10× or 50×, not whether the budget is blown. The one
place the critique genuinely bites — undo via Loro checkout at 50–62 ms — is already converted into
an M1 task (engine-side undo stack). The scope rule (re-run only if two reports *contradict*) is not
triggered: no two spike numbers conflict. **Verdicts stand; M1 must measure the composed pipeline and
M2 must measure real-scene render — both are in the breakdown below.**

## 5. M1 work breakdown

See `progress.md` "Now"/"Next" for the ordered, acceptance-tested task list. Headline: monorepo →
ECS wrapper API (absorbs the Flecs ergonomics fallout + the ADR-006 two-backend constraint) →
metadata registry → seeded stress scene (+ DontFragment memory re-measure) → 16 ms CI query gate →
ECS↔Loro commit pipeline + merge-validation + engine-side undo stack.
