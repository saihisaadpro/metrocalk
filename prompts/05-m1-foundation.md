# Prompt 05 — M1 Foundation: monorepo, wrapper API, metadata registry

> Use with `00-orchestrator.md` (v2) as system prompt, AFTER the gate review (prompt 04) has settled all ADRs. Effort `xhigh`. This is the first production-quality code session — the spike bar no longer applies. Benchmark discipline still applies to every number.

---

<task>
Build the permanent foundation the vertical slice stands on: the monorepo, the project-owned ECS wrapper API around the gate-review winner, the component metadata registry, and CI that enforces the <16 ms query gate from day one. The intent: after this session, every future session works inside a real codebase with the invariants mechanically enforced, not remembered.
</task>

<scope>
In scope: repo structure per architecture.md (`/core`, `/transport` as empty crate with the trait sketch, `/spikes` retained), the wrapper crate, the registry crate, the seeded stress-scene generator promoted from spike code to a shared test utility, CI.
Explicitly out of scope: the ECS↔Loro commit pipeline (next session, M1–2), any UI or Tauri shell (M2–3), rendering, transport implementations beyond the trait definition, Rules layer, anything Phase 2. Do not port spike code wholesale — spikes were throwaway; extract only the stress-scene generator and the lessons.
</scope>

<deliverables>
1. **Monorepo** — cargo workspace; `/core/ecs-wrapper`, `/core/registry`, `/transport` (trait only), `/tools/scene-gen`. Workspace-level lints: `clippy::pedantic` tuned in one shared config, `unsafe_code = "forbid"` everywhere except the ecs-wrapper crate (which contains and documents every unsafe interaction with the underlying ECS).
2. **ECS wrapper API** (`/core/ecs-wrapper`) — the invariant-5 boundary: no underlying ECS types in the public API. Public surface: entity create/destroy · component attach/detach · capability pairs (`provides`, `requires`) · binding edges · the compatibility query ("entities providing X, not yet bound toward Y") · subscription hooks (observer registration — the reactive seam M2's deltas will ride on). Design for the fallback: a doc comment on the crate root states what `bevy_ecs` would have to implement if ADR-001's fallback ever fires.
3. **Component metadata registry** (`/core/registry`) — components registered with: JSON Schema for fields · provides/requires capabilities · tags + aliases (the describe-to-create search surface) · UI hints. Serializable; validated at registration (a component with a malformed schema is rejected loudly). Ship 10 real example components (Health, HealthBar, Transform, Sprite, RigidBody…) registered via a derive-style macro or builder — whichever is less magic.
4. **Stress scene generator** (`/tools/scene-gen`) — seeded, parameterized (entity count, component distribution, binding density), used by both tests and benches. The 5k and 20k scenes from the spikes become named presets.
5. **CI** — fmt + clippy + tests + the wasm-tripwire job (from spike ③) + a criterion benchmark job that runs the compatibility query on the 5k preset and **fails the build above 16 ms p99** on the runner hardware (calibrate the threshold to the runner with a recorded margin; document the calibration).
</deliverables>

<success_criteria>
Done when: `cargo test` green across the workspace · the compatibility query through the wrapper API (not raw ECS calls) meets <16 ms p99 on the 5k preset locally AND the CI gate is calibrated and green · registry rejects invalid schemas with actionable errors · all 10 example components register, query, and round-trip serialize · no underlying ECS type appears in any public signature (enforce with a CI grep or a compile test).
</success_criteria>

<verification>
Independent review pass on your own diff against invariants 1–5 before finishing — especially invariant 5 leakage in the wrapper's public API. Run the benchmark suite twice per discipline. Clean-clone build test: fresh clone → documented setup → green tests, no undocumented steps.
</verification>

<definition_of_done>
☐ workspace builds, tests green · ☐ wrapper public API leak-free (mechanically checked) · ☐ registry + 10 components working · ☐ scene-gen presets shared by tests/benches · ☐ CI: fmt/clippy/test/wasm-tripwire/16ms-gate all green, calibration documented · ☐ clean-clone test passed · ☐ progress.md log + Now/Next updated · ☐ architecture.md repo section updated from "planned" to actual · ☐ working tree clean, committed.
</definition_of_done>
