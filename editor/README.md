# editor/ — React + TypeScript UI

**Not a cargo workspace member.** This is the editor front-end (React/TS + Zustand + React Flow),
intentionally outside the Rust workspace. It is scaffolded in M2–M3 alongside the Tauri 2 shell and
the transport trait's three impls (ADR-003).

Until then this is a placeholder so the repo shape matches `architecture.md`. The viewport and all
hot interactions render Rust-side via wgpu (invariant 4); this layer holds panels, the schema-driven
inspector, the React Flow binding graph, and optimistic local echo, talking to the core only through
the deltas-only transport trait (`transport/`, invariant 2).
