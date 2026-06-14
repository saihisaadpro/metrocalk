# Progress

## Now
- **M2 build underway.** Transport wire is **real (M2.4)**: deltas-only `DeltaTransport` + Loro-Protocol-v1 framing + Loro-`update` payload; shared session (coalesce/ACK/outbox-collapse backpressure/fragments); 3 impls (Tauri Channel default · WebSocket · in-proc-WASM); `/core` producer hook + PeerID handshake. `/transport` links no Loro (CI grep green). See Done. (M1 complete + hardened.)

## Next (milestone M2)
- **M2.5 editor UI** (consumes the M2.4 transport contract; owns `/editor`) → then **M2.6** convergence (shell + viewport + transport; needs M2.3 ADR-008 + M2.4 + M2.5 + M2.2 render verdict). Parallel lanes in flight: M2.1 (Tauri gate, `m2.1`), M2.2 (render, `m2.2-render-gate`), M2.3 (shell-composite).
- **Deferred to M2.6 (from M2.4):** end-to-end Tauri **Channel** latency re-confirm (~3.4 ms over WebView2) + the **Windows initial-snapshot-load** cliff — both need the real shell (on the unmerged `m2.1` branch).
- **M2 risk-gates underway.** Render gate **M2.2 PASSED** (2026-06-14): the M1.4 stress scene (5k+20k) renders well inside budget native + browser via instancing + GPU frustum culling (compute→indirect) + render bundle — see Done. M1 remains complete + hardened (M1.6 closed four audit gaps).

## Next (milestone M2)
- **M2.1 Tauri WebView2 IPC gate** (the remaining M2 exit gate — 60 Hz drag, worst-case delta payload on Windows; fallback CEF). Once M2.1 + M2.2 both pass → author the **M2 build** (real shell + transport 3 impls + binary delta protocol) from the gate evidence.
- **M2 build underway.** Shell composition resolved (**M2.3**): **single-window** transparent WebView2 over native wgpu passes on dGPU+iGPU — M2.1 1b "FAIL" was a GDI capture artifact (ADR-008); no DComp/CEF. See Done. (M1 complete + hardened.)

## Next (milestone M2)
- **M2.6 convergence**: build the real single-window shell, wire the `shell-input-routing` layer + M2.4 transport, integrate the M2.2 render verdict; run the deferred DPI-100↔200/min-spec cases there.
- **M2 build underway.** Editor UI scaffolded (**M2.5**): `/editor` is a projection of the core (invariant 1) — Zustand/`useSyncExternalStore` store, JSON Forms inspector, React Flow graph, optimistic echo with rejection-as-UX; selective re-render verified at 5k (0 of 5000 rows on a field edit), 11 tests green. ADR-010. See Done. (M1 complete + hardened.)

## Next (milestone M2)
- **M2.6 convergence**: mount the M2.5 editor in the M2.3 single-window shell; point the desktop transport binding at the real Tauri Channel (M2.4) + swap `MockCore` → WASM/Rust core; wire the viewport input hand-off; integrate the M2.2 render verdict; run the deferred DPI/min-spec cases.
- **Carry-forward (later):** getrandom `js` for Loro-in-browser + the Phase-2 pure-Rust query backend (ADR-006).
- **M2 underway — M2.1 Tauri exit-gate RESOLVED (2026-06-14).** Risk-first M2 entry ([ADR-007](decisions/007-m2.1-tauri-gate-result.md), `spikes/tauri-shell`). **IPC PASS:** real 103-byte Loro deltas at 60 Hz over WebView2 — Channel p99 3.4–3.6 ms / WebSocket p99 1.3–1.7 ms RTT, 0 dropped (overhead-bound, *not* the "~200 ms / 10 MB" bandwidth case ADR-003 feared). **Compositing FAIL:** transparent WebView2 works on its own, but a native wgpu surface on the same window HWND blacks/collapses it — Graphite's problem, reproduced → plan the shell around **self-composite** (UI-as-texture in wgpu), child-webview/DComp follow-up before any CEF pivot. *(Next M2 gate: M2.2 render cost — prompt 13.)*
- **M1 complete + hardened.** All deliverables verified; M1.6 pipeline-hardening pass closed four audit gaps (precise additive-undo, atomic pre-validated commit, O(1) `tid→eid`, Loro-error propagation) before M2 builds on the pipeline — see Done below.

## Next (milestone M2)
- **M2.2 render gate** (prompt 13): real-scene wgpu cost @ ≥5k entities (native + wasm). **M2 build** (real shell + transport impls): author from the M2.1/M2.2 evidence — shell is **self-composite** per ADR-007; transport is deltas-only over Channel/WebSocket (proven on Windows).
- **Carry-forward (later):** getrandom `js` for Loro-in-browser + the Phase-2 pure-Rust query backend (ADR-006); the Tauri child-webview/DComp compositing follow-up (ADR-007).
- **Carry-forward (Phase 2, with collab):** `merge()` rebuilds entities from Loro but **not their ECS tags/pairs** — capabilities are ECS-only, never written to Loro, so the **compatibility query is empty after a merge**. Fix wires the registry (component-kind → capabilities) into `rebuild_ecs_from_loro`; schedule with collab. (Surfaced by M1.6 audit; see `progress/M1.md`.)

## Done (milestone-level)
- **M2.4 transport protocol (2026-06-14):** the deltas-only wire. `DeltaTransport` byte-trait + **Loro-Protocol-v1 framing** (`%LOR`/`%ACK`/`%HSK`/`%FRG`/`%EPH`-reserved/ping) carrying opaque Loro-`update` payloads; shared `DeltaSession` does coalescing (version-vector diff), batch-id/ACK, **outbox-collapse backpressure** (one update spans the gap, not N), fragmentation/reassembly, and a reconciliation hook (Wait-for-Ack; upgrade-ready, separate from user-undo). 3 impls (Tauri Channel default · WebSocket · in-proc-WASM); `/core` producer hook + PeerID handshake. **15 tests green** (10 transport + 5 core: out-of-order convergence, idempotent re-import, reconnect-resync, echo no-op, backpressure-collapse, fragments). Envelope encode+decode p99 ≤1.3 µs. `/transport` links no Loro (CI grep green). ADR-009. Channel e2e + Windows snapshot-load **deferred to M2.6** (need the real shell). Detail → `progress/M2.md`.
- **M2.2 render gate PASSED (2026-06-14):** new throwaway `spikes/render-scene` renders the M1.4 stress scene (5k + 20k — instanced cubes + per-entity gizmos + grid) through one wgpu 29.0.3 crate, native + browser, via **instancing + GPU frustum culling (compute → compacted `visible[]` → indirect draws, no multi-draw-indirect / `first_instance=0`) + a grid render bundle**, GPU-time and CPU-submit measured separately. **Native** (Vulkan/RTX 4060): GPU p99 **0.60 ms** @5k / **0.88–0.95 ms** @20k (budgets 8.3 / 16.6 ms → ~14×/17× headroom, no spikes). **Browser** (Chrome 149 + Edge 149/Dawn): GPU p99 **1.34 ms** @5k / **3.26 ms** @20k (clears 60/30 fps bars). **Draw calls constant at 3** as entities go 4× (GPU time only 1.4× ⇒ instancing engages); largest storage buffer **0.61 MB ≪ 128 MB**. Verdict: render approach holds. **Gap:** Firefox 141 not run (not installed) — low risk. ADR-003 status-updated. Detail → `progress/M2.md`, numbers → `spikes/render-scene/RESULTS.md`.
- **M2.3 shell-composition gate (2026-06-14):** `spikes/shell-composite` proved **single-window** (transparent WebView2 over native wgpu on one HWND) composites correctly on Windows on **both** the RTX 4060 dGPU and the Intel Iris Xe iGPU — real panel layout, under motion/resize/overlapping-input. M2.1's 1b "FAIL" was a **GDI capture artifact** (GDI can't see a flip-model swapchain; DXGI Desktop Duplication shows a clean composite; the 16×16 collapse never recurred). No DComp/CEF (~170 MB avoided); fallback ladder not exercised. Path-agnostic input-routing layer built first (7 tests). ADR-008. Detail → `progress/M2.md`. (ADR-007 status-note deferred to the m2.1↔m2.3 merge.)
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
<<<<<<< HEAD
- [progress/M2.md](progress/M2.md) — **current milestone** (M2 risk-gates → build)
>>>>>>> m2.3
=======
- [progress/M2.md](progress/M2.md) — **current milestone** (M2 — risk-first exit gates + build)
- [progress/M1.md](progress/M1.md) — foundation build (M1.1–M1.6)
- [progress/M0.md](progress/M0.md) — foundation, 3 spikes, gate review (2026-06-12 → 06-13)
