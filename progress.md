# Progress

## Now
- **M2 build — lanes M2.1–M2.5 complete and assembled** on branch `m2-integration` (off the M1.6 base; `master` still at M1.5 — see Risks). The integrated workspace **builds + tests green** (one M1.6 latency benchmark flakes under cold-build load, passes in isolation); both CI grep gates (loro-outside-`/core`, flecs-outside-`/ecs`) green. ADRs 001–010 present. **Remaining for M2 close: M2.6** — the real `/editor-shell` that composites the M2.5 UI over the M2.2 viewport, wires the M2.4 transport, and proves the click→inspect→edit/bind→commit→echo round-trip with a measured frame budget (the M2 go/no-go). **Not yet built — M2 is NOT declared complete.**

## Next (milestone M2)
- **M2.6 shell↔viewport integration** (the convergence / M2-close): single-window Tauri shell (ADR-008) compositing the editor over the wgpu viewport rendering M2.2's instanced scene; picking + camera in Rust; bridge `/core`'s Loro-`update` deltas ↔ the editor's `ProjectionDelta` (the one real integration gap — the editor consumes projection deltas, the core emits Loro bytes); edit/bind round-trip + undo; instrument zero-per-frame-IPC drag; measure the end-to-end edit-loop + frame budget; wasm32 parity. Then flip status → M2 complete → M3 (binding UX).
- **Deferred to M2.6 (from M2.4):** end-to-end Tauri **Channel** latency re-confirm (~3.4 ms, now have the real shell) + the **Windows initial-snapshot-load** cliff. **From M2.3:** the DPI 100%↔200% monitor move + a min-spec machine.
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
