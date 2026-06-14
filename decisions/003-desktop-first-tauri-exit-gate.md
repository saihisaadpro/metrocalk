# ADR-003: Desktop-first delivery, Tauri 2 shell with a hard exit gate

**Date:** 2026-06-12 · **Status:** Accepted (gated at M2) — **browser-render leg PROVEN 2026-06-13** (`spikes/wasm`); **real-scene render gate (M2.2) PASSED 2026-06-14** (`spikes/render-scene`); Tauri WebView2 IPC gate (M2.1) still pending at M2 · **Supersedes:** —

> **Real-scene render gate (M2.2, 2026-06-14):** the M1.4 stress scene (5k + 20k entities — instanced cubes + per-entity gizmos + grid) renders well inside budget on the same one wgpu 29.0.3 crate, native + browser, via **instancing + GPU frustum culling (compute → compacted `visible[]` → indirect draws) + a grid render bundle**. **Native** (Vulkan, RTX 4060): GPU-frame-time p99 **0.60 ms** @5k / **0.88–0.95 ms** @20k (budgets 8.3 / 16.6 ms → ~14×/17× headroom), no spikes. **Browser** (Chrome 149 + Edge 149, Dawn): GPU p99 **1.34 ms** @5k / **3.26 ms** @20k (~750 / ~305 fps-equiv — clears the 60 fps / 30 fps bars). **Draw calls constant at 3** as entities go 4× (GPU time only 1.4× ⇒ instancing engages); largest storage buffer **0.61 MB ≪ 128 MB**; the no-multi-draw-indirect / `first_instance=0` path runs identically on all three. Confirms this ADR's "non-bindless web path from day 1" is sufficient for the editor scene. **Gap:** Firefox 141 not run (not installed on this machine) — low risk (Firefox WebGPU is built on the same `wgpu`); see `spikes/render-scene/RESULTS.md`.
**Date:** 2026-06-12 · **Status:** Accepted (gated at M2) — **browser-render leg PROVEN 2026-06-13** (`spikes/wasm`); **M2.1 Windows-WebView2 gate RESOLVED 2026-06-14** → IPC confirmed (deltas-only is overhead-bound, not the "~200 ms / 10 MB" bandwidth case), single-window wgpu-under-webview compositing **fails** → self-composite ([ADR-007](007-m2.1-tauri-gate-result.md)) · **Supersedes:** —

> **Browser-leg result (2026-06-13):** One wgpu 29.0.3 crate renders a spinning triangle on **native** (Vulkan, RTX 4060: 8.3 ms/frame p99 9.9 ms, ~120 fps) and in **Chrome 149 + Edge 149** via WebGPU (`crossOriginIsolated === true` under COOP/COEP; browser TTFF 0.4–0.8 s; 512×512 render verified by pixel readback + screenshot). Minimal-triangle transfer size ≈ **130 KB brotli** (118 KB wasm + 12 KB JS glue) — the funnel's load-time baseline. Native vs WebGPU adapter diff confirms **no bindless on the web** (all binding-array/non-uniform features false in-browser vs true on native) → the renderer's non-bindless path is mandatory, as this ADR already requires. CI tripwire `.github/workflows/wasm-tripwire.yml` builds wasm32 on every push — **verified on `github.com/saihisaadpro/metrocalk`: green in 54 s (cold), and verified to fail (at the build step) on a deliberate wasm32 break, then reverted.** **Browser matrix gap:** only Dawn-based browsers (Chrome/Edge) verified; Firefox/Safari (2nd engine) not testable on this Windows machine — fast follow-up, low risk (Firefox WebGPU is built on `wgpu`).
>
> **Web-incompatibility flagged against [ADR-001](001-flecs-over-bevy-ecs.md) "revisit when":** `flecs_ecs` 0.2.2 **does not build for `wasm32-unknown-unknown`** — its C core needs clang + a wasm libc/sysroot the bare target lacks (verbatim: `cc-rs: failed to find tool "clang"`; fundamentally, no libc on `wasm32-unknown-unknown`). `loro` 1.13.1 **does** build (needs `getrandom` `js` backend at runtime). Consequence: the browser lite-editor cannot run the Flecs ECS client-side as-is. Resolve in M1/Phase-2 planning — options: compile Flecs via `wasm32-wasi`/emscripten + sysroot, OR run a different (Loro-document-backed, pure-Rust) query layer in the browser, OR thin-client. Desktop is unaffected (native Flecs validated in `spikes/flecs`). Details + numbers in `spikes/wasm/CONSTRAINTS.md`.

## Context

The engine targets both native desktop and browser from one Rust core. A fully browser-hosted editor is proven viable (Figma, PlayCanvas, Rerun, Graphite) but constrained today: wasm32 4 GB ceiling, no Safari Memory64, no WebGPU bindless, COOP/COEP header requirements. Tauri 2 is the lightest desktop shell for a web UI, but its IPC has a hard floor on Windows WebView2 (~200 ms per 10 MB vs ~5 ms on macOS), and Graphite abandoned Tauri for CEF over engine-grade payloads.

## Decision

Ship desktop-first via Tauri 2. UI is React/TS; viewport and all hot interactions render in Rust/wgpu; the wire carries deltas only, behind a transport trait with three impls (in-process WASM / Tauri channels / WebSocket). Browser is a CI build target from M2 so web-incompatible decisions are caught immediately; a browser lite-editor ships Phase 2. Cloud-streamed editor rejected (cost + latency).

## Consequences

- One UI codebase for desktop and browser; collab transport falls out of the same trait.
- We must benchmark the worst-case payload on **Windows WebView2**, not macOS.
- If Tauri fails the gate, the fallback is a CEF wrapper (Graphite's path) — same web UI, different shell, no rewrite.
- Renderer must maintain a non-bindless path for the web target from day 1.

## Revisit when

M2 gate: 60 Hz interaction benchmark on Windows WebView2. Pass → keep Tauri. Fail after delta-protocol tuning → switch to CEF. Also revisit if Tauri ships zero-copy IPC or the Verso webview matures.
