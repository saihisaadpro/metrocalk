//! **metrocalk-dst — Deterministic Simulation Testing (M13.1 / ADR-050).**
//!
//! The lead frontier capability (the dossier's §5 step 1): Metrocalk already has the rarest thing in game
//! engines — a **bit-identical, replayable simulation** (M8.1 / ADR-020: `rapier3d-f64` +
//! `enhanced-determinism`, a measured cross-ISA hash) and a **typed op-log**. DST turns that latent property
//! into a shipped capability: a gameplay/sim bug ships as a **`seed + op-log`** and the engine **replays it
//! bit-for-bit to any frame** on the CPU/sim path — a divergence is *reproduced*, not chased; an AI-authored
//! or AI-played sequence is **validated in simulation, not live**; "simulate years in minutes" is a test
//! mode. Unreal/Unity structurally cannot do this (render-coupled, non-injectable I/O — "On Determinism of
//! Game Engines", FM-T8.2).
//!
//! **Honest scope (load-bearing):**
//! - **The guarantee is CPU/sim-path only; GPU rendering is a SEPARABLE layer named OUT of it** (shader-
//!   compiler + cross-vendor FP variance defeat bit-identical *rendering*, FF-N2). We assert the **sim state
//!   hash**; we deliberately do **not** assert pixels.
//! - **Native-only.** ADR-020 is re-confirmed test-first in the current toolchain (1.92.0): native
//!   bit-identical, wasm32 diverges (FMA) ⇒ web = server-authoritative. "Deterministic across native AND
//!   web" is a **future gated on the Wasm-3.0 deterministic-profile runtime** (W3C 2026-05-27), measured
//!   when a runtime ships it — never claimed now.
//! - **Reuse, don't fork:** DST unifies *seed + op-log → replay* over the **shipped** channels — the M8.4
//!   `physics::{Recording, Replay}` (here) and the M12.5 Rules `RuleRecording`/`RuleReplay` (the sibling for
//!   logic divergences). No third replay subsystem.
//! - **Inject ALL non-determinism** (the TigerBeetle-VOPR shape): time (a fixed `dt`), RNG (a seeded
//!   [`Rng`]), and input (the op-log) are all data on the [`Scenario`] — nothing reads a wall clock, a GPU,
//!   or system entropy inside the recorded path (the determinism audit enforces it).

use serde::{Deserialize, Serialize};

use metrocalk_physics::{Recording, Replay};

/// The reproducibility envelope (ADR-020 #5) — the determinism contract a replay must match or be REJECTED,
/// not silently desynced. The bit-identical guarantee is scoped to this toolchain + physics config; a replay
/// under a different toolchain / precision / ISA-class is out-of-envelope (it may legitimately differ).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Envelope {
    /// rustc version the hash is scoped to (ADR-020 pins 1.92.0 — codegen affects the last FP bits).
    pub toolchain: String,
    /// The physics backend + version (rapier3d-f64).
    pub physics: String,
    /// The deterministic rapier config (libm, no SIMD/parallel). A successful build is itself the proof
    /// the deterministic path is active (it's mutually exclusive with simd/parallel — ADR-020 #1).
    pub enhanced_determinism: bool,
    /// The FIXED timestep in micros. Runtime-adaptive substepping is BANNED from the authoritative path
    /// (incompatible with a reproducible config — ADR-020 #4).
    pub dt_micros: u64,
}

impl Envelope {
    /// The current native deterministic envelope (the toolchain ADR-020 re-confirmed: 1.92.0, f64 +
    /// enhanced-determinism, fixed 1/60 dt).
    #[must_use]
    pub fn current() -> Self {
        Self {
            toolchain: "rustc-1.92.0".to_string(),
            physics: "rapier3d-f64-0.33".to_string(),
            enhanced_determinism: true,
            dt_micros: 16_666, // 1/60 s
        }
    }
}

/// A seed-driven deterministic RNG (`splitmix64`) — the **injected** randomness source. Every "random"
/// choice in a scenario draws from this, so a run is reproducible from its seed alone (the VOPR shape). An
/// un-seeded / system RNG inside the recorded path is forbidden by the determinism audit. Pure integer math
/// ⇒ no FP / ISA variance (unlike the physics path, this is bit-identical even on wasm).
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A reproducible f64 in [0, 1) — the 53 high bits, the standard construction (still pure integer math
    /// + one exact division, so it carries no FP non-determinism).
    pub fn next_f64(&mut self) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let num = (self.next_u64() >> 11) as f64;
        num / 9_007_199_254_740_992.0 // 2^53
    }
}

/// A DST scenario = **`seed` + op-log (`recording`) + `envelope`**. THIS is the "**a bug is a file**"
/// artifact (deliverable 3): serialize it, ship it, and it re-runs the exact failure **bit-for-bit on the
/// CPU/sim path** on any in-envelope machine. The op-log reuses the M8.4 [`Recording`] channel verbatim (no
/// parallel replay subsystem). The Rules-decision channel (M12.5) is the sibling for logic divergences.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Scenario {
    /// The injected RNG seed (any stochastic gameplay/AI choice is reproducible from it).
    pub seed: u64,
    /// The op-log: initial bodies + the frame-stamped input stream (the M8.4 deterministic input channel).
    pub recording: Recording,
    /// The reproducibility contract.
    pub envelope: Envelope,
}

impl Scenario {
    #[must_use]
    pub fn new(seed: u64, recording: Recording) -> Self {
        Self {
            seed,
            recording,
            envelope: Envelope::current(),
        }
    }

    /// Replay deterministically to `frame` and return the **CPU/sim state hash** — the equality key (the
    /// rapier world hash, proven bit-identical across native ISAs in ADR-020 / M8.1). NOTHING here reads a
    /// wall clock, a GPU, or an un-seeded RNG. This is the *only* thing the DST guarantee asserts — pixels
    /// are explicitly NOT hashed (the GPU path is out of the guarantee).
    #[must_use]
    pub fn state_hash_at(&self, frame: u64) -> String {
        let mut replay = Replay::new(self.recording.clone());
        replay.seek(frame);
        replay.world_hash()
    }

    /// The reproducibility check — the heart of DST: replay to `frame` `runs` times and confirm every hash
    /// is identical. A divergence is a determinism regression (the CI gate). `runs.max(2)` enforces the
    /// "≥2 runs" discipline (a single match isn't proof).
    #[must_use]
    pub fn reproduces_at(&self, frame: u64, runs: usize) -> bool {
        let first = self.state_hash_at(frame);
        (1..runs.max(2)).all(|_| self.state_hash_at(frame) == first)
    }

    /// The "**bug = a file**" artifact — **bincode** (binary, LOSSLESS). A determinism artifact MUST be
    /// bit-exact: JSON's shortest-float text round-trip can shift an f64 by 1 ULP, which empirically changes
    /// the deterministic trajectory hash (an M13.1 finding) — so the reproduction artifact is binary, where
    /// every f64 is its raw bytes. This is the file you ship: it re-runs the exact failure bit-for-bit on any
    /// in-envelope machine.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        bincode::serialize(self).expect("scenario serializes (pure data)")
    }

    /// Reload a "bug = a file" artifact (bincode).
    ///
    /// # Errors
    /// Returns the bincode error if the artifact is malformed / out of format.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(bytes)
    }

    /// A **human-readable summary** of the artifact (pretty JSON — diffable, shareable) for a bug report /
    /// PR. NOT the reproduction artifact: JSON float text is not guaranteed bit-exact, so reproduction always
    /// goes through [`Self::to_bytes`]/[`Self::from_bytes`]. This is the "here's what the bug is" view.
    #[must_use]
    pub fn summary(&self) -> String {
        serde_json::to_string_pretty(&serde_json::json!({
            "seed": self.seed,
            "envelope": self.envelope,
            "bodies": self.recording.bodies.len(),
            "inputs": self.recording.inputs.len(),
            "note": "reproduction is via the bincode artifact (to_bytes); this JSON is a human summary only",
        }))
        .expect("summary serializes")
    }
}

/// The verdict from validating an AI-authored/played sequence in the DST sim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimVerdict {
    pub frame: u64,
    /// The CPU/sim state hash at the validated frame (so a passing run is itself a reproducible artifact).
    pub hash: String,
    /// Whether the convergence predicate held — the gate before going live.
    pub converged: bool,
}

/// **AI-validated-in-sim** (leg #3 × #5, deliverable 4): run an AI-authored/played sequence (a [`Scenario`])
/// in the DST sim and assert a **convergence predicate** on the replayed state **before it ever touches a
/// live session**. This is the substrate moat — the AI is validated in *simulation*, not live. This builds
/// the **validation hook**; the LLM / the verifiable-authoring grammar is M13.5 (named seam). The
/// `partition_deterministic` guard (M12.5) keeps any non-deterministic plugin/neural op out of the recorded
/// path, so what's validated here is exactly what runs live.
#[must_use]
pub fn validate_in_sim<F: Fn(&Replay) -> bool>(
    scenario: &Scenario,
    frame: u64,
    converges: F,
) -> SimVerdict {
    let mut replay = Replay::new(scenario.recording.clone());
    replay.seek(frame);
    SimVerdict {
        frame,
        hash: replay.world_hash(),
        converged: converges(&replay),
    }
}

/// **FF-N2 — the Core-ECS commutativity leg** (Kuper, OOPSLA 2025): a system-set whose component **write-
/// sets are disjoint** can run in **any order with a bit-identical result**, so it *could* parallelize
/// deterministically. This module is the **spike-level proof on ONE system-set** — full effect-typed
/// scheduling is the M13.7-adjacent future; we do **NOT** rewrite the scheduler here. The compile-time
/// foundation under DST: determinism that's a *typed property*, not just an empirical hash.
pub mod commute {
    use std::collections::BTreeMap;

    /// A component store keyed by name (a `BTreeMap` — ORDERED iteration, so its hash is process-stable,
    /// unlike a `HashMap` with a per-process random seed; the same discipline the determinism audit wants).
    pub type Store = BTreeMap<&'static str, i64>;

    /// A system declares which components it **writes**. Two systems COMMUTE iff their write-sets are
    /// disjoint (neither reads the other's writes in this model — a deliberately simple, checkable property).
    pub struct System {
        pub name: &'static str,
        pub writes: &'static [&'static str],
        pub run: fn(&mut Store),
    }

    /// Disjoint write-sets ⇒ the systems commute (order-independent, bit-identical). The checkable property.
    #[must_use]
    pub fn commutes(a: &System, b: &System) -> bool {
        !a.writes.iter().any(|w| b.writes.contains(w))
    }

    /// Run a system-set over a store in a given order.
    pub fn run_order(systems: &[&System], order: &[usize], state: &mut Store) {
        for &i in order {
            (systems[i].run)(state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrocalk_physics::{
        BodyDesc, BodyKind, ColliderDesc, ColliderShape, PhysicsConfig, Recording,
    };

    /// The DST seed (the M8.1 spike seed — `"METROCAL"` as bytes, the documented reproducible-RNG seed).
    const SEED: u64 = 0x4D45_5452_4F43_414C;

    /// A real, **ordering-sensitive** divergence scenario: a 6-box stack + a shove at frame 120 that knocks
    /// it over. The settled arrangement is sensitive to contact ordering + the solver — EXACTLY the class of
    /// scenario where a non-deterministic engine diverges run-to-run (FM-T8.2's CARLA result). Our engine
    /// reproduces it bit-for-bit (ADR-020 f64 + enhanced-determinism).
    fn stack_scenario() -> Scenario {
        let mut rec = Recording::new(PhysicsConfig::default());
        rec.add_body(
            BodyDesc::new(BodyKind::Fixed, [0.0, 0.0, 0.0]),
            ColliderDesc::new(ColliderShape::Cuboid {
                half_extents: [20.0, 0.5, 20.0],
            }),
        );
        for i in 0..6 {
            rec.add_body(
                BodyDesc::new(BodyKind::Dynamic, [0.0, 1.2 + f64::from(i) * 0.9, 0.0]),
                ColliderDesc::new(ColliderShape::Cuboid {
                    half_extents: [0.4, 0.4, 0.4],
                }),
            );
        }
        rec.add_input(120, 6, [4.0, 0.0, 0.0]); // the shove that diverges the stack
        Scenario::new(SEED, rec)
    }

    #[test]
    fn the_spike_a_real_divergence_reproduces_bit_for_bit() {
        // THE GO/NO-GO GATE (deliverable 1, measured ≥2 runs): a divergence-sensitive scenario replays
        // bit-for-bit to the diverging frame on the CPU/sim path. Same seed + op-log → IDENTICAL state hash,
        // every run (3 here ≥ the 2-run rule). A NON-deterministic engine would drift here; ours doesn't.
        let s = stack_scenario();
        let h = s.state_hash_at(300);
        assert!(
            s.reproduces_at(300, 3),
            "bit-for-bit across 3 replays at the settled frame: GO"
        );
        assert!(
            s.reproduces_at(121, 3),
            "bit-for-bit at the diverging (post-shove) frame too"
        );

        // The hash actually REFLECTS the trajectory (it isn't a constant): drop the shove → a different end
        // state → a different hash (so "reproduces" is a real property, not a vacuous one).
        let mut quiet = stack_scenario();
        quiet.recording.inputs.clear();
        assert_ne!(s.state_hash_at(300), quiet.state_hash_at(300));

        println!("[DST spike] stack@frame300 hash = {h} — bit-identical across 3 replays (GO, {} toolchain)", s.envelope.toolchain);
    }

    #[test]
    fn a_bug_is_a_file_round_trips_bit_for_bit() {
        // Deliverable 3 — the "a bug is a seed+op-log file" artifact: serialize the scenario, reload it, and
        // it reproduces the EXACT same state hash. The shareable, version-locked reproducible bug.
        let s = stack_scenario();
        // The LOSSLESS bincode artifact round-trips EXACTLY (every f64 as raw bytes) — so the reloaded file
        // reproduces the failure BIT-FOR-BIT, both at the post-shove diverging frame and the settled frame.
        let artifact = s.to_bytes();
        let loaded = Scenario::from_bytes(&artifact).expect("artifact reloads");
        assert_eq!(
            loaded, s,
            "the bincode artifact round-trips bit-exactly (seed + op-log + envelope)"
        );
        assert_eq!(
            loaded.state_hash_at(121),
            s.state_hash_at(121),
            "the file reproduces the post-shove frame bit-for-bit"
        );
        assert_eq!(
            loaded.state_hash_at(300),
            s.state_hash_at(300),
            "the file reproduces the settled frame bit-for-bit"
        );

        // The human summary (JSON) is for a bug report — it names the seed + envelope but is NOT the
        // reproduction path (JSON float text isn't guaranteed bit-exact — the M13.1 finding).
        let summary = s.summary();
        assert!(
            summary.contains("\"seed\"")
                && summary.contains("rustc-1.92.0")
                && summary.contains("human summary only")
        );
    }

    #[test]
    fn injected_rng_is_reproducible_and_seed_sensitive() {
        // Inject-all-non-determinism: the seeded RNG is reproducible (same seed → same stream) AND
        // seed-sensitive (different seed → different stream) — no un-seeded/system RNG in the recorded path.
        let seq = |seed: u64| {
            let mut r = Rng::new(seed);
            [r.next_u64(), r.next_u64(), r.next_u64()]
        };
        assert_eq!(
            seq(SEED),
            seq(SEED),
            "same seed → identical stream (reproducible)"
        );
        assert_ne!(
            seq(SEED),
            seq(SEED + 1),
            "different seed → different stream (seed-sensitive)"
        );
        // f64 draws stay in [0,1) and are likewise reproducible.
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..100 {
            let x = a.next_f64();
            assert!((0.0..1.0).contains(&x));
            assert_eq!(x, b.next_f64());
        }
    }

    #[test]
    fn ai_authored_sequence_is_validated_in_sim_before_going_live() {
        // Deliverable 4 — the substrate moat (leg #3 × #5): an AI-played sequence (a Scenario) is validated
        // in the DST sim. A converging assertion PASSES; a non-converging one is REJECTED — before it ever
        // touches a live session. (The validation hook; the LLM/grammar is the M13.5 seam.)
        let s = stack_scenario();
        // converges: after the shove + settle, no box has fallen through the ground plane (a sane outcome).
        let good = validate_in_sim(&s, 300, |r| r.transforms().iter().all(|(p, _)| p[1] > -1.0));
        assert!(
            good.converged,
            "a physically-sane AI sequence validates in sim"
        );
        // a bogus claim (every box ends exactly at the origin) is REJECTED by the sim — caught before live.
        let bad = validate_in_sim(&s, 300, |r| {
            r.transforms()
                .iter()
                .all(|(p, _)| p[0] == 0.0 && p[1] == 0.0)
        });
        assert!(
            !bad.converged,
            "a non-converging AI sequence is rejected in sim, not live"
        );
        // the verdict carries the reproducible hash (a passing validation is itself a shareable artifact).
        assert_eq!(good.hash, s.state_hash_at(300));
    }

    #[test]
    fn ff_n2_disjoint_writes_commute_bit_identically() {
        // FF-N2 spike sub-result (deliverable 6): two systems with DISJOINT component writes run in either
        // order with a BIT-IDENTICAL result (they commute → could parallelize deterministically); two
        // systems writing the SAME component do NOT commute (the property is real, not vacuous). ONE
        // system-set proven; full effect-typed scheduling is the M13.7-adjacent named future (NOT here).
        use commute::{commutes, run_order, Store, System};
        static GRAVITY: System = System {
            name: "gravity",
            writes: &["vel"],
            run: |s| *s.entry("vel").or_insert(0) -= 10,
        };
        static HEAL: System = System {
            name: "heal",
            writes: &["hp"],
            run: |s| *s.entry("hp").or_insert(100) += 5,
        };
        static DAMAGE: System = System {
            name: "damage",
            writes: &["hp"],
            run: |s| *s.entry("hp").or_insert(100) -= 3,
        };
        // The checkable property:
        assert!(
            commutes(&GRAVITY, &HEAL),
            "disjoint writes (vel vs hp) commute"
        );
        assert!(
            !commutes(&HEAL, &DAMAGE),
            "both write hp → do NOT commute (real, not vacuous)"
        );

        // The consequence: commuting systems are order-INDEPENDENT → bit-identical either way.
        let run = |order: &[usize]| {
            let mut st: Store = Store::new();
            run_order(&[&GRAVITY, &HEAL], order, &mut st);
            st
        };
        assert_eq!(
            run(&[0, 1]),
            run(&[1, 0]),
            "commuting system-set: order-independent → bit-identical"
        );

        // And the counter-example: non-commuting systems CAN differ by order (proving the disjointness check
        // is load-bearing — here the order doesn't change the final hp sum, so use a read-modify that does).
        static SET10: System = System {
            name: "set10",
            writes: &["hp"],
            run: |s| {
                s.insert("hp", 10);
            },
        };
        static DOUBLE: System = System {
            name: "double",
            writes: &["hp"],
            run: |s| {
                let v = *s.get("hp").unwrap_or(&1);
                s.insert("hp", v * 2);
            },
        };
        let run2 = |order: &[usize]| {
            let mut st: Store = Store::new();
            run_order(&[&SET10, &DOUBLE], order, &mut st);
            st
        };
        assert!(!commutes(&SET10, &DOUBLE));
        assert_ne!(
            run2(&[0, 1]),
            run2(&[1, 0]),
            "non-commuting (both write hp): order CHANGES the result"
        );
    }

    #[test]
    fn the_recorded_path_injects_all_nondeterminism_audit() {
        // Determinism audit (deliverable 5 / the AVOID list): the recorded artifact is PURE DATA — seed +
        // op-log + envelope. There is no field that can HOLD a wall clock, a GPU handle, or system entropy
        // (the Scenario is `Serialize`able by construction). This asserts the envelope discipline + the
        // injected seed; the type system enforces the rest (a clock/GPU handle isn't serde data).
        let s = stack_scenario();
        assert!(
            s.envelope.enhanced_determinism,
            "the deterministic rapier config (libm, no SIMD) — ADR-020 #1"
        );
        assert!(
            s.envelope.dt_micros > 0,
            "a FIXED dt — runtime-adaptive substepping is banned (ADR-020 #4)"
        );
        assert_ne!(
            s.seed, 0,
            "an EXPLICIT injected seed, not system entropy (the VOPR shape)"
        );
        // The op-log IS the shipped M8.4 Recording channel (no parallel replay subsystem — reuse, don't fork).
        let _: &Recording = &s.recording;
        // The envelope round-trips (a replay can reject an out-of-envelope artifact rather than desync).
        assert_eq!(Envelope::current(), s.envelope);
    }

    #[test]
    fn years_in_minutes_replay_throughput() {
        // "Simulate years in minutes" as a test mode (deliverable 1 framing): headless replay throughput.
        // Not a gate (a perf observation, single-box) — measured, never invented; the min-spec number is
        // owed-tracked. 3600 frames = 1 minute of sim @60 Hz over the 7-body stack.
        let s = stack_scenario();
        let mut replay = Replay::new(s.recording.clone());
        let n = 3600u64;
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            replay.advance();
        }
        let secs = t0.elapsed().as_secs_f64();
        let fps = f64::from(u32::try_from(n).unwrap()) / secs;
        // A sanity floor only (the real number is logged): headless replay must outrun real time by a lot.
        assert!(
            fps > 60.0,
            "headless replay must beat real-time (got {fps:.0} frames/s)"
        );
        println!("[DST] headless replay throughput: {fps:.0} frames/s ({:.1}x real-time) over the stack scenario", fps / 60.0);
    }
}
