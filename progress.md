# Progress

## Now
- **M1.1 — Monorepo skeleton.** Workspace `/core /editor /transport /plugins /spikes`; CI builds the workspace + keeps the wasm-tripwire green. **Acceptance:** `cargo build` + `cargo test` green on the workspace; `wasm-tripwire` still green; one trivial test runs in `/core`.
- M0 gate review DONE (2026-06-13): all three gates passed, ADRs settled, browser-ECS resolved by ADR-006. Full reasoning + frame-budget arithmetic + adversarial pass in `M0-gate-review.md`.

## Next (M1, ordered — each one focused session, with acceptance test)
- **M1.2 — ECS wrapper API + Flecs backend.** Hide ALL `flecs_ecs` types behind our trait; expose entity/pair add·remove·set, build+iterate query, read-target; deferred mutation; safety locks ON. *Absorbs spike fallout:* the Flecs ergonomics sharp edges (`with`/`without` value-vs-type — expose ONE clear form) **and** the ADR-006 two-backend constraint (API must be a backend-agnostic relational-query surface a pure-Rust browser backend can also implement). **Acceptance:** spike ②'s compat query expressed through the wrapper; `grep` finds no `flecs_ecs::` outside the wrapper crate; safety locks ON.
- **M1.3 — Component metadata registry (JSON Schema).** provides/requires/observes; runtime component registration. **Acceptance:** register N component kinds from JSON Schema; "what provides Health?" returns them; malformed schema rejected with a clear error.
- **M1.4 — Seeded stress-scene generator + DontFragment memory re-measure.** Shared deterministic 5k & 20k scene (CI fixture, same SplitMix64 seed as the spikes); mark capability relationships `DontFragment`/sparse and re-measure (spike ② F1). **Acceptance:** byte-identical scene across runs; bytes/entity with DontFragment recorded and **< the 14.8 KB/entity baseline**; cached compat query still <16 ms p99.
- **M1.5 — 16 ms compatibility-query CI gate.** Wire the stress-scene query in as a perf gate. **Acceptance:** CI fails if cached compat-query p99 > 16 ms on the 5k scene (currently ~12 µs).
- **M1–2 — ECS↔Loro commit pipeline + merge-validation + engine-side undo stack.** Every mutation = one transaction; regular Loro containers (not `ensure_mergeable_*`, F1-loro); peer-namespaced entity IDs (F3-loro); **engine-side in-memory inverse-op undo stack** for interactive undo (F2-loro — NOT Loro `checkout`, which is 50–62 ms for bulk undo); merge-validation re-checks the 8 invalid-state classes from spike ①. **Acceptance:** 100% undo/redo property tests incl. entity resurrection; latest-op undo <5 ms p99; two-fork merge converges + validator repairs every injected invalid state; flecs_ecs M1 integration go/no-go recorded.
- **Carry-forward (later):** getrandom `js` for Loro-in-browser + the Phase-2 pure-Rust query backend (ADR-006); real-scene render cost @ ≥5k entities (M2 stress scene); Tauri WebView2 IPC (M2 gate).

## Done (milestone-level)
- Pre-M0: feasibility plan v2 (locked stack), research sweep (~30 sources), doc structure + ADRs 001–005 + Opus 4.8 prompt set.
- **M0 complete (2026-06-13):** 3 spikes — ① Loro ADOPT, ② Flecs ADOPT, ③ wasm/WebGPU browser-render PROVEN + CI tripwire live — and the gate review. New decision: ADR-006 (browser query backend). Detail → `progress/M0.md`, consolidation → `M0-gate-review.md`.

---

## Log

Detailed dated entries are sharded by milestone under `progress/` (keeps this dashboard thin).
Append to the **current milestone's** file, newest first, one entry per session, with measured
numbers + ADR links. Live state stays here in Now/Next above.

- [progress/M1.md](progress/M1.md) — **current milestone** (foundation build)
- [progress/M0.md](progress/M0.md) — foundation, 3 spikes, gate review (2026-06-12 → 06-13)
