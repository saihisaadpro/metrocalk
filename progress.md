# Progress

## Now
- M0 spikes (2-week box) **ALL DONE**: ① Loro — ADOPT; ② Flecs — ADOPT; ③ wasm/WebGPU — browser-render leg PROVEN (CI tripwire added)
- M0 gate review next: reconcile the three spikes, settle the browser-ECS question

## Next
- **M0 gate review** (prompt 04): cross-spike reconciliation + frame-budget arithmetic; settle the browser-ECS path (flecs doesn't compile to wasm32 — decide Loro-backed query layer vs wasi/emscripten Flecs vs thin-client)
- Monorepo + ECS wrapper API + component metadata registry (M0–1) — wrapper must hide all `flecs_ecs` types, expose deferred mutation + safe query surface
- ECS↔Loro commit pipeline + merge-validation layer (M1–2) — validation-layer spec in `spikes/loro/README.md`
- M1 follow-ups: `DontFragment`/sparse for capability pairs (spike ②); engine-side inverse-op undo stack (spike ① F2); getrandom `js` for Loro-in-browser (spike ③)

## Done
- Feasibility plan v1 (stack assessment, hosting, browser-vs-desktop analysis)
- State-of-the-art research sweep (ECS/data, editor/UI, sync/plugins/AI/web — ~30 sources)
- Plan v2: locked stack, three subsystem merges (Loro, Extism, MCP)
- Documentation structure + first 3 ADRs + Opus 4.8 prompt set

---

## Log

*Append-only. Newest first. One entry per working session: date — what happened, decisions made (link ADR), blockers. Archive to `progress-YYYY.md` when this slows you down.*

### 2026-06-13 (M0 spike ③ — wasm32 + WebGPU)
- **Browser-render leg of ADR-003 PROVEN.** One wgpu 29.0.3 / winit 0.30 crate (`spikes/wasm`) renders a spinning triangle on **native** (Vulkan, RTX 4060: 8.3 ms median / 9.9 ms p99, ~120 fps) and in **Chrome 149 + Edge 149** via WebGPU. `crossOriginIsolated === true` under a COOP/COEP dev server; 512×512 render verified by pixel readback (118/119 distinct colors) + screenshots. Browser TTFF 0.4–0.8 s. Chose raw `wasm-bindgen` over trunk (transparent, measurable steps).
- **Sizes (funnel baseline):** raw cargo wasm 1361 KB → wasm-bindgen 378 KB → wasm-opt -Oz 335 KB → **brotli 118 KB**; +12 KB brotli JS glue = **~130 KB transfer** for a minimal wgpu triangle. (wasm-opt needs `--enable-reference-types --enable-bulk-memory …` for wasm-bindgen 0.2.125 output.)
- **Adapter diff (bindless flagged):** native Vulkan exposes all binding-array/non-uniform-indexing (bindless) features + huge limits (1 TB buffers, 8 bind groups); **WebGPU exposes none** (2 GB buffers, 4 bind groups, 16 storage buffers/stage) → non-bindless web path mandatory.
- **Critical finding (flagged vs ADR-001):** `flecs_ecs` 0.2.2 **does not build for wasm32-unknown-unknown** (C core needs clang + a wasm libc/sysroot; verbatim `cc-rs: failed to find tool "clang"`). `loro` 1.13.1 **does** build (needs getrandom `js` at runtime). → browser lite-editor can't run Flecs client-side as-is; resolve in M0 gate review / M1. Desktop unaffected.
- CI tripwire `.github/workflows/wasm-tripwire.yml` (builds wasm32 every push, `Swatinem/rust-cache`). **Verified on `github.com/saihisaadpro/metrocalk` (public): green in 54 s cold; a deliberate wasm32 break failed at the build step; branch reverted.** Repo pushed to `main` this session (had no remote before; user provided it). Toolchain note: this env had no `rustup` and no wasm32 std; installed the official `rust-std` wasm32 component + `wasm-bindgen-cli` 0.2.125 + binaryen 130 manually.

### 2026-06-13 (M0 spike ② — Flecs)
- **ADR-001 query/binding spike PASSED → ADOPT Flecs v4.1.2 via `flecs_ecs` 0.2.2** (behind the wrapper, safety locks ON). Built `spikes/flecs` (throwaway): seeded 5k/20k scene with `(Provides,cap)` + `(BindsTo,target)` pairs and role tags; 5 benchmarks + churn-correctness + a criterion cross-check. Two runs each for safety ON and OFF, all structurally identical (matched 211/5k, 830/20k; 1,999 edges).
- Latency table (cached compatibility query `(Provides,Health)` without `(BindsTo,*)`, safety ON): **@5k 8.7 µs median / 12.2 µs p99**; **@20k 39.8 µs median / 58.3 µs p99**; **under 100-mutation churn 25/41 µs**; wildcard traversal of all edges 130/162 µs; uncached @20k 0.9 ms. All ≪16 ms (≥275× margin). Churn: **zero stale results**. Criterion cross-check: ~15 µs mean (mean-vs-median skew vs the 9 µs hand-rolled median). Safety-lock ON↔OFF delta 0–10% (noise) → ship ON.
- Finding F1 (not a blocker): capabilities-as-pairs fragments archetypes → ~14.8 KB/entity at 20k (~1 table/entity); query latency unaffected. M1: mark relationships `DontFragment`/sparse or model capability sets as data — validate then.
- Binding assessment: ~1,180 unsafe sites (FFI + iteration pointer deref); `flecs_safety_locks` = per-(id,table) read/write counter = runtime borrow-check, contains the aliasing landmine at ~0–10% cost; deferred mode makes mutation-during-iteration safe. 1-maintainer 0.x crate → adopt only behind the wrapper (ADR-001's condition). API sharp edge: `with`/`without` take values, wildcard via `id::<flecs::Wildcard>()`.

### 2026-06-13 (M0 spike ① — Loro)
- **ADR-002 M0 gate PASSED → ADOPT Loro 1.13.1 as the document layer.** Built `spikes/loro` (throwaway): 5k-entity / 2k-edge synthetic scene in a MovableTree + nested component maps + binding-edge map, seeded SplitMix64 (`0x4D4554524F434131`), 5 benchmarks + a doc-only invalid-state validator. Two full runs, structurally identical.
- Headline numbers (5k scene): 10k-mutation run **0.56 s** (≈18k mut/s, median 13 µs/op); single-op undo of latest **70 µs / 0.13 ms p99**; full snapshot **4.51 MB** (shallow at 100k history collapses to **218 KB**); bidirectional merge **converged**; checkout 5k-ops-back ~90–215 ms. All four adopt criteria met (undo p99 <5 ms, run <10 s, snapshot <20 MB, all invalid states detectable+repairable).
- Three constraints for M1 (documented in ADR-002 status note + spike README): **F1** `ensure_mergeable_*` breaks under undo/redo → use regular containers + merge-validation layer; **F2** undo uses full-doc checkouts → keep transaction groups small (latest-op undo fast, bulk undo tens of ms); **F3** entity IDs must be peer-namespaced (concurrent create collided on eids → 23 dup-eid violations).
- Merge failure-mode catalog written (8 classes; observed: dangling edges 17, orphan records 17, dup-eid 23, missing-record 1; **no cycles**, **no asset-ref corruption** under adversarial reparents/edits) — this is the M1 merge-validation spec. Detour: a fragile children-by-meta scan for created TreeIDs desynced the shadow model; fixed by capturing `tree.create`'s returned TreeID directly.

### 2026-06-12 (start-readiness)
- Rules layer (When/If/Then + state machines as data, registry-fed builder, explainability) recorded in plan §2 and architecture.md — was chat-only knowledge, now session-readable.
- Prompts 04 (M0 gate review — cross-spike reconciliation, frame-budget arithmetic, ADR settlement) and 05 (M1 foundation — monorepo, wrapper API, registry, 16ms CI gate) created. Prompt set now covers M0 spikes → gate review → first build session.

### 2026-06-12 (ops decision)
- Hosting strategy flipped to self-hosted-first (ADR-005): one Hetzner AX42 (~€63–75/mo flat) runs site, CI runner, GlitchTip, PostHog hobby, Postgres, backups; Cloudflare free in front. Identity & money gated: Stripe always managed; self-hosted auth only after the ops-discipline gate. hosting.md rewritten (v2), plan §6 marked superseded.
- Prompts 00–03 enhanced (v2): vision-v1 framing, Phase guard, benchmark discipline (seeded RNG, env blocks, serial runs), blocker timebox, git hygiene, Definition-of-Done checklists.

### 2026-06-12 (vision v1)
- First concrete product vision locked (ADR-004): free engine forever, indie-first; revenue = token economy ($10 ≈ 100 tokens; generate ~10, marketplace 2–4 with 70/30 split, LLM-edit 1–2; buy+edit always < regenerate).
- Flagship feature defined: describe-to-create with resolution order local → marketplace → generate; text-to-3D via wrapped providers; LLM editing of existing assets; placeholder-then-swap UX.
- Docs updated: metrocalk.md (vision), plan §1.5 (product & revenue), architecture.md (asset pipeline + generation rows), hosting.md (Stripe Connect, generation COGS).

### 2026-06-12 (later)
- Hosting researched and decided (`hosting.md`): Cloudflare DNS+R2, Vercel, Supabase, Sentry, PostHog, GitHub Actions now; Stripe/Fly.io/Hetzner deferred. Phase 1 ≈ $46–120/mo, Phase 2 ≈ $250–600/mo. 9 setup tasks listed.

### 2026-06-12
- Plan v2 finalized after research sweep: Flecs over Bevy ECS (ADR-001), Loro replaces custom WAL (ADR-002), desktop-first with Tauri exit gate (ADR-003), Extism over Lua, MCP for AI layer.
- License traps documented: SpacetimeDB BSL, dead deps (CozoDB, KuzuDB, litegraph.js).
- Doc structure created: metrocalk.md / architecture.md / progress.md / decisions/ / prompts/.
- First 4 Opus 4.8 prompts written (orchestrator + 3 spikes).
- Next session: run the three M0 spikes.
