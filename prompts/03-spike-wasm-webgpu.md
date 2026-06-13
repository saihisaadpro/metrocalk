# Prompt 03 — M0 Spike ③: wasm32 + WebGPU browser target

> Use with `00-orchestrator.md` (v2) as system prompt. Effort `xhigh`. Throwaway spike in `/spikes/wasm` — the deliverable is a working pipeline and a constraints document, not a product. Benchmark discipline applies to every number.

---

<task>
Prove the browser leg of ADR-003 (`decisions/003-desktop-first-tauri-exit-gate.md`): that our Rust core toolchain compiles to `wasm32-unknown-unknown` and renders via WebGPU in a browser today, and produce the CI job that keeps it true forever. The browser build is the future adoption funnel (free lite editor, shareable project links — ADR-004), so this tripwire protects business value, not just engineering convenience: web-incompatible decisions must be caught the week they're made.
</task>

<scope>
In scope: a minimal Rust + wgpu app (spinning triangle + frame-time overlay), compiled natively AND to wasm32, served locally with correct COOP/COEP headers, plus a GitHub Actions workflow and a constraints write-up.
Explicitly out of scope: integrating Flecs or Loro into the WASM build (compile-compatibility check only — do they *build* for wasm32?), any real renderer architecture, Tauri, asset pipeline, threads/SharedArrayBuffer usage (we only verify the headers that enable them later).
Use latest stable `wgpu` (29.x as of June 2026); record exact versions.
</scope>

<deliverables>
1. `/spikes/wasm` crate: one codebase, two targets (native window + browser canvas). Pick `trunk` or raw `wasm-bindgen` — one sentence justifying the choice. Apply `wasm-opt` to the release artifact.
2. Local dev server with COOP/COEP headers set; verify `crossOriginIsolated === true` in the browser console and capture the evidence (screenshot or logged output).
3. `.github/workflows/wasm-tripwire.yml`: builds the wasm32 target on every push, fails loudly, runs <5 min via caching (`Swatinem/rust-cache` or sccache).
4. `spikes/wasm/CONSTRAINTS.md` — the operative document: WASM binary size (raw + after wasm-opt + brotli) · time-to-first-frame browser vs native · steady-state frame time both targets · adapter limits/features dump for native vs WebGPU backend on this machine, flagging bindless and any limit we'd plausibly hit · build-only check: do `flecs_ecs` and `loro` compile for wasm32? (errors quoted verbatim if not) · browsers + versions tested (Chrome required, at least one of Firefox/Safari beside it).
</deliverables>

<success_criteria>
Pass if: the same crate renders on native and in ≥2 browsers · CI green and <5 min, verified by an actual triggered run, not by reading the YAML · CONSTRAINTS.md complete with real numbers. Binary size has no hard gate — record it; it becomes the baseline the funnel's load time is measured against. There is no fallback path: if something fails, the failure analysis IS the deliverable, because it changes the Phase 2 browser timeline and the ADR-004 funnel assumptions.
</success_criteria>

<verification>
Clean-clone into a fresh directory and follow your own README start to finish; any step that fails or needs undocumented knowledge → fix docs, repeat until clean. The CI run must be observed succeeding once and failing once (introduce a deliberate break, confirm it trips, revert).
</verification>

<definition_of_done>
☐ native + browser render from one crate · ☐ `crossOriginIsolated === true` evidenced · ☐ CI verified green AND verified to fail on breakage · ☐ CONSTRAINTS.md complete (sizes, frame times, adapter diff, flecs/loro wasm32 build check, browser matrix) · ☐ clean-clone test passed · ☐ progress.md log entry with size + frame numbers · ☐ architecture.md gains its "Browser target: CI-enforced" line · ☐ ADR-003 status notes the browser leg result; flecs/loro wasm32 failures flagged against ADR-001/002 revisit clauses · ☐ working tree clean, committed.
</definition_of_done>
