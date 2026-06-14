# Progress

## Now
- **M2 build underway.** Shell composition resolved (**M2.3**): **single-window** transparent WebView2 over native wgpu passes on dGPU+iGPU — M2.1 1b "FAIL" was a GDI capture artifact (ADR-008); no DComp/CEF. See Done. (M1 complete + hardened.)

## Next (milestone M2)
- **M2.6 convergence**: build the real single-window shell, wire the `shell-input-routing` layer + M2.4 transport, integrate the M2.2 render verdict; run the deferred DPI-100↔200/min-spec cases there.
- **Carry-forward (later):** getrandom `js` for Loro-in-browser + the Phase-2 pure-Rust query backend (ADR-006).
- **Carry-forward (Phase 2, with collab):** `merge()` rebuilds entities from Loro but **not their ECS tags/pairs** — capabilities are ECS-only, never written to Loro, so the **compatibility query is empty after a merge**. Fix wires the registry (component-kind → capabilities) into `rebuild_ecs_from_loro`; schedule with collab. (Surfaced by M1.6 audit; see `progress/M1.md`.)

## Done (milestone-level)
- **M2.3 shell-composition gate (2026-06-14):** `spikes/shell-composite` proved **single-window** (transparent WebView2 over native wgpu on one HWND) composites correctly on Windows on **both** the RTX 4060 dGPU and the Intel Iris Xe iGPU — real panel layout, under motion/resize/overlapping-input. M2.1's 1b "FAIL" was a **GDI capture artifact** (GDI can't see a flip-model swapchain; DXGI Desktop Duplication shows a clean composite; the 16×16 collapse never recurred). No DComp/CEF (~170 MB avoided); fallback ladder not exercised. Path-agnostic input-routing layer built first (7 tests). ADR-008. Detail → `progress/M2.md`. (ADR-007 status-note deferred to the m2.1↔m2.3 merge.)
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
