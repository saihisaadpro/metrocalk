# ADR-011: Intent ranking for binding-by-intent (proximity · affinity · recency)

**Date:** 2026-06-15 · **Status:** Accepted — M3.1 reveal engine (`editor-shell/src/reveal.rs`) · **Builds on:** north-star #1 (metrocalk.md), the M1.5 compat query, [ADR-010](010-editor-projection-architecture.md).

## Context

North-star test #1: click an entity → the engine reveals what it can connect to, **ranked by intent**,
every "no" explained. The candidate set is the existing M1.5 `World` compat query (providers of a
required capability not yet bound — ~12 µs). M3.1 must order that set so the *right* target is at the
top, **deterministically and fully offline** (no LLM — AI is a guest, not here), and fast enough to
run on select within the frame budget on a 5k scene.

## Decision

Rank compatible candidates by a fixed, deterministic key (no learned weights, no network):

1. **Proximity** — Euclidean distance from the selection's `Transform` to the candidate's (nearest
   first). The dominant signal: you almost always mean the thing near what you clicked.
2. **Affinity** — how many of the selection's required capabilities the candidate provides (more →
   higher). Derived by **counting which per-capability queries the candidate matched**, not a
   per-entity capability read (see Consequences — that read blew the budget).
3. **Recency** — last-touched sequence (more recent → higher); ties broken toward what you just edited.
4. **Stable tiebreak** — entity id ascending, so the same scene always yields the same order
   (determinism is a hard requirement of test #1).

"Every 'no' explained" is **not** part of the ranked hot path: it's computed **per target on demand**
(`why_not`, O(1)) for the bounded set the UI greys — `MissingCapability(name)` / `AlreadyBound` /
`NoCapability`, each a specific, registry-derived string.

## Consequences

- **Deterministic + offline:** same scene → same order; no model, no network. Test #1's determinism
  clause holds by construction.
- **Holds the budget:** reveal (compat query + tally + rank) is **p50 0.706 ms / p99 1.107 ms** on a
  5k capability-bearing scene (release). Two budget traps were found and avoided in development — an
  eager all-entities "why not" scan and a per-match `targets()` affinity read, each ~25–33 ms at 5k;
  the committed design uses the indexed query + a membership tally + on-demand `why_not` instead.
- **Tunable later, without a rewrite:** the key is a pure function of (distance, affinity, recency);
  weighting/normalization can change here alone. Learned ranking (Phase 2) would slot in as an
  additional signal, never replacing the deterministic offline floor.
- **Recency needs a source:** the engine takes a `recency` map (last-touched seq); wiring it to the
  edit log is a small follow-up (defaults to 0 today — proximity + affinity already give a strong
  order).

## Revisit when

The dogfood verdict says the order feels wrong on a real scene (then re-weight here), or Phase-2 adds
a learned/described-intent signal on top of this deterministic base.
