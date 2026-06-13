# ADR-002: Loro CRDT over custom WAL/ARIES undo system

**Date:** 2026-06-12 · **Status:** Accepted (gated at M0 spike) · **Supersedes:** the WAL/ARIES design in the v1 plan and engineering deep-dive doc

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
