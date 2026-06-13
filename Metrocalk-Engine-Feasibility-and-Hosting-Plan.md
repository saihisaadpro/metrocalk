# Metrocalk Engine — The Plan

**v2.1 · 12 June 2026** · Supersedes v1. Based on the 5 docs in Drive › "Project 5 - metrocalk.com" + a fresh state-of-the-art sweep (June 2026: repos, releases, licenses verified — sources in §9). v2.1 adds §1.5 Product & revenue model (ADR-004).

---

## 1. The decision

Build the intent-driven engine as **one Rust core with two delivery targets — desktop-first (native), browser as a designed-in second target** — and replace three planned hand-built subsystems with mature 2026 tech that didn't exist (or wasn't ready) when your docs were written:

1. **Loro (CRDT) replaces the custom WAL/ARIES undo system** — and brings collaboration, time-travel, and persistence in the same dependency.
2. **WASM plugins (Extism) replace Lua** — scripting, modding, and sandboxing merge into one layer whose plugin binaries run identically on desktop and in the browser.
3. **MCP server + schema-constrained JSON Patch replaces a bespoke AI integration** — every AI agent (Claude, ChatGPT, Copilot…) can edit scenes through the same transactional pipeline as humans, for near-zero extra cost.

These three merges remove roughly 40% of the custom infrastructure in the original blueprint and are the highest-leverage outcome of the research.

---

## 1.5 Product & revenue model (ADR-004)

**Audience: indies.** Solo creators and tiny teams; 2D/2.5D/3D from one engine. **The engine is free forever** — desktop download (signed installer, auto-update) and browser lite editor as the zero-install funnel. The engine runs fully offline; hosting only serves the site, downloads, updates, and optional online features.

**Flagship feature — describe-to-create.** Type what you want on a selected entity ("companion", "rusty medieval sword"). Resolution order, enforced by pricing: ① local registry (deterministic fuzzy/alias search + compatibility queries — offline, milliseconds, no LLM), ② marketplace index (metadata synced locally like a package manager — searchable offline, download needs network), ③ AI generation (text-to-3D via providers wrapped behind our API; placeholder drops into the scene immediately, generated mesh streams in and lands through the standard import pipeline, arriving pre-componentized). LLM editing of existing assets (retexture "make it rustier", variations, refine) makes buy+edit beat regeneration. Optional small local embedding model gives semantic search offline; the full LLM tier (composition from sentences, code-gen for new components) goes through MCP + schema-validated JSON Patches, never load-bearing.

**Revenue — token economy:** new accounts get a few free generations; $10 ≈ 100 tokens. Generate ≈ 10 tokens · marketplace asset ≈ 2–4 (creator ~70% / platform ~30%) · LLM edit ≈ 1–2. Invariant: **buy + edit < regenerate** — curated supply wins, per-user GPU cost shrinks as the marketplace grows, platform earns on both paths. Marketplace needs perceptual dedup, ratings, and quality-ranked search from day one. Creator cash-out via Stripe Connect only after legal review. Generation launches scoped to props/environment (rigged characters when the tech matures).

**Asset reality:** users bring content from anywhere — Blender exports, Sketchfab/Fab/Poly Haven/itch downloads, Mixamo characters. Import pipeline converts everything (FBX→glTF, textures→KTX2, LODs, colliders, humanoid rig detection + retargeting), then the metadata system offers components ("Humanoid detected — add CharacterController?"). File-to-playable in under five minutes is the demo that sells the engine.

All of §1.5 is Phase 2 build scope. The M0–M6 vertical slice (§7) is unchanged — it proves the deterministic core these features stand on.

---

## 2. The locked stack

| Layer | Decision | Why (June 2026 state) |
|---|---|---|
| Language | **Rust** | Perf + safety + the only credible native↔WASM dual-target story |
| Semantic ECS | **Flecs v4.1 core via `flecs_ecs` (MIT)**, wrapped behind our own query API | The only shipping ECS with first-class many-to-many relationships, wildcard queries (`(Provides, *)` = "find all providers of Health"), transitive traits, reflection, JSON ser. Bevy relationships are still one-to-many only (many-to-many open issue #18121). Our product *is* relationship queries — use the engine that has them today. **[M0 gate 2026-06-13: ADOPT confirmed — 12–58 µs p99 queries. Native-only for wasm (C core won't build for `wasm32-unknown-unknown`); the browser uses a pure-Rust query backend over Loro — ADR-006.]** |
| ECS fallback | `bevy_ecs` 0.19 standalone | If the `flecs_ecs` binding (0.x, single maintainer) fails the M1 gate. The wrapper API makes the swap survivable |
| Scene format | Own format, **BSN-compatible where cheap** + **BRP interop** | Bevy 0.19 (landing now) ships BSN; speaking it lets us ride the Bevy ecosystem without adopting Bevy |
| Document/undo/collab/persistence | **Loro 1.13 (MIT)** | UndoManager (collab-aware), time-travel checkout, git-like fork/merge, snapshot+oplog export, and a **MovableTree CRDT** that solves concurrent scene-graph reparenting — the hardest data problem on the roadmap. Its oplog *is* the WAL we were going to write |
| Rendering | **wgpu 29 + WGSL** | Native Vulkan/Metal/DX12 + WebGPU. Constraint: **bindless is native-only** — web render path must be non-bindless from day 1 |
| Editor UI | **React + TypeScript + Zustand + React Flow 12 (`@xyflow/react`)** | Talent pool, ecosystem, and 1:1 browser parity. Viewport + all hot interactions (gizmos, drag feedback) render Rust-side; UI gets optimistic local echo with coalesced commits |
| Desktop shell | **Tauri 2.11** — with a hard M2 exit gate | Channels + raw binary IPC are adequate *if* the wire carries only deltas. Known floor: ~200 ms/10 MB on Windows WebView2. Gate: worst-case payload benchmark on Windows at M2; fallback is a CEF wrapper (the exact retreat Graphite made) — same web UI, different shell, no rewrite |
| Transport | **One protocol trait, three impls**: in-process WASM call (browser) / Tauri channels (desktop) / WebSocket (collab + remote) | This single rule keeps browser, desktop, and collaboration the same codebase |
| Game logic layer | **Rules as data: When (event) / If (conditions) / Then (actions) + state-machine components** | Conditions/actions assembled from the metadata registry (no free-form logic, no typos); visual state graphs via the same React Flow layer; live explainability ("✅ state = FacingBoss, ❌ KillCounter 3/4"). Rules orchestrate; algorithmic behavior (boss AI) stays in code components (WASM plugins). Post-slice scope (M6+), but the registry and transaction pipeline it rides on are slice work |
| Plugins/scripting/modding | **Extism 1.30 (BSD-3) WASM plugins** | Sandboxed, 15+ guest languages, official browser JS SDK → same plugin binary everywhere. Proven pattern (Zed, Zellij, Lapce, Shopify). Revisit WIT/Component-Model (WASI 0.3 shipped Feb 2026, Wasmtime 45) when WASI 1.0 lands. Lua only if frame-hot gameplay scripting ever demands it |
| AI layer | **Engine = MCP server**; LLM output = RFC 6902 JSON Patch enforced by structured outputs | MCP is the standard (native in Claude/OpenAI/Google/Microsoft; Unity/Unreal/Godot/Blender MCP servers already prove the pattern). All major APIs do ~99.9% schema-constrained generation. Every AI edit lands as an undoable Loro commit |
| Physics / audio / netcode | Rapier / kira / lightyear 0.26 | All deferred. Not vertical-slice scope. lightyear confirms server-authoritative + prediction is solved in the Rust ecosystem — deterministic lockstep stays an R&D track, not a requirement |
| Assets | glTF 2.0 + KTX2/Basis; pipeline **server-side from day 1** | Mandatory for browser anyway; gives desktop users cloud builds for free |

**Licensing:** everything above is MIT/Apache-2.0/BSD — clean for a commercial engine. Two explicit avoid-list entries from the research: **SpacetimeDB** (BSL 1.1 — one production instance, forbids services where third parties control schemas: nearly a description of an engine project store; use it as architectural inspiration only) and dead/archived deps (**CozoDB** dormant since 2023, **KuzuDB** archived Oct 2025, **litegraph.js** archived Aug 2025).

---

## 3. Architecture rules (non-negotiable)

1. **One source of truth.** Authoritative state lives in the ECS; the Loro document is the durable, mergeable mirror of it; the UI holds projections only. No second graph store (your own docs already killed petgraph — correct).
2. **Deltas only on every boundary.** ECS↔Loro, core↔UI, client↔server: revision-counter + coalesced invalidation + binary/JSON-Patch deltas. Never full-state snapshots over a wire.
3. **Everything is a transaction.** Human click, plugin call, AI patch — all enter through the same commit pipeline, all undoable. CRDTs guarantee convergence, not semantic validity, so a **merge-validation layer** re-checks ECS invariants after every merge — this is the one hard piece Loro does *not* give us, and it's where we spend the saved WAL effort.
4. **Hot path never crosses the JS boundary.** Viewport, gizmos, scrubbing render in Rust/wgpu; JS gets optimistic echo + final commit.
5. **Wrap every pre-1.0 dependency** (`flecs_ecs`, Loro, Tauri, wgpu, GPUI-watch) behind our own trait. Swaps must be survivable.

---

## 4. Browser strategy (answering the hosted-engine question)

Browser-hosted engines of this complexity are now proven — Figma (C++→WASM), PlayCanvas (editor frontend MIT-open-sourced July 2025), Rerun (same Rust viewer native **and** WASM), Graphite (Rust core→WASM, web UI). We follow the same trajectory:

- **Phase 1 (M0–6):** desktop vertical slice. Browser exists only as a CI target — `wasm32` build must compile and render the stress scene from M2 onward, so web-incompatible decisions are caught the week they're made.
- **Phase 2:** browser **lite editor / shared-project viewer** as the adoption funnel (open a link, inspect bindings, tweak, run). wasm32 only (Memory64 still absent in Safari, perf-penalized elsewhere; 4 GB ceiling). Non-bindless render path. COOP/COEP headers required for threads — hosting must control headers (Vercel/Cloudflare do; GitHub Pages doesn't). **[M0 gate 2026-06-13: `loro`+`wgpu` reach wasm; `flecs_ecs` does NOT. Browser runs Loro (source of truth) + a pure-Rust query backend + wgpu — no Flecs client-side (ADR-006). Funnel intact; new Phase-2 scope = build+benchmark the pure-Rust backend. Funnel transfer baseline ≈ 130 KB brotli for a bare wgpu app. `crossOriginIsolated` verified under COOP/COEP.]**
- **Phase 3:** full collab authoring in browser when limits ease; the Loro + WebSocket transport already built makes this incremental, not a project.
- **Rejected:** cloud-streamed editor (GPU-per-user economics + latency kill a "vibe" product).

---

## 5. What to be careful with

1. **Scope.** Months 0–6 = binding UX + transactions + queries + minimal render. No physics, audio, netcode, MOBA, or AI features. (The AI *seam* — MCP tool schema — is designed, not built.)
2. **`flecs_ecs` binding risk.** C core is AAA-battle-tested; the Rust binding is 0.x with effectively one maintainer. Mitigation: M1 gate + wrapper API + budget to contribute upstream or vendor the binding. We're funded — sponsoring the maintainer is cheap insurance.
3. **Loro merge semantics.** Undo/time-travel needs retained history (size cost); merged states can violate engine invariants. The merge-validation layer (rule 3) is vertical-slice scope, not later.
4. **Windows WebView2 is the IPC floor.** Benchmark on Windows, not macOS — the 40x asymmetry (~5 ms vs ~200 ms per 10 MB) is exactly the trap that bit Graphite.
5. **Query latency is the product.** <16 ms compatibility queries on a 5k-entity stress scene, benchmarked in CI from month 1. Flecs cached queries + wildcard pairs are the tool; ranking heuristics (proximity/recency/affinity) + explainability ("why is this greyed out?") ship in the MVP, or the one-click promise dies in real projects.
6. **Bevy alignment, not adoption.** Track 0.19/BSN/BRP and the community Jackdaw editor (closest prior art — study its BSN write-back). There will be no first-party Bevy editor for years; that's the market gap we're filling.
7. **Schema evolution from v0.1.** Versioned headers + migration pipeline on every file format. Loro helps (op-log history) but doesn't absolve component-schema migrations.
8. **Hiring/bus factor.** ≥2 people on the ECS↔Loro↔sync core. Rust gamedev talent is scarce; the React UI side hires easily — staff accordingly.

---

## 6. Hosting & infrastructure

> **Superseded by ADR-005 / hosting.md v2 (self-hosted first, DevOps by the team).** The table below is kept as the managed-services exit path.

| Need | Decision |
|---|---|
| Site + docs | **Vercel** (connected) — supports the COOP/COEP headers the browser build needs |
| Binary/asset CDN | **Cloudflare R2** — zero egress on multi-GB downloads |
| Auth, projects DB, cloud storage | **Supabase** (connected) |
| Licensing/payments | **Stripe** (connected) — editor seats + cloud features |
| Crash + telemetry | **Sentry** (Rust + browser SDKs) + **PostHog** (time-to-bind, funnel drop-offs — the KPI is UX) |
| CI | GitHub Actions + sccache; wasm32 + Windows benchmark jobs mandatory; self-hosted runners when build times hurt |
| Collab sync (Phase 2) | Rust WebSocket service on **Fly.io** relaying Loro updates — the same delta protocol as the editor bridge |
| Asset pipeline / cloud builds (Phase 2) | Queue + autoscaling workers (Fly Machines or AWS Batch) |
| Game servers (only if a MOBA returns to scope) | Hathora or Agones — not engine-product scope |

Cost: ≈ £100–200/mo through the slice; £500–2k/mo in Phase 2. Immaterial vs payroll.

---

## 7. Roadmap (months 0–6) and gates

**Slice (unchanged from the validation doc):** create character with Health → create HealthBar → click → ranked compatible targets → one click binds → live update on Health change → undo → reload preserves state.

| Phase | Work | Gate |
|---|---|---|
| **M0 (2 wks)** | Three parallel spikes: ① Loro — model the scene doc, benchmark undo + file size; ② `flecs_ecs` — wildcard pair queries on 5k-entity scene; ③ wasm32 — core compiles, renders a triangle via WebGPU | ✅ **DONE 2026-06-13** — ① ADOPT, ② ADOPT, ③ browser-render PROVEN + CI tripwire live. Gate review: `M0-gate-review.md`. New decision: ADR-006 (browser query backend). Surfaced for later phases: Flecs won't build to wasm (browser uses pure-Rust queries); real-scene render cost still unmeasured (M2) |
| **M0–1** | Monorepo, ECS wrapper API, component metadata registry (JSON Schema), stress scene | Compatibility query <16 ms |
| **M1–2** | ECS↔Loro commit pipeline, merge-validation layer, transactions | 100% undo/redo property tests incl. entity resurrection; **flecs_ecs go/no-go** |
| **M2–3** | Tauri shell, transport trait (3 impls stubbed), binary delta protocol | 60 Hz drag, zero stutter, **benchmarked on Windows WebView2**; Tauri go / CEF fallback decision; wasm32 target green in CI |
| **M3–5** | Binding UX: context reveal, ranked suggestions (proximity/recency/affinity), explainability, schema-driven inspector | Bind in ≤2 interactions; every incompatible target shows "why not" |
| **M5–6** | Polish, Sentry/PostHog wiring, MCP tool-schema design doc, external test (5–10 devs) | Users complete the flow unaided; time-to-bind measured |

**M6 go/no-go:** if outside users don't describe the binding UX as obviously better than Unity/Unreal inspector workflows, fix UX before adding any engine breadth.

---

## 8. Steal/study list (verified active, license-compatible)

- **Rerun** (MIT/Apache) — Arrow chunk-store architecture, `egui_tiles` docking, the standing proof of one Rust core → native + browser.
- **PlayCanvas Editor frontend** (MIT, open-sourced 2025) — the best shipping browser-game-editor codebase to mine.
- **Jackdaw + BSN write-back** (Bevy community) — closest prior art to our transactional editor-document model.
- **BitCraftPublic** (Clockwork Labs, Jan 2026) — readable production reference for WAL + subscription-replication architecture (reference only; BSL license).
- **Zed's DeltaDB** — operation-based CRDT version control, open-sourcing promised; the watch-item for Phase 3 collab versioning.
- **Unity/Unreal/Godot/Blender MCP servers** — tool-surface design for ours.

---

## 9. Sources

**ECS/data:** [Bevy 0.18](https://bevy.org/news/bevy-0-18/) · [relationships PR #17398](https://github.com/bevyengine/bevy/pull/17398) · [many-to-many issue #18121](https://github.com/bevyengine/bevy/issues/18121) · [BSN PR #23413](https://github.com/bevyengine/bevy/pull/23413) · [Flecs 4.1](https://ajmmertens.medium.com/flecs-4-1-is-out-fab4f32e36f6) · [flecs_ecs](https://crates.io/crates/flecs_ecs) · [SpacetimeDB license](https://github.com/clockworklabs/SpacetimeDB/blob/master/LICENSE.txt) · [Jackdaw](https://github.com/jbuehler23/jackdaw)
**Editor/UI:** [Tauri IPC](https://v2.tauri.app/concept/inter-process-communication/) · [Tauri channels](https://v2.tauri.app/develop/calling-frontend/) · [IPC benchmark discussion](https://github.com/orgs/tauri-apps/discussions/11915) · [Graphite on LWN](https://lwn.net/Articles/1051242/) · [PlayCanvas editor open-source](https://blog.playcanvas.com/playcanvas-editor-frontend-is-now-open-source/) · [Rerun architecture](https://github.com/rerun-io/rerun/blob/main/ARCHITECTURE.md) · [GPUI](https://crates.io/crates/gpui) · [@xyflow/react](https://www.npmjs.com/package/@xyflow/react)
**Sync/plugins/AI/web:** [Loro](https://loro.dev/) · [Loro movable tree](https://loro.dev/blog/movable-tree) · [Automerge 3](https://automerge.org/blog/automerge-3/) · [WASI 0.3.0](https://github.com/WebAssembly/WASI/releases/tag/v0.3.0) · [Extism](https://extism.org/) · [Zed extensions (WIT/Wasm)](https://zed.dev/blog/zed-decoded-extensions) · [Zed DeltaDB](https://zed.dev/blog/introducing-deltadb) · [MCP 2026 roadmap](https://blog.modelcontextprotocol.io/posts/2026-mcp-roadmap/) · [Claude structured outputs](https://platform.claude.com/docs/en/build-with-claude/structured-outputs) · [WebGPU status](https://github.com/gpuweb/gpuweb/wiki/Implementation-Status) · [Chrome 146 WebGPU](https://developer.chrome.com/blog/new-in-webgpu-146) · [Wasm features](https://webassembly.org/features/) · [COOP/COEP](https://web.dev/articles/coop-coep) · [lightyear](https://github.com/cBournhonesque/lightyear)
**Project docs:** the five files in Drive › "Project 5 - metrocalk.com"
