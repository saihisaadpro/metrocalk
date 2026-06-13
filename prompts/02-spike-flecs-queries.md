# Prompt 02 — M0 Spike ②: Flecs wildcard queries at editor scale

> Use with `00-orchestrator.md` (v2) as system prompt. Effort `xhigh`. Throwaway spike in `/spikes/flecs` — measurements over polish. Benchmark discipline from the orchestrator prompt applies to every number.

---

<task>
Validate or refute ADR-001 (`decisions/001-flecs-over-bevy-ecs.md`): that Flecs v4.1 via the `flecs_ecs` crate delivers the compatibility-query performance our product is built on — "find all entities providing Health" answered in <16 ms in a realistic editor scene — and that the Rust binding is safe enough to build a company on. This query engine is the beating heart of both north-star UX tests (click-to-bind AND describe-to-create resolution step ①); if it can't hit interactive latency, we must know before M1.
</task>

<scope>
In scope: a Rust prototype in `/spikes/flecs` exercising relationship pairs, wildcard queries, and cached queries under mutation load, plus a binding-quality assessment.
Explicitly out of scope: Loro integration, rendering, UI, the registry's name/alias text search (trivial, not the risk), any wrapper API design beyond what the spike needs, and upstream contribution PRs (note needs; don't do them).
Use the latest stable `flecs_ecs` (0.2.x as of June 2026); record exact versions of binding and C core.
</scope>

<design_constraints>
Model Metrocalk semantics in Flecs idioms: capability provision as pairs (`(Provides, Health)`) · binding edges as pairs (`(BindsTo, target)`) · roles as tags (`Player`, `Enemy`, `UIElement`). Queries created once and iterated every selection change while the world mutates between iterations — the editor's steady state, not a static benchmark.
</design_constraints>

<benchmarks>
Synthetic scene, seeded RNG (document the seed): 5,000 entities, 40 component types, `(Provides, Health)` matching ~300 entities, 2,000 binding pairs. Report median + p99:
1. The real compatibility question, cold vs cached: "all entities with `(Provides, Health)` that lack a `(BindsTo, *)` pointing at them."
2. Query latency under mutation: interleave each iteration with 100 random pair add/removes. Measure cache invalidation cost. Run this twice: safety-lock feature ON and OFF — report the delta.
3. Wildcard traversal: every binding edge in the scene (powers the relationship visualizer).
4. Churn correctness: 1,000 entities created and destroyed between queries — verify zero stale results, measure latency impact.
5. Scale: repeat benchmark 1 at 20,000 entities. Also record approximate memory per entity (peak RSS / entity count).
</benchmarks>

<binding_assessment>
Assess `flecs_ecs` itself, with file/line references into its source: unsafe surface and aliasing model · API gaps vs the C API that our use hits · build times and debugging experience · include a short "ergonomics exhibit" — the actual Rust code of benchmark 1's query — in the report, since M1 engineers will live in this API. Close with one paragraph: "would you build a company on this binding, given the wrapper-isolation rule?"
</binding_assessment>

<success_criteria>
Adopt if: benchmark 1 cached < 16 ms p99 at 5k AND 20k entities · benchmark 2 < 16 ms p99 under mutation (with safety-lock in whichever configuration you'd ship) · zero stale results in benchmark 4 · no soundness landmine the wrapper can't contain. Otherwise recommend the ADR-001 fallback (`bevy_ecs` 0.19 + hand-built relationship index) with a half-page sketch of what that index costs to build in M1.
</success_criteria>

<verification>
Re-run the suite from scratch; report the second run; investigate >25% variance. Cross-check benchmark 1 against a criterion (`cargo bench`) harness to confirm you're not measuring noise.
</verification>

<definition_of_done>
☐ `/spikes/flecs` builds and runs with one documented command · ☐ README with results table + environment block + seed · ☐ safety-lock ON/OFF delta reported · ☐ ergonomics exhibit included · ☐ binding assessment written · ☐ ADR-001 status updated (or superseding ADR drafted) · ☐ progress.md log entry with the latency table · ☐ architecture.md "Open questions" flecs line updated · ☐ working tree clean, committed.
</definition_of_done>
