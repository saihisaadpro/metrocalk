# Progress

## Now
- **M2 complete; north-star #1 real + MACHINE-VERIFIED live; every prior milestone audited & re-verified green.** A full verification pass (2026-06-16) ran the whole test matrix — **~88 workspace tests · clippy `-D warnings` · fmt · wasm32 build · src-tauri release build · E2E 7/7** — and a systematic **per-prompt deliverable audit (16 milestones)**: **all met, 0 blockers, 0 majors.** The only outstanding items are documented human/hardware/Phase-2 deferrals (dogfood verdict · ≥60 s flicker · DPI · min-spec · Firefox WebGPU · Channel-e2e live re-confirm · real-browser store-apply).
- **The differentiator, live + verified.** A real-`.exe` E2E harness (WebdriverIO + tauri-driver, `editor-shell/e2e/`) drives the packaged app's WebView2 — incl. the transparent viewport `<div>` → native pick — so **north-star test #1 passes 7/7 live**: launch + connect (5000) · requirer → ranked reveal · one-click bind → tracking · single-step undo · **viewport pick** · field edit. On `master`/`origin/main`: the live `/editor-shell` (Tauri 2, ADR-008 composite over the real `/core`, no MockCore) — click a HealthBar → ranked-by-(proximity·affinity·**recency**, all live) compatible + every-no-explained → one-click undoable bind; **survives reload** (box 5: deterministic-seed + replay-log); camera **orbit/zoom + viewport hot path zero-per-frame-IPC** (inv. 4). The viewport pick was found+fixed via the harness (frame-cadence race → computed synchronously in the command).
- **Measured @5k (release, i9-13900H / Iris Xe iGPU):** render-submit p50 0.74 ms · reveal p99 1.523 ms · commit p99 ~1.5 ms — all ≪16 ms (interactive budget holds). One-shot heavy ops (not per-frame): `project_full` on connect/undo ~70 ms; snapshot-merge-load ~350 ms. Residuals: snapshot-load **measured**; Channel-e2e ~3.4 ms (M2.1, cited); min-spec partial-signal (the budget holds on the integrated Iris Xe).
- **M3.2 — north-star #2 (describe-to-create) real locally + verified.** Type a description → `core/src/resolve.rs` (ADR-012) resolves it **offline** over the curated stdlib (p99 ~85 µs) → `editor-shell` drops in a **pre-componentized working object** (real capability pairs, one undoable commit) → the M3.1 reveal offers a **one-click attach** (≤2 interactions) → it **survives reload** (replay-log, ADR-013). Marketplace + generate are honest seams (never on the happy path). Proven **headless** (`north_star_2.rs`) + **live** (E2E **9/9**: describe→create+attach; no-match→seam). Both signature loops — click-to-bind (#1) and describe-to-create (#2) — now work in the window, machine-verified.
- **Live reload now *surfaces* restored state — not just persists it (2026-06-19).** A user report ("binds don't survive close→reopen") was diagnosed by **measurement**: the engine always restored correctly (live exe prints `restored 14 (0 skipped)`; `project_full` carries the edges) — the gap was the UI didn't **show** the restored binds on reload. Fixed at the cause: a **tracking badge + auto-focus** in the panel (so a restored, high-id HealthBar surfaces regardless of list order) and **3D tracking lines** in the wgpu viewport (`render.rs`/`scene.wgsl`), plus **window-position restore** ("reopen where it was left"). Regression-locked headless (`reload_surfacing.rs`, the live `Bind`/`Edit`/`Describe`/`Undo` stream at `SCENE_N=5000`, asserting net state **and** the `project_full` surfacing seam) + live reload E2E **4/4**. ADR-013 unchanged (dated status note appended) — the strategy was always correct.
- **M4 — entities render as real imported meshes (Phase-2 asset tier, local).** A real glTF/glb imports
  through a project-owned trait (`/assets`, no `gltf::`/`image::` leak — CI-gated) → internal mesh →
  **content-addressed store beside the doc**; an entity carries only the asset **handle** in
  `MeshRenderer.mesh` (inv. 1/2) and the live viewport draws it as that mesh — per-asset **instanced**,
  non-bindless (ADR-003), hot path off JS (inv. 4) — over the M2.2 cube placeholder/fallback.
  Describe-to-create now drops a *visible* object (a resolved kind with a catalog asset renders as its
  mesh; no-asset kinds keep the honest cube); place/import is one undoable tx that **survives reload**
  (handle re-resolves, content-addressed). **`wasm32`-portable** import (CI tripwire green). **Measured
  (release, RTX 4060):** import one-shot ~21 µs / ~10 µs; 5k-cube + 200-instanced-mesh scene CPU+GPU p99
  ~0.4 ms ≪ 16 ms. Evidence: `editor-shell/evidence/m4-mesh-scene.png`. ADR-014.
- **Handed off (human/hardware/Phase-2 — instrumented, not fabricated):** the **dogfood verdict** (does it *feel* like the win — both loops); drag-feel; DPI · ≥60 s flicker · min-spec · Firefox WebGPU · Channel-e2e re-confirm · real-browser store-apply; test #2's "pick-up-able / Press Play" (gated on the runtime tier). **M4 deferred (named, not stubbed):** KTX2/basis transcode (C++ FFI → native-only), in-shader texture sampling, collider/LOD/rig generation, base64/external-buffer `.gltf`, a UI import affordance, and the live in-window mesh screenshot. *(Live close→reopen: machine-verified.)* See `progress/M4.md` · `progress/M3.md`.

## Next (Phase-2 gate)
- **Marketplace tier + capability namespacing (prompt 24):** the **local import + render pipeline is done
  (M4, ADR-014)** — next is the **marketplace index** (pre-componentized assets from an index, reusing the
  same handle/store path), then **text-to-3D generation** + the **token economy**. Resolves the
  **capability-namespacing** open question (architecture.md) at the same gate; replaces the resolver's
  token-overlap with a learned/embedding index behind the same `resolve_local` signature.
- **Follow-ups (non-blocking):** incremental undo delta (replace `project_full`-on-undo, the ~70 ms hitch at 5k); the capability-rebuild carry-forward (so a future Loro merge/reload keeps capabilities); log compaction (the append-only replay-log grows with session lifetime). *(Recency ranking is now live — done.)*
- **Carry-forward (later):** getrandom `js` for Loro-in-browser + the Phase-2 pure-Rust query backend (ADR-006).
- **Carry-forward (Phase 2, with collab):** `merge()` rebuilds entities from Loro but **not their ECS tags/pairs** — capabilities are ECS-only, so the **compatibility query is empty after a merge**. Fix wires the registry into `rebuild_ecs_from_loro`; schedule with collab. (M1.6 audit; see `progress/M1.md`.)

## Done (milestone-level)
- **M4 local asset tier (2026-06-19, ADR-014, `/assets` + `editor-shell`):** trait-wrapped glTF/glb
  import → internal mesh → content-addressed store-beside-doc → asset **handle** in the ECS (inv. 1/2);
  the live viewport renders imported **meshes** per-asset instanced, non-bindless, hot path off JS; cube
  placeholder/fallback retained. Describe-to-create drops a *visible* object; place/import is one undoable
  tx that survives reload. `wasm32`-portable import (CI). Import one-shot ~21/10 µs; 5k+200-mesh frame
  CPU+GPU p99 ~0.4 ms. assets 8 + editor-shell 28 green; clippy/fmt clean; new wasm + leak-grep CI gates.
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

- [progress/M4.md](progress/M4.md) — **current milestone** (Phase-2 asset gate: local import + render)
- [progress/M3.md](progress/M3.md) — binding UX / north-star loops (M3.1 bind-by-intent · M3.2 describe-to-create)
- [progress/M2.md](progress/M2.md) — desktop shell convergence (M2 build)
- [progress/M1.md](progress/M1.md) — foundation build (M1.1–M1.6)
- [progress/M0.md](progress/M0.md) — foundation, 3 spikes, gate review (2026-06-12 → 06-13)
