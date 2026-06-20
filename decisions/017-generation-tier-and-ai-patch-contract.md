# ADR-017: Generation tier + the AI-patch contract — placeholder-first stream-in, provider trait, schema-validated transactional patches, metered last resort

**Date:** 2026-06-20 · **Status:** Accepted — M6 (`editor-shell/src/{ai,generate}.rs` + `capscene.rs`;
`src-tauri/src/main.rs`; `web/index.html`) · **Completes:** the resolution order
`local → marketplace → generate` ([ADR-012](012-describe-to-create-resolver.md) tier 3,
[ADR-015](015-capability-identity-and-marketplace-index.md) tier 2). · **Builds on:**
[ADR-014](014-asset-model-and-import-pipeline.md) (a generated mesh imports through the prompt-23
pipeline), [ADR-013](013-live-persistence-replay-log.md). · **Seams (real in prompt 26):** the token
ledger/settlement/payout ([ADR-004](004-free-engine-token-economy.md)), remote hosting, the real
text-to-3D provider, the MCP server.

## Context

Prompts 23–24 made describe-to-create resolve `local → marketplace` with real, pre-componentized,
*visible* objects. The third and final tier is **generation** — invoked only when nothing local or on
the marketplace fits. Generation needs the network + a paid provider, so by design it is the **last
resort**: the engine must stay fully functional with it off (everything built so far works offline,
unchanged). "AI is a guest, never the foundation" — so every AI/generation output must enter through one
validated, transactional path, never a raw mutation.

## Decision

**1. The AI-patch contract (the seam, invariant 3).** Every AI/generation scene mutation enters as a
small **allow-listed, schema-validated patch** (`ai::AiPatch`, `SetField` today — *not* arbitrary
RFC-6902) and is applied through the **one commit pipeline**: each op is checked against the registry
schema + engine state (entity exists, component is a known kind, field is in its schema, value's JSON
type **strictly** matches), then committed as a single undoable transaction. Any invalid op rejects the
**whole** patch (all-or-nothing, rejection-as-UX with the specific reason); nothing is applied. There is
no raw/unvalidated LLM mutation path. This is the MCP-surface contract (the **AI-edit** sibling — "make
it rustier" = a `SetField` patch metered at the edit rate — rides the same path); the MCP *server* stays
a seam.

**2. Provider trait (invariant 5).** A project-owned `generate::MeshGenerator` returns glb **bytes**
(the caller imports them via the prompt-23 pipeline — generation is a *source* of assets, not a second
asset path); no provider SDK type crosses it. A deterministic offline `FakeGenerator` (returns checked-in
bytes after a simulated latency) makes the loop CI-testable; the real provider is a documented
`RemoteGenerator` seam (unconfigured ⇒ unavailable, never on a happy path). `available()` lets the
caller degrade to an honest "generation unavailable offline" seam.

**3. Placeholder-first, stream-in (invariants 2 & 3).** Accepting "Generate?" drops a **grey placeholder**
working object immediately — Transform + an empty `MeshRenderer` (renders as the M2.2 cube placeholder)
+ `provides Renderable` + `requires Spatial`, so it's bindable at once — as one undoable transaction. The
provider runs on a **worker thread** (off the hot path); when it returns, the engine thread imports the
bytes and **streams the real mesh in** as a validated AI patch (`SetField MeshRenderer.mesh = handle`) —
same entity, same id, a **targeted delta** (inv. 2). Undo peels the stream-in (→ grey placeholder), then
the placeholder (→ gone). Persisted as `Record::Generate { prompt, pos, mesh }`; a completed generation
survives reload (the asset is content-addressed; the deterministic fake re-resolves).

**4. Metered last resort (ADR-004 — seam).** Each generation/edit records a token cost + checks a balance
through the `TokenMeter` seam (`StubMeter`: Generate ≈ 10, Edit ≈ 2; always allows + logs). The cost is
shown honestly ("≈ 10 tokens"); **no money moves**.

## Consequences

- **Describe-to-create is complete end to end** — `local → marketplace → generate`, with AI firmly a
  guest: never on the offline path (a no-anywhere-match offers an **opt-in** Generate), always last,
  always metered, always through the validated pipeline.
- **Offline-safe:** with the provider off, local + marketplace are unaffected and the grey placeholder is
  real + usable regardless of whether generation returns (the adversarial guard against a hung/garbage
  provider); a malformed/huge generated mesh is rejected by the prompt-23 import validation/size limits.
- **Measured (release):** the interactive part — `place_generation_placeholder` — is p99 ~0.8–1.2 ms @5k
  (instant). The generation round-trip is provider-latency-bound (network) — not measured, by design.
- **wasm parity:** the placeholder/apply path + the AI-patch contract are native bridge logic (the
  provider call is network); the `wasm32` tripwire (`metrocalk-assets`) is unaffected — green.

## Revisit when

The real text-to-3D provider lands (a `MeshGenerator` impl over a provider SDK + config/key), or the
token economy is built (prompt 26 — the real ledger behind the `TokenMeter` seam, plus persisting a novel
generated asset's bytes across reload), or the AI-patch op set grows (add ops + their schema checks).
