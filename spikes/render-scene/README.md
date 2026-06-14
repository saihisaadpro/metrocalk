# M2.2 — Real-scene render gate (wgpu, native + browser)

Resolves the architecture open-question **"real-scene render cost at ≥5k entities"** (M0 spike ③ only
proved a *triangle* — ≈ 0 render work). This renders the M1.4 stress scene (5k / 20k entities) as
instanced meshes through one wgpu codebase, native AND in the browser, and measures GPU frame time
and CPU submit **separately**.

> Throwaway spike (excluded from the workspace, like the M0 spikes). Deliverable is the measurement
> + verdict in **`RESULTS.md`**, not a product. One crate, two targets.

## What it renders

The M1.4 cloud — entity cubes + a per-entity 3-axis gizmo + a ground grid — built from the **same
SplitMix64 seed** ("METROCA1") as `ecs/src/scene.rs` (M1.4 is a *relational* scene with no spatial
component, so the transforms are generated here from that shared seed; reproducible across runs/OS).

The frame budget is held at scale by, per the task's verified research findings:

- **Instancing** — cubes and gizmos are each ONE instanced draw; the grid is a **render bundle**.
  Draw-call count is therefore **constant (3) regardless of entity count** — the instancing sanity
  check.
- **GPU frustum culling → indirect draws** — a compute pass tests each instance's bounding sphere
  against the 6 frustum planes, compacts survivors into a `visible[]` list, and `atomicAdd`s the
  count into a counter copied into the draw-args buffers. We then issue ONE `draw_indexed_indirect`
  (cubes) + ONE `draw_indirect` (gizmos). Designed **without** multi-draw-indirect and **without**
  `indirect-first-instance` (both browser-absent): `first_instance` stays 0 and the vertex shader
  indexes `visible[]` by `instance_index`.
- **Separate GPU vs CPU timing** — `TIMESTAMP_QUERY` (pass-boundary timestamps — the portable
  WebGPU mechanism, *not* the native-only `TIMESTAMP_QUERY_INSIDE_ENCODERS`) gives GPU frame time,
  double-buffered for non-blocking readback; wall-clock around encode+submit gives CPU submit.

## Native (one command)

```
# visual: SCENE_N picks the preset (5000 default / 20000)
cargo run --release --bin scene                       # 5k, windowed, live overlay
SCENE_N=20000 cargo run --release --bin scene         # 20k

# headless bench: SPIKE_SECS runs a timed measurement then prints the table and exits
SPIKE_SECS=60 SCENE_N=5000  cargo run --release --bin scene
SPIKE_SECS=60 SCENE_N=20000 cargo run --release --bin scene
```

(`cargo` is not on PATH in this environment — prepend the rustup bin; see the repo memory.)

## Browser (build → serve → drive)

```
./build.ps1                       # cargo wasm32 → wasm-bindgen → wasm-opt → web/pkg/ (~379 KB)
node serve.mjs 8085               # COOP/COEP dev server (crossOriginIsolated → finer timestamps)
# open http://localhost:8085/?n=5000        (or ?n=20000 ; add &secs=20 for a timed bench)

# headless bench via CDP (Chrome/Edge), reads globalThis.__benchresult + a screenshot:
node verify-browser.mjs --browser "C:\Program Files\Google\Chrome\Application\chrome.exe" \
  --n 5000 --secs 20 --port 8085 --out chrome-5k
```

## Results

See **`RESULTS.md`** — native & browser p50/p95/p99/max tables (2 runs/discipline), the
draw-call-vs-entity instancing check, the <128 MB storage-buffer confirmation, and the verdict.
Screenshots `chrome-5k.png` / `chrome-20k.png` / `edge-*.png` are the in-browser render proof.
