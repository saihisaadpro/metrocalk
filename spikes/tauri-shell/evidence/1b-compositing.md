# Sub-gate 1b — compositing (the Graphite-killer): result

**Environment (pinned):** Windows 11, WebView2 **149.0.4022.69**, Tauri 2.11.2 / wry 0.55.1,
wgpu 29.0.3 **Vulkan** backend (DX12 disabled — `windows`-crate diamond conflict with Tauri),
GPU **NVIDIA RTX 4060 Laptop**, surface `Bgra8Unorm`, `alpha_modes = [Opaque, PreMultiplied]`.

Architecture tested: the most direct reading of "transparent webview over a native wgpu viewport in
**one window**" — a wgpu surface created on the Tauri `WebviewWindow`'s HWND (`transparent: true`),
with the React UI as a transparent-region webview on top.

## What was measured (controlled A/B)

Automated GDI screen-capture (`CopyFromScreen`) of the live window, plus a control that runs the
**same** transparent webview with the wgpu layer disabled (`COMPOSITE_NOWGPU=1`).

| variant | window size | non-black pixels (sampled) | what the capture shows |
|---|---|---|---|
| **control — webview only, no wgpu** (`1b-control-nowgpu.png`, committed) | 1016×739 stable | **1806 / 5270 (~34%)** | React UI renders perfectly; **desktop icons show through the transparent regions** → `transparent:true` genuinely composites on Windows |
| **webview + wgpu surface on same HWND** | 1016×739 → **collapses to 16×16** | **0 / 5270 (0%)** at full size | entire window black — the webview chrome that was capturable in the control is gone; the window then destabilizes to a 16×16 rect |

(The with-wgpu captures are black rectangles / a collapsed 16×16 window — not committed as images; the
numeric readings above are the evidence. The committed `1b-control-nowgpu.png` is the informative one.)

## Reading

1. **Transparency itself works.** The control proves a transparent WebView2 composites over what is
   behind the window (desktop visible through the viewport). So Tauri v2 transparency is *not* the
   blocker on this build (contra some of the older issue reports).
2. **A native wgpu surface on the webview's HWND breaks it.** Adding the wgpu Vulkan swapchain to the
   same window turns the whole thing black: the previously-GDI-capturable webview chrome stops
   compositing. wgpu logs confirm it initialized and presented frames (`adapter=RTX 4060 Vulkan`,
   "first native frame presented"), so both layers are *live* — they **fight over the window
   surface** and the result is not a clean composite. This is exactly the documented failure:
   - Graphite [#2541](https://github.com/GraphiteEditor/Graphite/issues/2541) — "render the viewport beneath the Tauri webview" (Graphite's own, why they left Tauri).
   - Tauri [#9220](https://github.com/tauri-apps/tauri/issues/9220) — flicker with raw-window-handle + wgpu + Tauri v2 + transparency.
   - Tauri [#11944](https://github.com/tauri-apps/tauri/discussions/11944) — wgpu-as-webview-overlay; the transparent-hole-over-native-viewport approach is reported blocked by Windows API.

## Honest limits of this evidence

- **GDI cannot capture a GPU flip-model/overlay swapchain** — so the all-black *could* partly be a
  capture limitation. But the control rules out the simple version: the webview chrome is GDI-
  capturable on its own and *disappears* once wgpu attaches, which is a real change in behaviour, not
  just an un-capturable triangle. A **human looking at the live window** is still the gold-standard
  check the prompt asks for (does anything render; flicker under motion/resize; input routing) — the
  harness runs with one command for exactly that.
- Not tested: the Tauri `unstable` **child-webview split** variant, or DComp visual-layering, which
  *might* composite where the single-HWND surface does not. So this is a fail of the **direct**
  single-HWND approach, not proof that *no* Tauri composition can work.
- Input routing, flicker-under-motion, DPI/fullscreen: **not reachable** here. The prompt asks for
  before/after captures *under motion and resize*, but there is no coherent composite to put in
  motion — the static frame already blacks out / the window collapses the instant wgpu attaches. So
  the failure is at the *first* frame, before any motion/resize test. (The harness does poll
  `inner_size` and reconfigure the surface on resize, so a working composite *would* survive resize;
  it never got that far.) These remain for the human visual pass once a composite strategy renders.
- **Linux/X11: not tested** (this is a Windows machine; WebView2 is the Windows binding target). Per
  the prompt, Linux/X11 is expected to be the weakest compositing path and a Linux-only failure is a
  known-risk fast-follow, not a Tauri-killer. Since Windows itself already fails the direct approach,
  Linux does not change the decision; record it as untested + lower-priority for the self-composite
  path (which sidesteps OS compositor behaviour entirely).

## Verdict

**1b FAIL on the direct single-window approach** (native wgpu surface under a transparent WebView2 on
one HWND), empirically, on the current Windows/WebView2/Tauri/wgpu stack — matching Graphite's
experience. Transparency works; native-viewport-under-webview in one window does not. → drives the
**CEF self-composite** escape hatch (offscreen-render the UI to a texture and composite it in wgpu —
Graphite's proven path), unless a follow-up proves the child-webview/DComp-layering variant.
