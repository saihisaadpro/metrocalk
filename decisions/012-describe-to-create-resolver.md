# ADR-012: Describe-to-create resolver — tiered local→marketplace→generate, token-overlap local match

**Date:** 2026-06-16 · **Status:** Accepted — M3.2 local tier (`core/src/resolve.rs`) · **Builds on:**
north-star #2 (`metrocalk.md`), the M1.3 registry, [ADR-006](006-browser-query-backend.md) (browser
parity), [ADR-011](011-intent-ranking.md) (the M3.1 ranking/attach reused for one-click attach).

## Context

North-star test #2: type a description → the engine finds it **locally, pre-componentized**, and offers
to attach it in one click. Resolution order is **local → marketplace → generate**, and the happy path
must work **fully offline** — "AI is a guest, never the foundation." Today there are no real art assets
and no marketplace / text-to-3D infra, so M3.2 builds the **local tier** against the curated stdlib
component library and **seams** the other two. The resolver must (a) match *semantically* (not just
exact strings), (b) run offline + in wasm, (c) never return confident nonsense.

## Decision

**Tiered resolver, local tier only built; marketplace + generate are documented stubs.** `resolve_local`
returns ranked matches or, when nothing clears a confidence threshold, an honest empty result with
`next_tier = Marketplace` (the seam). The happy path never needs the upper tiers.

**Local match = token-overlap scoring, not a bundled embedding model.** For a curated ~12-kind stdlib
this is the lightest approach that is genuinely semantic: tokenize the query (lowercase, split, drop
stopwords, tiny synonym map → curated vocab), and score each `ComponentMeta` by weighted overlap with
its **camelCase-split name + aliases** (1.0), **tags + provided/required capabilities** (0.6), and
substring/prefix (0.3), normalized to `[0,1]`. A `MIN_SCORE` gate (0.5) turns weak matches into an
honest no-match. Ranking is deterministic (score desc, then name) — no network, no model, no LLM.

**A match carries real capabilities.** Each `Match` includes the kind's `provides`/`requires`, so the
instantiated result is a *working object* (its components + capability pairs), not dead geometry, and
its `requires` drives the M3.1 reveal for one-click attach.

## Consequences

- **Offline + wasm by construction:** pure metadata search (no ECS/Loro) → identical native + browser
  (ADR-006, deliverable 6). No bundled model to ship to wasm.
- **Honest failure:** "rusty medieval sword" (no local asset) and gibberish both return no match +
  the marketplace seam — never a wrong match. The threshold is the guard against confident nonsense.
- **Slots in unchanged:** the tier enum + `next_tier` are the seam; wiring marketplace/generate later
  adds tiers without changing the local path or the caller.
- **Scales to a rewrite point, not a wall:** token-overlap is right for a curated stdlib; at marketplace
  scale (thousands of author-namespaced kinds) this is replaced by a learned/embedding index behind the
  same `resolve_local` signature — revisit then, with the capability-namespacing question (architecture
  open-questions) at the same gate.

## Revisit when

The local library grows past curated scale (marketplace), or a described-intent/LLM signal is added on
top — both Phase-2, behind the same tiered interface.
