# M2.1 spike — critical / adversarial audit

A self-review of the spike's methods and the threats that could overturn its conclusions. Written
against the prompt's verification discipline ("the case this passes on the dev box but stutters on
min-spec").

## 1a — IPC delta wire

**What's sound**
- Real encoding, not synthetic: payloads are Loro update bytes from `core::Engine` through the real
  commit pipeline (`export_updates_since`). Receive side decodes+integrates into a mirror CRDT.
- All three transports carry **raw binary** (`InvokeBody::Raw` / binary WS frames) — no JSON byte
  inflation. Verified: the `invoke` path sends a `Uint8Array`, not `Array.from(...)`.
- 3600 frames/path × **two** independent 60 s runs; results within noise (channel p99 3.55↔3.40,
  ws 1.70↔1.30, invoke 6.80↔6.70). Non-flaky.
- 60 Hz actually achieved: JS paths via `setTimeout` (Chromium raises the timer res), channel via a
  spin-wait after the first run exposed `tokio::time::sleep` only pacing 35 Hz on Windows.

**Threats to validity (honest)**
- **RTT, not one-way.** All latencies are round trips on a single clock (the only skew-free option).
  "End-to-end one-way ≈ RTT/2" is an estimate; paths may be asymmetric. I report RTT and say so.
- **Cross-path clocks differ.** invoke/ws are JS-clock RTT; the channel is Rust-clock RTT (it's a
  push). Within-path numbers are clean; cross-path comparison is indicative, not exact.
- **One delta shape.** The drag delta is **103 bytes** (2 fields). Real edits vary — a multi-entity
  bulk op or a large paste is bigger. The sweep covers 1 KB–1 MB separately, and even 1 MB stays at
  66 ms, but I did not drive a 60 Hz stream of *large* deltas. Claim is scoped to the interactive-
  drag case the gate names.
- **Idempotent receive.** The 600-delta ring repeats, so after cycle 1 the mirror `import` is largely
  dedup work, slightly under-counting novel-delta decode cost. Small; the IPC hop dominates anyway.
- **Dev box only.** RTX 4060 laptop, fast NVMe, warm caches. The streaming paths carry ~4–10× frame-
  budget headroom, so a 3–5× slower min-spec machine still fits — but that is **argued, not measured
  on min-spec**. A genuine min-spec run is the open follow-up.

**Would-overturn:** if a representative editor session streams deltas >> 103 bytes at 60 Hz, re-run
with that distribution; the invoke path's ~4 ms floor would bite sooner than the streaming paths.

## 1b — compositing

**What's sound**
- Controlled A/B isolates the cause: the *same* transparent webview renders perfectly **without**
  wgpu (desktop shows through → transparency works), and goes **all-black + window-collapse** the
  moment a wgpu surface attaches to the same HWND. That's a behaviour change, not just an
  un-capturable triangle.

**Threats to validity (honest)**
- **GDI can't capture GPU overlay swapchains.** `CopyFromScreen`/`PrintWindow` cannot read a Vulkan
  flip-model swapchain, so a *pure* "triangle invisible to GDI but fine to the eye" is not fully
  excluded. The control narrows it (the webview chrome itself disappears under wgpu), but the
  **gold-standard check is a human looking at the live window** — which the harness supports with one
  command (`GATE_MODE=composite`). I did not have eyes-on hardware confirmation in this run.
- **One composition strategy tested.** I tested the direct single-HWND surface. **Not** tested: the
  Tauri `unstable` child-webview split, or explicit DComp visual layering — either *might* composite
  where this fails. So this is a fail of the *direct* approach, not a proof that no Tauri path works.
- **Input routing / flicker-under-motion / DPI / fullscreen:** not exercised (they need the human
  visual pass). Recorded as open.

**Would-overturn:** a working child-webview/DComp-layering demo on this stack would downgrade the
finding from "single-window overlay fails" toward "Tauri viable with the unstable split."

## Process caveats
- Spike is a **throwaway**: default lints, `.unwrap()`/`.expect()` on the measurement path, no resize
  event wiring (the render loop polls `inner_size`). Appropriate for a gate, not for product code.
- wgpu **DX12 disabled** (Vulkan only) to dodge a `windows`-crate diamond conflict with Tauri. The
  product renderer is Vulkan-first anyway (ADR-003), but a DX12-only machine is untested here.
- 1a's window was opaque during the measured runs; `transparent:true` was added later for 1b and does
  not affect the 1a numbers.
