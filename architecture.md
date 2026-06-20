# Architecture — Current State

> Rules for this file: current state only, max ~2 pages. No rationale — link the ADR instead. Prune on every change. Status: **M2 — complete (code + all measurable gates); north-star #1 real.** `/editor-shell` runs live (ADR-008 composite, real `/core`, 5k); north-star test #1 works in the window (ranked reveal · every-no-explained · one-click undoable bind — 4/5 boxes human-confirmed) and **survives reload** (deterministic-seed + replay-log, box 5). Camera orbit/zoom + the viewport hot path are zero-per-frame-IPC (inv. 4, instrumented). Measured @5k (release, i9-13900H / Iris Xe): render-submit p50 0.74 ms · reveal p99 1.5 ms · commit p99 ~1.5 ms (all ≪16 ms); `project_full` on connect/undo ~70 ms and snapshot-merge-load ~350 ms are heavy one-shot ops (not per-frame). **Handed off (human/hardware/driver, not blocking):** the dogfood verdict; live close→reopen + drag-feel confirmation; DPI 100↔200, min-spec, ≥60 s flicker; Channel-e2e re-confirm (~3.4 ms, M2.1); real-browser store-apply. **M3 north-star loops live (#1 click-to-bind, #2 describe-to-create); M4 local asset tier: entities render as imported glTF meshes — asset-by-handle, non-bindless instanced draw, wasm32-portable import ([014](decisions/014-asset-model-and-import-pipeline.md)). M5 marketplace gate: capability identity is namespaced (std-vocab + `(AliasOf, std:*)`, cross-author safe) and describe-to-create resolves `local→marketplace` for real ([015](decisions/015-capability-identity-and-marketplace-index.md)); generation + token economy stay seamed.**

## System shape

```
┌─────────────────────────────── Editor UI (React + TS) ───────────────────────────────┐
│  panels · schema-driven inspector · React Flow binding graph · optimistic local echo │
└──────────────────────────────────────┬───────────────────────────────────────────────┘
              DeltaTransport (deltas only · Loro-Protocol-v1 framing, M2.4)
            ┌─────────────────────────┼─────────────────────────┐
      in-process WASM call      Tauri Channel (default)      WebSocket
        (browser build)         (desktop build)           (collab/remote)
            └─────────────────────────┼─────────────────────────┘
┌──────────────────────────────────────▼───────────────────────────────────────────────┐
│                                 Rust Core                                            │
│  Semantic ECS (Flecs) ←→ commit pipeline ←→ Loro document (undo · history · collab)  │
│  component metadata registry (JSON Schema) · intent ranking · merge validation       │
│  wgpu renderer (viewport, gizmos — all hot interactions stay on this side)           │
│  Extism plugin host · MCP server surface (Phase 2+)                                  │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

> **The query layer is backend-split** ([ADR-006](decisions/006-browser-query-backend.md)): the diagram's *Semantic ECS (Flecs)* is the **native** backend; the **browser** build runs a pure-Rust index over the Loro projection behind the *same* query-API trait (Flecs doesn't compile to wasm32). Invariant 1 holds per-target — ECS authoritative on native, Loro-projection authoritative in the browser.

## Layers and choices

| Layer | Choice | ADR |
|---|---|---|
| Semantic ECS | Flecs v4.1 via `flecs_ecs`, behind our own query API | [001](decisions/001-flecs-over-bevy-ecs.md) |
| Document / undo / collab / persistence | Loro 1.x | [002](decisions/002-loro-over-custom-wal.md) |
| Shell + UI | Tauri 2 + React/TS; viewport in Rust/wgpu. **Composition: single-window** — transparent WebView2 over the native wgpu surface on one HWND (no DComp / no CEF; M2.1 1b "FAIL" was a GDI capture artifact, disproven on dGPU+iGPU). Per-pixel input routing splits UI vs viewport. | [003](decisions/003-desktop-first-tauri-exit-gate.md) · [008](decisions/008-shell-composition.md) |
| Editor shell | **live (M2.6, `/editor-shell`)**: Tauri 2 — transparent WebView2 editor over a native wgpu viewport on one HWND, OS-composited (ADR-008). The `!Send` Flecs `Engine` runs on a dedicated thread; editor `EditTx`→`invoke`→commit, `ProjectionDelta`→Tauri `Channel` (desktop binding of the M2.4 wire). Viewport = M2.2 instanced render of `/core` Transforms; camera + ray-pick in Rust (inv. 4). | [008](decisions/008-shell-composition.md) |
| Editor UI | **real scaffold (M2.5)**: a projection of the core (invariant 1) — Zustand/`useSyncExternalStore` store (entity-keyed, immutable per-entity, summary projection) · JSON Forms inspector (over RJSF) · React Flow neighborhood graph · optimistic echo + rejection-as-UX · JSON-Patch edit language. Viewport hot-input stays native (invariant 4). | [010](decisions/010-editor-projection-architecture.md) |
| Binding by intent | **live + reload-persistent (M3.1, `editor-shell/src/{reveal,capscene,persist}.rs` + `src-tauri`)**: on select, run the M1.5 compat query on the engine's world → rank compatible targets (proximity · affinity · recency, deterministic offline — no LLM) + explain every "no" (`why_not`, O(1)/target); click a candidate → one-transaction bind (Loro edge + ECS `BindsTo` pair, undoable). Surfaced in the running shell ("Bind by intent" + "Requirers" panels; `reveal_targets`/`bind_target`). North-star test #1 confirmed live (4/5 boxes); live per-click reveal **p99 1.523 ms @5k**. **M3.3:** a second, direct route — right-click an entity → context menu → `Bind…` (the action model gates it on an unbound required cap), still ≤2 interactions / every-"no"-explained. | [011](decisions/011-intent-ranking.md) · [016](decisions/016-viewport-action-model-and-interaction.md) |
| Describe-to-create | **live local tier (M3.2, `core/src/resolve.rs` + `editor-shell/src/capscene.rs`)**: free text → `resolve_local` (token-overlap over stdlib name/aliases/tags/caps + synonyms; offline, deterministic, pure metadata → wasm-portable) → instantiate a pre-componentized working object (component + capability pairs, one undoable commit, replay-persisted) → M3.1 reveal offers one-click attach (≤2 interactions). Tiered **local→marketplace→generate**; marketplace/generate are stubs (honest no-match → seam, never on the happy path). Resolve p99 ~85 µs. North-star test #2 buildable boxes live (E2E 9/9). **M4:** a resolved kind with a catalog asset instantiates carrying its mesh **handle** → renders as that mesh, not a cube (no-asset kinds keep the honest cube). **M5:** `local → marketplace` is real — a no-local-match resolves the marketplace index → a pre-componentized entry (namespaced caps + mesh) applies already wired; generate stays the seam. | [012](decisions/012-describe-to-create-resolver.md) · [014](decisions/014-asset-model-and-import-pipeline.md) · [015](decisions/015-capability-identity-and-marketplace-index.md) |
| Persistence | **live (M2, `persist.rs`)**: deterministic re-seed (fixed seed → identical `EntityId`s) + replay an append-only `EditTx`/bind/undo/**describe** log on launch, then `clear_history` (restored scene non-undoable). Deliberately **not** Loro merge-on-start (merge drops the ECS capability pairs the reveal needs). A bind / described entity survives close→reopen (test-1 box 5, test-2 reload). | [002](decisions/002-loro-over-custom-wal.md) · [013](decisions/013-live-persistence-replay-log.md) |
| Viewport input | **native, zero-per-frame-IPC (M2, inv. 4)**: left-click ray-pick (normalized cursor, DPI-safe); right-drag orbit + wheel zoom update in the render loop by polling the OS cursor — only `drag_start`/`drag_end` cross JS (2×/gesture), never per frame. `render::IPC_CALLS` counter reports ipc/frame for proof. **M3.3 context reveal:** right-**click** (a right-press that moved <6 px — disambiguated from orbit) opens a context menu populated by the registry-driven **action model** (`actions_for`: valid actions + every-"no"-explained; p99 ~3 µs @5k) — Remove/Duplicate are one undoable pipeline tx each (Remove frees dependents + cleans edges; M1.6 resurrection undo; replay-persisted), Focus sets the camera `cam_target`. **Hover** uses a non-mutating `viewport_peek` + `entity_details`, fetched **on hovered-entity change only** (debounced) → 0 per-frame IPC. | [008](decisions/008-shell-composition.md) · [016](decisions/016-viewport-action-model-and-interaction.md) |
| Rendering | **real (M2.2 + M4)**: wgpu 29 + WGSL, **non-bindless** (web-required: WebGPU exposes no binding-array features). The live viewport draws imported **meshes** — per-asset vertex/index buffers **instanced** per entity (`vs_mesh`) — over the M2.2 instanced-cube placeholder/fallback + grid + tracking lines; all built/uploaded on the render thread (inv. 4). Frame budget holds on a 5k-cube + 200-instanced-mesh scene: CPU+GPU p99 ~0.4 ms ≪ 16 ms (RTX 4060). | [003](decisions/003-desktop-first-tauri-exit-gate.md) · [014](decisions/014-asset-model-and-import-pipeline.md) |
| Browser target | **CI-enforced**: `wasm32-unknown-unknown` builds on every push (`.github/workflows/wasm-tripwire.yml`); native+browser render proven from one wgpu crate (`spikes/wasm`) | [003](decisions/003-desktop-first-tauri-exit-gate.md) |
| Query backend | Native: Flecs (behind the wrapper). Browser: pure-Rust index over the Loro projection — Flecs is native-only (won't compile to wasm32) | [006](decisions/006-browser-query-backend.md) |
| Transport / wire | **real (M2.4)**: Loro-Protocol-v1 framing + opaque Loro-`update` payload behind the byte-only `DeltaTransport` trait; shared session does coalescing/ACK/backpressure/fragments; impls = Tauri Channel (default) · WebSocket · in-proc-WASM. `/transport` links no Loro | [009](decisions/009-transport-protocol-loro-framing.md) |
| Plugins / scripting | Extism WASM plugins | plan §2 |
| AI layer | MCP server + JSON-Schema-constrained JSON Patch | plan §2 |
| Scene format | Own format; BSN-compatible where cheap; BRP interop | plan §2 |
| Logic layer | Rules (When/If/Then) + state machines as data, registry-fed builder; code behavior via WASM plugins (post-slice) | plan §2 |
| Asset pipeline | **real local tier (M4, `/assets`)**: glTF/glb (+PNG texture) import behind the project-owned `MeshSource` trait (no `gltf::`/`image::` past it — CI-gated, like Flecs-in-`/ecs`) → internal `MeshAsset` → **content-addressed store beside the doc**; an entity carries only the asset **handle** in `MeshRenderer.mesh` (inv. 1/2). `wasm32`-portable (no core/Loro/C-FFI — CI tripwire builds it). KTX2 (basis-universal = C++ FFI → native-only) · LODs · colliders · rig detection **deferred** (physics/runtime tier). | [014](decisions/014-asset-model-and-import-pipeline.md) · plan §1.5 |
| Asset generation + marketplace | **index tier real (M5, `core/src/marketplace.rs`)**: a `MarketplaceIndex` trait (a remote impl slots in unchanged) with a checked-in `LocalCatalog` of **pre-componentized** entries (namespaced caps + a prompt-23 asset handle + an inert price); the resolver's `resolve` is now tiered for real — local short-circuits offline, marketplace is the **2nd** tier, generate the seam. Choosing an entry applies a working object (caps + mesh) as one undoable, replay-persisted tx with the M3.1 attach. **Token economy + payout + text-to-3D generation stay seamed** — no money moves, nothing generated. | [015](decisions/015-capability-identity-and-marketplace-index.md) · [004](decisions/004-free-engine-token-economy.md) |
| Physics / Audio / Netcode | Picks **revised, pending spikes** → Rapier (physics) · Firewheel (audio, was kira) · tiered Loro/renet2/GGRS (netcode, was lightyear — Bevy-coupled). Determinism = enabling substrate. | [physics-audio-networking-plan.md](physics-audio-networking-plan.md) |

## Invariants (non-negotiable)

1. One source of truth: ECS authoritative, Loro is its durable mergeable mirror, UI holds projections.
2. Deltas only across every boundary; never full-state snapshots.
3. Everything is a transaction through one commit pipeline (human, plugin, AI). Merge validation re-checks ECS invariants after every CRDT merge.
4. Hot path never crosses the JS boundary.
5. Every pre-1.0 dependency lives behind our own trait.

## Repository

Cargo workspace at root (`Cargo.toml`); members `core` + `ecs` + `transport` + `plugins` + `tools/*` (measurement crates).

```
/ecs         Rust lib — the `World` query trait + native Flecs backend; the ONE crate with
             flecs_ecs + unsafe (ADR-001/006). M1.2 real.                   (workspace member)
/core        Rust lib — component metadata registry (real, M1.3); commit pipeline (atomic /
             pre-validated, M1.6) + engine-side undo/redo + merge-validation (real, M1–2);
             describe-to-create resolver + capability namespacing + marketplace index (M3.2/M5,
             pure metadata → wasm-portable); depends on /ecs                  (workspace member)
/transport   Rust lib — deltas-only `DeltaTransport` + Loro-Protocol-v1 framing + shared session
             (coalesce/ACK/backpressure/fragments); 3 impls (Channel/WS/in-proc). Links no Loro.
             Real (M2.4, ADR-009)                                            (workspace member)
/assets      Rust lib — the local asset substrate (M4, ADR-014): trait-wrapped glTF/glb import →
             internal mesh → content-addressed store → GPU vertex packing. No core/ecs/Loro/C-FFI,
             so it compiles to wasm32 (ADR-006); `gltf::`/`image::` stay behind the importer.
                                                                             (workspace member)
/plugins     Rust lib — Extism host + MCP seam (Phase 2+, stub)             (workspace member)
/tools       Rust bins — measurement only: scene-bench (F1 memory), query-gate (the <16 ms
             compat-query CI perf gate, M1.5). Default lints, not production. (workspace members)
/editor      React/TS UI — NOT a cargo member (scaffolded M2–3)
/spikes      M0 throwaway spikes (loro, flecs, wasm) — excluded from the workspace; build standalone
```

The `World` trait is the backend-agnostic relational-query surface (pair-match / wildcard / negation
/ read-target); native = Flecs, browser (Phase 2) = pure-Rust over Loro, behind the **same** trait
(ADR-006). Shared lints in `[workspace.lints]`: `clippy::pedantic` (tuned) + `unsafe_code = "forbid"`;
`/ecs` is the documented exception (own lints: `deny` unsafe + pedantic). CI — three gates: `ci.yml`
(fmt + clippy `-D warnings` + test + greps forbidding `flecs_ecs` outside `/ecs`, `loro` outside
`/core`, AND `gltf::`/`image::` outside `assets/src/gltf_import.rs`), `wasm-tripwire.yml` (wasm32 build
of `spikes/wasm` **and** `metrocalk-assets`; never `ecs`/`core`/Flecs, per ADR-006), and
`perf-gate.yml` (M1.5: **fails the build if the cached compat query's p99 > 16 ms** on M1.4's 5k
preset through the wrapper — north-star test #1; ~776× runner headroom; calibration in
`tools/query-gate/README.md`).

**Real-`.exe` E2E** (`editor-shell/e2e/`, WebdriverIO + `tauri-driver`, run locally — not in CI as it
needs the GUI/WebView2 + a matched `msedgedriver`): drives the packaged app's WebView2 DOM, including
the transparent viewport `<div>` whose clicks fire the native pick, so **north-star test #1's full live
round-trip is machine-verified** (launch→connect→reveal→bind→undo→viewport-pick→edit; 7/7). Setup +
the run-via-`node` gotcha (the repo path's ` & ` breaks npm's shim) in `editor-shell/e2e/README.md`.

## Open questions (gated, not debated)

Resolved at the M0 gate review (2026-06-13) — kept here struck-through for traceability:

- ~~`flecs_ecs` binding viability~~ → **ADOPT, confirmed** ([ADR-001](decisions/001-flecs-over-bevy-ecs.md), `spikes/flecs`): 12–58 µs p99 (≪16 ms), safety locks ON, zero stale. **M1 integration go/no-go: GO.** The commit pipeline runs through the wrapper with latest-op undo p99 0.24–0.30 ms and entity-resurrection undo p99 0.72 ms — both ≫ under the 5 ms budget (n=500, under parallel test load; M1.6). Two-fork merge converges; all 8 invalid-state classes detected+repaired. 49 tests green. No `flecs_ecs` type leaks past `/ecs`.
- ~~Loro history size / merge semantics at scale~~ → **ADOPT, confirmed** ([ADR-002](decisions/002-loro-over-custom-wal.md), `spikes/loro`).
- ~~Browser ECS path~~ → **resolved** ([ADR-006](decisions/006-browser-query-backend.md)): browser runs a pure-Rust query backend over the Loro projection; Flecs is native-only. `loro`+`wgpu` reach wasm; `flecs_ecs` does not.

Genuinely open (gated, not debated):

- ~~Tauri IPC on Windows WebView2~~ → **RESOLVED (M2.1, [ADR-007](decisions/007-m2.1-tauri-gate-result.md))**: real 103-byte deltas at 60 Hz over WebView2 — Channel p99 3.4–3.6 ms / WS 1.3–1.7 ms RTT, 0 dropped, overhead- not bandwidth-bound. The M2.1 single-window compositing "FAIL" was a GDI capture artifact — **disproven (M2.3/M2.6, [ADR-008](decisions/008-shell-composition.md))**: the transparent WebView2 composites over the native wgpu surface on one HWND, confirmed on dGPU+iGPU and live in `/editor-shell`. (The earlier self-composite/DComp/CEF fallbacks are retired.)
- ~~Real-scene render cost at ≥5k entities~~ → **resolved — render gate PASSED (M2.2, [ADR-003](decisions/003-desktop-first-tauri-exit-gate.md) status)** and measured live in the integrated editor (M2): CPU-submit p50 0.74 ms @5k · reveal p99 1.5 ms · commit p99 ~1.5 ms (all ≪16 ms; i9-13900H / Iris Xe iGPU). One-shot heavy ops (not per-frame): `project_full` on connect/undo ~70 ms (incremental-undo-delta is the follow-up); snapshot-merge-load ~350 ms (which is why persistence uses seed+replay, not merge). Browser GPU p99 1.34 ms @5k (Chrome/Edge); Firefox not run — low risk.
- ~~Capability identity / namespacing~~ → **RESOLVED (M5, [ADR-015](decisions/015-capability-identity-and-marketplace-index.md))**: caps intern by **canonical namespaced** key — bare stdlib names are the `std:` standard vocabulary, author caps are namespaced (`acme:Health`), distinct by construction; a custom cap opts into the standard web via a one-directional `(AliasOf, std:Cap)` pair resolved into the provides pairs at apply. Cross-author collision impossible; reveal/bind works across namespaces; no bare-string identity in marketplace-facing APIs. (Live in `core/src/caps.rs` + `editor-shell/src/capscene.rs`.)

## Future directions (deferred — do not gate V1)

- **Scientific-grade kernel** — a validated, `f64`, deterministic solver behind a capability, reusing the existing transaction log + intent front end. **This is seam-preservation, not a feature to build now:** keep the solver/simulation layer behind its trait with no `f32`-only or nondeterministic assumptions baked into `/core`, so the option stays open. Revisit as a real ADR at the physics milestone.
