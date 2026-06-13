# Architecture — Current State

> Rules for this file: current state only, max ~2 pages. No rationale — link the ADR instead. Prune on every change. Status: **pre-code (M0 spikes pending)**.

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

## Layers and choices

| Layer | Choice | ADR |
|---|---|---|
| Semantic ECS | Flecs v4.1 via `flecs_ecs`, behind our own query API | [001](decisions/001-flecs-over-bevy-ecs.md) |
| Document / undo / collab / persistence | Loro 1.x | [002](decisions/002-loro-over-custom-wal.md) |
| Shell + UI | Tauri 2 + React/TS; viewport in Rust/wgpu | [003](decisions/003-desktop-first-tauri-exit-gate.md) |
| Rendering | wgpu + WGSL (non-bindless path required for web) | plan §2 |
| Plugins / scripting | Extism WASM plugins | plan §2 |
| AI layer | MCP server + JSON-Schema-constrained JSON Patch | plan §2 |
| Scene format | Own format; BSN-compatible where cheap; BRP interop | plan §2 |
| Logic layer | Rules (When/If/Then) + state machines as data, registry-fed builder; code behavior via WASM plugins (post-slice) | plan §2 |
| Asset pipeline | Import anything (FBX/glTF/PNG…) → glTF + KTX2, LODs, colliders, rig detection; local now, server-side Phase 2 | plan §1.5 |
| Asset generation + marketplace | Text-to-3D providers wrapped behind our API; token economy; local → marketplace → generate (Phase 2) | [004](decisions/004-free-engine-token-economy.md) |
| Deferred | Physics (Rapier), audio (kira), netcode (lightyear) | plan §2 |

## Invariants (non-negotiable)

1. One source of truth: ECS authoritative, Loro is its durable mergeable mirror, UI holds projections.
2. Deltas only across every boundary; never full-state snapshots.
3. Everything is a transaction through one commit pipeline (human, plugin, AI). Merge validation re-checks ECS invariants after every CRDT merge.
4. Hot path never crosses the JS boundary.
5. Every pre-1.0 dependency lives behind our own trait.

## Repository (planned)

```
/core        Rust: ECS wrapper, registry, commit pipeline, renderer
/editor      React/TS UI
/transport   protocol trait + 3 impls
/plugins     Extism host + SDK
/spikes      M0 throwaway spikes (loro, flecs, wasm)
```

## Open questions (gated, not debated)

- `flecs_ecs` binding viability → M1 gate (fallback: `bevy_ecs`)
- Tauri IPC on Windows WebView2 → M2 gate (fallback: CEF shell)
- Loro history size / merge semantics at scale → M0 spike
