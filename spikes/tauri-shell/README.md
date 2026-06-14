# spikes/tauri-shell — M2.1 Tauri/WebView2 exit-gate (ADR-003)

Throwaway spike. Resolves ADR-003's M2 exit gate on **Windows WebView2** with two co-equal
sub-gates, and produces the **Tauri-go vs CEF** decision on measured evidence.

- **1a — IPC delta wire.** Can the real coalesced-delta encoding ship across the WebView2 IPC at a
  sustained 60 Hz inside frame budget? Three transports: Tauri **Channel** (Rust→JS push),
  **`invoke` raw-binary** (JS→Rust, `InvokeBody::Raw` — *not* JSON), and a **local WebSocket** (the
  CEF-style alternative). The wire carries real Loro update bytes from `metrocalk-core`'s commit
  pipeline (`Engine::export_updates_since`), not a synthetic blob.
- **1b — compositing (the Graphite-killer).** Render a native **wgpu** surface (a triangle) and float
  the React UI as a **transparent-region webview composited over it** in one window (Tauri v2
  `unstable` multi-webview). The risk Graphite actually left Tauri over: the OS compositor not
  showing the native layer through the transparent webview.

> **Research correction (in the prompt):** ADR-003's "~200 ms / 10 MB" is a *bandwidth* figure
> (~50 MB/s), not a per-frame budget. Metrocalk's wire is small coalesced deltas, so 1a is expected
> to pass; 1b is the real gate.

## Prerequisites (Windows)

- Rust stable (MSVC), Node ≥ 20, pnpm. WebView2 Evergreen runtime (record the version — see below).
- `pnpm install` once.

## Run

```powershell
# one-time
pnpm install

# Sub-gate 1a — IPC bench. Runs 60 s/path × 3 transports + a 1 KB→1 MB sweep, writes
#   _gate_out/1a-<run>.json, then exits. Run twice (RUN=run-1, then RUN=run-2).
$env:GATE_MODE="bench"; $env:RUN="run-1"; pnpm tauri dev

# Sub-gate 1b — compositing. Opens the transparent-webview-over-wgpu window for visual inspection.
$env:GATE_MODE="composite"; pnpm tauri dev

# Default (no GATE_MODE): selftest — proves the JS↔Rust roundtrip + prints the WebView2 version.
pnpm tauri dev
```

`_gate_out/` holds the machine-readable results and is the evidence source for the ADR.

## Pass criteria (from the prompt)

- **1a PASS:** sustained 60 Hz, **p99 end-to-end ≤ ~4 ms**, **p99 IPC budget ≤ ~5 ms** (≤ ⅓ of
  16.6 ms), **zero dropped frames** over 60 s with the real encoding; per-frame payload well under
  the measured Windows ceiling.
- **1b PASS:** flicker-free, correctly composited (scene shows through transparent regions), correct
  input routing on Windows for a sustained session.
- **Decision:** Tauri-go iff **both** pass on Windows; else CEF (the compositing escape hatch:
  offscreen-render the UI to a texture + self-composite — Graphite's proven path).

## What is and isn't automated

1a is measured end-to-end by the harness and emits numbers. 1b's harness builds and runs, but the
flicker / compositing / input-routing **verdict is a human visual judgment** (see the prompt's
verification note) — the harness captures evidence; a person rules PASS/FAIL on Windows + min-spec.

## Environment pinning (fill at run time)

- WebView2 runtime: `149.0.4022.69` (dev box; record per run)
- Hardware / GPU: _record_
- OS: Windows 11
