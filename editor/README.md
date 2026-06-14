# editor/ — React + TypeScript UI (M2.5 scaffold)

**Not a cargo workspace member.** The editor front-end: a **projection of the authoritative core**
(invariant 1), fed by deltas over the M2.4 transport — never a second source of truth. See
[ADR-010](../decisions/010-editor-projection-architecture.md).

## Stack

- **Projection store** — Zustand over `useSyncExternalStore` (`src/store/projection.ts`): entity-keyed,
  immutable per-entity, with a separate `{id,name,parentId}` **summary projection** so a field edit
  never re-renders the tree. `displayed = base ⊕ pending` (optimistic overlay).
- **Transport client** — TS mirror of the M2.4 Loro-Protocol-v1 envelope (`src/transport/frame.ts`) +
  a `DeltaClient` (`%LOR` committed deltas + `%EPH` ephemeral path) + an in-process binding and a
  desktop binding stub (M2.6 → Tauri Channel). A `MockCore` stands in for the WASM core.
- **Inspector** — JSON Forms with custom renderers via testers (color, `entity-ref` bind-target, enum);
  a field edit → optimistic update + a JSON-Patch transaction (the AI layer's language).
- **Binding graph** — React Flow, neighborhood-scoped (Sigma.js noted for 50k+).
- **Hierarchy** — virtualized rows over the summary projection (5k entities, ~30 mounted).
- **Reconciliation** — optimistic echo; rejection is first-class UX ("every 'no' explained").
- **Input ownership** (`src/input/ownership.ts`) — chrome-only for React; viewport events belong to the
  native wgpu layer (invariant 4), wired for real in M2.6.

## Run

```
pnpm install
pnpm dev      # vite dev server
pnpm test     # vitest — 11 tests (render-count @5k, tear-free, reject path, envelope, app wiring)
pnpm build    # tsc -b && vite build
```

## Verified (M2.5)

- **Selective subscription at 5k**: editing one entity re-renders exactly its subscribed detail and
  **0 of 5000** hierarchy rows; a name change re-renders exactly that one row.
- **Tear-free** under a React 19 concurrent transition (atomic multi-field delta never observed
  half-applied).
- **Reject path**: an incompatible optimistic bind is reverted and the core's reason is surfaced.
- Single-entity field-edit apply+render ≈ **24–70 ms at 5k in jsdom** (the apply is an O(n) map copy;
  only the 1 subscribed component re-renders — see ADR-010 for the structural-sharing upgrade note).

## Stubbed (wired later)

Viewport hot-input → native layer (M2.6); the `MockCore` → real WASM/Rust core (M2.6); no real assets;
the AI/describe-to-create layer and the Rules UI are post-slice (they reuse the JSON-Patch `EditTx`).
