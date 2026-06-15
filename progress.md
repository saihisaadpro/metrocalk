# Progress

## Now
- **M2.6 — the editor is real and runs.** `/editor-shell` (Tauri 2, on `master`/`origin/main`): a transparent WebView2 editor over a native wgpu viewport on one HWND, OS-composited (ADR-008), **live on the real `/core` (no MockCore)** — verified: the **5000-entity** scene renders as instanced cubes + grid in the viewport, the editor panel composites over it with **no blackout** (Desktop-Duplication: black 0.1% / 943 colors), the editor shows the real 5000 entities (`project_full` round-trips over the real `Channel`+`invoke`), and the **viewport holds the frame budget @5k: CPU-submit p50 0.74 ms / p99 ≤2.5 ms** (GPU ~0.6 ms per M2.2). Edit/bind/undo + reject-with-reason logic is tested through the real engine (6 tests).
- **M2 go/no-go:** the *measurable* gate is **green** (composite + real-core + 5k frame budget); the *interactive* gate (live click round-trip + Ctrl-Z in the window, zero-IPC-drag) and the **six residuals** (DPI/min-spec/flicker = human/hardware; Channel-e2e/snapshot/browser-store = need a WebView2-CDP driver) are **pending a human/driver/hardware pass**. **M2 is NOT yet declared fully complete** — integrated + render-gate-passed, interactive/residual verification pending (see `progress/M2.md`).

## Next (milestone M2 → M3)
- **Close M2 (interactive + residual pass):** a human or a **WebView2-CDP driver** confirms the live click→inspect→edit/bind→echo round-trip + Ctrl-Z in the window; wire **native input routing** so a viewport drag stays in Rust (zero-per-frame IPC — make the transparent viewport region click-through to the native HWND); measure the residuals — **Channel e2e latency**, **Windows snapshot-load cliff**, **real-browser store-apply** (driver), and **DPI 100↔200 / min-spec / sustained ≥60 s flicker** (human/hardware). Then flip status → M2 complete.
- **M3.1 binding-by-intent** (north-star #1): once M2 closes, author from this evidence.
- **Carry-forward (later):** getrandom `js` for Loro-in-browser + the Phase-2 pure-Rust query backend (ADR-006).
- **Carry-forward (Phase 2, with collab):** `merge()` rebuilds entities from Loro but **not their ECS tags/pairs** — capabilities are ECS-only, so the **compatibility query is empty after a merge**. Fix wires the registry into `rebuild_ecs_from_loro`; schedule with collab. (M1.6 audit; see `progress/M1.md`.)

## Done (milestone-level)
- **M2.1 Tauri exit-gate RESOLVED (2026-06-14, ADR-007, `spikes/tauri-shell`):** IPC **PASS** — real 103-byte Loro deltas at 60 Hz over WebView2, Channel p99 3.4–3.6 ms / WebSocket 1.3–1.7 ms RTT, 0 dropped (overhead-bound, not the bandwidth case ADR-003 feared). Single-window compositing flagged FAIL by automated GDI — **later disproven by M2.3**.
- **M2.2 render gate PASSED (2026-06-14, ADR-003 status, `spikes/render-scene`):** M1.4 stress scene (5k+20k, instanced cubes + per-entity gizmos + grid) via instancing + GPU frustum culling (compute→indirect) + render bundle. Native GPU p99 **0.60 ms** @5k / **0.88–0.95 ms** @20k (~14×/17× headroom); browser (Chrome/Edge) **1.34 ms** @5k / **3.26 ms** @20k. Draw calls constant at 3; resolves the "real-scene render cost" open question. Gap: Firefox 141 not run. Numbers → `spikes/render-scene/RESULTS.md`.
- **M2.3 shell composition PASSED (2026-06-14, ADR-008, `spikes/shell-composite`):** **single-window** transparent WebView2 over native wgpu composites on **dGPU + Intel iGPU** (real panels, motion, resize, input) — M2.1's 1b "FAIL" was a **GDI capture artifact** (Desktop Duplication sees the swapchain; window never collapsed). No DComp / no CEF (~170 MB avoided). Path-agnostic input-routing layer (7 tests). Gap: DPI 100↔200 monitor move + min-spec.
- **M2.4 transport protocol (2026-06-14, ADR-009):** deltas-only `DeltaTransport` + **Loro-Protocol-v1 framing** (`%LOR`/`%ACK`/`%HSK`/`%FRG`/`%EPH`/ping) carrying opaque Loro-`update` payloads; shared session (coalesce/ACK/outbox-collapse backpressure/fragments/reconcile hook); 3 impls (Channel/WS/in-proc); `/core` producer hook + PeerID handshake. 15 tests; envelope p99 ≤1.3 µs. `/transport` links no Loro.
- **M2.5 editor UI scaffold (2026-06-14, ADR-010):** `/editor` as a projection of the core (invariant 1). Zustand/`useSyncExternalStore` store (entity-keyed, summary projection); JSON Forms inspector (custom renderers; edit→JSON-Patch tx); React Flow neighborhood graph; virtualized 5k hierarchy; optimistic echo + rejection-as-UX. 11 tests; selective re-render at 5k (edit one → 0 of 5000 rows).
- Pre-M0: feasibility plan v2 (locked stack), research sweep (~30 sources), doc structure + ADRs 001–005 + Opus 4.8 prompt set.
- **M0 complete (2026-06-13):** 3 spikes (Loro ADOPT · Flecs ADOPT · wasm/WebGPU PROVEN + CI tripwire) + gate review; ADR-006. Detail → `progress/M0.md`.
- **M1 complete (M1.1–M1.6, 2026-06-13):** monorepo + CI · ECS `World` wrapper + Flecs · metadata registry · seeded stress-scene + F1 verdict · 16 ms compat-query CI gate · ECS↔Loro commit pipeline + engine-side undo/redo + merge-validation · M1.6 hardening (precise additive-undo, atomic commit, O(1) `tid→eid`, Loro-error propagation). 49 tests. Detail → `progress/M1.md`.

---

## Log

Detailed dated entries are sharded by milestone under `progress/` (keeps this dashboard thin).
Append to the **current milestone's** file, newest first, one entry per session, with measured
numbers + ADR links. Live state stays here in Now/Next above.

- [progress/M2.md](progress/M2.md) — **current milestone** (M2 build)
- [progress/M1.md](progress/M1.md) — foundation build (M1.1–M1.6)
- [progress/M0.md](progress/M0.md) — foundation, 3 spikes, gate review (2026-06-12 → 06-13)
