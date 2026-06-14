# Progress

## Now
- **M2 build underway.** Transport wire is **real (M2.4)**: deltas-only `DeltaTransport` + Loro-Protocol-v1 framing + Loro-`update` payload; shared session (coalesce/ACK/outbox-collapse backpressure/fragments); 3 impls (Tauri Channel default · WebSocket · in-proc-WASM); `/core` producer hook + PeerID handshake. `/transport` links no Loro (CI grep green). See Done. (M1 complete + hardened.)

## Next (milestone M2)
- **M2.5 editor UI** (consumes the M2.4 transport contract; owns `/editor`) → then **M2.6** convergence (shell + viewport + transport; needs M2.3 ADR-008 + M2.4 + M2.5 + M2.2 render verdict). Parallel lanes in flight: M2.1 (Tauri gate, `m2.1`), M2.2 (render, `m2.2-render-gate`), M2.3 (shell-composite).
- **Deferred to M2.6 (from M2.4):** end-to-end Tauri **Channel** latency re-confirm (~3.4 ms over WebView2) + the **Windows initial-snapshot-load** cliff — both need the real shell (on the unmerged `m2.1` branch).
- **Carry-forward (later):** getrandom `js` for Loro-in-browser + the Phase-2 pure-Rust query backend (ADR-006).
- **Carry-forward (Phase 2, with collab):** `merge()` rebuilds entities from Loro but **not their ECS tags/pairs** — capabilities are ECS-only, never written to Loro, so the **compatibility query is empty after a merge**. Fix wires the registry (component-kind → capabilities) into `rebuild_ecs_from_loro`; schedule with collab. (Surfaced by M1.6 audit; see `progress/M1.md`.)

## Done (milestone-level)
- **M2.4 transport protocol (2026-06-14):** the deltas-only wire. `DeltaTransport` byte-trait + **Loro-Protocol-v1 framing** (`%LOR`/`%ACK`/`%HSK`/`%FRG`/`%EPH`-reserved/ping) carrying opaque Loro-`update` payloads; shared `DeltaSession` does coalescing (version-vector diff), batch-id/ACK, **outbox-collapse backpressure** (one update spans the gap, not N), fragmentation/reassembly, and a reconciliation hook (Wait-for-Ack; upgrade-ready, separate from user-undo). 3 impls (Tauri Channel default · WebSocket · in-proc-WASM); `/core` producer hook + PeerID handshake. **15 tests green** (10 transport + 5 core: out-of-order convergence, idempotent re-import, reconnect-resync, echo no-op, backpressure-collapse, fragments). Envelope encode+decode p99 ≤1.3 µs. `/transport` links no Loro (CI grep green). ADR-009. Channel e2e + Windows snapshot-load **deferred to M2.6** (need the real shell). Detail → `progress/M2.md`.
- Pre-M0: feasibility plan v2 (locked stack), research sweep (~30 sources), doc structure + ADRs 001–005 + Opus 4.8 prompt set.
- **M0 complete (2026-06-13):** 3 spikes — ① Loro ADOPT, ② Flecs ADOPT, ③ wasm/WebGPU browser-render PROVEN + CI tripwire live — and the gate review. New decision: ADR-006 (browser query backend). Detail → `progress/M0.md`, consolidation → `M0-gate-review.md`.
- **M1 complete (M1.1–M1.5 + M1–2 + M1.6, 2026-06-13):** monorepo + CI · ECS `World` wrapper + Flecs backend · component-metadata registry · shared seeded stress-scene + F1 storage verdict (keep dense) · 16 ms compat-query CI perf gate (3rd CI tripwire) · **ECS↔Loro commit pipeline + engine-side undo/redo + merge-validation (M1–2)** · **pipeline hardening (M1.6)**: precise additive-undo (`Op::RemoveField` — no more sibling-field destruction), atomic pre-validated commit (all-or-nothing), O(1) `tid→eid`, Loro-error propagation in `apply_*`. **flecs_ecs M1 go/no-go: GO** — undo p99 0.24–0.72 ms (≫ under the 5 ms target), resurrection robust, two-fork merge converges, all 8 invalid-state classes detected+repaired, 49 tests green. Detail → `progress/M1.md`.

---

## Log

Detailed dated entries are sharded by milestone under `progress/` (keeps this dashboard thin).
Append to the **current milestone's** file, newest first, one entry per session, with measured
numbers + ADR links. Live state stays here in Now/Next above.

- [progress/M2.md](progress/M2.md) — **current milestone** (M2 build)
- [progress/M1.md](progress/M1.md) — foundation build (M1.1–M1.6)
- [progress/M0.md](progress/M0.md) — foundation, 3 spikes, gate review (2026-06-12 → 06-13)
