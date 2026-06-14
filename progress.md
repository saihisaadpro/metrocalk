# Progress

## Now
- **M2 build underway.** Editor UI scaffolded (**M2.5**): `/editor` is a projection of the core (invariant 1) — Zustand/`useSyncExternalStore` store, JSON Forms inspector, React Flow graph, optimistic echo with rejection-as-UX; selective re-render verified at 5k (0 of 5000 rows on a field edit), 11 tests green. ADR-010. See Done. (M1 complete + hardened.)

## Next (milestone M2)
- **M2.6 convergence**: mount the M2.5 editor in the M2.3 single-window shell; point the desktop transport binding at the real Tauri Channel (M2.4) + swap `MockCore` → WASM/Rust core; wire the viewport input hand-off; integrate the M2.2 render verdict; run the deferred DPI/min-spec cases.
- **Carry-forward (later):** getrandom `js` for Loro-in-browser + the Phase-2 pure-Rust query backend (ADR-006).
- **Carry-forward (Phase 2, with collab):** `merge()` rebuilds entities from Loro but **not their ECS tags/pairs** — capabilities are ECS-only, never written to Loro, so the **compatibility query is empty after a merge**. Fix wires the registry (component-kind → capabilities) into `rebuild_ecs_from_loro`; schedule with collab. (Surfaced by M1.6 audit; see `progress/M1.md`.)

## Done (milestone-level)
- **M2.5 editor UI scaffold (2026-06-14):** `/editor` as a projection of the core (invariant 1), delta-fed over M2.4. Zustand/`useSyncExternalStore` projection store (entity-keyed, immutable per-entity, separate summary projection); TS transport client mirroring the M2.4 envelope (`%LOR` + `%EPH`); JSON Forms inspector (custom renderers via testers; edit → JSON-Patch tx); React Flow neighborhood graph (Sigma.js noted); virtualized 5k hierarchy; optimistic echo + **rejection-as-UX** ("every 'no' explained"); input-ownership stub (viewport → native, invariant 4). **11 Vitest tests**: selective subscription at 5k (edit one entity → 0 of 5000 rows re-render), tear-free under a React 19 transition, reject path, envelope round-trip, app wiring. Single-edit apply+render ≈ 24–70 ms @5k (jsdom). ADR-010. Detail → `progress/M2.md`.
- Pre-M0: feasibility plan v2 (locked stack), research sweep (~30 sources), doc structure + ADRs 001–005 + Opus 4.8 prompt set.
- **M0 complete (2026-06-13):** 3 spikes — ① Loro ADOPT, ② Flecs ADOPT, ③ wasm/WebGPU browser-render PROVEN + CI tripwire live — and the gate review. New decision: ADR-006 (browser query backend). Detail → `progress/M0.md`, consolidation → `M0-gate-review.md`.
- **M1 complete (M1.1–M1.5 + M1–2 + M1.6, 2026-06-13):** monorepo + CI · ECS `World` wrapper + Flecs backend · component-metadata registry · shared seeded stress-scene + F1 storage verdict (keep dense) · 16 ms compat-query CI perf gate (3rd CI tripwire) · **ECS↔Loro commit pipeline + engine-side undo/redo + merge-validation (M1–2)** · **pipeline hardening (M1.6)**: precise additive-undo (`Op::RemoveField` — no more sibling-field destruction), atomic pre-validated commit (all-or-nothing), O(1) `tid→eid`, Loro-error propagation in `apply_*`. **flecs_ecs M1 go/no-go: GO** — undo p99 0.24–0.72 ms (≫ under the 5 ms target), resurrection robust, two-fork merge converges, all 8 invalid-state classes detected+repaired, 49 tests green. Detail → `progress/M1.md`.

---

## Log

Detailed dated entries are sharded by milestone under `progress/` (keeps this dashboard thin).
Append to the **current milestone's** file, newest first, one entry per session, with measured
numbers + ADR links. Live state stays here in Now/Next above.

- [progress/M2.md](progress/M2.md) — **current milestone** (M2 build)
- [progress/M1.md](progress/M1.md) — foundation build (M1.1–M1.6)
- [progress/M0.md](progress/M0.md) — foundation, 3 spikes, gate review (2026-06-12 → 06-13)
