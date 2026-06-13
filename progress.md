# Progress

## Now
- **M1 complete.** All deliverables verified — see Done below.

## Next (milestone M2)
- **Carry-forward (later):** getrandom `js` for Loro-in-browser + the Phase-2 pure-Rust query backend (ADR-006); real-scene render cost @ ≥5k entities (M2 stress scene); Tauri WebView2 IPC (M2 gate).

## Done (milestone-level)
- Pre-M0: feasibility plan v2 (locked stack), research sweep (~30 sources), doc structure + ADRs 001–005 + Opus 4.8 prompt set.
- **M0 complete (2026-06-13):** 3 spikes — ① Loro ADOPT, ② Flecs ADOPT, ③ wasm/WebGPU browser-render PROVEN + CI tripwire live — and the gate review. New decision: ADR-006 (browser query backend). Detail → `progress/M0.md`, consolidation → `M0-gate-review.md`.
- **M1 complete (M1.1–M1.5 + M1–2, 2026-06-13):** monorepo + CI · ECS `World` wrapper + Flecs backend · component-metadata registry · shared seeded stress-scene + F1 storage verdict (keep dense) · 16 ms compat-query CI perf gate (3rd CI tripwire) · **ECS↔Loro commit pipeline + engine-side undo/redo + merge-validation (M1–2)**. **flecs_ecs M1 go/no-go: GO** — undo p99 0.145 ms (34× under 5 ms target), resurrection p99 0.282 ms, two-fork merge converges, all 8 invalid-state classes detected+repaired, 44 tests green. Detail → `progress/M1.md`.

---

## Log

Detailed dated entries are sharded by milestone under `progress/` (keeps this dashboard thin).
Append to the **current milestone's** file, newest first, one entry per session, with measured
numbers + ADR links. Live state stays here in Now/Next above.

- [progress/M1.md](progress/M1.md) — **current milestone** (foundation build)
- [progress/M0.md](progress/M0.md) — foundation, 3 spikes, gate review (2026-06-12 → 06-13)
