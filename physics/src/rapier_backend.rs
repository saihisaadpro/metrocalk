//! The Rapier-backed [`Physics`] impl — the **only** module that names `rapier::`/`parry::` types (the
//! seam the CI grep-gate confines). Mirrors the M8.1 spike's deterministic step loop + serialized-world
//! hash, wrapped so the public surface stays our own boundary types (`[f64;3]`/`[f64;4]`, handles).
//!
//! Precision is the compile-time feature (`deterministic` ⇒ `f64` + `enhanced-determinism`, the M8.1
//! authoritative config). Determinism is preserved through the wrapper: fixed `dt`, rapier's deterministic
//! body/contact ordering, and the same 8-component serialized snapshot the spike hashed.

#![allow(clippy::cast_precision_loss)] // energy sums over tiny mass/velocity scalars — no meaningful loss

use rapier::prelude::*;
use rapier3d_f64 as rapier;

use crate::{
    BodyDesc, BodyHandle, BodyKind, BroadPhase, ColliderDesc, ColliderHandle, ColliderShape,
    Contact, DerivedCollider, Diagnostics, FrameHash, JointDesc, JointHandle, Physics,
    PhysicsConfig, PhysicsError, Provenance, Quat, Vec3,
};

/// blake3 hex of bytes — the deterministic hash primitive (matches the M8.1 spike).
fn hash_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn vec(v: Vec3) -> Vector {
    Vector::new(v[0], v[1], v[2])
}
fn unvec(v: Vector) -> Vec3 {
    [v.x, v.y, v.z]
}

/// Our `[x,y,z,w]` quaternion → rapier's rotation (the glam `DQuat` re-export).
fn rot(q: Quat) -> Rotation {
    Rotation::from_xyzw(q[0], q[1], q[2], q[3])
}
/// Rapier's rotation → our `[x,y,z,w]` quaternion.
fn unquat(r: &Rotation) -> Quat {
    [r.x, r.y, r.z, r.w]
}

const fn pack(index: u32, generation: u32) -> u64 {
    ((index as u64) << 32) | (generation as u64)
}
const fn unpack(h: u64) -> (u32, u32) {
    ((h >> 32) as u32, (h & 0xFFFF_FFFF) as u32)
}

/// The Rapier-backed physics world.
pub struct RapierPhysics {
    bodies: RigidBodySet,
    colliders: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    islands: IslandManager,
    broad_phase: BroadPhaseBvh,
    narrow_phase: NarrowPhase,
    ccd: CCDSolver,
    params: IntegrationParameters,
    pipeline: PhysicsPipeline,
    gravity: Vector,
    config: PhysicsConfig,
    /// Steps taken — drives the provenance + per-frame hash sampling.
    steps: u64,
    /// Sampled per-frame world hashes (every `SAMPLE_EVERY` steps) — the provenance frame record.
    frames: Vec<FrameHash>,
}

const SAMPLE_EVERY: u64 = 1000;

fn make_broad_phase(mode: BroadPhase) -> BroadPhaseBvh {
    match mode {
        BroadPhase::Default => BroadPhaseBvh::new(),
        BroadPhase::DeterministicResume => {
            BroadPhaseBvh::with_optimization_strategy(BvhOptimizationStrategy::None)
        }
    }
}

impl RapierPhysics {
    /// A fresh, empty world configured per `config`.
    #[must_use]
    pub fn new(config: PhysicsConfig) -> Self {
        let params = IntegrationParameters {
            dt: config.fixed_dt,
            ..IntegrationParameters::default()
        };
        Self {
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            islands: IslandManager::new(),
            broad_phase: make_broad_phase(config.broad_phase),
            narrow_phase: NarrowPhase::new(),
            ccd: CCDSolver::new(),
            params,
            pipeline: PhysicsPipeline::new(),
            gravity: vec(config.gravity),
            config,
            steps: 0,
            frames: Vec::new(),
        }
    }

    fn rb_handle(h: BodyHandle) -> RigidBodyHandle {
        let (i, g) = unpack(h.0);
        RigidBodyHandle::from_raw_parts(i, g)
    }

    fn shared_shape(shape: &ColliderShape) -> Result<SharedShape, PhysicsError> {
        match shape {
            ColliderShape::Ball { radius } => Ok(SharedShape::ball(*radius)),
            ColliderShape::Cuboid { half_extents } => Ok(SharedShape::cuboid(
                half_extents[0],
                half_extents[1],
                half_extents[2],
            )),
            ColliderShape::Capsule {
                half_height,
                radius,
            } => Ok(SharedShape::capsule_y(*half_height, *radius)),
            ColliderShape::ConvexHull { points } => {
                let pts: Vec<Vector> = points.iter().map(|p| vec(*p)).collect();
                SharedShape::convex_hull(&pts).ok_or_else(|| {
                    PhysicsError::UnsupportedShape(
                        "convex hull is degenerate (collinear/coincident points)".into(),
                    )
                })
            }
            ColliderShape::TriMesh { vertices, indices } => {
                if !indices.len().is_multiple_of(3) {
                    return Err(PhysicsError::UnsupportedShape(
                        "tri-mesh index count is not a multiple of 3".into(),
                    ));
                }
                let verts: Vec<Vector> = vertices.iter().map(|p| vec(*p)).collect();
                let tris: Vec<[u32; 3]> = indices
                    .chunks_exact(3)
                    .map(|c| [c[0], c[1], c[2]])
                    .collect();
                SharedShape::trimesh(verts, tris).map_err(|e| {
                    PhysicsError::UnsupportedShape(format!("tri-mesh build failed: {e:?}"))
                })
            }
            ColliderShape::ConvexDecomposition { .. } => Err(PhysicsError::UnsupportedShape(
                "convex decomposition (VHACD) is a seam — not wired in M8.2; \
                 use ConvexHull for a dynamic body or TriMesh for static geometry"
                    .into(),
            )),
            ColliderShape::Voxels { .. } => Err(PhysicsError::UnsupportedShape(
                "voxels are experimental in Parry 0.28 (no auto mass/inertia for dynamic bodies, \
                 no shape-casting, no voxel↔voxel/voxel↔mesh) — declined for M8.2"
                    .into(),
            )),
            ColliderShape::Sdf => Err(PhysicsError::UnsupportedShape(
                "SDF dynamic colliders are a deferred seam (M8.5)".into(),
            )),
        }
    }

    /// Serialize the world the same way the M8.1 spike did — the 8-component snapshot is the determinism
    /// key (the `PhysicsPipeline` holds no persistent state, so it's excluded; gravity is included).
    fn snapshot_bytes(&self) -> Vec<u8> {
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

    fn energy(&self) -> f64 {
        let mut e = 0.0f64;
        for (_, rb) in self.bodies.iter() {
            if rb.is_dynamic() {
                let m = rb.mass();
                let speed2 = rb.linvel().length_squared();
                let h = rb.translation().y;
                e += 0.5 * m * speed2 + m * 9.81 * h;
            }
        }
        e
    }
}

impl Physics for RapierPhysics {
    fn config(&self) -> PhysicsConfig {
        self.config
    }

    fn add_body(&mut self, desc: &BodyDesc) -> BodyHandle {
        let builder = match desc.kind {
            BodyKind::Dynamic => RigidBodyBuilder::dynamic(),
            BodyKind::Fixed => RigidBodyBuilder::fixed(),
            BodyKind::KinematicPosition => RigidBodyBuilder::kinematic_position_based(),
            BodyKind::KinematicVelocity => RigidBodyBuilder::kinematic_velocity_based(),
        };
        let rb = builder
            .translation(vec(desc.translation))
            .linvel(vec(desc.linvel))
            .angvel(vec(desc.angvel))
            .build();
        let h = self.bodies.insert(rb);
        // Apply the spawn rotation separately (glam-rapier exposes no `Isometry` ctor in the prelude);
        // a no-op for the identity default.
        if let Some(b) = self.bodies.get_mut(h) {
            b.set_rotation(rot(desc.rotation), false);
        }
        let (i, g) = h.into_raw_parts();
        BodyHandle(pack(i, g))
    }

    fn add_collider(
        &mut self,
        body: BodyHandle,
        desc: &ColliderDesc,
    ) -> Result<ColliderHandle, PhysicsError> {
        let shape = Self::shared_shape(&desc.shape)?;
        let rb = Self::rb_handle(body);
        if !self.bodies.contains(rb) {
            return Err(PhysicsError::UnknownHandle);
        }
        // A tri-mesh has no well-defined volume → invalid for a dynamic body.
        if matches!(desc.shape, ColliderShape::TriMesh { .. }) && self.bodies[rb].is_dynamic() {
            return Err(PhysicsError::InvalidForBody(
                "a tri-mesh collider has no mass — attach it to a Fixed body, or use ConvexHull for dynamics"
                    .into(),
            ));
        }
        let col = ColliderBuilder::new(shape)
            .density(desc.density)
            .friction(desc.friction)
            .restitution(desc.restitution)
            .build();
        let ch = self.colliders.insert_with_parent(col, rb, &mut self.bodies);
        let (i, g) = ch.into_raw_parts();
        Ok(ColliderHandle(pack(i, g)))
    }

    fn add_joint(
        &mut self,
        a: BodyHandle,
        b: BodyHandle,
        desc: &JointDesc,
    ) -> Result<JointHandle, PhysicsError> {
        let (ra, rb) = (Self::rb_handle(a), Self::rb_handle(b));
        if !self.bodies.contains(ra) || !self.bodies.contains(rb) {
            return Err(PhysicsError::UnknownHandle);
        }
        let joint: GenericJoint = match desc {
            JointDesc::Revolute {
                axis,
                anchor_a,
                anchor_b,
            } => RevoluteJointBuilder::new(vec(*axis))
                .local_anchor1(vec(*anchor_a))
                .local_anchor2(vec(*anchor_b))
                .into(),
            JointDesc::Fixed { anchor_a, anchor_b } => FixedJointBuilder::new()
                .local_anchor1(vec(*anchor_a))
                .local_anchor2(vec(*anchor_b))
                .into(),
            JointDesc::Spherical { anchor_a, anchor_b } => SphericalJointBuilder::new()
                .local_anchor1(vec(*anchor_a))
                .local_anchor2(vec(*anchor_b))
                .into(),
        };
        let jh = self.impulse_joints.insert(ra, rb, joint, true);
        let (i, g) = jh.into_raw_parts();
        Ok(JointHandle(pack(i, g)))
    }

    fn remove_body(&mut self, body: BodyHandle) {
        let rb = Self::rb_handle(body);
        self.bodies.remove(
            rb,
            &mut self.islands,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            true,
        );
    }

    fn set_transform(&mut self, body: BodyHandle, translation: Vec3, rotation: Quat) {
        let rb = Self::rb_handle(body);
        if let Some(b) = self.bodies.get_mut(rb) {
            b.set_translation(vec(translation), true);
            b.set_rotation(rot(rotation), true);
        }
    }

    fn transform(&self, body: BodyHandle) -> Option<(Vec3, Quat)> {
        let rb = Self::rb_handle(body);
        self.bodies.get(rb).map(|b| {
            let t = unvec(b.translation());
            (t, unquat(b.rotation()))
        })
    }

    fn set_velocity(&mut self, body: BodyHandle, linvel: Vec3, angvel: Vec3) {
        let rb = Self::rb_handle(body);
        if let Some(b) = self.bodies.get_mut(rb) {
            b.set_linvel(vec(linvel), true);
            b.set_angvel(vec(angvel), true);
        }
    }

    fn velocity(&self, body: BodyHandle) -> Option<(Vec3, Vec3)> {
        let rb = Self::rb_handle(body);
        self.bodies
            .get(rb)
            .map(|b| (unvec(b.linvel()), unvec(b.angvel())))
    }

    fn apply_impulse(&mut self, body: BodyHandle, impulse: Vec3) {
        let rb = Self::rb_handle(body);
        if let Some(b) = self.bodies.get_mut(rb) {
            b.apply_impulse(vec(impulse), true);
        }
    }

    fn step(&mut self) {
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
        self.steps += 1;
        if self.steps.is_multiple_of(SAMPLE_EVERY) {
            let d = self.diagnostics();
            self.frames.push(FrameHash {
                frame: self.steps,
                world_hash: self.world_hash(),
                energy: d.total_energy,
                contacts: d.contact_count,
                max_penetration: d.max_penetration,
            });
        }
    }

    fn snapshot(&self) -> Vec<u8> {
        self.snapshot_bytes()
    }

    fn world_hash(&self) -> String {
        hash_bytes(&self.snapshot_bytes())
    }

    fn diagnostics(&self) -> Diagnostics {
        let mut contacts = Vec::new();
        let mut max_penetration = 0.0f64;
        for pair in self.narrow_phase.contact_pairs() {
            if !pair.has_any_active_contact() {
                continue;
            }
            let body_of = |ch| -> Option<BodyHandle> {
                let h = self.colliders.get(ch)?.parent()?;
                let (i, g) = h.into_raw_parts();
                Some(BodyHandle(pack(i, g)))
            };
            let (Some(a), Some(b)) = (body_of(pair.collider1), body_of(pair.collider2)) else {
                continue;
            };
            // Material coefficients are properties of the two colliders; report the effective (averaged)
            // value the contact behaves with — the friction cone the debugger draws. (Rapier's default
            // combine rule is the average; labelled as effective so it's honest, not a fabricated number.)
            let col1 = self.colliders.get(pair.collider1);
            let col2 = self.colliders.get(pair.collider2);
            let friction = col1
                .zip(col2)
                .map_or(0.0, |(c1, c2)| (c1.friction() + c2.friction()) * 0.5);
            let restitution = col1
                .zip(col2)
                .map_or(0.0, |(c1, c2)| (c1.restitution() + c2.restitution()) * 0.5);
            // Collider 1's world placement transforms each manifold point (stored in its local frame) to
            // world space — the exact contact point the overlay marks + the click-to-explain target.
            let pos1 = col1.map(rapier::geometry::Collider::position).copied();
            for m in &pair.manifolds {
                let normal = unvec(m.data.normal);
                for pt in &m.points {
                    let depth = -pt.dist;
                    if depth > max_penetration {
                        max_penetration = depth;
                    }
                    let point =
                        pos1.map_or_else(|| [0.0; 3], |p| unvec(p.transform_point(pt.local_p1)));
                    let normal_impulse = pt.data.impulse;
                    // Friction impulse is a 2-vector in the contact tangent plane; report its magnitude.
                    let tangent_impulse = {
                        let t = pt.data.tangent_impulse;
                        (t[0] * t[0] + t[1] * t[1]).sqrt()
                    };
                    let friction_saturated = normal_impulse > 1e-9
                        && tangent_impulse >= friction * normal_impulse * (1.0 - 1e-3);
                    contacts.push(Contact {
                        body_a: a,
                        body_b: b,
                        point,
                        normal,
                        depth,
                        normal_impulse,
                        tangent_impulse,
                        friction,
                        restitution,
                        friction_saturated,
                        manifold_id: pt.fid1.0 ^ (pt.fid2.0.rotate_left(16)),
                    });
                }
            }
        }
        let sleeping = self.bodies.iter().filter(|(_, b)| b.is_sleeping()).count();
        Diagnostics {
            contact_count: contacts.len(),
            max_penetration,
            total_energy: self.energy(),
            sleeping_bodies: sleeping,
            contacts,
        }
    }

    fn provenance(&self) -> Provenance {
        Provenance {
            backend: "rapier3d-f64 0.33 / parry3d 0.28".into(),
            precision: if cfg!(feature = "deterministic") {
                "f64".into()
            } else {
                "f64 (fast/non-deterministic config)".into()
            },
            enhanced_determinism: cfg!(feature = "deterministic"),
            fixed_dt: self.config.fixed_dt,
            substep_policy:
                "fixed, recorded (no runtime-adaptive substepping in the authoritative config)"
                    .into(),
            gravity: unvec(self.gravity),
            broad_phase: match self.config.broad_phase {
                BroadPhase::Default => "BroadPhaseBvh (default)".into(),
                BroadPhase::DeterministicResume => {
                    "BroadPhaseBvh::None (deterministic resume)".into()
                }
            },
            contact_ordering: "rapier deterministic (enhanced-determinism)".into(),
            units: "meters; gravity in m/s²".into(),
            frame_hashes: self.frames.clone(),
            final_world_hash: self.world_hash(),
            toolchain: env!("CARGO_PKG_VERSION").into(),
            steps: self.steps,
            body_count: self.bodies.len(),
            joint_count: self.impulse_joints.len(),
        }
    }

    fn body_count(&self) -> usize {
        self.bodies.len()
    }

    fn joint_count(&self) -> usize {
        self.impulse_joints.len()
    }
}

/// M8.3 collision-shape generation (the M4/ADR-014-deferred piece): derive a dynamic-body collider from a
/// mesh — a convex hull of the vertices + the **fit error** vs the mesh's own volume, so the authoring
/// layer can report "convex hull, fit error N %" on a concave mesh rather than silently approximating.
/// Pure geometry (Parry's hull + the divergence-theorem mesh volume); the boundary types are ours.
///
/// # Errors
/// [`PhysicsError::UnsupportedShape`] if the mesh has < 4 vertices or its points are degenerate (a hull
/// can't be built) — surfaced, never a silent empty collider.
pub fn derive_collider(
    vertices: &[Vec3],
    indices: &[u32],
) -> Result<DerivedCollider, PhysicsError> {
    if vertices.len() < 4 {
        return Err(PhysicsError::UnsupportedShape(
            "mesh has too few vertices (< 4) to derive a collider".into(),
        ));
    }
    let pts: Vec<Vector> = vertices.iter().map(|v| vec(*v)).collect();
    let hull = SharedShape::convex_hull(&pts).ok_or_else(|| {
        PhysicsError::UnsupportedShape("degenerate hull (collinear/coincident points)".into())
    })?;
    // density 1 ⇒ mass == volume; compare the hull's volume to the mesh's own enclosed volume.
    let hull_vol = hull.mass_properties(1.0).mass().max(1e-9);
    let mesh_vol = mesh_volume(vertices, indices).abs();
    #[allow(clippy::cast_possible_truncation)]
    let fit_error = (((hull_vol - mesh_vol) / hull_vol).clamp(0.0, 1.0)) as f32;
    Ok(DerivedCollider {
        shape: ColliderShape::ConvexHull {
            points: vertices.to_vec(),
        },
        fit_error,
        concave: fit_error > 0.1,
        vertex_count: vertices.len(),
    })
}

/// Plain-language "why" for a contact — the M3.1 / ADR-016 explain discipline applied to *running*
/// physics ("debug by looking"). It narrates the **measured** contact (penetration, post-solve impulses,
/// friction saturation) and names the likely cause of a jitter — the physics analog of "why is this greyed
/// out?" / "✅ state = FacingBoss, ❌ KillCounter 3/4". Pure formatting over a [`Contact`]; it never
/// invents a quantity that wasn't measured (the per-island solver residual rapier 0.33 doesn't expose is
/// named as the geometric penetration residual, not faked).
#[must_use]
pub fn explain_contact(c: &Contact) -> String {
    let mut s = format!(
        "penetration {:.2} mm · normal impulse {:.2} N·s · friction impulse {:.2} N·s (μ≈{:.2}, restitution≈{:.2})",
        c.depth * 1000.0,
        c.normal_impulse,
        c.tangent_impulse,
        c.friction,
        c.restitution,
    );
    if c.friction_saturated {
        s.push_str(
            " · friction SATURATED (the surfaces are slipping) — a classic jitter source: raise friction, \
             lower the timestep, or add rolling resistance",
        );
    }
    if c.depth > 0.005 {
        s.push_str(
            " · deep penetration (position residual > 5 mm) — soft contact / too few solver iterations; \
             this is why it sinks and jitters",
        );
    } else if !c.friction_saturated {
        s.push_str(" · resting cleanly (shallow penetration, friction within the cone)");
    }
    s
}

/// Signed volume enclosed by a triangle mesh (divergence theorem: Σ of the origin tetrahedra over the
/// triangles). `abs()` at the call site handles winding. A non-multiple-of-3 index tail is ignored.
fn mesh_volume(vertices: &[Vec3], indices: &[u32]) -> f64 {
    let mut v = 0.0f64;
    for tri in indices.chunks_exact(3) {
        let a = vertices[tri[0] as usize];
        let b = vertices[tri[1] as usize];
        let c = vertices[tri[2] as usize];
        let cross = [
            b[1] * c[2] - b[2] * c[1],
            b[2] * c[0] - b[0] * c[2],
            b[0] * c[1] - b[1] * c[0],
        ];
        v += (a[0] * cross[0] + a[1] * cross[1] + a[2] * cross[2]) / 6.0;
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BodyDesc, BodyKind, ColliderDesc, ColliderShape};

    /// A seeded scene through the TRAIT: a ground + a falling ball + a small stack — enough to exercise
    /// gravity, contacts, and resting, and to produce a deterministic hash through the wrapper.
    fn scene() -> RapierPhysics {
        let mut p = RapierPhysics::new(PhysicsConfig::default());
        let ground = p.add_body(&BodyDesc::new(BodyKind::Fixed, [0.0, 0.0, 0.0]));
        p.add_collider(
            ground,
            &ColliderDesc::new(ColliderShape::Cuboid {
                half_extents: [20.0, 0.5, 20.0],
            }),
        )
        .unwrap();
        for i in 0..8 {
            let b = p.add_body(&BodyDesc::new(
                BodyKind::Dynamic,
                [f64::from(i % 3) * 0.6 - 0.6, 2.0 + f64::from(i) * 1.1, 0.0],
            ));
            p.add_collider(b, &ColliderDesc::new(ColliderShape::Ball { radius: 0.5 }))
                .unwrap();
        }
        p
    }

    #[test]
    fn determinism_holds_through_the_trait() {
        let mut a = scene();
        let mut b = scene();
        for _ in 0..600 {
            a.step();
            b.step();
        }
        assert_eq!(
            a.world_hash(),
            b.world_hash(),
            "two identical runs through the trait must hash identically (P1 holds through the wrapper)"
        );
    }

    #[test]
    fn diagnostics_is_non_mutating() {
        let mut p = scene();
        for _ in 0..400 {
            p.step();
        }
        let before = p.world_hash();
        let d = p.diagnostics(); // read-only seam
        let _ = p.diagnostics();
        let after = p.world_hash();
        assert_eq!(
            before, after,
            "reading diagnostics must NOT perturb the sim"
        );
        assert!(
            d.contact_count > 0,
            "settled balls rest on the ground → contacts exist"
        );
    }

    #[test]
    fn contacts_carry_solved_impulses_and_explain_themselves() {
        // A settled stack produces resting contacts; each must carry the post-solve normal impulse (a
        // resting box pushes back against gravity) + a world point + a material μ — the data the M8.4
        // overlay color-codes and `explain_contact` narrates. None of it is fabricated: a frictionless
        // float would show zero impulse, so a positive impulse here is the solver's real output.
        let mut p = scene();
        for _ in 0..400 {
            p.step();
        }
        let d = p.diagnostics();
        assert!(d.contact_count > 0, "settled balls rest → contacts exist");
        let resting =
            d.contacts.iter().find(|c| c.normal_impulse > 0.0).expect(
                "a resting contact carries a positive normal impulse (supports the weight)",
            );
        assert!(
            resting.point[1].abs() < 50.0 && resting.friction >= 0.0,
            "the contact has a finite world point + a material friction coefficient"
        );
        let why = explain_contact(resting);
        assert!(
            why.contains("penetration") && why.contains("normal impulse"),
            "explain narrates the measured contact ({why})"
        );
    }

    #[test]
    fn a_ball_falls_under_gravity() {
        let mut p = RapierPhysics::new(PhysicsConfig::default());
        let b = p.add_body(&BodyDesc::new(BodyKind::Dynamic, [0.0, 10.0, 0.0]));
        p.add_collider(b, &ColliderDesc::new(ColliderShape::Ball { radius: 0.5 }))
            .unwrap();
        let y0 = p.transform(b).unwrap().0[1];
        for _ in 0..60 {
            p.step();
        }
        let y1 = p.transform(b).unwrap().0[1];
        assert!(y1 < y0 - 1.0, "the ball fell ({y0} → {y1})");
    }

    #[test]
    fn unsupported_shapes_are_explained_not_faked() {
        let mut p = RapierPhysics::new(PhysicsConfig::default());
        let b = p.add_body(&BodyDesc::new(BodyKind::Dynamic, [0.0, 1.0, 0.0]));
        let err = p
            .add_collider(b, &ColliderDesc::new(ColliderShape::Sdf))
            .unwrap_err();
        assert!(matches!(err, PhysicsError::UnsupportedShape(_)));
    }

    #[test]
    fn provenance_reflects_the_config_and_steps() {
        let mut p = scene();
        for _ in 0..1000 {
            p.step();
        }
        let prov = p.provenance();
        assert_eq!(prov.precision, "f64");
        assert!(prov.enhanced_determinism);
        assert_eq!(prov.steps, 1000);
        assert!(
            !prov.frame_hashes.is_empty(),
            "sampled at least one frame hash"
        );
        assert_eq!(prov.final_world_hash, p.world_hash());
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "release-only timing measurement (run --release)"
    )]
    fn step_fits_the_frame_budget_at_scale() {
        // Min-spec budget (product principle 3): a fixed-`dt` step at a representative body count must fit
        // one 60 Hz frame (<16 ms). Measured in release (debug timing is meaningless). 100 dynamic balls
        // on a ground — the M8.2 demo scale. An absolute, order-of-magnitude gate (jitter-proof on a
        // shared runner, same rationale as perf-gate); the determinism config (single-thread libm) is the
        // worst case for speed, so this is the honest authoritative-path cost.
        let mut p = RapierPhysics::new(PhysicsConfig::default());
        let ground = p.add_body(&BodyDesc::new(BodyKind::Fixed, [0.0, 0.0, 0.0]));
        p.add_collider(
            ground,
            &ColliderDesc::new(ColliderShape::Cuboid {
                half_extents: [30.0, 0.5, 30.0],
            }),
        )
        .unwrap();
        for i in 0..100u32 {
            let b = p.add_body(&BodyDesc::new(
                BodyKind::Dynamic,
                [
                    f64::from(i % 10) * 1.1 - 5.0,
                    3.0 + f64::from(i / 10) * 1.1,
                    0.0,
                ],
            ));
            p.add_collider(b, &ColliderDesc::new(ColliderShape::Ball { radius: 0.5 }))
                .unwrap();
        }
        for _ in 0..30 {
            p.step(); // warm
        }
        let mut times = Vec::with_capacity(300);
        for _ in 0..300 {
            let t0 = std::time::Instant::now();
            p.step();
            times.push(t0.elapsed().as_secs_f64() * 1e3);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p50 = times[times.len() / 2];
        let p99 = times[times.len() * 99 / 100];
        eprintln!("[M8.2] physics step @100 dynamic bodies: p50={p50:.3}ms p99={p99:.3}ms");
        assert!(
            p99 < 16.0,
            "a physics step (p99={p99:.3}ms) must fit one 60 Hz frame at 100 bodies"
        );
    }

    #[test]
    fn derive_collider_reports_fit_and_concavity() {
        // A unit cube — convex, so its hull fits perfectly (fit_error ≈ 0, not concave).
        let cube = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
        ];
        let cube_idx: [u32; 36] = [
            0, 1, 2, 0, 2, 3, 4, 6, 5, 4, 7, 6, 0, 5, 1, 0, 4, 5, 3, 2, 6, 3, 6, 7, 0, 3, 7, 0, 7,
            4, 1, 5, 6, 1, 6, 2,
        ];
        let d = derive_collider(&cube, &cube_idx).unwrap();
        assert!(matches!(d.shape, ColliderShape::ConvexHull { .. }));
        assert_eq!(d.vertex_count, 8);
        assert!(
            d.fit_error < 0.05,
            "a convex cube fits its hull (got {})",
            d.fit_error
        );
        assert!(!d.concave);

        // The SAME 8 corner points but only a flat slab of triangles → the mesh encloses ~no volume while
        // the hull is the full cube → a high fit error → flagged concave ("this needs a hull/voxels").
        let d2 = derive_collider(&cube, &[0u32, 1, 2, 0, 2, 3]).unwrap();
        assert!(
            d2.fit_error > 0.5,
            "near-empty mesh in a full hull is concave (got {})",
            d2.fit_error
        );
        assert!(d2.concave);

        // Too few vertices → an explained error, never a silent empty collider.
        assert!(derive_collider(&cube[..3], &[0u32, 1, 2]).is_err());
    }
}
