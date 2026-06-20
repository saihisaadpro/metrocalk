# ADR-015: Capability identity = standard vocabulary + author-namespaced caps + `(AliasOf, std:*)`; marketplace index behind a trait

**Date:** 2026-06-20 · **Status:** Accepted — M5 (marketplace gate; `core/src/{caps,marketplace,resolve}.rs`
+ `editor-shell/src/capscene.rs`) · **Resolves:** the **capability identity / namespacing** open question
in `architecture.md` ("decide + ADR at the marketplace gate"). · **Builds on:** [ADR-012](012-describe-to-create-resolver.md)
(the resolver's marketplace seam), [ADR-014](014-asset-model-and-import-pipeline.md) (an entry carries an
asset handle), the M1.3 registry. · **Seams (not built):** the token ledger / settlement / payout, remote
hosting, and text-to-3D generation ([ADR-004](004-free-engine-token-economy.md)).

## Context

M3.2 made describe-to-create resolve **local-only**; the marketplace + generate tiers were honest stubs.
The marketplace tier needs an index of **pre-componentized** entries (working objects, not dead files) so
a no-local-match resolves to something that drops in already wired. Doing that safely **forces** a
capability-identity decision: the M1.3 registry (and the shell's cap scene) interned capabilities **by
bare string**, so two authors' `"Health"` collide — fine for the curated stdlib, wrong for an open
marketplace + describe-to-create. Capabilities are already `Entity` ids, so the fix is an intern-key +
alias-pair change, not a rewrite.

## Decision

**1. Capability identity = a canonical namespaced string.** One rule (`core/src/caps.rs::canonical`): a
**bare** name is the **standard vocabulary** (`Health` → `std:Health`); an already-namespaced name
(`std:Health`, `acme:Health`) passes through. So the curated stdlib's bare `"Health"` and a marketplace
entry's `"std:Health"` intern to the **same** entity, while `acme:Health` and `brandx:Health` are
**distinct** entities — the bare-string collision is impossible. Caps intern by this canonical key;
`cap_name` carries a friendly **display** name (`Health`, `Health (acme)`). **Rule:** no bare-string
capability identity in a marketplace-facing API.

**2. Custom caps opt into the standard web via a one-directional `(AliasOf, std:Cap)` pair.** An author's
`acme:Health` may declare `AliasOf std:Health` ("my Health IS-A std Health"). At **apply** time a provider
of an aliased cap also gets the **standard cap's** `Provides` pair — so it satisfies a `std:Health`
requirer (covariant) **across authors**, while a `std:Health` provider does **not** auto-satisfy an
`acme:Health` requirer. The compatibility query (reveal/bind) is **unchanged** — it matches by entity
identity; the alias is resolved into the provides pairs at write time. Aliasing is **opt-in, toward the
standard vocab** — a malicious author cannot hijack `std:Health`; they can only *opt their own* cap into
satisfying it.

**3. Marketplace index behind a trait (invariant 5).** A `MarketplaceIndex` trait — query by description →
ranked **pre-componentized** entries (namespaced provides/requires + an asset handle + an inert token
price) — with a checked-in `LocalCatalog` impl; a remote index implements it unchanged. The resolver gains
a real tiered `resolve`: **local short-circuits** (offline, deterministic — the marketplace is queried
**only** on a no-local-match), then marketplace, then the **generate** seam (unbuilt). Choosing an entry
instantiates a working object (component + namespaced caps + mesh) through the commit pipeline as **one
undoable transaction**, persisted (`Record::ApplyMarketplace`), with the M3.1 one-click attach — identical
UX to local, only the source differs.

**4. Economy + generation stay seamed.** An entry may carry a token `price`, surfaced with the "buy ≈ N
tokens / creator keeps ~70%" framing as an **inert UI seam** — no ledger, no settlement, no purchase, no
generation (ADR-004).

## Consequences

- **Open-ecosystem safe:** cross-author same-name caps never collide; the compat web still works across
  namespaces; aliasing is the only bridge and it's opt-in + one-directional (the adversarial guard).
- **No rewrite:** an intern-key change + an alias resolution at apply; the reveal/bind query is untouched.
- **Offline floor intact:** `resolve_local` is logic-unchanged and never touches the network (a spy index
  proves the local happy path doesn't query the marketplace). Measured (release): local resolve p99 ~42 µs
  (unchanged order, ADR-012), marketplace query p99 ~34 µs (2nd tier, local catalog).
- **wasm parity by construction:** `caps` + `marketplace` are pure metadata (no ECS/Loro) — wasm-portable
  like the resolver; they live in native-only `/core`, so the full browser *run* awaits the Phase-2 browser
  backend (same divergence as ADR-012; the `wasm32` tripwire stays green — `metrocalk-assets` unaffected).
- **Honest no-match still wins:** the `MIN_SCORE` gate (ADR-012) carries to the marketplace tier — a weak
  match is a no-match, never a confident-but-wrong entry.

## Revisit when

The marketplace becomes remote (a `MarketplaceIndex` impl over a network/D1 index — the trait is the seam),
or the token economy / generation / payout is built (ADR-004), or token-overlap is replaced by a learned
index at real catalog scale (revisit with ADR-012).
