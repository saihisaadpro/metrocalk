//! Sim-replay — the deterministic regeneration channel (M8.4), **distinct from Loro time-travel**
//! (ADR-021). A [`Recording`] holds the initial bodies + the ordered input log; replaying it re-derives
//! the trajectory **bit-identically** (the M8.1 P2 input-replay result, now through the `Physics` trait).
//!
//! **Scrub = [`Replay::seek`]; resume = seek-then-[`Replay::advance`].** Resume-from-scrub is itself
//! deterministic because a rewind **rebuilds the world from the recording** (a fresh broad-phase) rather
//! than deserializing a snapshot — which **sidesteps rapier #910** (the broad-phase-serde non-determinism
//! the M8.1 P3 spike pinned down). The M8.1 finding was: *fresh broad-phase rebuild reproduces the hash in
//! BOTH broad-phase modes*, so this rebuild-replay path is deterministic without needing
//! [`crate::BroadPhase::DeterministicResume`] — the #910 trap never bites because nothing is deserialized.
//!
//! This is the substrate the M8.4 transport (pause/scrub/resume) + the contact debugger run on: the
//! `Replay` cursor IS the live timeline engine, and the headless tests here are the P2/P3 proof.

use serde::{Deserialize, Serialize};

use crate::{
    BodyDesc, BodyHandle, ColliderDesc, Diagnostics, Physics, PhysicsConfig, Quat, RapierPhysics,
    Vec3,
};

/// One body in the recorded setup — a body + its collider, in **insertion order** (the index is the
/// body's stable identity across every replay; the caller maps it back to an ECS entity).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct RecordedBody {
    pub body: BodyDesc,
    pub collider: ColliderDesc,
}

/// A recorded input event — a one-shot impulse applied to a recorded body at a specific frame (the
/// "shove" in the test-#3 barrel scenario; the sim-replay **input channel**, M8.1 P2). Frame-stamped +
/// ordered so the replay is deterministic. The input is applied **before** the step that advances out of
/// `frame` (so `frame: 0` perturbs the very first step).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct InputEvent {
    pub frame: u64,
    /// Index into [`Recording::bodies`].
    pub body: usize,
    pub impulse: Vec3,
}

/// The full deterministic description of a sim run — initial state + ordered input log + config. This is
/// the artifact Loro stores as a document (ADR-021) and the **version-locked, shareable replay** a
/// captured bug becomes (M8.4 deliverable 3): the same recording regenerates bit-identical behaviour
/// across runs, sessions, and machines (per the M8.1 native-determinism verdict). Replaying it never
/// re-reads Loro — it re-runs the sim.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)]
pub struct Recording {
    pub config: PhysicsConfig,
    pub bodies: Vec<RecordedBody>,
    /// Frame-stamped impulses, kept sorted by frame (the deterministic input channel).
    pub inputs: Vec<InputEvent>,
}

impl Recording {
    #[must_use]
    pub fn new(config: PhysicsConfig) -> Self {
        Self {
            config,
            bodies: Vec::new(),
            inputs: Vec::new(),
        }
    }

    /// Record a body + its collider; returns its stable index (insertion order).
    pub fn add_body(&mut self, body: BodyDesc, collider: ColliderDesc) -> usize {
        self.bodies.push(RecordedBody { body, collider });
        self.bodies.len() - 1
    }

    /// Record an input impulse on a body at `frame` (kept frame-sorted for deterministic application).
    pub fn add_input(&mut self, frame: u64, body: usize, impulse: Vec3) {
        self.inputs.push(InputEvent {
            frame,
            body,
            impulse,
        });
        self.inputs.sort_by_key(|e| e.frame);
    }

    /// Build a fresh world from the recorded setup (no stepping). Returns the world + the body handles in
    /// insertion order (index → handle), so a caller maps a recorded body / result back to its entity.
    /// A collider that fails to build (a seam shape) is surfaced by the trait and skipped here — the
    /// handle is still pushed so the index parallelism with [`Self::bodies`] holds.
    #[must_use]
    pub fn build(&self) -> (RapierPhysics, Vec<BodyHandle>) {
        let mut world = RapierPhysics::new(self.config);
        let mut handles = Vec::with_capacity(self.bodies.len());
        for rb in &self.bodies {
            let h = world.add_body(&rb.body);
            let _ = world.add_collider(h, &rb.collider); // seam shapes explained by the trait, not faked
            handles.push(h);
        }
        (world, handles)
    }
}

/// A live replay **cursor** over a [`Recording`] — owns the world at the current [`Self::frame`]. This is
/// the M8.4 timeline engine: [`Self::advance`] steps one deterministic frame; [`Self::seek`] scrubs
/// (rebuild-from-recording on rewind, replay-forward otherwise). The rebuild-on-rewind is the P3 path that
/// sidesteps rapier #910. Holds no Loro/undo state — the trajectory is a regenerable projection (ADR-021).
pub struct Replay {
    recording: Recording,
    world: RapierPhysics,
    handles: Vec<BodyHandle>,
    frame: u64,
}

impl Replay {
    #[must_use]
    pub fn new(recording: Recording) -> Self {
        let (world, handles) = recording.build();
        Self {
            recording,
            world,
            handles,
            frame: 0,
        }
    }

    #[must_use]
    pub fn frame(&self) -> u64 {
        self.frame
    }

    #[must_use]
    pub fn recording(&self) -> &Recording {
        &self.recording
    }

    #[must_use]
    pub fn handles(&self) -> &[BodyHandle] {
        &self.handles
    }

    #[must_use]
    pub fn world(&self) -> &RapierPhysics {
        &self.world
    }

    /// One deterministic step: apply this frame's recorded inputs, advance one fixed `dt`, bump the
    /// cursor. Identical given an identical recording — the P2 guarantee.
    pub fn advance(&mut self) {
        for ev in &self.recording.inputs {
            if ev.frame == self.frame {
                if let Some(h) = self.handles.get(ev.body) {
                    self.world.apply_impulse(*h, ev.impulse);
                }
            }
        }
        self.world.step();
        self.frame += 1;
    }

    /// Scrub to `target`: on a **rewind**, rebuild the world from the recording (a fresh broad-phase — the
    /// P3 path that sidesteps #910), then replay forward to `target`. The resulting world is bit-identical
    /// to a continuous run to `target` (proven by [`mod@tests`]) — so resume-from-scrub is deterministic.
    pub fn seek(&mut self, target: u64) {
        if target < self.frame {
            let (world, handles) = self.recording.build();
            self.world = world;
            self.handles = handles;
            self.frame = 0;
        }
        while self.frame < target {
            self.advance();
        }
    }

    /// The current frame's body transforms, index-parallel to [`Recording::bodies`] — the projection the
    /// renderer reads (never Loro/undo). A removed body reads as identity.
    #[must_use]
    pub fn transforms(&self) -> Vec<(Vec3, Quat)> {
        self.handles
            .iter()
            .map(|h| {
                self.world
                    .transform(*h)
                    .unwrap_or(([0.0; 3], [0.0, 0.0, 0.0, 1.0]))
            })
            .collect()
    }

    /// Read-only contact/solver diagnostics at the current frame (the debugger seam) — non-mutating.
    #[must_use]
    pub fn diagnostics(&self) -> Diagnostics {
        self.world.diagnostics()
    }

    /// The deterministic world hash at the current frame — the P1/P2 equality key.
    #[must_use]
    pub fn world_hash(&self) -> String {
        self.world.world_hash()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BodyKind, ColliderShape};

    /// A small "barrel-ish" scenario: a ground, a stack of boxes, and a shove input partway through (the
    /// test-#3 setup, headless). Enough to exercise gravity, contacts, resting, and a recorded input.
    fn barrel_scene() -> Recording {
        let mut rec = Recording::new(PhysicsConfig::default());
        let ground = rec.add_body(
            BodyDesc::new(BodyKind::Fixed, [0.0, 0.0, 0.0]),
            ColliderDesc::new(ColliderShape::Cuboid {
                half_extents: [20.0, 0.5, 20.0],
            }),
        );
        let _ = ground;
        for i in 0..5 {
            rec.add_body(
                BodyDesc::new(BodyKind::Dynamic, [0.0, 1.2 + f64::from(i) * 0.9, 0.0]),
                ColliderDesc::new(ColliderShape::Cuboid {
                    half_extents: [0.4, 0.4, 0.4],
                }),
            );
        }
        // A shove on the top box at frame 120 (after it settles) — the "knock the stack" input.
        rec.add_input(120, 5, [3.0, 0.0, 0.0]);
        rec
    }

    #[test]
    fn replay_reproduces_the_continuous_trajectory() {
        // P2: a fresh replay-to-N hashes identically to a continuous run that never paused.
        let rec = barrel_scene();

        let mut cont = Replay::new(rec.clone());
        for _ in 0..300 {
            cont.advance();
        }
        let continuous = cont.world_hash();

        let mut replay = Replay::new(rec);
        replay.seek(300);
        assert_eq!(
            continuous,
            replay.world_hash(),
            "replay-to-N must reproduce the continuous-run hash (M8.1 P2 through the cursor)"
        );
    }

    #[test]
    fn resume_from_scrub_equals_continuous_across_cycles() {
        // P3 — the #910 divergence trap: scrub back, resume forward, and the end hash must equal the
        // continuous reference EVERY cycle (a single lucky match isn't proof; divergence shows up over
        // repeated rewind/replay). Rebuild-on-rewind (fresh broad-phase) is what makes this hold.
        let rec = barrel_scene();

        let mut reference = Replay::new(rec.clone());
        for _ in 0..400 {
            reference.advance();
        }
        let reference = reference.world_hash();

        let mut cursor = Replay::new(rec);
        for cycle in 0..3 {
            cursor.seek(150); // scrub back to the contact frame
            assert_eq!(cursor.frame(), 150);
            while cursor.frame() < 400 {
                cursor.advance(); // resume
            }
            assert_eq!(
                reference,
                cursor.world_hash(),
                "resume-from-scrub must equal the continuous run on cycle {cycle} (P3 — no #910 drift)"
            );
        }
    }

    #[test]
    fn scrub_back_reproduces_an_earlier_frames_hash() {
        // Scrubbing to an earlier frame reproduces THAT frame's state exactly (a forward run and a
        // rewind-replay agree) — the property the timeline slider relies on.
        let rec = barrel_scene();

        let mut fwd = Replay::new(rec.clone());
        for _ in 0..200 {
            fwd.advance();
        }
        let at_200 = fwd.world_hash();
        fwd.advance(); // move past 200 so seek(200) is a genuine rewind
        fwd.seek(200);
        assert_eq!(at_200, fwd.world_hash(), "scrub-back reproduces frame 200");
    }

    #[test]
    fn diagnostics_do_not_perturb_the_replay() {
        // The overlay is read-only: querying diagnostics (overlay ON) must not change the hash vs not
        // querying (overlay OFF) — the adversarial "the overlay perturbs the sim it inspects" guard.
        let rec = barrel_scene();

        let mut a = Replay::new(rec.clone());
        let mut b = Replay::new(rec);
        for _ in 0..250 {
            a.advance();
            let _ = a.diagnostics(); // overlay open: query every frame
            b.advance(); // overlay closed: never query
        }
        assert_eq!(
            a.world_hash(),
            b.world_hash(),
            "reading the diagnostic overlay must NOT perturb the trajectory"
        );
    }
}
