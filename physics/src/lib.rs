//! `metrocalk-physics` — the project-owned physics subsystem (M8.2). Rapier (0.33 + Parry 0.28) wrapped
//! behind the [`Physics`] trait so **no `rapier::` / `parry::` type crosses this crate's public surface**
//! (invariant 5, CI grep-gated — the same discipline as `flecs_ecs`-in-`/ecs` and `gltf::`-in-the-importer).
//!
//! Precision + determinism follow the **M8.1 verdict** (ADR-020): `f64` + `enhanced-determinism` is the
//! AUTHORITATIVE config — bit-identical across native ISAs (x86_64 + arm64), the substrate for sim-replay
//! and rollback netcode. The SIMD/parallel `fast` config (a build feature, mutually exclusive with
//! determinism) is for throwaway non-networked single-player only.
//!
//! **Boundary math type (a deliberate decision per the M8.2 brief):** the public surface uses our OWN
//! plain arrays — [`Vec3`] = `[f64; 3]`, [`Quat`] = `[f64; 4]` (xyzw) — not glam and not rapier's glam.
//! Zero external-type coupling, trivially serde-able, wasm-clean; the renderer/ECS convert to glam `f32`
//! on their side. A `rapier::`/`parry::` type crossing this boundary would be the leak the gate catches.
//!
//! **Determinism is preserved through the wrapper** (the M8.1 hash holds): the step loop is fixed-`dt`,
//! the body/contact ordering is rapier's deterministic order, and [`Physics::world_hash`] hashes the same
//! serialized world the spike did. Snapshot/restore honors the M8.1 P3 finding — pick
//! [`BroadPhase::DeterministicResume`] when a declared fidelity needs resume-determinism (rapier #910).
//!
//! `sim-replay is a distinct channel from Loro time-travel` (ADR-021): a deterministic engine + recorded
//! ordered input + fixed `dt` regenerates the trajectory; Loro stores the initial state + input log as a
//! document and never re-runs the sim. This crate owns the former; it shares only the initial snapshot.

mod rapier_backend;
pub mod replay;

pub use rapier_backend::{derive_collider, explain_contact, RapierPhysics};
pub use replay::{InputEvent, RecordedBody, Recording, Replay};

use serde::{Deserialize, Serialize};

/// A 3-vector in world space (meters, or m/s for velocities). Our own boundary type — no glam/rapier leak.
pub type Vec3 = [f64; 3];
/// A unit quaternion `[x, y, z, w]`. Our own boundary type.
pub type Quat = [f64; 4];

/// Opaque, stable handle to a rigid body (packs rapier's generational arena index; survives snapshot/
/// restore). The inner value is not interpretable outside this crate.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct BodyHandle(pub(crate) u64);

/// Opaque, stable handle to a collider.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct ColliderHandle(pub(crate) u64);

/// Opaque, stable handle to a joint.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct JointHandle(pub(crate) u64);

/// How a body participates in the simulation (mirrors the registry's `RigidBody` kind, M8.2).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum BodyKind {
    /// Fully simulated — gravity, contacts, joints move it.
    Dynamic,
    /// Immovable world geometry (the ground, a wall) — infinite mass.
    Fixed,
    /// Moved by the game's code each frame (position-driven); pushes dynamics but isn't pushed.
    KinematicPosition,
    /// Moved by a commanded velocity (velocity-driven kinematic).
    KinematicVelocity,
}

/// A collider's geometry — an **open** enum (invariant: `/core` must not bake in a convex-only
/// assumption). Primitive + convex-hull + tri-mesh are real (Parry 0.28); the experimental/deferred
/// rungs surface their limits honestly via [`PhysicsError::UnsupportedShape`] rather than silently
/// faking a shape.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ColliderShape {
    /// A sphere of `radius` (the canonical dynamic primitive — pairs with the ball test mesh).
    Ball { radius: f64 },
    /// An axis-aligned box of the given half-extents.
    Cuboid { half_extents: Vec3 },
    /// A capsule (cylinder + hemispherical caps) along the Y axis.
    Capsule { half_height: f64, radius: f64 },
    /// The convex hull of a point cloud (e.g. a mesh's vertices) — real (Parry quickhull).
    ConvexHull { points: Vec<Vec3> },
    /// A static triangle mesh (level/ground geometry from an imported mesh) — real (Parry `TriMesh`).
    /// Not valid for a *dynamic* body (no well-defined mass) — use a convex approximation for dynamics.
    TriMesh {
        vertices: Vec<Vec3>,
        /// Triangle indices, flattened (length must be a multiple of 3).
        indices: Vec<u32>,
    },
    /// Approximate a concave mesh as a union of convex pieces (VHACD). **Seam:** not wired in M8.2 —
    /// resolves to [`PhysicsError::UnsupportedShape`] with the reason, never a silent convex hull.
    ConvexDecomposition {
        vertices: Vec<Vec3>,
        indices: Vec<u32>,
    },
    /// Parry 0.28 experimental voxels. **Seam (limits surfaced):** no auto mass/inertia for dynamic
    /// voxel bodies, no shape-casting, no voxel↔voxel / voxel↔mesh — so M8.2 declines it for dynamic
    /// bodies with an explicit reason rather than producing a body that silently misbehaves.
    Voxels { size: Vec3, resolution: u32 },
    /// Signed-distance-field dynamic collider. **Deferred seam** (M8.5) — explained, not faked.
    Sdf,
}

/// A collider to attach to a body.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ColliderDesc {
    pub shape: ColliderShape,
    /// Mass density (kg/m³) — drives a dynamic body's mass/inertia. `1.0` is a sane default.
    pub density: f64,
    pub friction: f64,
    pub restitution: f64,
}

impl ColliderDesc {
    /// A unit-density collider of `shape` with mild friction and no bounce.
    #[must_use]
    pub fn new(shape: ColliderShape) -> Self {
        Self {
            shape,
            density: 1.0,
            friction: 0.5,
            restitution: 0.0,
        }
    }
}

/// A collider derived from a mesh — the M8.3 collision-shape generation (the piece M4/ADR-014 deferred).
/// Geometry only; no rapier/parry type crosses this. Carries the FIT metrics so the authoring layer can
/// explain a concave-dynamic choice ("convex hull, fit error N %") instead of silently approximating.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct DerivedCollider {
    /// The generated shape — a [`ColliderShape::ConvexHull`] of the mesh vertices (Parry hulls them at
    /// collider build). A dynamic body needs a convex shape; this is the honest default.
    pub shape: ColliderShape,
    /// Fraction of the convex hull's volume NOT filled by the mesh (`0.0` = a perfect convex fit; higher
    /// = more concave). The "report the error" the concave-dynamic warning surfaces.
    pub fit_error: f32,
    /// `true` when `fit_error` exceeds the concavity threshold (10 %) — the mesh is concave, so a dynamic
    /// body should use this hull (or voxels, or stay static), never the raw concave mesh.
    pub concave: bool,
    pub vertex_count: usize,
}

/// A body to add to the world.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct BodyDesc {
    pub kind: BodyKind,
    pub translation: Vec3,
    pub rotation: Quat,
    pub linvel: Vec3,
    pub angvel: Vec3,
}

impl BodyDesc {
    /// A body of `kind` at `translation`, identity-rotated and at rest.
    #[must_use]
    pub fn new(kind: BodyKind, translation: Vec3) -> Self {
        Self {
            kind,
            translation,
            rotation: [0.0, 0.0, 0.0, 1.0],
            linvel: [0.0; 3],
            angvel: [0.0; 3],
        }
    }
}

/// A joint constraining two bodies.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub enum JointDesc {
    /// A hinge about `axis`, anchored at `anchor_a` on body A and `anchor_b` on body B (local frames).
    Revolute {
        axis: Vec3,
        anchor_a: Vec3,
        anchor_b: Vec3,
    },
    /// A rigid weld — bodies hold their relative pose.
    Fixed { anchor_a: Vec3, anchor_b: Vec3 },
    /// A ball-and-socket (3 rotational DOF) at the anchors.
    Spherical { anchor_a: Vec3, anchor_b: Vec3 },
}

/// Which broad-phase strategy the world uses — the M8.1 P3 knob.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum BroadPhase {
    /// Default BVH — best query quality; resume-from-snapshot is NOT bit-reproducible (rapier #910).
    Default,
    /// `BvhOptimizationStrategy::None` — resume-from-snapshot IS reproducible, at a BVH-quality cost.
    /// Use when a declared fidelity needs deterministic resume (rollback Tier-3, scrub/resume M8.4).
    DeterministicResume,
}

/// The fixed simulation configuration (M8.1: substep policy is fixed + recorded — runtime-adaptive
/// substepping is incompatible with the authoritative config and is therefore not offered).
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct PhysicsConfig {
    /// The fixed timestep (seconds). Default 1/60.
    pub fixed_dt: f64,
    pub gravity: Vec3,
    pub broad_phase: BroadPhase,
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self {
            fixed_dt: 1.0 / 60.0,
            gravity: [0.0, -9.81, 0.0],
            broad_phase: BroadPhase::Default,
        }
    }
}

/// Fidelity as a **declared property** (P7) — the physics sibling of "networking as a declared property."
/// A body/scene declares the fidelity it wants; [`Fidelity::resolve`] maps it to a real M8.1 config, and
/// any gap is *explained*, never silently downgraded. Only the rungs M8.1 proved are real; the rest
/// resolve to the nearest real config and say what changes when they land (the M8.3 surface / M8.5 fill).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Fidelity {
    /// Fastest, roughest — editor preview / placement. (Real: gameplay config; preview is a tuning, not
    /// a different solver, so it resolves exactly.)
    Preview,
    /// The default interactive game tier — deterministic f64, fixed dt. **Real (M8.1).**
    Gameplay,
    /// Film-grade contact softness / sub-stepping. **Seam:** resolves to gameplay (explained).
    Cinematic,
    /// Engineering-validation accuracy (energy/penetration bounds). **Seam:** resolves to gameplay.
    Validation,
    /// Maximum stability for tall stacks / stiff joints. **Seam:** resolves to gameplay.
    Robust,
    /// Differentiable / RL inference. **Seam (interop-only, indefinitely — ADR-020):** resolves to gameplay.
    Inference,
}

/// The outcome of resolving a declared [`Fidelity`] to a real config — with the honesty payload.
#[derive(Clone, Debug, PartialEq)]
pub struct FidelityResolution {
    pub requested: Fidelity,
    /// The fidelity actually used — equals `requested` only when that rung is built.
    pub resolved: Fidelity,
    /// `true` when the requested rung exists; `false` when we fell back to the nearest real one.
    pub is_exact: bool,
    /// The config the engine will run.
    pub config: PhysicsConfig,
    /// Human-readable: what was used and — if a fallback — what changes when the real rung lands.
    pub explanation: String,
}

impl Fidelity {
    /// Resolve to a real M8.1 config. Built rungs resolve exactly; unbuilt ones fall back to the nearest
    /// real config with an explanation (the resolve-and-explain guard — never a silent downgrade, never a
    /// fake tier claiming quality it doesn't deliver).
    #[must_use]
    pub fn resolve(self) -> FidelityResolution {
        let gameplay = PhysicsConfig::default();
        match self {
            Self::Preview => FidelityResolution {
                requested: self,
                resolved: Self::Gameplay,
                is_exact: true, // preview is a tuning of the gameplay solver, not a separate rung
                config: gameplay,
                explanation: "preview = the deterministic gameplay solver (M8.1 f64 config)."
                    .into(),
            },
            Self::Gameplay => FidelityResolution {
                requested: self,
                resolved: Self::Gameplay,
                is_exact: true,
                config: gameplay,
                explanation:
                    "gameplay = the deterministic f64 + enhanced-determinism config (M8.1).".into(),
            },
            higher => {
                let what_lands = match higher {
                    Self::Cinematic => "higher sub-step counts + softer contact materials for film-grade settling",
                    Self::Validation => "energy/penetration error bounds + a validated solver for engineering accuracy",
                    Self::Robust => "more solver iterations + CCD for tall stacks and stiff joints",
                    Self::Inference => "a differentiable/interop path (ADR-020 keeps this interop-only)",
                    _ => "the requested rung",
                };
                FidelityResolution {
                    requested: self,
                    resolved: Self::Gameplay,
                    is_exact: false,
                    config: gameplay,
                    explanation: format!(
                        "{self:?} not yet built — running the gameplay config (M8.1 f64). When it lands: {what_lands}."
                    ),
                }
            }
        }
    }
}

/// One active contact, exposed read-only for the M8.4 contact debugger (the M8.1 diagnostics, live). The
/// post-solve impulses + friction state are what the overlay color-codes and [`explain_contact`] narrates
/// — the physics analog of "✅/❌ why this is greyed out" (the M3.1/ADR-016 discipline). Every field is
/// **measured from the solved manifold**, never estimated; the seams (per-island solver residual,
/// time-of-impact) are named honestly where rapier 0.33 doesn't expose them, not faked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Contact {
    pub body_a: BodyHandle,
    pub body_b: BodyHandle,
    /// The deepest manifold point in **world space** (the contact the overlay draws + the click target).
    pub point: Vec3,
    /// Contact normal (unit, pointing from A toward B).
    pub normal: Vec3,
    /// Penetration depth (positive = overlapping). The position residual the solver couldn't fully
    /// resolve this frame — the honest "residual" proxy (rapier 0.33 exposes no per-island solver
    /// residual; this is the geometric one, labelled as such).
    pub depth: f64,
    /// Normal impulse applied by the solver this step (N·s) — how hard the contact pushed.
    pub normal_impulse: f64,
    /// Tangential (friction) impulse magnitude this step (N·s).
    pub tangent_impulse: f64,
    /// Combined friction coefficient at this contact (the friction cone's slope).
    pub friction: f64,
    /// Combined restitution (bounciness) at this contact.
    pub restitution: f64,
    /// `true` when the friction impulse is at the cone limit (`|tangent| ≈ μ·normal`) — the contact is
    /// **slipping**, a classic jitter source the debugger flags.
    pub friction_saturated: bool,
    /// Persistent feature id of the deepest point — stable while the same surface features touch. A value
    /// that **flickers** frame-to-frame is the signature of jitter (the overlay/`explain` surfaces it).
    pub manifold_id: u32,
}

/// Read-only solver/contact diagnostics — the seam M8.4's debugger consumes. Producing it MUST NOT
/// perturb the sim (asserted by a determinism test: hash before == hash after a `diagnostics()` call).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Diagnostics {
    pub contacts: Vec<Contact>,
    pub contact_count: usize,
    pub max_penetration: f64,
    /// Total mechanical energy (kinetic + potential) — the M8.1 residual/energy diagnostic.
    pub total_energy: f64,
    pub sleeping_bodies: usize,
}

/// A sampled per-frame world hash + diagnostics (the M8.1 provenance frame record).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrameHash {
    pub frame: u64,
    pub world_hash: String,
    pub energy: f64,
    pub contacts: usize,
    pub max_penetration: f64,
}

/// The bake/replay PROVENANCE envelope (M8.1 deliverable 6) every bake/replay carries — the shared
/// standard for sim scrub/resume (M8.4) and rollback netcode (NET-4). Emitted from the step loop.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub backend: String,
    pub precision: String,
    pub enhanced_determinism: bool,
    pub fixed_dt: f64,
    pub substep_policy: String,
    pub gravity: Vec3,
    pub broad_phase: String,
    pub contact_ordering: String,
    pub units: String,
    pub frame_hashes: Vec<FrameHash>,
    pub final_world_hash: String,
    pub toolchain: String,
    pub steps: u64,
    pub body_count: usize,
    pub joint_count: usize,
}

/// A physics operation that can't be honored — surfaced, never hidden (the "explain every no" discipline).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PhysicsError {
    /// A handle that doesn't exist (e.g. a joint between an unknown body).
    UnknownHandle,
    /// A shape that isn't wired (the seam rungs) — carries the honest reason + what to use instead.
    UnsupportedShape(String),
    /// A shape used in an invalid context (e.g. a `TriMesh` on a dynamic body).
    InvalidForBody(String),
}

impl std::fmt::Display for PhysicsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownHandle => write!(f, "unknown physics handle"),
            Self::UnsupportedShape(why) => write!(f, "unsupported collider shape: {why}"),
            Self::InvalidForBody(why) => write!(f, "shape invalid for this body: {why}"),
        }
    }
}

impl std::error::Error for PhysicsError {}

/// The project-owned physics world. The one seam between the engine and Rapier — no `rapier::`/`parry::`
/// type appears in any signature here (invariant 5). The engine drives it: ECS-authoritative setup
/// (add bodies/colliders/joints), a fixed-`dt` [`Physics::step`] loop, and transform read-back to sync
/// to the viewport via deltas.
pub trait Physics {
    /// The fixed configuration this world runs (precision is compile-time; see the crate features).
    fn config(&self) -> PhysicsConfig;

    /// Add a body; returns its stable handle.
    fn add_body(&mut self, desc: &BodyDesc) -> BodyHandle;
    /// Attach a collider to `body`. Unsupported/seam shapes return a [`PhysicsError`] with the reason.
    fn add_collider(
        &mut self,
        body: BodyHandle,
        desc: &ColliderDesc,
    ) -> Result<ColliderHandle, PhysicsError>;
    /// Constrain two bodies with a joint.
    fn add_joint(
        &mut self,
        a: BodyHandle,
        b: BodyHandle,
        desc: &JointDesc,
    ) -> Result<JointHandle, PhysicsError>;
    /// Remove a body (and its colliders + attached joints).
    fn remove_body(&mut self, body: BodyHandle);

    /// Teleport a body (position-level). For kinematics this is the per-frame drive.
    fn set_transform(&mut self, body: BodyHandle, translation: Vec3, rotation: Quat);
    /// Read a body's world transform — the value synced out to the renderer as a delta. `None` if gone.
    fn transform(&self, body: BodyHandle) -> Option<(Vec3, Quat)>;
    fn set_velocity(&mut self, body: BodyHandle, linvel: Vec3, angvel: Vec3);
    fn velocity(&self, body: BodyHandle) -> Option<(Vec3, Vec3)>;
    /// Apply a one-shot linear impulse (a recorded input — the sim-replay input channel).
    fn apply_impulse(&mut self, body: BodyHandle, impulse: Vec3);

    /// Advance one fixed timestep (`config().fixed_dt`). Deterministic given identical ordered inputs.
    fn step(&mut self);

    /// Serialize the full world (the snapshot sim-replay/rollback share as the initial state). Restoring
    /// honors the M8.1 P3 finding via [`BroadPhase`].
    fn snapshot(&self) -> Vec<u8>;
    /// The deterministic world hash (blake3 of the serialized world) — the P1 equality key, held through
    /// the wrapper.
    fn world_hash(&self) -> String;

    /// Read-only contact/solver diagnostics for the M8.4 debugger. MUST NOT perturb the sim.
    fn diagnostics(&self) -> Diagnostics;
    /// The bake/replay provenance envelope (M8.1 deliverable 6), reflecting the steps taken so far.
    fn provenance(&self) -> Provenance;

    fn body_count(&self) -> usize;
    fn joint_count(&self) -> usize;
}
