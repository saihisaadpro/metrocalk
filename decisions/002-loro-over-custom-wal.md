# ADR-002: Loro CRDT over custom WAL/ARIES undo system

**Date:** 2026-06-12 · **Status:** Accepted — **M0 gate PASSED 2026-06-13** (`spikes/loro`) · **Supersedes:** the WAL/ARIES design in the v1 plan and engineering deep-dive doc

> **Gate result (2026-06-13):** All four adopt criteria met on the 5k-entity scene — single-op undo 0.13 ms p99 (≪5 ms), 10k-mutation run 0.56 s (<10 s), full snapshot 4.51 MB (<20 MB), every post-merge invalid state mechanically detectable + repairable. Merge converged; MovableTree produced no cycles under conflicting reparents; asset-ref strings survived merges intact. Three implementation constraints surfaced (do not change this decision): **(F1)** Loro's `ensure_mergeable_*` helper does not survive undo/redo → use regular containers + the merge-validation layer below; **(F2)** undo computes its inverse via full-document checkouts, so cost tracks *distance from the tip*, not change size — two axes: a 50-op group undo is tens of ms (fix: keep transaction groups small), and *consecutive* multi-level Ctrl-Z walks back from the tip to ~53 ms median / 240 ms p99 by ~50 steps (≈ the 16 ms frame budget), which small commits do **not** fix → M1 must build an engine-side in-memory inverse-op stack for interactive undo (invariant 1: ECS authoritative, Loro the durable mirror), reserving Loro `checkout` for deep history/time-travel; **(F3)** application entity IDs must be peer-namespaced (Loro's `TreeID` is `(peer,counter)`; a peer-local counter collides on concurrent create). See `spikes/loro/README.md`.

## Context

The original blueprint required a custom write-ahead log with ARIES-style recovery, tombstoned entity resurrection, and transaction buffering — months of high-risk correctness work. Loro 1.x (MIT, actively maintained) provides: collaboration-aware UndoManager, time-travel checkout, git-like fork/merge on an op DAG, snapshot + oplog export (the oplog *is* a WAL), and a MovableTree CRDT solving concurrent scene-graph reparenting. No Rust crate ecosystem alternative offers this combination; Automerge lacks tree-move and undo-manager, yrs garbage-collects history.

## Decision

Use Loro as the document layer: durable persistence, undo/redo, history, and (Phase 2) real-time collaboration. The ECS remains authoritative at runtime; every commit flows ECS → Loro through one transactional pipeline.

## Consequences

- ~Months of WAL engineering replaced by integration work; collab comes nearly free later.
- CRDTs guarantee convergence, not semantic validity: we must build a **merge-validation layer** that re-checks ECS invariants after merges. This is where the saved effort goes.
- Undo/time-travel requires retained history → file-size cost; shallow snapshots trade history away. Measure in spike.
- Component-schema migrations are still ours to build (Loro versions ops, not schemas).

## Revisit when

M0 spike fails on: undo latency, file size at 5k-entity scenes, or merge behavior incompatible with ECS invariants → fallback is the original custom WAL design (kept in the v1 plan docs).
