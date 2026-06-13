# Prompt 01 — M0 Spike ①: Loro as the Metrocalk document layer

> Use with `00-orchestrator.md` (v2) as system prompt. Effort `xhigh`. Timebox: throwaway spike in `/spikes/loro` — code quality bar is "trustworthy measurements", not "production". Benchmark discipline from the orchestrator prompt applies to every number.

---

<task>
Validate or refute ADR-002 (`decisions/002-loro-over-custom-wal.md`): that Loro 1.x can replace our planned custom WAL/ARIES undo system as the document layer — undo/redo, persistence, history — for an ECS-backed scene document. Produce measurements and a written adopt/fallback recommendation. The intent is risk reduction before M1 builds the real ECS↔Loro commit pipeline on top of whichever choice wins.
</task>

<scope>
In scope: a minimal Rust prototype in `/spikes/loro` modeling a Metrocalk scene as a Loro document, benchmarks, and the ADR-002 status update.
Explicitly out of scope — do not build any of these: real ECS integration, rendering, UI, collaboration networking, plugin or AI surfaces, token/marketplace anything, abstractions intended for reuse. This applies to the whole spike, not just the first iteration.
Use the latest stable `loro` crate (1.13+ as of June 2026); record the exact version.
</scope>

<design_constraints>
Model the scene document with: entity hierarchy in Loro's MovableTree (concurrent reparenting is the hard case we're buying Loro for) · component data as nested maps keyed by stable entity IDs, including asset-reference fields (string paths/IDs — they must survive merges intact) · binding edges (`HealthBar bindsTo Character`) as data. Propose and justify list vs map representation for edges before implementing — one paragraph in the report, not an ADR.
</design_constraints>

<benchmarks>
Synthetic scene, seeded RNG (document the seed): 5,000 entities, 3–8 components each, ~2,000 binding edges. Report median + p99, methodology stated:
1. 10,000 sequential mutations (property set 70%, reparent 10%, entity create/delete 10%, binding add/remove 10%) — throughput, total wall time, and peak RSS.
2. Undo/redo latency: single ops and a 50-op transaction group, at history depths of 1k and 10k ops.
3. File sizes: full snapshot, shallow snapshot, oplog export — at 10k and 100k history ops.
4. Time-travel checkout to a version 5k ops in the past.
5. Merge stress: fork, apply 500 divergent ops per branch including conflicting reparents (move A under B / move B under A) and conflicting edits to the same component field, merge. Document: convergence result · undo behavior immediately after a merge (does local-only undo hold?) · every class of post-merge invalid state you can produce (cycles? orphans? duplicate bindings? dangling asset refs?). This list feeds the merge-validation layer design — completeness matters more than elegance.
</benchmarks>

<success_criteria>
Adopt if all hold: single-op undo < 5 ms p99 at 10k history depth · 10k-mutation run < 10 s · full snapshot of the 5k-entity scene < 20 MB · every post-merge invalid state found is mechanically detectable and repairable. Otherwise recommend fallback (custom WAL per the v1 plan) with the failing numbers and a one-paragraph estimate of what the fallback costs us in M1–M2 time.
</success_criteria>

<verification>
Re-run the full suite from scratch; report the second run; investigate and explain any >25% run-to-run variance before reporting. Numbers without a recorded environment block are not results.
</verification>

<definition_of_done>
☐ `/spikes/loro` builds and runs with one documented command · ☐ README with results table + environment block + RNG seed · ☐ benchmarks re-run verified · ☐ merge failure-mode list written · ☐ ADR-002 status updated (or superseding ADR drafted) · ☐ progress.md log entry with headline numbers · ☐ architecture.md "Open questions" Loro line updated · ☐ working tree clean, committed.
</definition_of_done>
