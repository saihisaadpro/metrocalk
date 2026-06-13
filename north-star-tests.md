# Metrocalk — North-Star UX Tests (manual checklist)

> **Purpose.** The felt-experience targets the engine must hit — each one a *complex workflow incumbents make painful*. Passing means we made it easy **without** breaking the five invariants (one source of truth · deltas only · everything a transaction · hot path off JS · pre-1.0 deps behind our traits).
>
> **Scope note.** Tests **1–2** exercise the deterministic core being built now (M1). Tests **3–6** target the deferred runtime systems (physics → audio → gameplay) — they are the bar to check *as those systems land*, and each names its gating spike from [`physics-audio-networking-plan.md`](physics-audio-networking-plan.md). They are not testable today; they are what "done well" must feel like.
>
> **Universal pass conditions (apply to every test, don't repeat them below):** ≤ the stated interaction count · every "no" is *explained*, never silent · every step is **one undoable transaction** · state **survives reload** · the happy path works **fully offline** (local/installed first; AI generation is the last resort, never required) · the relationship/compat query stays **< 16 ms**.

---

## 1 — Bind by intent  *(core · in build now)*

**Hard elsewhere:** in Unity you drag a HealthBar prefab, then hand-wire a script reference to the target's Health, null-check it, and write the update subscription.

1. Add a HealthBar to the scene; click it.
2. The panel shows **ranked compatible targets** (entities that have `Health`, ranked by proximity/recency). Incompatible entities are greyed, with the reason on hover ("no `Health` — add one?").
3. Click a target → the engine wires the relationship in one transaction; the bar starts tracking live.
4. Ctrl+Z undoes the entire bind. Reload → the bind persists.

**Pass:** ☐ ranked targets on click ☐ ≤2 interactions to bind ☐ every greyed "no" explains itself ☐ single-step undo ☐ survives reload.

---

## 2 — Describe to create  *(core)*

**Hard elsewhere:** find/model a sword, import it, fix materials, then author pickup + equip + socket logic — an evening's work.

1. Scene has a character. Type **"rusty medieval sword."**
2. Resolution order **local → marketplace → generate**: installed/marketplace results appear instantly; only if nothing fits does **"Generate?"** run text-to-3D — a grey placeholder drops in immediately and the real mesh streams in to replace it.
3. It arrives **pre-componentized** (`Equippable` + `Pickupable` + `DamageSource`); the engine offers to attach it to the character's hand socket. One click.
4. Press Play → it's pick-up-able / in hand.

**Pass:** ☐ description → working pick-up-able object in minutes ☐ local/marketplace instant & offline; generation only as last resort ☐ arrives as a working object, not dead geometry ☐ one undoable transaction ☐ survives reload.

---

## 3 — Physics: physically correct in one click, and *prove why it behaved*  *(gated: spikes P1/P3, plan §2)*

**The complex aspect:** physical-correctness setup + the classic beginner traps + reproducing a physics glitch. **Hard elsewhere:** Unity makes you add a Rigidbody, pick the *right* collider (mesh colliders must be convex or decomposed), tune mass/drag — and when a barrel jitters you sprinkle `Debug.Log` and pray it reproduces (it usually won't; physics isn't deterministic by default).

1. Drag `barrel.glb` in. The import pipeline already generated a collision shape, so the engine offers: *"Looks dynamic — add `RigidBody` + `Collider`?"* One click.
2. The engine catches the classic mistakes **before** runtime: a `RigidBody` with no collider → *"this will fall through the floor — add one?"*; a concave mesh used for dynamics → *"concave mesh → using a generated convex hull (or keep static?)."*
3. Stack three barrels; press Play; shove them with the character — they tumble with correct mass and friction.
4. One barrel jitters. **Pause, scrub the timeline backward** to the exact contact frame, inspect it, nudge the friction, **resume** — and because the sim is deterministic (fixed timestep + recorded input), the same seed reproduces the exact behaviour every run, so the bug is repeatable instead of a ghost.

**Pass:** ☐ dead model → physically-correct dynamic body in ≤2 clicks, no code ☐ every common mistake (no collider / concave dynamic collider) caught + one-click fix, *not* discovered at runtime ☐ a physics moment can be paused, scrubbed back, edited, resumed ☐ same seed + inputs reproduce bit-identical behaviour (the determinism gate, P1) ☐ resume-from-scrub is itself deterministic (P3 — the snapshot/restore gate).

---

## 4 — Audio: describe the soundscape — spatial *and* adaptive  *(gated: audio spikes A/B/D, plan §3)*

**The complex aspect:** sourcing SFX + spatialization + occlusion + adaptive music — the whole FMOD/Wwise surface. **Hard elsewhere:** author banks, define events + RTPCs, attach emitters, build reverb zones, wire a music state machine, and keep the middleware project synced with the scene — days of work in a separate tool.

1. Select the character, type **"footstep sounds."** Resolution order local → marketplace → generate; the clip arrives as a **bound `AudioSource`** with sane spatial defaults, and the engine **auto-suggests the `AudioListener`** on the camera. One click to confirm.
2. Type **"this cave should echo."** The engine adds a reverb/occlusion zone (Steam Audio on desktop; reduced-fidelity panner in the browser per ADR-006). Walk in and out — hear it open up.
3. Type **"music shifts to combat when enemies are within 15 m."** The engine builds a `MusicState` machine (Explore ↔ Combat) whose transition is a **declared Rule** fed by the proximity query — *reusing the gameplay logic layer*, not a separate audio DSL.
4. Reload → it all persists. Pull the network cable → installed/generated sounds still play.

**Pass:** ☐ "describe a sound" → audible, spatialized, bound source in ≤2 interactions; listener auto-suggested ☐ spatial reverb/occlusion by description, degrading gracefully in the browser ☐ adaptive music is data (Rule + state machine), no middleware project ☐ installed sounds play fully offline; generation optional ☐ AI-SFX round-trips through the import pipeline as one undoable transaction (spike D).

---

## 5 — Gameplay: a multi-step conditional, authored *and debugged* without code  *(gated: Rules layer, plan §2 / architecture.md logic row)*

**The complex aspect:** the canonical hard case — *"the rusty sword catches fire, but only after the player defeats 4 enemies **and** reaches the boss arena."* Counters, a state machine, event rules, and the easily-forgotten "off" switch. **Hard elsewhere:** a C# script with a static kill counter, event subscriptions, a state enum, references to the sword's particle system and the arena trigger, the inverse cleanup, and `Debug.Log` to find out why it broke.

1. Two ways to author the *same* data: **(a) by clicks** — select the sword → "Effects → starts when…" → a structured builder where every dropdown is fed by the metadata registry (it already knows the `EnemyDied` event, the `KillCounter`, and the boss-arena zone exist), so there is no free-text logic, no typos, no nil-refs; or **(b) by sentence** — type the whole sentence and the LLM composes the three pieces (a `KillCounter`, a `QuestState` machine Hunting→ReadyForBoss→FacingBoss, two rules) as schema-validated patches, listed for review.
2. The engine **proactively offers the mirror rule** ("remove the flame when leaving `FacingBoss`?") — because half of all game bugs are the missing "off" switch.
3. Press Play, kill 3 enemies, walk to the arena — the sword doesn't burn. **Click the sword:** the engine shows the live rule state — ✅ `state = FacingBoss`, ❌ `KillCounter = 3 of 4`. The "why" is *visible*, not logged.
4. Scrub backward to watch exactly when the counter last incremented (transactions are time-travelable).

**Pass:** ☐ a 3-part conditional authored by registry-fed clicks (typo-proof) **or** one sentence (LLM → validated patches), as one undoable transaction ☐ engine offers the inverse/cleanup rule unprompted ☐ on failure, the engine explains the live truth-state on click — debug by *looking*, not `Debug.Log` ☐ decision history is time-travelable ☐ no code for orchestration (genuinely algorithmic behaviour drops to a WASM-plugin component — the honest ceiling, below).

---

## 6 — Capstone: one sentence → a playable slice  *(integration of 3 + 4 + 5)*

**The point:** the systems compound. Empty scene → **"a knight who picks up a rusty sword that bursts into flame after 4 kills, with footsteps that echo in the dungeon"** → a playable moment.

1. The engine assembles the chain: asset resolution (knight + sword) → physics pickup (test 3) → audio (test 4) → the conditional flame quest (test 5), each piece listed and inspectable.
2. Click any piece to see/edit it; Ctrl+Z peels the whole thing back as transactions.
3. Once assets are local, the slice plays with the network cable pulled.

**Pass:** ☐ one description yields a coherent, inspectable, undoable multi-system slice ☐ every sub-piece is an ordinary entity/component/rule (no privileged god-objects) ☐ runs offline once assets are local ☐ the same slice opens and plays from a shared project link in the browser (the funnel).

---

## The honest ceiling (so a test never over-promises)

Rules **orchestrate** (quests, unlocks, doors, waves, dialogue gates, the test-5 conditional). Genuinely **algorithmic** behaviour — boss combat AI, procedural generation, a custom solver — is a **code component compiled to a sandboxed WASM plugin**, authored by a developer, the marketplace, or the LLM's code tier. Any "north-star" that would require hand-written per-frame logic is *by design* out of the no-code path and into the plugin tier. Keeping that line is what stops the Rules layer from collapsing into no-code spaghetti.
