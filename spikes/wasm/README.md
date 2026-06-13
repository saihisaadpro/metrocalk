# M0 Spike ③ — wasm32 + WebGPU browser target

Proves the browser leg of **ADR-003**: the Rust core toolchain compiles to
`wasm32-unknown-unknown` and renders via WebGPU in a browser today, and ships the CI tripwire that
keeps it true. The browser build is the adoption funnel (free lite editor, shareable links —
ADR-004), so this protects business value, not just engineering convenience.

> Throwaway spike. Deliverable is a working pipeline + a constraints document (`CONSTRAINTS.md`),
> not a product. One crate, two targets: native window and browser canvas.

## Tooling choice: raw `wasm-bindgen`, not `trunk`

Raw `wasm-bindgen` (+ `wasm-opt`) is the minimal, transparent path — every step (cargo build →
`wasm-bindgen` → `wasm-opt` → serve) is explicit and individually measurable, which is exactly what
a constraints-measuring spike needs; `trunk` would add a build-tool dependency and hide the
artifact-size and header steps we're here to quantify.

## Prerequisites

- Rust stable with the `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`).
- `wasm-bindgen-cli` **matching the `wasm-bindgen` crate version** (0.2.125 here — a mismatch is a
  hard, confusing runtime error).
- `wasm-opt` (from [binaryen](https://github.com/WebAssembly/binaryen/releases)).
- Node (for the COOP/COEP dev server; no npm packages needed).
- A WebGPU browser: Chrome/Edge 113+ (Firefox needs `dom.webgpu.enabled`; Safari 18+).

## Native (one command)

```
cargo run --release --bin triangle
```

Opens a window with a spinning triangle; the title bar is the frame-time overlay. Set
`SPIKE_FRAMES=600` to run a headless-friendly bench that prints TTFF + steady frame-time stats and
exits.

## Browser (build → serve → open)

```
# 1. compile the cdylib to wasm32
cargo build --release --target wasm32-unknown-unknown --lib

# 2. generate JS glue (--target web) into web/pkg/
wasm-bindgen --target web --no-typescript \
  --out-dir web/pkg \
  target/wasm32-unknown-unknown/release/metrocalk_wasm_spike.wasm

# 3. shrink the artifact
wasm-opt -Oz -o web/pkg/metrocalk_wasm_spike_bg.wasm web/pkg/metrocalk_wasm_spike_bg.wasm

# 4. serve with COOP/COEP headers (so crossOriginIsolated === true)
node serve.mjs 8080
# open http://localhost:8080  → spinning triangle + overlay showing crossOriginIsolated + ms/frame
```

`build.ps1` runs steps 1–3 in one go.

## What this spike measures

See **`CONSTRAINTS.md`** for the real numbers: wasm binary size (raw / wasm-opt / brotli),
time-to-first-frame and steady frame time (native vs browser), the native-vs-WebGPU adapter limits
and features diff (with bindless flagged), and the **build-only** check of whether `flecs_ecs` and
`loro` compile for wasm32 (they don't both — see the finding).

## CI tripwire

`.github/workflows/wasm-tripwire.yml` builds the wasm32 target on every push/PR and fails loudly if
the web target breaks. Caching (`Swatinem/rust-cache`) keeps reruns under 5 minutes.
