# M2.2 render-gate results

The operative output of the render spike: real frame-cost numbers for the M1.4 stress scene at 5k +
20k entities, native and in the browser, and the gate verdict. **GPU frame time** (the gate metric —
it's independent of vsync) and **CPU submit** are measured separately. All timings are headless
benches (`SPIKE_SECS`/`?secs=`) on the machine below; inputs are the seeded M1.4 cloud.

## Verdict

**RENDER APPROACH HOLDS — gate PASS** (native + Chromium-engine browsers). Instanced + GPU-culled +
indirect rendering clears both frame budgets with large headroom; draw-call count is constant (3) as
entity count quadruples (instancing engages). One documented gap: the Firefox 141 leg could not be
run (Firefox is not installed on this machine) — low risk, see §5.

## Environment

| | |
|---|---|
| OS | Windows 11 Home 10.0.26200 |
| CPU | 13th Gen Intel Core i9-13900H (14C/20T) |
| GPU | NVIDIA GeForce RTX 4060 Laptop GPU |
| RAM | 47.6 GB |
| rustc / cargo | 1.92.0 (stable-x86_64-pc-windows-msvc, ded5c06cf 2025-12-08) |
| wgpu / winit | **29.0.3** / 0.30.13 |
| glam / bytemuck | 0.30 / 1.19 |
| wasm-bindgen (crate + CLI) | 0.2.125 · wasm-opt binaryen_130 |
| Browsers | Chrome 149.0.7827.114 · Edge 149.0.4022.69 (both Chromium / **Dawn** WebGPU) · Node v24.11.1 |
| Native backend / present | **Vulkan** / `AutoVsync` (FIFO) — GPU/CPU times measured independently of present |
| Browser backend | `BrowserWebGpu` (Dawn over D3D/ANGLE); `crossOriginIsolated = true` (finer timestamps) |
| RNG seed | `0x4D45_5452_4F43_4131` ("METROCA1") — same as M1.4 / the M0 spikes |

## 1. Native — GPU frame time + CPU submit (2 runs each, ≥60 s)

Gate: **5k p99 ≤ 8.3 ms** (≥120 fps) · **20k p99 ≤ 16.6 ms** (≥60 fps), no frame-time spikes.

| scene | run | frames | **GPU** p50 / p95 / p99 / max (ms) | **CPU-submit** p50 / p95 / p99 / max (ms) | visible min–max |
|---|---|---|---|---|---|
| 5k  | 1 | 7223 | 0.564 / 0.602 / **0.607** / 1.190 | 0.715 / 0.942 / 1.154 / 4.291 | 2852–4999 (100%) |
| 5k  | 2 | 7284 | 0.558 / 0.593 / **0.598** / 1.156 | 0.754 / 0.928 / 1.112 / 3.323 | 2852–4999 (100%) |
| 20k | 1 | 7248 | 0.774 / 0.859 / **0.881** / 1.321 | 0.537 / 0.843 / 1.012 / 1.501 | 11203–19988 (100%) |
| 20k | 2 | 7255 | 0.774 / 0.855 / **0.948** / 1.428 | 0.534 / 0.865 / 1.039 / 1.748 | 11202–19988 (100%) |

- **5k GPU p99 ≈ 0.60 ms** vs the 8.3 ms budget → **~14× headroom**. ✅
- **20k GPU p99 ≈ 0.88–0.95 ms** vs the 16.6 ms budget → **~17× headroom**. ✅
- No spikes: GPU max ≤ 1.43 ms across all runs (no wgpu periodic-slow-frame pattern). The one CPU
  outlier (4.29 ms max, run 5k-1) is a single isolated frame — p99 stays at 1.15 ms — and is still
  inside the 8.3 ms budget; not reproduced run-to-run.
- Run-to-run swing: GPU p99 5k +1.5%, 20k +7.6% — both well under the 25% investigate-threshold.

## 2. Worst-case "everything visible" frame

The camera path breathes between a close vantage (heavy culling) and a pulled-back vantage. Every run
captured a frame at **~100 % visible** (5k: 4999/5000; 20k: 19988/20000) — the worst-case
all-in-frustum frame is included in the p99/max above. Culling does engage at the close vantage
(visible drops to ~57 % / ~56 %), so the indirect instance count tracks the frustum, not a constant.

## 3. Browser — Chrome + Edge (2 browsers, 5k + 20k)

Gate: **5k p99 ≤ 16.6 ms** (≥60 fps) Chrome/Edge · **20k ≥ 30 fps acceptable** (≥33.3 ms p99).

| browser | scene | frames | **GPU** p50 / p95 / p99 / max (ms) | **CPU-submit** p99 / max (ms) | visible max |
|---|---|---|---|---|---|
| Chrome | 5k  | 2471 | 1.224 / 1.329 / **1.335** / 2.231 | 0.405 / 3.470 | 4998 (100%) |
| Chrome | 20k | 2475 | 3.138 / 3.256 / **3.261** / 3.271 | 0.390 / 3.850 | 19979 (100%) |
| Edge   | 5k  | 2434 | 0.709 / 1.327 / **1.341** / 2.854 | 0.450 / 3.640 | 4999 (100%) |
| Edge   | 20k | 2477 | 2.026 / 3.257 / **3.266** / 5.107 | 0.420 / 4.245 | 19979 (100%) |

- **5k GPU p99 ≈ 1.34 ms** (~750 fps-equiv) vs 16.6 ms → passes with ~12× headroom. ✅
- **20k GPU p99 ≈ 3.26 ms** (~305 fps-equiv) vs the 33.3 ms (30 fps) acceptable bar → passes with
  ~10× headroom; comfortably ≥60 fps, not just ≥30. ✅
- Gap to native (20k: 3.26 ms browser vs 0.95 ms native, ~3.4×) is the expected Dawn/ANGLE-over-D3D
  GPU overhead — **not** a draw-call explosion: draw calls stayed **3** and the visible counts match
  native, so it's per-frame GPU cost, not a correctness/instancing bug.
- `TIMESTAMP_QUERY` **is** exposed by Dawn (Chrome/Edge) with COI on, so these are real GPU times.
- Render proof: `chrome-20k.png` etc. show the full instanced cloud + gizmos + grid + live overlay.
  (The CDP 2D-canvas pixel-readback reports 1 distinct color — the known headless WebGPU→2d-canvas
  compositing limitation from M0 spike ③ — but `Page.captureScreenshot` captures the real frame, and
  the GPU timestamps over 2400+ frames prove the pipeline executed.)

## 4. Instancing & storage-buffer checks

- **Draw calls = 3, constant** at 5k AND 20k (grid bundle + cube indirect + gizmo indirect). It does
  **not** scale with entity count. GPU time grows only **0.56 → 0.77 ms (1.4×)** native for a **4×**
  entity increase — strongly sublinear ⇒ instancing is engaged (a non-instanced path would be ~4×).
- **Largest storage buffer:** instances = 32 B × N → **0.15 MB at 5k, 0.61 MB at 20k** — far under
  the **128 MB** browser ceiling. We build against `Limits::downlevel_defaults()`, whose
  `max_storage_buffer_binding_size` **is** exactly 128 MB, so compiling+running against that floor
  proves we stay within the browser limit (and `max_storage_buffers_per_stage = 4` ≥ the 3 we use).
- **No multi-draw-indirect / no `indirect-first-instance`:** confirmed — the cull→compact→single
  indirect-draw-per-mesh path ran identically native and in both browsers, producing matching visible
  counts; `first_instance` is fixed at 0.

## 5. Gap (honest): Firefox 141

The task asks the browser leg to also run on **Firefox 141**. **Firefox is not installed on this
machine** (the M0 spike had a winget copy 151.x that could not be driven headlessly; it is now absent
entirely), so the Firefox run was **not performed** — recorded as a gap, not estimated.

**Risk: low.** Firefox's WebGPU is built on **the same `wgpu` 29 this spike renders through**, and the
spike deliberately uses only the portable WebGPU subset that motivated the design (no
multi-draw-indirect, `first_instance = 0`, `downlevel_defaults` limits, pass-boundary timestamps).
The two browsers verified share Dawn, so this proves the Dawn path; a Firefox/Gecko pass remains a
fast follow-up (install Firefox 141, then open `http://localhost:8085/?n=20000&secs=20` and read the
on-page overlay / `globalThis.__benchresult`; Firefox uses a different remote protocol than CDP, so
the `verify-browser.mjs` auto-driver would need a Gecko adaptation or a manual read).
