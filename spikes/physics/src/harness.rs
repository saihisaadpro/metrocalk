//! The precision-generic rapier-0.33 harness: a seeded ~500-body + joints scene, a fixed-`dt` step loop
//! that samples per-frame world hashes + diagnostics, input-replay (P2), and a snapshot/restore probe in
//! both broad-phase modes (P3). The math API is glam-migrated (rapier 0.32+) and the broad-phase is the
//! new `BroadPhaseBvh` (0.33) — `with_optimization_strategy(BvhOptimizationStrategy::None)` is the
//! snapshot-determinism-preserving mode the probe contrasts against the default.
//!
//! BUILD NOTE: the rapier-0.33 type/signature details (the glam `Vector` alias, the `step` argument list,
//! the `BroadPhaseBvh` constructors) are finalized against the compiler when the toolchain is available;
//! the *logic* (scene shape, hashing, diagnostics, replay, snapshot probe, provenance) is the spike.

#[cfg(feature = "f32")]
use rapier3d as rapier;
#[cfg(feature = "f64")]
use rapier3d_f64 as rapier;

use rapier::prelude::*;

use crate::{hash_bytes, FrameHash, Lcg, Provenance, FIXED_DT, PRECISION, SEED, STEPS};

/// The active math scalar (f64 or f32) — rapier's own alias.
pub type Real = rapier::math::Real;

/// How the snapshot probe (P3) configures the broad-phase.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BroadPhaseMode {
    /// Default broad-phase — expected to DIVERGE on resume-from-snapshot (rapier #910 / parry #402:
    /// the `BvhWorkspace` rebuild indices are `#[serde(skip)]`).
    Default,
    /// `BvhOptimizationStrategy::None` — determinism-preserving on resume, at a BVH-quality cost.
    NoneStrategy,
}

/// The seeded scene + the rapier state to step it. A ground plane + a settling pile of ~480 cuboids +
/// a 16-link revolute chain (the joints), so the run exercises contacts, islands, and articulation.
pub struct Sim {
    bodies: RigidBodySet,
    colliders: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    islands: IslandManager,
    broad_phase: BroadPhaseBvh,
    narrow_phase: NarrowPhase,
    ccd: CCDSolver,
    params: IntegrationParameters,
    gravity: Vector,
    pipeline: PhysicsPipeline,
    /// The chain bodies we feed recorded inputs to (P2).
    chain: Vec<RigidBodyHandle>,
    collider_shape_hashes: Vec<String>,
}

impl Sim {
    /// Build the seeded scene deterministically (same SEED → byte-identical scene on every target).
    #[must_use]
    pub fn seeded(mode: BroadPhaseMode) -> Self {
        let mut rng = Lcg::new(SEED);
        let mut bodies = RigidBodySet::new();
        let mut colliders = ColliderSet::new();
        let mut impulse_joints = ImpulseJointSet::new();
        let mut shape_hashes = Vec::new();

        // Ground.
        let ground = bodies.insert(RigidBodyBuilder::fixed().build());
        let ground_co = ColliderBuilder::cuboid(50.0, 0.5, 50.0).build();
        shape_hashes.push(hash_bytes(b"cuboid:50,0.5,50"));
        colliders.insert_with_parent(ground_co, ground, &mut bodies);

        // ~480 dynamic cuboids in a jittered grid that settles into a pile (deterministic positions).
        for i in 0..480u32 {
            let layer = i / 48;
            let within = i % 48;
            let x = ((within % 8) as Real) * 1.1 - 4.0 + rng.range(-0.05, 0.05) as Real;
            let z = ((within / 8) as Real) * 1.1 - 3.0 + rng.range(-0.05, 0.05) as Real;
            let y = 1.0 + (layer as Real) * 1.2;
            let rb = RigidBodyBuilder::dynamic()
                .translation(Vector::new(x, y, z))
                .build();
            let h = bodies.insert(rb);
            let s = 0.5;
            let co = ColliderBuilder::cuboid(s, s, s).density(1.0).build();
            colliders.insert_with_parent(co, h, &mut bodies);
        }
        shape_hashes.push(hash_bytes(b"cuboid:0.5,0.5,0.5"));

        // A 16-link revolute chain hanging from an anchor (the joints / articulation).
        let mut chain = Vec::new();
        let anchor = bodies.insert(
            RigidBodyBuilder::fixed()
                .translation(Vector::new(0.0, 20.0, 0.0))
                .build(),
        );
        let mut prev = anchor;
        for k in 0..16u32 {
            let link = bodies.insert(
                RigidBodyBuilder::dynamic()
                    .translation(Vector::new((k as Real + 1.0) * 0.6, 20.0, 0.0))
                    .build(),
            );
            let co = ColliderBuilder::cuboid(0.25, 0.1, 0.1).density(1.0).build();
            colliders.insert_with_parent(co, link, &mut bodies);
            let joint = RevoluteJointBuilder::new(Vector::Z)
                .local_anchor1(Vector::new(0.3, 0.0, 0.0))
                .local_anchor2(Vector::new(-0.3, 0.0, 0.0));
            impulse_joints.insert(prev, link, joint, true);
            chain.push(link);
            prev = link;
        }
        shape_hashes.push(hash_bytes(b"cuboid:0.25,0.1,0.1"));

        let broad_phase = match mode {
            BroadPhaseMode::Default => BroadPhaseBvh::new(),
            BroadPhaseMode::NoneStrategy => {
                BroadPhaseBvh::with_optimization_strategy(BvhOptimizationStrategy::None)
            }
        };

        let mut params = IntegrationParameters::default();
        params.dt = FIXED_DT as Real;

        Sim {
            bodies,
            colliders,
            impulse_joints,
            multibody_joints: MultibodyJointSet::new(),
            islands: IslandManager::new(),
            broad_phase,
            narrow_phase: NarrowPhase::new(),
            ccd: CCDSolver::new(),
            params,
            gravity: Vector::new(0.0, -9.81, 0.0),
            pipeline: PhysicsPipeline::new(),
            chain,
            collider_shape_hashes: shape_hashes,
        }
    }

    /// One fixed-`dt` step.
    pub fn step(&mut self) {
        self.pipeline.step(
            self.gravity,
            &self.params,
            &mut self.islands,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.ccd,
            &(),
            &(),
        );
    }

    /// Apply one recorded input frame (a deterministic torque impulse on the chain tip) — the P2 input
    /// stream, replayed identically.
    pub fn apply_input(&mut self, frame: u32) {
        let mut rng = Lcg::new(SEED ^ u64::from(frame));
        let tip = *self.chain.last().expect("chain");
        if let Some(rb) = self.bodies.get_mut(tip) {
            let tq = rng.range(-2.0, 2.0) as Real;
            rb.apply_torque_impulse(Vector::new(0.0, 0.0, tq), true);
        }
    }

    /// The deterministic world hash — blake3 over the serialized 8-component snapshot (the P1 equality
    /// key; the `PhysicsPipeline` holds no persistent state, so it's excluded).
    #[must_use]
    pub fn world_hash(&self) -> String {
        hash_bytes(&self.snapshot_bytes())
    }

    /// Serialize the full world (the snapshot the provenance + restore path use).
    #[must_use]
    pub fn snapshot_bytes(&self) -> Vec<u8> {
        let snap = (
            &self.islands,
            &self.narrow_phase,
            &self.bodies,
            &self.colliders,
            &self.impulse_joints,
            &self.multibody_joints,
            &self.ccd,
            &self.params,
            &self.gravity,
        );
        serde_json::to_vec(&snap).expect("serialize world")
    }

    /// Quality diagnostics at the current state (deliverable 3): total mechanical energy, contact count,
    /// max penetration.
    #[must_use]
    #[allow(clippy::cast_lossless, clippy::cast_possible_truncation)]
    pub fn diagnostics(&self) -> (f64, usize, f64) {
        let mut energy = 0.0f64;
        for (_, rb) in self.bodies.iter() {
            if rb.is_dynamic() {
                let m = rb.mass() as f64;
                let speed2 = rb.linvel().length_squared() as f64; // glam DVec3/Vec3
                let h = rb.translation().y as f64;
                energy += 0.5 * m * speed2 + m * 9.81 * h;
            }
        }
        let mut contacts = 0usize;
        let mut max_pen = 0.0f64;
        for pair in self.narrow_phase.contact_pairs() {
            if pair.has_any_active_contact() {
                contacts += 1;
                for mani in &pair.manifolds {
                    for pt in &mani.points {
                        max_pen = max_pen.max(-(pt.dist as f64));
                    }
                }
            }
        }
        (energy, contacts, max_pen)
    }

    /// The chain handles (stable arena indices, preserved across serialize/deserialize) — so a restored
    /// sim can be re-fed the same inputs in the snapshot probe.
    #[must_use]
    pub fn chain(&self) -> Vec<RigidBodyHandle> {
        self.chain.clone()
    }
    #[must_use]
    pub fn body_count(&self) -> usize {
        self.bodies.len()
    }
    #[must_use]
    pub fn joint_count(&self) -> usize {
        self.impulse_joints.len()
    }
    #[must_use]
    pub fn shape_hashes(&self) -> Vec<String> {
        self.collider_shape_hashes.clone()
    }
}

/// Run the determinism harness: build → step `STEPS` times (sampling frame hashes + diagnostics every
/// 1000 frames) with the recorded input stream → assemble the provenance envelope. Returns it.
#[must_use]
pub fn run(mode: BroadPhaseMode) -> Provenance {
    let mut sim = Sim::seeded(mode);
    let mut frames = Vec::new();
    #[cfg(not(target_arch = "wasm32"))]
    let mut step_us: Vec<f64> = Vec::with_capacity(STEPS as usize);
    for frame in 0..STEPS {
        sim.apply_input(frame);
        #[cfg(not(target_arch = "wasm32"))]
        let _t0 = std::time::Instant::now();
        sim.step();
        #[cfg(not(target_arch = "wasm32"))]
        step_us.push(_t0.elapsed().as_secs_f64() * 1e6);
        if frame % 1000 == 999 {
            let (energy, contacts, max_penetration) = sim.diagnostics();
            frames.push(FrameHash {
                frame,
                world_hash: sim.world_hash(),
                energy,
                contacts,
                max_penetration,
            });
        }
    }
    let final_world_hash = sim.world_hash();
    #[cfg(not(target_arch = "wasm32"))]
    let (step_us_p50, step_us_p99) = {
        step_us.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let n = step_us.len();
        (Some(step_us[n / 2]), Some(step_us[n * 99 / 100]))
    };
    #[cfg(target_arch = "wasm32")]
    let (step_us_p50, step_us_p99) = (None, None);
    Provenance {
        backend: "rapier3d-f64 0.33.0 / parry3d 0.28".to_string(),
        precision: PRECISION.to_string(),
        enhanced_determinism: true,
        fixed_dt: FIXED_DT,
        substep_policy: "fixed, recorded (no runtime-adaptive substepping in the authoritative config)"
            .to_string(),
        seed: SEED,
        body_creation_order: "deterministic index order from the seeded generator".to_string(),
        contact_ordering: "rapier deterministic (enhanced-determinism)".to_string(),
        units: "meters; gravity = -9.81 m/s² (y)".to_string(),
        broad_phase: match mode {
            BroadPhaseMode::Default => "BroadPhaseBvh (default)".to_string(),
            BroadPhaseMode::NoneStrategy => "BroadPhaseBvh::None (snapshot-determinism mode)".to_string(),
        },
        collider_shape_hashes: sim.shape_hashes(),
        frame_hashes: frames,
        final_world_hash,
        toolchain: env!("CARGO_PKG_VERSION").to_string(),
        steps: STEPS,
        body_count: sim.body_count(),
        joint_count: sim.joint_count(),
        step_us_p50,
        step_us_p99,
    }
}

/// Empirical precision probe: `(size_of::<Real>(), size_of::<Vector>())`. f64 → (8, 24 for a 3-component
/// f64 vector); f32 → (4, 12). Settles whether the public math type is genuinely double-precision or the
/// glam migration quantizes the API to f32 even in the `-f64` crate (a load-bearing finding).
#[must_use]
pub fn precision_sizes() -> (usize, usize) {
    (
        core::mem::size_of::<Real>(),
        core::mem::size_of::<Vector>(),
    )
}

/// Input-replay (P2): a fresh seeded sim, the SAME recorded input stream, re-stepped — the end hash MUST
/// equal the original. Returns (original_hash, replayed_hash).
#[must_use]
pub fn replay_check() -> (String, String) {
    let a = run(BroadPhaseMode::Default).final_world_hash;
    let b = run(BroadPhaseMode::Default).final_world_hash;
    (a, b)
}

/// Snapshot/restore probe (P3) — the rapier #910 / parry #402 question: is **resume-from-snapshot
/// REPRODUCIBLE**? (Not "restore == an uninterrupted run" — the broad-phase BVH isn't serialized, so a
/// resumed run legitimately differs from a continuous one; what #910 breaks is that two resumes of the
/// SAME snapshot diverge, because the `BvhWorkspace` rebuild indices are `#[serde(skip)]`.) Step half,
/// snapshot once, then restore + finish TWICE and compare the two resumed end-hashes:
///   default broad-phase → expected DIVERGE (a==b false): #910 present.
///   `BvhOptimizationStrategy::None` → expected REPRODUCIBLE (a==b true): the determinism-preserving fix.
/// Returns (resume_a_hash, resume_b_hash); equal = deterministic resume.
#[must_use]
pub fn snapshot_check(mode: BroadPhaseMode) -> (String, String) {
    let mut sim = Sim::seeded(mode);
    let half = STEPS / 2;
    for frame in 0..half {
        sim.apply_input(frame);
        sim.step();
    }
    let chain = sim.chain();
    let bytes = sim.snapshot_bytes();

    let finish = |bytes: &[u8], chain: Vec<RigidBodyHandle>| -> String {
        let mut r = Sim::from_snapshot(bytes, mode, chain);
        for frame in half..STEPS {
            r.apply_input(frame);
            r.step();
        }
        r.world_hash()
    };
    let a = finish(&bytes, chain.clone());
    let b = finish(&bytes, chain);
    (a, b)
}

impl Sim {
    /// Restore a sim from a serialized snapshot (the resume-from-snapshot path P3 probes). The broad-phase
    /// is reconstructed in `mode` (the default rebuild is non-deterministic per #910; `NoneStrategy` is
    /// the determinism-preserving rebuild).
    #[must_use]
    pub fn from_snapshot(bytes: &[u8], mode: BroadPhaseMode, chain: Vec<RigidBodyHandle>) -> Self {
        let (islands, narrow_phase, bodies, colliders, impulse_joints, multibody_joints, ccd, params, gravity): (
            IslandManager,
            NarrowPhase,
            RigidBodySet,
            ColliderSet,
            ImpulseJointSet,
            MultibodyJointSet,
            CCDSolver,
            IntegrationParameters,
            Vector,
        ) = serde_json::from_slice(bytes).expect("deserialize world");
        let broad_phase = match mode {
            BroadPhaseMode::Default => BroadPhaseBvh::new(),
            BroadPhaseMode::NoneStrategy => {
                BroadPhaseBvh::with_optimization_strategy(BvhOptimizationStrategy::None)
            }
        };
        Sim {
            bodies,
            colliders,
            impulse_joints,
            multibody_joints,
            islands,
            broad_phase,
            narrow_phase,
            ccd,
            params,
            gravity,
            pipeline: PhysicsPipeline::new(),
            chain, // stable handles from the pre-snapshot sim (so re-fed inputs target the same tip)
            collider_shape_hashes: Vec::new(),
        }
    }
}
