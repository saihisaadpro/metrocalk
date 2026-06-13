# Metrocalk

**The vibe-coding game engine.** Intent-driven, UX-first: click anything → see what it can connect to → bind in one click. Describe anything → it appears, componentized and working. No inspector spelunking, no manual plumbing.

## What we're building

A game engine where the editor understands *relationships*, not just hierarchies. Components declare what they provide, require, and observe; the engine answers "what can this bind to?" instantly, ranks the answers by intent, and explains every incompatibility. Every mutation — human, plugin, or AI — is a transaction you can undo.

On top of that deterministic core, a **describe-to-create** layer: type "companion" on a selected character and the engine finds, composes, or — last resort — AI-generates what you need. Resolution order is always **local → marketplace → generate**: instant offline search of installed components/assets first, the marketplace index second, text-to-3D generation only when nothing fits. The magic works fully offline; AI is a guest, never the foundation.

## Who it's for

**Indies first.** Solo creators and tiny teams who want exponential creation with less friction. 2D, 2.5D, and 3D from the same engine — the intent system doesn't care about dimensions.

## How it makes money (ADR-004)

The engine is **free forever** — desktop download and browser lite editor (the zero-install funnel). Revenue is a token economy: new accounts get a few free generations, then $10 ≈ 100 tokens. Generating an asset ~10 tokens; buying a comparable marketplace asset 2–4 tokens (creator keeps ~70%); LLM-editing an existing asset ("make it rustier") 1–2 tokens. Buying + editing is always cheaper than regenerating, so curated supply wins, the GPU bill shrinks, and creators are paid to fill the gaps. Marketplace assets arrive pre-componentized — working game objects, not dead files.

## Why it wins

- Traditional engines are component-first, inspector-heavy, code-driven. We are intent-first: the system does the wiring.
- One Rust core, two targets: native desktop (pro performance) and browser (zero-install adoption).
- AI-native by design (engine = MCP server; LLM output = schema-validated transactional patches) yet fully functional offline without any LLM.
- The economy compounds: more components/assets → better describe-to-create → more users → more supply.

## North-star UX tests

1. Add a HealthBar → click → ranked compatible targets → one click binds → undo works → reload preserves everything. **≤2 interactions, <16 ms, every "no" explained.**
2. Empty scene → type "rusty medieval sword" → working, pick-up-able sword in the character's hand within minutes. **File-to-playable in one sentence.**

> Detailed, manually-checkable suite — these two plus complex **physics → audio → gameplay** workflows → [north-star-tests.md](north-star-tests.md).

## Where things live

- `Metrocalk-Engine-Feasibility-and-Hosting-Plan.md` — the full plan (stack, product model, risks, hosting, roadmap)
- `architecture.md` — current state of the system
- `decisions/` — why we chose what we chose (ADRs, one page each, immutable; 001–006)
- `progress.md` — now / next; per-milestone logs sharded under `progress/`
- `M0-gate-review.md` — M0 consolidation: gate verdicts, frame-budget arithmetic, adversarial pass
- `physics-audio-networking-plan.md` — runtime-systems plan (physics · audio · netcode), pending spikes
- `north-star-tests.md` — manually-checkable UX targets (physics → audio → gameplay)
- `hosting.md` — providers, costs, setup tasks
- `prompts/` — operating prompts for AI dev sessions (Opus 4.8)
