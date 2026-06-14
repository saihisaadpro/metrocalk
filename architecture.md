# Architecture — Current State

> Rules for this file: current state only, max ~2 pages. No rationale — link the ADR instead. Prune on every change. Status: **M2 — build** (M0+M1 complete; shell composition resolved — single-window, ADR-008).

## System shape

```
┌─────────────────────────────── Editor UI (React + TS) ───────────────────────────────┐
│  panels · schema-driven inspector · React Flow binding graph · optimistic local echo │
└──────────────────────────────────────┬───────────────────────────────────────────────┘
                          transport trait (deltas only)
            ┌─────────────────────────┼─────────────────────────┐
      in-process WASM call      Tauri channels             WebSocket
        (browser build)        (desktop build)           (collab/remote)
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
| Rendering | wgpu 29 + WGSL (non-bindless path required for web — confirmed: WebGPU exposes no binding-array features) | [003](decisions/003-desktop-first-tauri-exit-gate.md) |
| Browser target | **CI-enforced**: `wasm32-unknown-unknown` builds on every push (`.github/workflows/wasm-tripwire.yml`); native+browser render proven from one wgpu crate (`spikes/wasm`) | [003](decisions/003-desktop-first-tauri-exit-gate.md) |
| Query backend | Native: Flecs (behind the wrapper). Browser: pure-Rust index over the Loro projection — Flecs is native-only (won't compile to wasm32) | [006](decisions/006-browser-query-backend.md) |
| Plugins / scripting | Extism WASM plugins | plan §2 |
| AI layer | MCP server + JSON-Schema-constrained JSON Patch | plan §2 |
| Scene format | Own format; BSN-compatible where cheap; BRP interop | plan §2 |
| Logic layer | Rules (When/If/Then) + state machines as data, registry-fed builder; code behavior via WASM plugins (post-slice) | plan §2 |
| Asset pipeline | Import anything (FBX/glTF/PNG…) → glTF + KTX2, LODs, colliders, rig detection; local now, server-side Phase 2 | plan §1.5 |
| Asset generation + marketplace | Text-to-3D providers wrapped behind our API; token economy; local → marketplace → generate (Phase 2) | [004](decisions/004-free-engine-token-economy.md) |
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
             renderer later; depends on /ecs                                  (workspace member)
/transport   Rust lib — deltas-only protocol trait; 3 impls land M2+        (workspace member)
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
(fmt + clippy `-D warnings` + test + greps forbidding `flecs_ecs` outside `/ecs` AND `loro` outside
`/core`), `wasm-tripwire.yml` (wasm32 build; never `ecs`/`core`/Flecs, per ADR-006), and
`perf-gate.yml` (M1.5: **fails the build if the cached compat query's p99 > 16 ms** on M1.4's 5k
preset through the wrapper — north-star test #1; ~776× runner headroom; calibration in
`tools/query-gate/README.md`).

## Open questions (gated, not debated)

Resolved at the M0 gate review (2026-06-13) — kept here struck-through for traceability:

- ~~`flecs_ecs` binding viability~~ → **ADOPT, confirmed** ([ADR-001](decisions/001-flecs-over-bevy-ecs.md), `spikes/flecs`): 12–58 µs p99 (≪16 ms), safety locks ON, zero stale. **M1 integration go/no-go: GO.** The commit pipeline runs through the wrapper with latest-op undo p99 0.24–0.30 ms and entity-resurrection undo p99 0.72 ms — both ≫ under the 5 ms budget (n=500, under parallel test load; M1.6). Two-fork merge converges; all 8 invalid-state classes detected+repaired. 49 tests green. No `flecs_ecs` type leaks past `/ecs`.
- ~~Loro history size / merge semantics at scale~~ → **ADOPT, confirmed** ([ADR-002](decisions/002-loro-over-custom-wal.md), `spikes/loro`).
- ~~Browser ECS path~~ → **resolved** ([ADR-006](decisions/006-browser-query-backend.md)): browser runs a pure-Rust query backend over the Loro projection; Flecs is native-only. `loro`+`wgpu` reach wasm; `flecs_ecs` does not.

Genuinely open (gated, not debated):

- **Tauri IPC on Windows WebView2** → M2 gate (60 Hz drag, worst-case delta payload; fallback: CEF shell). Not tested in M0.
- **Real-scene render cost at ≥5k entities** → M2 stress-scene measurement. Spike ③ proved the wgpu pipeline with a *triangle* (≈0 render work); the editor scene's actual frame cost is unmeasured.
- **Capability identity / namespacing** → Phase-2 (marketplace) gate. The M1.3 registry interns capabilities by **bare string**, so two authors' `"Health"` collide — fine for the curated stdlib, wrong for an open marketplace + describe-to-create. *Direction:* a curated **standard vocabulary** (canonical stdlib caps) + **author/package-namespaced** custom caps that opt into the standard web via `(AliasOf, std:Cap)` pairs; the describe-to-create embedding model steers authors toward existing caps to curb fragmentation. *Clean seam* — capabilities are already `Entity` ids, so this is an intern-key + alias-pair change, not a rewrite. **Rule until decided:** never expose bare-string capability identity in a public/marketplace-facing API. (Surfaced in M1.3 — see `progress/M1.md`; decide + ADR at the marketplace gate.)
