# Browser-target constraints (M0 Spike ③)

The operative output of the wasm/WebGPU spike: real numbers and the hard limits the Phase-2 browser
funnel (ADR-003 / ADR-004) must design around. All measured on the machine below; the synthetic
artifact is the minimal wgpu spinning triangle.

## Environment

| | |
|---|---|
| OS | Windows 11 Home 10.0.26200 |
| CPU | 13th Gen Intel Core i9-13900H (14C/20T) |
| GPU (native) | NVIDIA GeForce RTX 4060 Laptop GPU — wgpu picked **Vulkan** |
| RAM | 47.6 GB |
| rustc / cargo | 1.92.0 (stable-x86_64-pc-windows-msvc) |
| wgpu / winit | 29.0.3 / 0.30.13 |
| wasm-bindgen (crate + CLI) | 0.2.125 (must match) |
| wasm-opt (binaryen) | version_130 |
| Browsers | Chrome 149.0.7827.114, Edge 149.0.4022.62 |
| RNG seed | n/a (no randomized scene in this spike) |

## 1. WASM binary size (the funnel's load-time baseline)

| stage | size | note |
|---|---|---|
| raw `cargo build` wasm (cdylib) | **1361 KB** | before wasm-bindgen |
| after `wasm-bindgen --target web` | **378 KB** | bindgen GC + our `opt-level="s"`/LTO/strip profile |
| after `wasm-opt -Oz` | **335 KB** | binaryen size pass |
| **brotli (q11) of the optimized wasm** | **118 KB** | what actually goes over the wire |
| gzip (l9), for comparison | 143 KB | |
| JS glue (`*.js`) raw / brotli | 84 KB / **12 KB** | wasm-bindgen loader |
| **total transfer (brotli wasm + brotli JS)** | **≈ 130 KB** | minimal wgpu triangle baseline |

Takeaway: a *bare* wgpu app is ~130 KB transfer. This is the floor; the real editor's wasm grows
with the core, UI, and (the big one) whatever ECS/query layer the browser ends up using — see §5.
`wasm-opt` requires `--enable-reference-types --enable-bulk-memory …`; wasm-bindgen 0.2.125 emits
those, and wasm-opt **fails validation without the flags** (captured the hard way; see `build.ps1`).

## 2. Time-to-first-frame & steady frame time — browser vs native

| metric | native (Vulkan, RTX 4060) | browser (WebGPU, Chrome/Edge headless) |
|---|---|---|
| TTFF (gfx init → first present) | ~4.8 s **cold**, dominated by Vulkan instance/adapter + first-pipeline compile | **0.4–0.8 s** |
| steady frame time | **8.3 ms median / 9.9 ms p99** (~120 fps, vsync-locked) | ~8.3 ms in pixel-readback runs (~120 fps); headless timing is approximate |

Notes: native TTFF is a cold-start outlier (driver load + shader compile on a hybrid-GPU laptop),
**not** the funnel metric — the browser TTFF is, and it is sub-second for a cold wasm fetch+init on
localhost. Steady-state easily clears the 16 ms frame budget on both. Browser frame time under
headless virtual-time isn't a trustworthy fps number; the meaningful result is "renders smoothly,
well inside budget". Real-network browser TTFF will add download of the ~130 KB transfer.

## 3. Adapter / limits / features — native vs WebGPU (bindless flagged)

Dumped at startup by the app on each target. **The web backend reports no bindless at all** — this
is the concrete justification for ADR-003's "renderer must maintain a non-bindless path for the web
target from day 1."

| | native (Vulkan / RTX 4060) | WebGPU (browser) |
|---|---|---|
| backend / type | Vulkan / DiscreteGpu | BrowserWebGpu / Other (name hidden for privacy) |
| `TEXTURE_BINDING_ARRAY` | **true** | **false** |
| `BUFFER_BINDING_ARRAY` | **true** | **false** |
| `STORAGE_RESOURCE_BINDING_ARRAY` | **true** | **false** |
| `…NON_UNIFORM_INDEXING` (bindless) | **true** | **false** |
| `PARTIALLY_BOUND_BINDING_ARRAY` | **true** | **false** |
| max_texture_dimension_2d | 32768 | 16384 |
| max_buffer_size | 1 TB (1099511627776) | 2 GB (2147483648) |
| max_bind_groups | 8 | 4 |
| max_storage_buffers_per_stage | 524288 | 16 |
| max_storage_textures_per_stage | 524288 | 8 |
| max_uniform_buffer_binding_size | 65536 | 65536 |
| max_bindings_per_bind_group | ~4.29e9 (unbounded) | 1000 |
| max_compute_invocations_per_wg | 1024 | 1024 |

**Limits we will plausibly hit on the web:** `max_bind_groups = 4` (vs 8) forces a tighter binding
layout; `max_storage_buffers_per_stage = 16` and `max_storage_textures_per_stage = 8` are the real
ceilings for any compute/storage-heavy path; **no bindless** means texture/material arrays must use
a bounded, non-uniform-free fallback (atlas or fixed-size arrays). The 2 GB `max_buffer_size` and
the wasm32 4 GB address-space ceiling together bound how big a single scene buffer can get in the
browser. We render against `Limits::downlevel_webgl2_defaults()` so the *same code* is portable down
to a WebGL2 fallback if WebGPU is unavailable.

## 4. Cross-origin isolation & browser matrix

`crossOriginIsolated === true` was verified in both browsers (the dev server sets
`Cross-Origin-Opener-Policy: same-origin` + `Cross-Origin-Embedder-Policy: require-corp`; localhost
is a secure context). Evidence: the on-page banner and console log captured by `verify-browser.mjs`,
and the screenshots `chrome.png` / `edge.png` (spinning gradient triangle + the banner). COI is the
precondition for `SharedArrayBuffer` / wasm threads we will want later (not used here).

| browser | version | engine | renders triangle | crossOriginIsolated | TTFF |
|---|---|---|---|---|---|
| Chrome | 149.0.7827.114 | Chromium / Dawn | ✅ (512×512, 118 distinct colors sampled) | ✅ true | ~0.6–0.8 s |
| Edge | 149.0.4022.62 | Chromium / Dawn | ✅ (512×512, 119 distinct colors sampled) | ✅ true | ~0.4–0.7 s |
| Firefox | 151.0.4 (installed via winget) | Gecko / **wgpu** | not verified — binary not locatable/drivable in this non-interactive session | — | — |
| Safari | — | WebKit | macOS-only; not available on this machine | — | — |

**Gap (honest):** the two verified browsers share the **Dawn** WebGPU implementation, so this proves
the Dawn path, not cross-*engine* portability. The intended second engine (Firefox) installed but
could not be driven headlessly here; Safari is macOS-only. Low risk — Firefox's WebGPU is built on
**the same `wgpu` this spike renders through** — but a Firefox/Safari pass remains a fast follow-up
(`firefox --headless --screenshot`, or a macOS runner for Safari).

Headless note: WebGPU **renders correctly but does not composite into `Page.captureScreenshot`** for
the GPU canvas in some headless configs; the screenshots here work because the canvas is explicitly
sized and read back via `drawImage`→`getImageData`. Also, winit's web backend left the canvas at 1×1
until we forced the backing size — a real gotcha for the M1 web shell (see `src/lib.rs`).

## 5. Build-only check: do `flecs_ecs` and `loro` compile for wasm32-unknown-unknown?

This is the spike's highest-stakes finding — it tests the ADR-001/002 revisit clauses and the
ADR-003 "one Rust core → native + browser" premise.

### `loro` 1.13.1 → **BUILDS ✅**

`cargo build --target wasm32-unknown-unknown --release` of a crate using `LoroDoc` finished clean
(2m08s cold). Pure Rust; its `getrandom 0.2` dependency compiles for wasm32. **Runtime caveat:** to
actually *run* in a browser, `getrandom` needs its `js` backend (`getrandom = { features = ["js"] }`
in the leaf binary, or the equivalent build cfg) — a one-line addition, build-compatible today. So
the document layer is web-ready.

### `flecs_ecs` 0.2.2 → **DOES NOT BUILD ❌**

`cargo build --target wasm32-unknown-unknown` fails in `flecs_ecs_sys`'s build script. Verbatim:

```
warning: flecs_ecs_sys@0.2.1: Compiler family detection failed due to error: ToolNotFound:
  failed to find tool "clang": program not found
error: failed to run custom build command for `flecs_ecs_sys v0.2.1`
  --- stderr
  error occurred in cc-rs: failed to find tool "clang": program not found
```

Two layers to this, both real:
1. **Proximate:** the Flecs C core is compiled by the `cc` crate, which for a wasm target needs
   `clang` (MSVC `cl.exe` cannot emit wasm). A stock Windows+MSVC Rust install (this one) has no
   clang → instant failure.
2. **Fundamental (does not go away with clang):** `wasm32-unknown-unknown` is a *bare* target with
   **no libc / no sysroot** — no `malloc`, `stdio`, `string.h`, threads. `flecs.c` `#include`s and
   calls libc throughout. Compiling C to wasm requires `wasm32-wasi` or `wasm32-unknown-emscripten`
   **plus a sysroot** (wasi-sdk / emscripten). There is no sysroot for this target, so the Flecs
   core fundamentally cannot target `wasm32-unknown-unknown`.

**Implication (flagged against ADR-001 "revisit when" and ADR-003):** the semantic ECS — the
product's beating heart — cannot run client-side on the browser's bare wasm target as-is. The
browser lite-editor (ADR-004 funnel) therefore needs one of:
- compile Flecs via `wasm32-wasi`/emscripten + sysroot and bridge it to the WebGPU canvas (heavy,
  non-standard for a web app; emscripten threads need COOP/COEP — which we now have);
- run the **query/ECS layer differently in the browser** (e.g. the Loro document is the in-browser
  source of truth with a pure-Rust query/index layer, no Flecs), keeping Flecs native/desktop-only;
- or a thin-client browser editor backed by a server-side core (contradicts "free offline lite
  editor", so dispreferred).

This does not block desktop (native Flecs is validated — see `spikes/flecs`). It **does** change the
Phase-2 browser timeline and is the reason the CI tripwire (which builds only the pure-Rust wgpu
crate, not Flecs) exists: to keep the *rest* of the core honest while this architectural question is
resolved. The tripwire deliberately excludes Flecs so it stays green and meaningful.
