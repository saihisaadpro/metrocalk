# ADR-008: Editor shell composition = single-window (transparent WebView2 over native wgpu)

**Date:** 2026-06-14 · **Status:** Accepted — M2.3 shell-composition spike (`spikes/shell-composite`,
throwaway) · **Builds on / corrects:** [ADR-007](007-m2.1-tauri-gate-result.md) (its 1b "compositing
FAIL" is reconciled below as a capture artifact) and [ADR-003](003-desktop-first-tauri-exit-gate.md).

## Context

ADR-007's sub-gate 1b concluded that a native wgpu surface under a transparent WebView2 on **one
HWND** blacks out on Windows — pointing the shell at a costly DComp visual-layering or CEF
self-composite (~170 MB) path. But that read came from **GDI `CopyFromScreen`, which cannot capture a
GPU flip-model / overlay swapchain** — ADR-007 flagged this itself and reserved a human visual pass as
the gold standard. A later human look saw the triangle render and drag responsively (no blackout).
M2.3 had to confirm or refute single-window rigorously, with **swapchain-aware capture**, before
paying for any fallback.

## Method (holding to the higher bar the prompt demanded)

- **Swapchain-aware capture, not GDI:** a DXGI **Desktop Duplication** tool (`spikes/shell-composite/
  capture`) grabs the **DWM's final composited output** (hardware overlays included) — exactly what
  GDI BitBlt misses. Plus direct visual inspection of the saved frames.
- **Real React panel layout** (not a bare triangle): opaque toolbar + opaque inspector side-panel + a
  transparent viewport + a **semi-transparent floating box that overlaps the viewport** + a live
  input-routing log.
- **Animated viewport** (rotating wgpu triangle) for the flicker-under-motion test.
- **Two GPUs** (M2.1's blackout was driver-sensitive): discrete **NVIDIA RTX 4060** (`PowerPreference
  ::HighPerformance`) and integrated **Intel Iris Xe** (`SHELL_GPU=low` → `LowPower`).
- **Path-agnostic input-routing layer** built first (`spikes/shell-composite/input-routing`,
  dependency-free, 7 unit tests) — per-pixel UI-vs-viewport hit test that carries to any shell.

Env (pinned): Windows 11 10.0.26200 · i9-13900H · WebView2 149.0.4022.69 · Tauri 2.11.2 / wry 0.55.1
· wgpu 29.0.3 Vulkan · surface `Bgra8Unorm`, `alpha_modes=[Opaque, PreMultiplied]`.

## Battery results (single-window, Desktop-Duplication + visual)

| gate | dGPU (RTX 4060) | iGPU (Iris Xe) | evidence |
|---|---|---|---|
| Real UI composited (panels over viewport) | **PASS** | **PASS** | black 0% · distinct 64–112 · UI + viewport both present; `evidence/*.png` |
| Transparent regions show viewport through | **PASS** | **PASS** | cyan triangle visible through the transparent centre **and** the semi-transparent floating box (visual) |
| Correct z-order | **PASS** | **PASS** | opaque chrome over the wgpu layer (visual) |
| No blackout / no 16×16 collapse | **PASS** | **PASS** | window stable 1016×739 over 12 s; black 0% (M2.1's collapse did **not** reproduce) |
| No flicker under motion | **PASS** | **PASS** | rotating triangle: viewport-clear varies 71.6–73.8% across consecutive frames, black stays 0% |
| Survives rapid resize | **PASS** | n/t | black 0%, composite present at 700/900/1240-px widths |
| Correct input routing (overlap) | **PASS** | **PASS** | viewport clicks logged at correct coords with the floating box overlapping (visual) |
| DPI 100%↔200% monitor move | **not covered** | **not covered** | not scriptable non-disruptively; physical-pixel resize (same surface-reconfigure path) passed — see boundaries |

## Decision

**Single-window is the shell.** Transparent WebView2 over a native wgpu surface on one HWND composites
correctly on Windows on **both** a discrete and an integrated GPU, with the real panel layout, under
motion, resize, and overlapping input routing. **ADR-007's 1b "FAIL" was a GDI capture artifact** —
Desktop Duplication shows a clean composite where GDI saw black, and the 16×16 collapse never
recurred. **No DComp visual-layering and no CEF self-composite are needed** (~170 MB and HAL-level
DX12-from-visual avoided). The fallback ladder was therefore **not exercised** (correctly — the rule
was to disprove single-window first).

This keeps ADR-003's plain-Tauri shell: IPC passes (ADR-007 1a), transparency works, and now the
viewport↔UI composition works in one window — the renderer does **not** need to own UI compositing.

## Consequences

- The editor shell (M2.6) is plain single-window Tauri: React UI as a transparent-region WebView2 over
  the wgpu viewport on the main HWND. No CEF dependency, no DComp visual tree.
- The input-routing layer (`shell-input-routing`) is the per-pixel UI/viewport split; it carries into
  M2.6 unchanged.
- ADR-007's compositing leg is superseded: its "self-composite" consequence is withdrawn. (ADR-007
  lives on the unmerged `m2.1` branch; apply its status-line note — "1b was a GDI capture artifact,
  see ADR-008" — at the m2.1↔m2.3 merge.)

## Revisit when

- A **high-DPI multi-monitor 100%↔200% move** or a genuinely **low-end** machine shows tearing/desync
  the dev box + iGPU didn't (the one battery gate not covered here).
- WebView2 / wry / Tauri major upgrades change the overlay path.

## Honest boundaries

Capture was automated **swapchain-aware Desktop Duplication + direct frame inspection**, not a human
sitting through an uninterrupted 60-second session; clean motion frames were sampled over ~10 s
stretches (window-minimize hygiene interrupted longer runs) — black stayed 0% and the viewport varied
across every clean frame, so no flicker was seen, but a continuous-60 s human watch is still the
ideal. The iGPU (the driver-sensitive case M2.1 feared) **passed**. The 100%↔200% DPI monitor move and
a truly low-end machine remain untested — explicitly the two adversarial cases to hold open.
