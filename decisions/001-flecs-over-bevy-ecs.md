# ADR-001: Flecs over Bevy ECS for the semantic core

**Date:** 2026-06-12 · **Status:** Accepted (gated at M1) · **Supersedes:** —

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
