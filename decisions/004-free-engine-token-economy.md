# ADR-004: Free engine + token economy + componentized marketplace

**Date:** 2026-06-12 · **Status:** Accepted (business-model v1) · **Supersedes:** the placeholder "editor seats / cloud features" revenue lines in plan v2 §6

## Context

The audience is indies (solo creators, tiny teams), not enterprises — seat-based SaaS pricing fits them poorly, and a paywalled download would kill the free-adoption funnel the whole browser/desktop strategy is built on. Meanwhile the flagship feature (describe-to-create with text-to-3D generation and LLM asset editing) has real per-use GPU cost, which maps naturally to consumable credits. Roblox proved the closed-loop token economy at scale; Unity's runtime-fee disaster proved what indies punish.

## Decision

Engine free forever (desktop + browser lite). Revenue = tokens:

- New accounts get a few free generations (taste the magic), then **$10 ≈ 100 tokens** starter pack.
- Fresh text-to-3D generation ≈ **10 tokens** (covers provider GPU cost with margin).
- Comparable marketplace asset ≈ **2–4 tokens**; creator keeps ~70%, platform ~30%.
- LLM edit of an existing asset (retexture, variation, refine) ≈ **1–2 tokens**.
- Pricing invariant: **buy + edit < regenerate**, so the resolution order local → marketplace → generate is enforced by economics, not policy.
- Marketplace assets are pre-componentized (Equippable, Pickupable… attached with sane defaults) — working game objects, not files.

## Consequences

- Platform earns on both paths: full margin on generation, a cut on every resale; per-user GPU cost falls as the marketplace grows.
- Needs from day one of marketplace launch: perceptual dedup (the 500-rusty-swords problem), ratings, quality-ranked search.
- Creator cash-out (vs tokens-only) triggers payout regulation: Stripe Connect for mechanics, **legal review required before enabling cash-out**.
- Generation providers (Meshy/Tripo/open models) are wrapped behind our own API like every volatile dependency; launch scope is props/environment (rigged-character generation still immature industry-wide).
- All business features are Phase 2 — the M0–M6 vertical slice is unchanged.

## Revisit when

Provider GPU prices shift enough to break the 10-token margin · cash-out legal review returns constraints · generation quality for rigged characters matures · free-generation abuse appears (rate-limit/identity response).
