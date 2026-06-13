# ADR-003: Desktop-first delivery, Tauri 2 shell with a hard exit gate

**Date:** 2026-06-12 · **Status:** Accepted (gated at M2) · **Supersedes:** —

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
