# Metrocalk — Physics, Audio & Networking Plan

> **Status:** Research-backed proposal, **pending spike validation** (not yet decided). · **Date:** 2026-06-13
> This resolves the three "Deferred" runtime systems in `architecture.md` (currently tentatively `Rapier / kira / lightyear`). It is written to be **validated afterward**: every pick is behind one of our own traits (invariant 5) and gated on a measurable spike, exactly like the M0 gates. No ADR is settled here — §7 lists the ADRs each gate will write once spiked.
> Built on a June-2026 research sweep (three parallel agents); sources in §8.

---

## 0. Recommendations at a glance

| Domain | Primary (behind our trait) | Fallback / studied-alt | License | Browser (wasm32) | Headline gate |
|---|---|---|---|---|---|
| **Physics** | **Rapier 0.33 + Parry 0.28** | Avian (Bevy-blocked, studied); Jolt-FFI (native-only escape) | Apache-2.0 | ✅ **pure Rust — same crate both targets** | P1: bit-identical sim hash across Win/Linux/wasm |
| **Audio (engine)** | **Firewheel 0.10** | Kira 0.12 | MIT/Apache-2.0 | ✅ explicit wasm backend | A: one crate plays native + Chrome |
| **Audio (spatial)** | **AudioNimbus / Steam Audio** (native) | `firewheel-spatial` / WebAudio panner (browser) | Apache-2.0 | ❌ native-only (C lib) → reduced-fidelity web path | B: ≥32 HRTF sources <20 ms native |
| **Audio (procedural)** | **FunDSP** (offline, deterministic) | — | MIT/Apache-2.0 | ✅ | E: synth SFX, zero token cost |
| **Networking** | **Tiered:** Loro+Loro-Protocol (T0/T1) · **renet2** (T2) · **GGRS** (T3) | lightyear / aeronet = **study only (Bevy-coupled)** | MIT/Apache-2.0 | transport split (WebTransport/WebRTC/WS) | NET-1: replication by declaration; NET-4: determinism |

**The one-line story:** keep the engine-agnostic, pure-Rust spirit of the stack — adopt **Rapier** (physics needs *no* second backend, unlike the ECS), revise the audio pick from Kira to **Firewheel** (it has the same "one crate, native + browser" property wgpu does), and make networking the headline innovation: **"networking as a declared property"** layered in four tiers over the transaction pipeline and CRDT we already own.

---

## 1. Cross-cutting themes (true of all three)

**A. The two-target rule recurs — and these three split three different ways.** Native desktop + browser wasm32 is the constant constraint (ADR-003/006). Physics is the *happy* case: Rapier/Avian are pure Rust and compile to wasm, so physics — unlike the Flecs ECS — needs **no second backend**. Audio is the *in-between* case: Firewheel ships an explicit wasm backend (one crate, both targets), whereas the best spatial DSP (Steam Audio) is a native-only C library, so the browser gets a reduced-fidelity spatial path. Networking is the *hard* case, isomorphic to the Flecs-on-wasm gap: the browser **cannot open raw UDP**, so the transport trait must resolve to different concrete backends per target.

**B. The "Bevy-coupling trap" is the recurring landmine.** The strongest pure-ECS options in each field are now hard-bound to Bevy: **Avian** (physics), **lightyear** and **aeronet** (networking) all pull `bevy_*` as hard dependencies. Metrocalk is Flecs + a pure-Rust Loro-projection backend — *not* Bevy. So each is unusable as-is and valuable only as a design reference. This is the same lesson as ADR-006: stay engine-agnostic behind our own traits (invariant 5), and prefer the engine-agnostic library every time (Rapier over Avian, renet2/GGRS over lightyear).

**C. Determinism is an *enabling substrate*, not a universal tax.** The owner wants a production-grade deterministic core. The honest framing from the research: build it once (fixed-timestep, software-math determinism), and let subsystems *opt in*. It is **required** for Tier-3 rollback netcode and for sim replay/distributed sim; it is **not required** for ordinary server-authoritative multiplayer or single-player. Crucially, the deterministic build trades performance (single-threaded, no SIMD) for reproducibility — so it's a **dual-config** decision, not a global switch.

**D. Intent-first extends to all three.** "Add `RigidBody` → engine suggests `Collider`." "Describe a footstep → it arrives as a bound `AudioSource`." "Tick `replicated` → the component is multiplayer." The same click-declare-engine-wires move that defines the editor is applied to physics, audio, and — most ambitiously — networking. This is the UX wedge against Unity/Unreal/Godot in all three.

**E. The token economy gains a second supply line.** AI audio generation (text-to-SFX) slots into the exact `local → marketplace → generate` pipeline and token mechanics as 3D assets (ADR-004), and is *cheaper per generation* than 3D. One license trap to design around (§3).

**F. Phasing: the M0–M6 vertical slice is unaffected.** All three remain post-slice. This plan is the integration blueprint + gates, not a call to build now. The single thing worth pulling forward conceptually is the **deterministic core**, because three different features depend on it (physics determinism, Tier-3 rollback, sim replay) — build it once, gate it once (§7).

---

## 2. Physics

### Recommendation
Adopt **Rapier 0.33 (+ Parry 0.28), Apache-2.0, behind our own `Physics` trait.** Keep **Avian** (XPBD) as the studied alternative — it is arguably the better *pure-ECS* design and more stable for stacks/joints at low iteration counts, but it hard-depends on Bevy ECS, so it's only viable if we ever reverse ADR-001. Keep **Jolt-via-FFI** as a native-only escape hatch (C++, won't cross to wasm).

### Why Rapier specifically
It is the only mature pure-Rust physics engine that is **ECS-agnostic** — it keeps its own physics world and we sync transforms in/out via deltas, which is exactly how we'd bridge it to Flecs/Loro without dragging in a second ECS. It's **pure Rust → identical engine native and in-browser** (no second backend, a real win over the ECS layer). And Dimforge's 2026 roadmap is explicitly steering it toward robotics/simulation accuracy (see real-world-sim below). Apache-2.0, multiple corporate sponsors — lower bus-factor than Avian's single maintainer, though still 0.x, so the trait wrapper is mandatory.

### Determinism strategy (and its honest limits)
Rapier's `enhanced-determinism` feature forces a pure-Rust software math library (`libm`) for the operations that otherwise diverge across platforms (FMA, transcendentals), giving **bit-identical world state across Windows / Linux / macOS / ARM / wasm** — verifiable by hashing a serialized snapshot. That is precisely the spike pass/fail criterion in §7.

Three limits to design around, not paper over:
1. **Deterministic ≠ fastest.** `enhanced-determinism` cannot coexist with SIMD or parallelism — the deterministic build is single-threaded and non-SIMD (~10–30% slower). So we ship **two configs**: the *deterministic* config is the authoritative core (replay, rollback, sim); a *SIMD/parallel* config is allowed only for throwaway non-networked single-player.
2. **Snapshot/restore is non-deterministic today.** Rapier issue #910 / parry #402 (unfixed as of this research): the broad-phase BVH workspace isn't fully serialized, so *resume-from-snapshot* diverges even though *continuous stepping* is deterministic. This is exactly the operation rollback netcode and "load a saved sim" rely on → **rollback (Tier 3) is gated on this being fixed or on us serializing the missing state ourselves** (spike P3 turns this from unknown into a known go/no-go).
3. **Determinism is end-to-end or nothing.** It only holds if *our* code (intent system, plugin/AI transactions, input handling) feeds the sim identical, ordered inputs — and is version-locked (engine/compiler bumps can change results, so replays must record versions). Fixed-point/soft-float is the documented break-glass option only if `libm` validation ever fails; **do not pursue fixed-point pre-1.0.**

### Real-world simulation — the honest verdict
**Position it as "deterministic mechanism/digital-twin simulation + interchange front-end," not "a MuJoCo competitor."** Tiered:
- **Rigid-body + articulated mechanisms** (arms, vehicles, linkages), kinematics, URDF/MJCF import, deterministic replay → **credible-later** (Dimforge's 2026 roadmap adds a MuJoCo-inspired solver, URDF/MJCF import, IK, GPU physics) — but their multibody solver has *known stability bugs today*, so track it, don't ship on it yet.
- **Soft bodies / cloth / FEM / fluids at sim grade** → **out of scope** on CPU Rapier (rigid-only; fluids only via the secondary Salva crate or GPU MPM prototypes).
- **Differentiable physics / massively-parallel RL / sensor-rich embodied-AI** → **interop-only, indefinitely.** That belongs to MuJoCo / NVIDIA Newton / Isaac / Genesis (Python + CUDA). Metrocalk's differentiator (intent-first authoring + deterministic transactional core + browser funnel) is *orthogonal* and could feed those pipelines via URDF/MJCF/USD interchange. Don't reimplement them.

### Integration
Physics arrives as ECS components + metadata (`RigidBody`, `Collider`, joints) with the intent treatment ("`RigidBody` with no `Collider` → 'this will fall through the floor — add one?'"). Mutations flow through the one commit pipeline as deltas (invariant 2/3); undo reverts them. **Sim replay is a distinct channel from Loro's document time-travel:** Loro time-travels *authoring/edit* history; gameplay *simulation* replay = deterministic engine + recorded ordered input + fixed timestep, regenerating the trajectory. Loro stores the scene/initial-conditions and the input log *as a document*; it does not re-run the sim. Design them as separate subsystems sharing only the initial-state snapshot.

### Gates → §7 (P1 determinism hash · P2 input replay · P3 snapshot/restore · P4 ECS-integration + no-leak · P5 browser parity · P6 robotics interop probe)

---

## 3. Audio

### Recommendation (revises the tentative "kira" pick)
- **Primary: Firewheel 0.10** behind an `AudioBackend` trait — an audio *graph* engine ("the wgpu of audio") with an **explicit WebAssembly backend** (one crate, native + browser, matching the wgpu story already proven in `spikes/wasm`), a mutex-free realtime path, an ECS-friendly data-driven parameter API, and Symphonia-based decoding. It's becoming the community's shared foundation (Bevy's audio working group builds on it). Pre-1.0 → the trait wrapper + the fallback are the safety net.
- **Fallback: Kira 0.12** — more mature, more turnkey game-audio features (mixer, tweens, clocks, modulators, spatial tracks), but a weaker single-maintainer wasm story and it *dropped sample-accurate clocks* in 0.12. It de-risks the layer: if Firewheel's wasm/spatial maturity disappoints at the gate, swap behind the trait with no API churn.
- **Spatial (native-only): AudioNimbus / Steam Audio** (Apache-2.0 since 2024) — best-in-class HRTF/occlusion/reverb. Native C lib → won't cross to wasm; the browser falls back to `firewheel-spatial` / WebAudio panning (reduced fidelity).
- **Procedural: FunDSP** — deterministic, offline-forever synthesis for parametric sounds (UI blips, engine drones); the "AI is a guest" complement.
- **Decoding: Symphonia** (MPL-2.0 — file-level copyleft, fine to link). **Do not adopt `oddio`** (abandoned since 2023).

### Web / WebAudio strategy + constraints
Run audio DSP inside the wasm AudioWorklet; JS sends only control deltas (honors invariant 4 — no per-sample JS crossing). Hard constraints, all browser-universal: (1) first sound must be gated behind a **user gesture** and an explicit `AudioContext.resume()`; (2) worklet threading needs `SharedArrayBuffer` → **COOP/COEP headers** (a hosting task — note for `hosting.md`), with a documented single-thread fallback; (3) higher, less-deterministic latency (tens of ms vs single-digit native) and a fixed (~48 kHz) context sample rate; (4) no Steam Audio in the browser.

### AI audio generation + the token economy (the strategic part)
**Feasible now, SFX-first.** "Add a footstep sound" → `local → marketplace → generate`, the same resolution order and token mechanics as 3D (ADR-004). Launch provider behind the usual wrapper: **ElevenLabs SFX** (48 kHz, foley-grade, ~$0.014–0.036/clip, 30 s cap); **Stable Audio 3.0** (open-weight, self-hostable, fine-tunable, up to 180 s) is the path to longer ambiences and an eventual in-house model that cuts per-call cost; music via Suno/Udio/MusicGen for themes/stems. Economics are *friendlier than 3D* (cheaper per generation); buy-and-edit-cheaper-than-regenerate holds unchanged. Honesty: music is legally murkier than SFX, and adaptive-music *structure* still has to be engine-assembled from stems — AI fills gaps and prototypes, it doesn't replace a sound designer.

> **License trap (design around it now):** ElevenLabs terms let you *use* generated SFX commercially but **forbid selling them "as isolated audio samples, sound libraries, or collections."** So the marketplace must only ever sell generated SFX **embedded inside a componentized game object/project — never as bare sound files.** Suno/Udio also no longer grant ownership (licensed-not-owned; pin model versions). Same legal review ADR-004 already mandates before cash-out.

### Integration
Audio as ECS components + JSON-Schema metadata: `AudioSource` (asset, gain, pitch, loop, bus, `spatial`, provides/requires/observes), `AudioListener` (on camera/character), `SpatialParams` (distance/attenuation/occlusion/doppler), `AudioBus`/`MixerTrack` (named buses as editable, undoable data), `MusicState` (adaptive music as When/If/Then Rules + state-machine *data*, reusing the logic layer — not a bespoke audio DSL). Marketplace audio arrives **pre-componentized**; add perceptual/fingerprint dedup at ingest (the "500 rusty swords" problem applies to sound too). The intent-first win vs FMOD/Wwise: incumbents require authoring banks, hand-binding events, RTPCs, emitters, bus routing, and keeping a separate middleware project in sync — Metrocalk collapses that to "describe → it arrives bound, listener auto-suggested, every step undoable."

### Gates → §7 (A dual-target playback · B spatial scaling+latency · C autoplay/threading · D AI-SFX round-trip · E procedural offline · F license gate)

---

## 4. Networking — the innovation

### The concept: "Networking as a Declared Property"
In Unity/Unreal/Godot, making something multiplayer means *writing netcode*: `[SyncVar]` annotations, hand-authored RPCs, then per-entity prediction/reconciliation. Metrocalk already routes **every** mutation — human, plugin, AI — through one transactional commit pipeline as deltas, mirrored into a Loro CRDT whose convergence is guaranteed and whose every merge is re-checked by the merge-validation layer (invariants 2/3, ADR-002). *That is the machinery a network needs.* So replication becomes a **declared facet of a component in the metadata registry**, not code the user writes:

```
component facets:  replicated: bool · authority: server | owner-client | shared-crdt
                   prediction: none | interpolate | rollback · interest: <relevance rule>
```

The engine reads the facets and auto-replicates that component's deltas over the **same transport trait and commit pipeline** it already uses for undo and editor collaboration. "Click → declare intent → engine wires it" becomes "tick a box → the component is multiplayer." **Why it's available to us and not to them:** incumbents' state lives in opaque mutable memory with no canonical change-stream, so sync is bolted on field-by-field; our invariant-2 delta stream *is* that canonical, ordered, mergeable change-stream already. Loro is already the editor's real-time collab substrate, and the **Loro Protocol** (MIT, shipped late 2025) already multiplexes CRDT doc sync + ephemeral presence + E2E rooms over one WebSocket — so Tier 0/1 plumbing is *already a dependency we own*.

> **The honest boundary — where this stops working.** CRDT-sync is eventual-consistency, last-writer-wins, every-peer-trusted *by construction*. It is excellent for **co-op, sandbox/creative, building sims, turn-based, async, late-join, and editor collab**. It does **not** replace authoritative or rollback netcode for **competitive / anti-cheat / fast-twitch / hit-detection / scores-currency-kill-credit** state — a CRDT trusts a cheating client's "my health = 100," and LWW resolves "who shot first" arbitrarily, not by simulation truth. The win is a **unified developer experience across all multiplayer styles (you always declare, never hand-plumb)** — *not* CRDT-for-everything. The declaration selects one of several real transports underneath.

### The tiered architecture
One transport trait (deltas only) + one commit pipeline; four authority/consistency policies on top. The user picks a tier by **declaration**, never by rewriting the game.

| Tier | What | Authority | Engine | Status |
|---|---|---|---|---|
| **0 — Editor collab** | Multiple editors on one scene, fork/merge, late-join free | shared-crdt | Loro + Loro Protocol | **Already have it** (ADR-002) |
| **1 — CRDT gameplay sync** | Co-op / creative / sandbox / async | shared-crdt | Loro Protocol; `EphemeralStore` for presence | New; ~free extension of Tier 0 |
| **2 — Server-authoritative + prediction** | Most games; the **locked default** | server | **renet2** (engine-agnostic, transport-swappable) | Default; no determinism needed |
| **3 — Deterministic rollback** | Competitive, twitch, P2P | peer (deterministic) | **GGRS** (engine-agnostic core) | Gated on the deterministic core |

All tiers emit/consume the same delta envelope through the same transport trait. Tier 0/1 are literally the *same Loro sync* with a different policy label. Tier 2 adds a server node owning the canonical ECS; client prediction is the engine optimistically replaying local transactions (we already do optimistic local echo in the editor — same mechanism). Tier 3 swaps the payload from *state-deltas* to *input-deltas* plus save/restore + re-simulate; the transport doesn't change, only what rides it.

### Libraries per tier (the big structural finding)
**The leading high-level Rust netcode crates — `lightyear` and `aeronet` — are now hard-coupled to Bevy** (the netcode twin of the Flecs-on-wasm gap; deserves its own ADR). They're unusable as our framework, valuable only as references. Engine-agnostic fits:
- **Tier 0/1:** Loro 1.10.x + Loro Protocol (MIT) — already ours.
- **Tier 2:** **renet2** (`renet2_netcode` — engine-agnostic byte-in/byte-out core, swappable UDP/WebTransport/WebSocket/in-memory transports). Maps directly onto our transport trait. We implement prediction/reconciliation/interest in our pipeline (we already have optimistic echo + relationship-query interest); study lightyear for *how*. (renet2 is a fork-of-a-fork → keep a vendoring contingency.)
- **Tier 3:** **GGRS** (MIT/Apache, 100% safe Rust, engine-agnostic request-based core; `bevy_ggrs` is just an optional adapter). Fallbacks: backroll-rs, fortress-rollback. Hard dependency: the deterministic core.

### Transport plan (native + browser)
**The hard constraint, stated like ADR-006:** the browser cannot open raw UDP, so the transport trait resolves to *different* backends per target.

| Path | Native | Browser | 2026 maturity | Role |
|---|---|---|---|---|
| **WebTransport** (HTTP/3 / QUIC) | quinn / wtransport | browser WebTransport API | **Baseline since Mar 2026** (Safari 26.4 shipped it); spec still pre-RFC | **Primary browser transport, Tiers 1–2** (reliable streams + unreliable datagrams) |
| **WebRTC datachannels** | matchbox | matchbox (wasm) | Stable; mature GGRS socket | **Tier 3 browser P2P** (needs signaling/STUN/TURN) |
| **WebSocket** | tungstenite | browser WS | Universal; TCP (head-of-line) | **Universal fallback**; native fit for Loro Protocol |
| **Raw UDP / QUIC** | quinn 0.11.9 | ❌ impossible | Production-grade | **Native-only** Tier 2/3 fast path |

Keep WebSocket as the always-works fallback under the same trait (hedges WebTransport's pre-RFC churn).

### Determinism reconciliation
The three directives aren't in tension. **Tier 2 (server-authoritative) needs no determinism** — the server is the single source of truth and broadcasts canonical state-deltas; it stays the default and ships first, before the deterministic core exists. **Determinism is the enabler** for Tier 3 (replicate *inputs only*, predict/rollback/re-simulate) and for distributed sim. wasm is an asset here (deterministic-by-design bytecode — the same property Croquet/Multisynq exploit for bit-identical replicated VMs). **Decision framing:** don't let determinism become a precondition for shipping ordinary co-op/authoritative multiplayer; build it to unlock the competitive + simulation tiers.

### Real-world / distributed simulation
HLA/DIS (IEEE 1278/1516) still rule defense/training federation but are **overkill** for our users — treat them as an optional export/bridge (a transport-trait impl or plugin) only if an enterprise customer demands federation. Our CRDT/transaction model is a **substantial asset** for distributed sim: deltas + op-DAG give late-join/rejoin convergence; the merge-validation layer is exactly the partition-reconciliation mechanism; the deterministic core gives the synchronized tick. The gap is *operational* (multi-node orchestration, partition/interest assignment, clock sync), not architectural.

### Prior art to credit
Local-first software (Ink & Switch); **Croquet/Multisynq** ("No Netcode" — deterministic synchronized computation via an input-reflector, the conceptual bridge between our Tier 1 and the deterministic core); Yjs/Automerge multiplayer experiments; HyperToken (2026 "game engine where state is a CRDT"). Nobody has paired CRDT-state with a *tiered escape hatch into authoritative/rollback* — that pairing is our differentiation.

### Gates → §7 (NET-1 declared replication · NET-2 two-browser CRDT convergence + merge-repair · NET-3 GGRS rollback budget · NET-4 determinism gate · NET-5 tier-opt-in-without-rewrite)

---

## 5. How this honors the invariants

1. **ECS authoritative, Loro the durable mirror** — physics syncs ECS↔world via deltas; networking's Tier 0/1 *is* the Loro mirror extended over the wire; Tier 2 server owns the canonical ECS.
2. **Deltas only** — physics transform sync, audio control messages, and all four netcode tiers carry deltas, never full snapshots. (Caveat: naive per-tick-transform CRDT sync has tombstone overhead → Tier 1 syncs discrete durable state via Loro, high-frequency transforms via ephemeral/unreliable channels.)
3. **One transactional commit pipeline** — physics mutations, audio changes, and replicated deltas all flow through it; the merge-validation layer doubles as the netcode/distributed-sim conflict repair.
4. **Hot path never crosses JS** — physics steps in wasm; audio DSP runs in the wasm AudioWorklet (JS gets control deltas only).
5. **Every pre-1.0 dep behind our own trait** — `Physics`, `AudioBackend`, and the existing transport trait wrap Rapier / Firewheel / renet2 / GGRS; no `rapier::` / `firewheel::` / netcode types leak (CI grep gate, mirroring ADR-001).

The **transport trait gains backends** (QUIC, WebTransport, WebRTC) with a native-vs-browser split, exactly as ADR-006 split the query layer. `architecture.md`'s "Deferred" row updates to point here (picks revised pending spikes).

---

## 6. Phasing & sequencing

The M0–M6 slice is untouched. Post-slice suggested order:
1. **Deterministic core first** (shared dependency: physics determinism + Tier-3 rollback + sim replay). Build once, gate once (P1/NET-4 — the same cross-platform-hash spike).
2. **Physics** (Rapier behind the trait) — most foundational; enables gameplay and the deterministic core.
3. **Audio** (Firewheel behind the trait) — independent; can run in parallel with physics.
4. **Networking by tier:** Tier 0 already exists (editor collab) → Tier 1 (CRDT gameplay, ~free) → Tier 2 (server-authoritative default) → Tier 3 (rollback, last, gated on the deterministic core + Rapier #910 fix).

---

## 7. Validation gates — the "to be validated afterward" part

Run as throwaway `/spikes/{physics,audio,netcode}` sessions, M0-discipline (seeded, median+p99, two runs, cited numbers). Each settles an ADR **after** it passes. The **determinism gate (P1 ≡ NET-4) is the linchpin** — it blocks the rollback tier and the sim-replay story for both physics and networking.

**Physics**
- **P1 — Cross-platform bit-determinism (headline):** `rapier3d` + `enhanced-determinism`, seeded ~500-body+joints scene, fixed dt, 10k steps; hash serialized world. *Pass:* identical hash on Win-x86_64, Linux-x86_64, wasm32 (Chrome+Firefox), ideally macOS-ARM. *Fail:* → fixed-point investigation.
- **P2 — Deterministic input replay:** record `{frame, ordered inputs}`; replay = reset+re-feed+re-step. *Pass:* replayed end-state hash == original, ≥10k frames, cross-platform.
- **P3 — Snapshot/restore (gates rollback):** step→serialize→deserialize→step both; compare. *Pass:* identical. *(Expected fail today per #910 → converts the bug to a known go/no-go.)*
- **P4 — ECS/transaction integration:** drive Rapier via the `Physics` trait; add-body→suggest-collider→bind in ≤2 interactions; step+sync within the 16 ms hot-path budget; **zero `rapier::`/`parry::`/`glam` types outside the wrapper (grep gate)**.
- **P5 — Browser parity/perf:** deterministic build runs the lite-editor scene at interactive rate single-threaded; SIMD build measurably faster (quantify the tradeoff).
- **P6 — Robotics interop probe (observe, not go/no-go):** import a URDF/MJCF arm; gauge multibody stability vs the "credible-later" claim.

**Audio**
- **A — Trait + dual-target playback:** one `AudioBackend`/Firewheel plays a loop native AND in Chrome wasm; control via deltas only, zero per-sample JS crossing.
- **B — Spatial scaling+latency:** native ≥32 HRTF sources (AudioNimbus) <20 ms latency, audio-thread <~25% of a core, no underruns; browser ≥16 sources <50 ms post-gesture.
- **C — Autoplay/threading compliance:** first sound only after gesture + `resume()`; COOP/COEP unlocks worklet threading; documented single-thread fallback when SAB absent.
- **D — AI-SFX round-trip (strategic):** "add a footstep" → ElevenLabs SFX → auto-import → decode → pre-componentized `AudioSource` + default `SpatialParams` + bus → intent suggests listener → one-click bind → spatialized playback → one undoable transaction → survives reload. <~10 s wall-clock; cost within the 10-token margin.
- **E — Procedural offline:** FunDSP UI blip plays native+browser, zero network/token cost ("AI is a guest").
- **F — License gate:** confirm in writing generated SFX ship only embedded in componentized objects/projects, never bare files (ElevenLabs terms) — before any audio-marketplace work.

**Networking**
- **NET-1 — Declared replication round-trips:** two desktop instances, component `replicated:true, authority:shared-crdt`; mutate on one. *Pass:* delta propagates over the WS transport and applies on the peer **through the same commit pipeline, with zero code beyond the metadata flag.**
- **NET-2 — Two-browser CRDT convergence + merge-repair:** two wasm lite-editors, Loro Protocol over WebTransport, inject concurrent conflicting reparent + component write. *Pass:* converge <500 ms; merge-validation repairs the injected invalid states; no divergence after 1000 mixed ops.
- **NET-3 — GGRS rollback budget:** deterministic mini-sim behind GGRS over our transport (UDP native; matchbox browser), 2→4 players, injected 50–150 ms RTT + 5% loss. *Pass:* rollback <4 ms/frame p99 @2p, <8 ms @4p, bit-identical peer state (periodic hash).
- **NET-4 — Determinism gate (≡ P1, the linchpin):** identical input logs on x86_64 + ARM, hash state every tick for 10k ticks. *Pass:* identical hashes. *Fail:* Tier 3 blocked until float-policy remediation.
- **NET-5 — Tier opt-in without rewrite:** flip NET-1's scene from `authority:shared-crdt` to `authority:server` + add a server node. *Pass:* runs server-authoritative with prediction, **no gameplay-code change** — only the declaration + transport backend changed. (Validates the central pitch.)

**ADRs each gate will write (after passing):** ADR-007 networking (tiered + "declared networking" + the Bevy-coupling/transport-split finding) · ADR-008 physics (Rapier + the dual-config determinism strategy + real-world-sim scope) · ADR-009 audio (Firewheel + AI-audio token tie-in + the ElevenLabs embed-only constraint). Written from measured reality, per our evidence-driven rule — not now.

---

## 8. Sources (June 2026)

**Physics:** rapier.rs/docs/user_guides/rust/determinism · docs.rs/crate/rapier3d/latest · docs.rs/crate/avian3d/latest · docs.rs/crate/parry3d/latest · github.com/dimforge/rapier/issues/910 · dimforge.com/blog/2026/01/09/the-year-2025-in-dimforge · deepwiki.com/avianphysics/avian/10.3-determinism · developer.nvidia.com/newton-physics · github.com/newton-physics/newton · genesis-world.readthedocs.io · randomascii.wordpress.com/2013/07/16/floating-point-determinism · gafferongames.com/post/floating_point_determinism · github.com/SecondHalfGames/jolt-rust

**Audio:** lib.rs/crates/kira · github.com/BillyDM/firewheel · github.com/MaxenceMaire/audionimbus · mxncmr.com/blog/introducing-audionimbus · github.com/SamiPerttu/fundsp · github.com/RustAudio/cpal/issues/663 · phoronix.com/news/Steam-Audio-SDK-Fully-Open · elevenlabs.io/docs/overview/capabilities/sound-effects · terms.law/ai-output-rights/elevenlabs · stability.ai/news-updates (Stable Audio 3) · developer.chrome.com/blog/web-audio-autoplay · emscripten.org/docs/api_reference/wasm_audio_worklets.html

**Networking:** loro.dev/blog/loro-protocol · loro.dev/docs/tutorial/ephemeral · github.com/cBournhonesque/lightyear · github.com/UkoeHB/renet2 · docs.rs/crate/renet2_netcode · github.com/gschup/ggrs · github.com/johanhelsing/matchbox · docs.rs/crate/quinn/latest · docs.rs/aeronet · webrtc.ventures/2026/04/webtransport-is-now-baseline · croquet.dev/faq · arxiv.org/abs/2503.17826 (CRDT game-state sync) · en.wikipedia.org/wiki/Local-first_software
