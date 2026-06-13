# Progress

## Now
- **M1.5 — 16 ms compatibility-query CI gate.** Wire the shared stress-scene (M1.4) compat query in as a CI perf gate. **Acceptance:** CI fails if cached compat-query p99 > 16 ms on the 5k scene (currently ~9–14 µs). Prompt: `prompts/09-m1.5-query-ci-gate.md`.
- M1.4 stress-scene + F1 **DONE (2026-06-13):** one shared seeded generator (`ecs::scene`, byte-identical, digest-pinned cross-OS via CI, 5k/20k presets) used by ecs bench + core tests + `tools/scene-bench`. **F1 verdict: keep DENSE storage.** DontFragment cuts memory 3.6× (8.8→2.4 KB/entity @20k) but **breaks per-entity `targets()`** (Flecs `target_for` limitation) and slows the query 3.5–8× (still ≪16 ms); the win doesn't bite at M1 scale and the browser uses Loro not Flecs (ADR-006). `set_sparse` kept as a measured lever for large native scenes once `targets()` is sparse-safe. Table + numbers: `progress/M1.md`.

## Next (M1, ordered — each one focused session, with acceptance test)
- **M1–2 — ECS↔Loro commit pipeline + merge-validation + engine-side undo stack.** Every mutation = one transaction; regular Loro containers (not `ensure_mergeable_*`, F1-loro); peer-namespaced entity IDs (F3-loro); **engine-side in-memory inverse-op undo stack** for interactive undo (F2-loro — NOT Loro `checkout`, which is 50–62 ms for bulk undo); merge-validation re-checks the 8 invalid-state classes from spike ①. **Acceptance:** 100% undo/redo property tests incl. entity resurrection; latest-op undo <5 ms p99; two-fork merge converges + validator repairs every injected invalid state; flecs_ecs M1 integration go/no-go recorded.
- **Carry-forward (later):** getrandom `js` for Loro-in-browser + the Phase-2 pure-Rust query backend (ADR-006); real-scene render cost @ ≥5k entities (M2 stress scene); Tauri WebView2 IPC (M2 gate).

## Done (milestone-level)
- Pre-M0: feasibility plan v2 (locked stack), research sweep (~30 sources), doc structure + ADRs 001–005 + Opus 4.8 prompt set.
- **M0 complete (2026-06-13):** 3 spikes — ① Loro ADOPT, ② Flecs ADOPT, ③ wasm/WebGPU browser-render PROVEN + CI tripwire live — and the gate review. New decision: ADR-006 (browser query backend). Detail → `progress/M0.md`, consolidation → `M0-gate-review.md`.

---

## Log

Detailed dated entries are sharded by milestone under `progress/` (keeps this dashboard thin).
Append to the **current milestone's** file, newest first, one entry per session, with measured
numbers + ADR links. Live state stays here in Now/Next above.

- [progress/M1.md](progress/M1.md) — **current milestone** (foundation build)
- [progress/M0.md](progress/M0.md) — foundation, 3 spikes, gate review (2026-06-12 → 06-13)
