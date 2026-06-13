# ADR-001: Flecs over Bevy ECS for the semantic core

**Date:** 2026-06-12 · **Status:** Accepted — **M0 query/binding spike PASSED 2026-06-13** (`spikes/flecs`); M1 integration gate still pending · **Supersedes:** —

> **Spike result (2026-06-13):** `flecs_ecs` 0.2.2 / Flecs C v4.1.2. All adopt criteria met with safety locks ON (the shipping config): compatibility query `(Provides,Health)` without `(BindsTo,*)` cached **12 µs p99 @5k / 58 µs p99 @20k** (≪16 ms), **41 µs p99 under mutation**, churn produced **zero stale results**, criterion cross-check confirms ~15 µs mean. `flecs_safety_locks` ON costs 0–10% (noise) and contains the aliasing landmine at runtime → ship ON. One finding for M1, not a blocker: **(F1)** modeling capabilities as `(Provides, cap)` pairs fragments archetypes (~14.8 KB/entity at 20k, ~one table/entity); query latency is unaffected, but mark relationships `DontFragment`/sparse or store capability sets as data — validate in M1. Binding is a 1-maintainer 0.x crate with ~1,180 unsafe sites: adopt **only behind the wrapper** (this ADR's condition); fallback (`bevy_ecs` + hand-built index) stays viable because nothing Flecs-shaped leaks past it. See `spikes/flecs/README.md`.

## Context

The product is relationship queries: "find all entities providing Health", many-to-many bindings, wildcard traversal, instant compatibility discovery. As of June 2026, Bevy ECS relationships are one-to-many only (many-to-many is open issue bevyengine/bevy#18121, design-stage). Flecs v4.1 ships first-class pairs, wildcard queries (`(Provides, *)`), transitive/symmetric/exclusive traits, runtime component registration, reflection, and JSON serialization — battle-tested C core (MIT), used in AAA engines. The Rust binding `flecs_ecs` (MIT) is 0.x with effectively one maintainer.

## Decision

Use Flecs v4.1 via `flecs_ecs` as the semantic ECS, wrapped entirely behind our own query API. No Flecs types leak outside the wrapper crate.

## Consequences

- We get editor-grade relationship queries today instead of building them on Bevy or waiting years.
- Binding risk is real: mitigate via the wrapper, budget to contribute upstream/vendor the binding, consider sponsoring the maintainer.
- We still track Bevy's ecosystem (BSN scene compat, BRP interop) without adopting it.
- Fallback is `bevy_ecs` + hand-built relationship indices; the wrapper makes the swap survivable.

## Revisit when

M1 gate: if the `flecs_ecs` spike or first integration shows safety/perf/maintenance problems → execute fallback. Also revisit if Bevy ships many-to-many fragmenting relationships.
