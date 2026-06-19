# ADR-014: Asset model + import pipeline — asset-by-handle · trait-wrapped importer · store-beside-doc · non-bindless mesh render

**Date:** 2026-06-19 · **Status:** Accepted — M4 (Phase-2 asset gate, **local import + render tier**;
`assets/`, `editor-shell`) · **Builds on:** [ADR-003](003-desktop-first-tauri-exit-gate.md) (wgpu,
non-bindless web path), [ADR-006](006-browser-query-backend.md) (wasm32 parity discipline),
[ADR-012](012-describe-to-create-resolver.md) (a resolved kind can carry a mesh),
[ADR-013](013-live-persistence-replay-log.md) (deterministic-id reload). · **Defers to later:** the
marketplace index + text-to-3D generation + the token economy (prompt 24+, [ADR-004](004-free-engine-token-economy.md)).

## Context

Until M4 every entity — HealthBars included — rendered as one M2.2 stress-scene cube; there was no real
art. That proved the intent engine but left the describe-to-create promise (a working, *visible* object)
and any fair dogfood/external verdict short: a described/marketplace object must be able to *look* like
itself. This is the local asset substrate the marketplace + generation tiers stand on. There is no art
library and no asset store yet; this milestone builds the **mechanism**, with a small set of own demo
meshes, and stays inside the five invariants. Collider generation, LODs, and rig detection (plan §1.5)
are physics/runtime-tier and deferred.

## Decision

1. **Asset by handle (invariants 1 & 2).** An entity references geometry only by a lightweight
   **handle** string carried in the stdlib `MeshRenderer.mesh` field (format `"asset"`) — the ECS and
   the Loro doc/projection deltas carry the handle, never geometry. The ECS stays authoritative over
   *which* asset an entity uses.
2. **Trait-wrapped importer (invariant 5).** A project-owned `MeshSource` trait (`assets/src/source.rs`)
   speaks only the internal `MeshAsset` + `ImportError`. The glTF/glb backend (`gltf` + `image`) lives
   **only** in `assets/src/gltf_import.rs` — no foreign decoder type crosses the public surface, enforced
   by a CI grep-gate (mirroring `flecs_ecs`-in-`/ecs`, `loro`-in-`/core`).
3. **Internal mesh model.** `MeshAsset` = positions/normals/uvs/indices + materials (PBR base-color
   factor + optional decoded RGBA8 texture refs) + bounds. Pure data, no foreign types.
4. **Content-addressed store, beside the doc.** `AssetStore` is id-keyed by a content hash (FNV-1a 128,
   zero-dep) and lives next to the engine, **not** inside the Loro doc. Identical bytes → identical
   handle (dedup + the same handle re-resolves after reload, ADR-013 determinism).
5. **Native render: per-asset, instanced, non-bindless (invariant 4 + ADR-003).** `MeshGpu` packs an
   asset to one interleaved vertex buffer + index buffer (`wasm32`-portable, `bytemuck`). The viewport
   binds one vertex/index buffer **per asset** and draws it instanced across the entities using it
   (non-bindless — web-required); the M2.2 instanced-cube path stays as the placeholder/fallback **and**
   the perf baseline. All built + uploaded on the render thread — the hot path never crosses JS.
6. **`wasm32` by construction (ADR-006, deliverable 6).** `assets/` depends on neither `/core` (Flecs)
   nor Loro nor any C FFI, so import + mesh-data prep compile to `wasm32-unknown-unknown` (CI tripwire).
   **KTX2/basis-universal GPU-texture compression is native-only** — `basis-universal` is a C++ FFI
   wrapper that does not build for `wasm32` (the same class of incompatibility as Flecs). It is therefore
   a deferred native-side normalization seam, **not** a dependency; that is the documented native/browser
   divergence. Texture *rendering* this tier bakes the material base-color into vertex color; in-shader
   sampling is the next render increment (the importer already decodes textures, tested).
7. **Describe-to-create carries the mesh (ADR-012).** A shell-owned `MeshCatalog` (kind → handle) lets a
   resolved kind with an associated asset instantiate carrying its mesh handle (visible, pre-componentized
   object); a kind with no asset honestly falls back to the cube. One undoable transaction; a new
   `Record::PlaceMesh` + the catalog on replay make a described/placed *visible* object survive reload.

## Consequences

- **Doc/delta size stays tiny** — only the handle crosses any boundary; a malformed/oversized glTF is
  rejected at import (64 MiB / 8M-element caps, fail-fast on declared accessor counts) and never reaches
  the doc, so it cannot blow up delta size or break reload (the adversarial case).
- **One mechanism, two targets** — the same `MeshGpu`/WGSL the native viewport uses is the web-proven
  primitive set (indexed vertex buffer + instanced draw, no bindless); the importer reaches the browser.
- **Honest boundaries** — KTX2 transcode, texture sampling, collider/LOD/rig generation, base64/external
  `.gltf` buffers, and a UI import affordance are deferred and named, not stubbed on the happy path.
- **Measured (release, i9-13900H / RTX 4060):** import one-shot ~21µs (healthbar) / ~10µs (prop); a
  5000-cube + 200-instanced-mesh scene renders CPU+GPU p99 ~0.40–0.52 ms ≪ 16 ms (≈30–40× headroom).

## Revisit when

The marketplace tier lands (pre-componentized assets from an index → the same handle/store path), or
texture-heavy assets justify in-shader sampling + the native KTX2/basis transcode step, or the
physics milestone brings collider/LOD/rig generation. Capability-namespacing is decided at the same
marketplace gate (architecture open-questions).
