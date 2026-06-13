# Progress

## Now
- **M2 underway — M2.1 Tauri exit-gate RESOLVED (2026-06-14).** Risk-first M2 entry ([ADR-007](decisions/007-m2.1-tauri-gate-result.md), `spikes/tauri-shell`). **IPC PASS:** real 103-byte Loro deltas at 60 Hz over WebView2 — Channel p99 3.4–3.6 ms / WebSocket p99 1.3–1.7 ms RTT, 0 dropped (overhead-bound, *not* the "~200 ms / 10 MB" bandwidth case ADR-003 feared). **Compositing FAIL:** transparent WebView2 works on its own, but a native wgpu surface on the same window HWND blacks/collapses it — Graphite's problem, reproduced → plan the shell around **self-composite** (UI-as-texture in wgpu), child-webview/DComp follow-up before any CEF pivot. *(Next M2 gate: M2.2 render cost — prompt 13.)*
- **M1 complete + hardened.** All deliverables verified; M1.6 pipeline-hardening pass closed four audit gaps (precise additive-undo, atomic pre-validated commit, O(1) `tid→eid`, Loro-error propagation) before M2 builds on the pipeline — see Done below.

## Next (milestone M2)
- **M2.2 render gate** (prompt 13): real-scene wgpu cost @ ≥5k entities (native + wasm). **M2 build** (real shell + transport impls): author from the M2.1/M2.2 evidence — shell is **self-composite** per ADR-007; transport is deltas-only over Channel/WebSocket (proven on Windows).
- **Carry-forward (later):** getrandom `js` for Loro-in-browser + the Phase-2 pure-Rust query backend (ADR-006); the Tauri child-webview/DComp compositing follow-up (ADR-007).
- **Carry-forward (Phase 2, with collab):** `merge()` rebuilds entities from Loro but **not their ECS tags/pairs** — capabilities are ECS-only, never written to Loro, so the **compatibility query is empty after a merge**. Fix wires the registry (component-kind → capabilities) into `rebuild_ecs_from_loro`; schedule with collab. (Surfaced by M1.6 audit; see `progress/M1.md`.)

## Done (milestone-level)
- Pre-M0: feasibility plan v2 (locked stack), research sweep (~30 sources), doc structure + ADRs 001–005 + Opus 4.8 prompt set.
- **M0 complete (2026-06-13):** 3 spikes — ① Loro ADOPT, ② Flecs ADOPT, ③ wasm/WebGPU browser-render PROVEN + CI tripwire live — and the gate review. New decision: ADR-006 (browser query backend). Detail → `progress/M0.md`, consolidation → `M0-gate-review.md`.
- **M1 complete (M1.1–M1.5 + M1–2 + M1.6, 2026-06-13):** monorepo + CI · ECS `World` wrapper + Flecs backend · component-metadata registry · shared seeded stress-scene + F1 storage verdict (keep dense) · 16 ms compat-query CI perf gate (3rd CI tripwire) · **ECS↔Loro commit pipeline + engine-side undo/redo + merge-validation (M1–2)** · **pipeline hardening (M1.6)**: precise additive-undo (`Op::RemoveField` — no more sibling-field destruction), atomic pre-validated commit (all-or-nothing), O(1) `tid→eid`, Loro-error propagation in `apply_*`. **flecs_ecs M1 go/no-go: GO** — undo p99 0.24–0.72 ms (≫ under the 5 ms target), resurrection robust, two-fork merge converges, all 8 invalid-state classes detected+repaired, 49 tests green. Detail → `progress/M1.md`.

---

## Log

Detailed dated entries are sharded by milestone under `progress/` (keeps this dashboard thin).
Append to the **current milestone's** file, newest first, one entry per session, with measured
numbers + ADR links. Live state stays here in Now/Next above.

- [progress/M2.md](progress/M2.md) — **current milestone** (M2 — risk-first exit gates + build)
- [progress/M1.md](progress/M1.md) — foundation build (M1.1–M1.6)
- [progress/M0.md](progress/M0.md) — foundation, 3 spikes, gate review (2026-06-12 → 06-13)
