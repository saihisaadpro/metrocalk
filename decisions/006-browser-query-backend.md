# ADR-006: Browser query backend — pure-Rust index over Loro, not Flecs-on-wasm

**Date:** 2026-06-13 · **Status:** Accepted (M0 gate review) · **Supersedes:** — · **Resolves:** the browser-ECS fork left open by [ADR-003](003-desktop-first-tauri-exit-gate.md) and the web-target caveat on [ADR-001](001-flecs-over-bevy-ecs.md)

## Context

M0 spike ③ (`spikes/wasm`) proved `flecs_ecs` 0.2.2 **does not compile to `wasm32-unknown-unknown`**: its C core is built by `cc`, which needs clang to emit wasm (MSVC can't), and — more fundamentally — the bare target has no libc/sysroot for the Flecs core's `<stdlib.h>`/`<string.h>` includes. `loro` 1.13.1 **does** build for wasm32 (needs `getrandom`'s `js` backend at runtime), and `wgpu` renders in-browser (Chrome/Edge verified). So the document layer and renderer reach the browser; the **semantic ECS does not**.

ADR-003's "one Rust core → native + browser" and ADR-004's "free offline browser lite-editor" both assumed the whole core crosses to wasm. The query layer is the exception. ADR-001 already requires Flecs to live **behind our own query-API trait** ("no Flecs types leak outside the wrapper crate"), and its named fallback is "`bevy_ecs` + a hand-built relationship index" — itself pure Rust.

## Decision

The query-API wrapper (ADR-001) gets **two backends**:

1. **Native (desktop, M1, default):** Flecs v4.1 via `flecs_ecs` — validated at 8.7 µs/12.2 µs-p99 cached queries (`spikes/flecs`).
2. **Browser (Phase 2):** a **pure-Rust relationship index** over the Loro-document projection — no Flecs in the wasm build. The compatibility queries the product needs (pair-match `(Provides, X)`, wildcard `(R, *)`, negation "lacks `(BindsTo, *)`", read-target) are answerable by plain Rust index maps rebuilt/maintained from the Loro document.

**Flecs is never required on wasm.** In the browser, the Loro document is the source of truth and the pure-Rust index is a derived projection answering the same query trait; on desktop, the ECS stays authoritative and Loro is its mirror (invariant #1 holds per-target). The wrapper API must therefore be a backend-agnostic *relational query* abstraction both implementations satisfy — this is a hard design constraint on the M1 wrapper task, not a Phase-2 afterthought.

## Alternatives rejected

- **Flecs via `wasm32-unknown-emscripten` (+ wasi-sdk/emscripten sysroot):** emscripten provides a libc so the C core *could* compile, but the rest of the app (wgpu web backend, wasm-bindgen glue) targets `wasm32-unknown-unknown`; mixing an emscripten-built C core with wasm-bindgen/wgpu in one module is a deep toolchain/ABI/allocator conflict. High-risk, **unmeasured in the spike**, dispreferred. May be revisited only if the pure-Rust backend cannot meet browser query latency.
- **Thin client (server-side core):** contradicts ADR-004's "free offline lite editor". Rejected.

## Consequences

- **ADR-004 funnel preserved:** the browser lite-editor still runs fully offline — Loro (document) + pure-Rust queries + wgpu (render), all wasm-proven. No change to the business model; ADR-004 is **not** relitigated.
- **New Phase-2 scope:** build + benchmark the pure-Rust query backend (target: same <16 ms compatibility-query budget, in-browser). Its performance is asserted here, not yet measured.
- **M1 constraint (fallout absorbed by the wrapper task):** the ECS wrapper API must be designed to admit the second backend from day one (relational query surface, no Flecs-shaped leakage). This is the same discipline ADR-001's bevy_ecs fallback already demanded — double duty.
- **`getrandom` `js`** must be enabled for Loro in the wasm build (one-line).
- Desktop (M1–M6) is entirely unaffected; this is Phase-2 architecture settled early so M1 doesn't build a Flecs-only wrapper.

## Revisit when

The Phase-2 pure-Rust query backend is benchmarked: if it cannot hit interactive query latency in-browser on the stress scene, revisit emscripten-Flecs or a reduced browser feature set. Also revisit if `flecs_ecs`/Flecs ships a usable `wasm32-unknown-unknown` path.
