# ADR-010: Editor UI = projection of the core (Zustand/useSyncExternalStore · JSON Forms · React Flow)

**Date:** 2026-06-14 · **Status:** Accepted — M2.5 editor scaffold (`/editor`) · **Builds on:** invariant 1
(one source of truth), [ADR-006](006-browser-query-backend.md) (browser is Loro-authoritative),
[ADR-009](009-transport-protocol-loro-framing.md) (the delta wire this UI consumes).

## Context

The editor must be a **projection cache of the authoritative core, not a second source of truth**
(invariant 1). It is fed by deltas over the M2.4 transport and must stay responsive at canvas scale
(5k entities) while a delta re-renders only the components whose slice changed. M2.5 picked the UI
stack and the projection/optimistic architecture.

## Decisions

1. **Projection store = Zustand over `useSyncExternalStore`.** Entity-keyed, updated **immutably
   per-entity** so reference equality changes only for touched entities (tldraw's canvas-scale model).
   A separate **summary projection** (`{id,name,parentId}`) backs the list/hierarchy, so a field edit
   (which changes `displayed[id]` but not `summaries[id]`) never re-renders the tree. Verified: editing
   one of 5000 entities re-renders exactly its subscribed detail component and **zero** of the 5000
   rows; tear-free under a React 19 concurrent transition.
2. **Optimistic overlay = `displayed = base ⊕ pending`.** `displayed[id]` is kept
   **reference-identical to `base[id]` when no pending op touches the entity**, which is what makes the
   overlay free of cross-entity re-renders. Each edit carries a client-op-id; the authoritative echo
   `confirms` (drop the pending op — no-op reconcile) or `rejects` it (revert + surface the reason).
   **Rejection is first-class UX** — a rejected bind removes the optimistic edge and shows the core's
   merge-validation reason (the north-star "every 'no' explained"). This reconciliation is kept strictly
   separate from the engine-side user undo stack (ADR-002 F2).
3. **Schema-driven inspector = JSON Forms (over RJSF).** Renderer-registry + **testers** route typed
   fields (color, `entity-ref`/bind-target, enum) to custom renderers keyed on a schema `format` — the
   right fit for typed/semantic fields, vs RJSF's template model. A field edit emits a **JSON-Patch
   transaction**, the same language the AI layer emits, so human and AI edits are one path.
4. **Binding graph = React Flow, neighborhood-scoped.** Only the selected entity's bound/candidate
   neighbours, memoized — never 5k nodes. **Sigma.js is the documented 50k+ fallback** (not built).
5. **Edit language = JSON-Patch + structured intent.** Outbound `EditTx` (UI→core); inbound
   `ProjectionDelta` (core→UI). The UI never parses Loro — in production the core decodes Loro update
   bytes into the same `ProjectionDelta` this store consumes.
6. **Input-ownership = chrome-only for React.** A pointer over the viewport rect is left for the native
   wgpu layer (invariant 4 — the hot path never crosses the JS boundary); the TS `ownership` module is
   the React twin of the M2.3 `shell-input-routing` Rust crate, wired for real in M2.6.

## Consequences

- The UI cannot become a second source of truth: it holds only projections; the only mutation path is
  an `EditTx` to the core, reconciled on the authoritative echo.
- The store's per-delta cost is an **O(n) shallow map copy** (≈ tens of ms at 5k in jsdom); fine for
  60 Hz incremental deltas, and a structural-sharing map (immer/Map persistence) is the noted upgrade
  if profiling demands it. Selective subscription (the real win) is exact regardless.
- M2.6 swaps the in-process `MockCore` for the real WASM/Rust core and the desktop transport binding
  for the Tauri Channel IPC — the `DeltaTransport` + `ProjectionDelta`/`EditTx` contract is unchanged.

## Revisit when

- Profiling on a real browser shows the O(n) per-delta copy is a bottleneck → structural-sharing store.
- The binding graph needs 50k+ nodes → Sigma.js renderer.
- The AI/describe-to-create layer lands → it emits the same `EditTx`, so it plugs into this path.
