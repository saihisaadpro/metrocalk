# Progress

## Now
- **M2 risk-gates underway.** Render gate **M2.2 PASSED** (2026-06-14): the M1.4 stress scene (5k+20k) renders well inside budget native + browser via instancing + GPU frustum culling (compute→indirect) + render bundle — see Done. M1 remains complete + hardened (M1.6 closed four audit gaps).

## Next (milestone M2)
- **M2.1 Tauri WebView2 IPC gate** (the remaining M2 exit gate — 60 Hz drag, worst-case delta payload on Windows; fallback CEF). Once M2.1 + M2.2 both pass → author the **M2 build** (real shell + transport 3 impls + binary delta protocol) from the gate evidence.
- **Carry-forward (later):** getrandom `js` for Loro-in-browser + the Phase-2 pure-Rust query backend (ADR-006).
- **Carry-forward (Phase 2, with collab):** `merge()` rebuilds entities from Loro but **not their ECS tags/pairs** — capabilities are ECS-only, never written to Loro, so the **compatibility query is empty after a merge**. Fix wires the registry (component-kind → capabilities) into `rebuild_ecs_from_loro`; schedule with collab. (Surfaced by M1.6 audit; see `progress/M1.md`.)

## Done (milestone-level)
- **M2.2 render gate PASSED (2026-06-14):** new throwaway `spikes/render-scene` renders the M1.4 stress scene (5k + 20k — instanced cubes + per-entity gizmos + grid) through one wgpu 29.0.3 crate, native + browser, via **instancing + GPU frustum culling (compute → compacted `visible[]` → indirect draws, no multi-draw-indirect / `first_instance=0`) + a grid render bundle**, GPU-time and CPU-submit measured separately. **Native** (Vulkan/RTX 4060): GPU p99 **0.60 ms** @5k / **0.88–0.95 ms** @20k (budgets 8.3 / 16.6 ms → ~14×/17× headroom, no spikes). **Browser** (Chrome 149 + Edge 149/Dawn): GPU p99 **1.34 ms** @5k / **3.26 ms** @20k (clears 60/30 fps bars). **Draw calls constant at 3** as entities go 4× (GPU time only 1.4× ⇒ instancing engages); largest storage buffer **0.61 MB ≪ 128 MB**. Verdict: render approach holds. **Gap:** Firefox 141 not run (not installed) — low risk. ADR-003 status-updated. Detail → `progress/M2.md`, numbers → `spikes/render-scene/RESULTS.md`.
- Pre-M0: feasibility plan v2 (locked stack), research sweep (~30 sources), doc structure + ADRs 001–005 + Opus 4.8 prompt set.
- **M0 complete (2026-06-13):** 3 spikes — ① Loro ADOPT, ② Flecs ADOPT, ③ wasm/WebGPU browser-render PROVEN + CI tripwire live — and the gate review. New decision: ADR-006 (browser query backend). Detail → `progress/M0.md`, consolidation → `M0-gate-review.md`.
- **M1 complete (M1.1–M1.5 + M1–2 + M1.6, 2026-06-13):** monorepo + CI · ECS `World` wrapper + Flecs backend · component-metadata registry · shared seeded stress-scene + F1 storage verdict (keep dense) · 16 ms compat-query CI perf gate (3rd CI tripwire) · **ECS↔Loro commit pipeline + engine-side undo/redo + merge-validation (M1–2)** · **pipeline hardening (M1.6)**: precise additive-undo (`Op::RemoveField` — no more sibling-field destruction), atomic pre-validated commit (all-or-nothing), O(1) `tid→eid`, Loro-error propagation in `apply_*`. **flecs_ecs M1 go/no-go: GO** — undo p99 0.24–0.72 ms (≫ under the 5 ms target), resurrection robust, two-fork merge converges, all 8 invalid-state classes detected+repaired, 49 tests green. Detail → `progress/M1.md`.

---

## Log

Detailed dated entries are sharded by milestone under `progress/` (keeps this dashboard thin).
Append to the **current milestone's** file, newest first, one entry per session, with measured
numbers + ADR links. Live state stays here in Now/Next above.

- [progress/M2.md](progress/M2.md) — **current milestone** (M2 risk-gates → build)
- [progress/M1.md](progress/M1.md) — foundation build (M1.1–M1.6)
- [progress/M0.md](progress/M0.md) — foundation, 3 spikes, gate review (2026-06-12 → 06-13)
