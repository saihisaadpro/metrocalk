# Progress

## Now
- M0 spikes (2-week box): ① Loro scene document — **DONE, ADOPT (gate passed)**; ② Flecs wildcard queries @5k entities — pending; ③ wasm32 + WebGPU triangle — pending
- Each spike ends with an adopt/fallback ADR

## Next
- M0 spikes ② Flecs and ③ wasm/WebGPU
- Monorepo + ECS wrapper API + component metadata registry (M0–1)
- ECS↔Loro commit pipeline + merge-validation layer (M1–2) — spec for the validation layer is now in `spikes/loro/README.md`
- Gate: compatibility query <16 ms on stress scene

## Done
- Feasibility plan v1 (stack assessment, hosting, browser-vs-desktop analysis)
- State-of-the-art research sweep (ECS/data, editor/UI, sync/plugins/AI/web — ~30 sources)
- Plan v2: locked stack, three subsystem merges (Loro, Extism, MCP)
- Documentation structure + first 3 ADRs + Opus 4.8 prompt set

---

## Log

*Append-only. Newest first. One entry per working session: date — what happened, decisions made (link ADR), blockers. Archive to `progress-YYYY.md` when this slows you down.*

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
