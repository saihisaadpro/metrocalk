# Progress

## Now
- **The editor runs + binding-by-intent is wired live and proven end-to-end (headless).** On `master`/`origin/main`: (1) **M2.6 live editor** — transparent WebView2 over a native wgpu viewport on one HWND, OS-composited (ADR-008), **live on the real `/core` (no MockCore)**; the 5000-entity scene renders (no blackout, Desktop-Duplication black 0.1%), `project_full` round-trips over the real `Channel`+`invoke`, viewport holds budget @5k (CPU-submit p50 0.74 ms). (2) **M3.1 binding-by-intent** (ADR-011) — the reveal→rank→explain engine wired to the real engine + surfaced in the running shell. **Headless north-star test #1 passes through the real engine** (ranked compatible · every-no-explained · one-transaction bind · single-step undo · survives-reload via export→merge). Live "Bind by intent" panel: click an entity → ranked compatible + greyed-with-reason + click-to-bind; Tauri binary builds clean. **Measured (release):** bare reveal p99 1.107 ms; full live per-click `reveal_targets` **p99 1.523 ms @5k** — ≪16 ms. Adversarial review (7 lenses): **0 blockers, 5/7 pass**.
- **Test #1 driven LIVE by the user (2026-06-15) — 4/5 boxes confirmed in the running window:** ranked-by-proximity targets on click ✓, ≤2-interaction bind ✓, every greyed "no" explained ✓, single-step undo ✓ (Ctrl-Z; after the seed-non-undoable fix `446397e`). A "Requirers" quick-list (`6babeeb`) makes HealthBars one click away. The composite + 5000-entity scene + click→pick→inspect are now human-witnessed too.
- **Not yet closed (no fabrication):** (a) **"survives reload" *live*** (the 5th box) — proven headless, but the shell has no persistence layer yet (entangled with the capability-rebuild carry-forward below); (b) the **dogfood verdict** — does it *feel* like the categorical win (the felt judgment, the prize); (c) M2's **zero-IPC-drag** (native input routing) + the **driver residuals** (Channel-e2e/snapshot/browser-store via WebView2-CDP) + **DPI/min-spec/flicker** (human/hardware). **M2 is integrated + all measurable gates green + test-1 mechanics live-confirmed; NOT declared complete** (reload-persistence + dogfood verdict pending). See `progress/M3.md`.

## Next (milestone M2 → M3)
- **The human pass (the prize):** run north-star test #1 in the running shell (click HealthBar → reveal → bind → Ctrl-Z) and record the **dogfood verdict**; do the **sustained ≥60 s flicker** + **min-spec** watches. These are instrumented + handed off, never fabricated.
- **Close the live remainder:** wire shell persistence so "survives reload" holds live (deterministic-seed + replay-binds-log sidesteps the merge limitation; the proper fix is the capability-rebuild carry-forward); wire **native input routing** for the zero-per-frame-IPC viewport drag; measure the residuals needing a **WebView2-CDP driver** (Channel e2e re-confirm, Windows snapshot-load cliff, real-browser store-apply). Then flip status → M2 complete + north-star #1 real.
- **Recency** ranking is plumbed but inert live — small follow-up to feed it the edit log.
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
