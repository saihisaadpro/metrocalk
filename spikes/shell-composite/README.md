# spikes/shell-composite — M2.3 shell composition gate → ADR-008

Throwaway spike (seeded from the M2.1 `tauri-shell` harness). Decides how the editor shell composites
the React UI over the native wgpu viewport on Windows, and **whether the M2.1 1b "FAIL" was real or a
capture artifact**. Verdict + evidence → [ADR-008](../../decisions/008-shell-composition.md).

## Result

**Single-window PASSES** — transparent WebView2 over a native wgpu surface on one HWND composites
correctly on Windows, on **both** a discrete (RTX 4060) and an integrated (Intel Iris Xe) GPU, with
the real panel layout, under motion, resize, and overlapping input routing. **M2.1's 1b "FAIL" was a
GDI capture artifact** — GDI can't see a flip-model/overlay swapchain; DXGI Desktop Duplication can,
and shows a clean composite. No DComp, no CEF (~170 MB) needed; the fallback ladder was not exercised.

## Why GDI misled M2.1, and what fixed it

M2.1 captured 1b with GDI `CopyFromScreen` → the live wgpu layer read as all-black (GDI cannot capture
a GPU overlay swapchain), and the window separately collapsed to 16×16. This spike captures with
**DXGI Desktop Duplication** (`capture/`), which grabs the DWM's final composited output (overlays
included). Under it: black 0%, the wgpu viewport-clear visible across ~70% of the window, and the
window stays a stable 1016×739 — the collapse never reproduced.

## Parts

- `capture/` — standalone `dd-capture` bin: Desktop-Duplication screenshot + pixel analysis
  (black %, distinct colours, viewport-clear %, bright-UI %). Swapchain-aware (not GDI).
- `input-routing/` — **deliverable 1**, the path-agnostic per-pixel UI-vs-viewport hit test
  (dependency-free, 7 unit tests). Carries unchanged to any shell (single-window / DComp / CEF).
- `src/composite.tsx` — the real React panel layout: opaque toolbar + inspector side-panel +
  transparent viewport + a semi-transparent floating box overlapping the viewport + input-routing log.
- `src-tauri/src/composite.rs` — the native wgpu layer (rotating cyan triangle) on the window HWND;
  `SHELL_GPU=low` selects the integrated GPU.

## Run (Windows)

```powershell
pnpm install            # one-time (pnpm store)
# single-window composite on the discrete GPU:
$env:GATE_MODE="composite"; pnpm tauri dev
# …on the Intel iGPU (the driver-sensitive 2nd-GPU battery):
$env:GATE_MODE="composite"; $env:SHELL_GPU="low"; pnpm tauri dev

# swapchain-aware capture of the window (bring it on top first), then analyse:
cargo run --release --manifest-path capture/Cargo.toml -- --rect <x,y,w,h> --out evidence/frame.bmp

# the path-agnostic input-routing layer:
cargo test --manifest-path input-routing/Cargo.toml
```

(`cargo` is not on PATH here — prepend the rustup bin; see the repo memory.)

## Battery (single-window) — Desktop-Duplication + visual

Env: Win11 10.0.26200 · i9-13900H · WebView2 149.0.4022.69 · Tauri 2.11.2 / wry 0.55.1 · wgpu 29.0.3
Vulkan · `Bgra8Unorm`, `alpha_modes=[Opaque, PreMultiplied]`.

| gate | dGPU RTX 4060 | iGPU Iris Xe |
|---|---|---|
| Real UI composited (panels over viewport) | PASS (black 0%, distinct 64–112) | PASS (black 0%, viewport ~74%) |
| Transparent regions show viewport through | PASS | PASS |
| Correct z-order | PASS | PASS |
| No blackout / no 16×16 collapse | PASS (stable 1016×739) | PASS |
| No flicker under motion (rotating triangle) | PASS (viewport 71.6–73.8% across frames) | PASS |
| Survives rapid resize | PASS (700/900/1240 px) | n/t |
| Input routing under overlap | PASS (viewport clicks logged) | PASS |
| DPI 100%↔200% monitor move | not covered (caveat) | not covered (caveat) |

Evidence: `evidence/crux-singlewindow.png` (first clean dGPU composite), `evidence/dgpu-composite-
motion.png` (rotated triangle + input-routing log), `evidence/igpu-composite.png` (Intel Iris Xe).

## Honest limits

Capture is automated swapchain-aware Desktop Duplication + direct frame inspection, not a human
watching an uninterrupted 60-second session; clean motion frames were sampled over ~10 s stretches
(window-minimize hygiene interrupted longer captures) — black stayed 0% and the viewport varied every
clean frame, so no flicker was seen. The **100%↔200% DPI monitor move** and a **truly low-end
machine** are the two cases not covered (the iGPU — the driver-sensitive case — was covered and passed).
