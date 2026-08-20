//! `metrocalk-skeleton` — the skeletal-posing runtime (M9.3 / G3), built from scratch on `glam` behind
//! our own `Skeleton`/`Pose` types (invariant 5). The literal request — *select the leg, pose it; grab the
//! foot, the leg solves; save the pose, reuse it* — falls out of three pieces:
//!
//! - **A skeleton IS the node hierarchy** (glTF joints are ordinary nodes), so **FK posing is G1's gizmo
//!   editing a bone's local TRS** — descendants follow because [`Skeleton::globals`] composes `parent·local`
//!   (exactly the M9.2 `global_transform`). The skinning matrix per joint = `global(i) · inverseBind(i)`.
//! - **"Grab the foot" is a 2-bone analytic IK solver** (law of cosines + pole) — exact, single-pass,
//!   deterministic ([`ik::two_bone_ik`]).
//! - **A pose IS a G2 override bundle** applied to joint TRS — a sparse [`Pose`] (changed joints only),
//!   IBMs unchanged.
//!
//! Deformation is **Linear Blend Skinning** ([`skin_position`]/[`skin_normal`]) — the universal default —
//! with a **DQS toggle** ([`SkinMethod`]) that fixes the LBS candy-wrapper twist but **cannot represent
//! non-uniform scale**, so it is gated off the non-uniform-scale path. **Normals use the inverse-transpose**
//! of the skinning matrix so non-uniform scale doesn't break lighting. The genuinely hard part —
//! **non-uniform bone scale** — is handled explicitly: globals stay full 4×4 (no quat-TRS round-trip that
//! would inject shear into children), and the [`bone_scale`] policy flags it + gates DQS.
//!
//! The PUBLIC surface is the gizmo crate's plain-array [`Transform`]/[`Mat4`]/[`Vec3`]/[`Quat`] — glam is an
//! internal detail (no foreign math type leaks; the /gizmo boundary discipline). Pure Rust → wasm-clean.

// Math-heavy crate: short names (l1/l2/d, m/n/q) are the canonical ones; the precise float constants in
// tests read clearer un-separated; index→f32 loses no precision at these counts.
#![allow(
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::unreadable_literal,
    clippy::cast_precision_loss,
    clippy::module_name_repetitions
)]

pub mod characterize;
pub mod humanoid;
pub mod ik;
pub mod pose_ops;
pub mod retarget;

use glam::{Mat3, Mat4 as GMat4, Quat as GQuat, Vec3 as GVec3, Vec4 as GVec4};
pub use metrocalk_gizmo::{mat_mul, Mat4, Quat, Transform, Vec3};

// ── glam conversions (the firewall: glam types live only inside this crate) ───────────────────────────

pub(crate) fn gv(v: Vec3) -> GVec3 {
    GVec3::from_array(v)
}
pub(crate) fn gm(m: Mat4) -> GMat4 {
    GMat4::from_cols_array_2d(&m)
}
pub(crate) fn unm(m: GMat4) -> Mat4 {
    m.to_cols_array_2d()
}

/// One joint (bone) of a [`Skeleton`]: its parent (or `None` for a root), its **bind-pose** local TRS, and
/// its `inverseBindMatrix` (from glTF — maps a vertex from mesh space into this joint's local space at bind).
#[derive(Clone, Debug, PartialEq)]
pub struct Joint {
    /// The joint's **own name**, as the source file spelled it (`mixamorig:LeftForeArm`, `lowerarm_l`).
    ///
    /// THE JOIN KEY, AND WHY IT IS ON THE JOINT. Without it a `Skeleton` is an anonymous array of
    /// indices, and nothing can be said about it: not which bone is the left forearm, not whether two
    /// rigs are the same rig, not whether a clip authored elsewhere applies. This crate's own
    /// [`humanoid`]/[`characterize`]/[`retarget`] chain rests entirely on it, and so does the ability to
    /// join a mesh-side skeleton to a clip-side skeleton signature. Empty is allowed (a procedurally
    /// built rig has no source names) and simply means "unnamed" — matching skips it rather than
    /// guessing.
    pub name: String,
    /// Parent joint index. **Must be `< self` (topological order, parent before child)** — the importer
    /// topo-sorts to guarantee this, so FK is a single forward pass.
    pub parent: Option<usize>,
    /// The joint's local transform in the bind pose (overridden per-pose by [`Pose`]).
    pub local_bind: Transform,
    /// The `inverseBindMatrix`: mesh-space → joint-local-space at bind. Pose never changes it (the save-pose
    /// guard: a pose rewrites joint TRS, never IBMs).
    pub inverse_bind: Mat4,
}

/// A rigged skeleton: joints in **topological order** (parent index < child index). The skeleton *is* the
/// node hierarchy; FK + skinning evaluate over it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Skeleton {
    pub joints: Vec<Joint>,
}

/// A **pose** = a sparse set of changed joint **local TRS** (a G2 override bundle on the rig). A joint not
/// in the map keeps its bind local. Applying a pose to a *selection* of bones = the map's keys (the Blender
/// pose-library model: non-destructive, applied to selected bones).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pose {
    /// joint index → posed local TRS (overrides the joint's `local_bind`).
    pub locals: std::collections::BTreeMap<usize, Transform>,
}

impl Pose {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// Set (override) a joint's local TRS in this pose — the FK posing edit (a gizmo bone edit), or one op
    /// of a re-applied pose bundle.
    pub fn set(&mut self, joint: usize, local: Transform) {
        self.locals.insert(joint, local);
    }
    /// The effective local TRS of a joint under this pose: the override if present, else the bind local.
    #[must_use]
    pub fn local(&self, joint: usize, skel: &Skeleton) -> Transform {
        self.locals
            .get(&joint)
            .copied()
            .unwrap_or(skel.joints[joint].local_bind)
    }
}

impl Skeleton {
    /// FK: the **global** (model-space) matrix of every joint under `pose`, composed `parent·local` in one
    /// forward pass (topological order). Full 4×4 throughout — so a non-uniformly-scaled parent's scale
    /// propagates correctly to children **without** a quat-TRS round-trip that would inject shear (the
    /// validated footgun). This is why **descendants follow** a parent (bone) edit.
    #[must_use]
    pub fn globals(&self, pose: &Pose) -> Vec<Mat4> {
        let mut g: Vec<Mat4> = Vec::with_capacity(self.joints.len());
        for (i, joint) in self.joints.iter().enumerate() {
            let local = pose.local(i, self).to_matrix();
            let m = match joint.parent {
                Some(p) => {
                    debug_assert!(
                        p < i,
                        "joints must be in topological order (parent before child)"
                    );
                    mat_mul(g[p], local)
                }
                None => local,
            };
            g.push(m);
        }
        g
    }

    /// The **skinning matrix** of every joint: `global(i) · inverseBind(i)` — the matrix LBS/DQS applies to
    /// mesh vertices weighted by `JOINTS_0`/`WEIGHTS_0`.
    #[must_use]
    pub fn skinning_matrices(&self, pose: &Pose) -> Vec<Mat4> {
        let g = self.globals(pose);
        self.joints
            .iter()
            .enumerate()
            .map(|(i, j)| mat_mul(g[i], j.inverse_bind))
            .collect()
    }

    /// The model-space **position** of a joint's origin under `pose` (the gizmo pivot / IK chain point).
    #[must_use]
    pub fn joint_position(&self, pose: &Pose, joint: usize) -> Vec3 {
        let g = gm(self.globals(pose)[joint]);
        (g * GVec4::new(0.0, 0.0, 0.0, 1.0)).truncate().to_array()
    }

    /// Recompute every joint's `inverse_bind` as the inverse of its current **bind-pose** global — for a
    /// **procedurally-built** rig (so a fresh-from-bind skinning matrix is identity). A glTF import uses the
    /// file's authored `inverseBindMatrices` directly; this is the no-glTF path (+ the test rigs).
    pub fn recompute_inverse_binds(&mut self) {
        let binds = self.globals(&Pose::new());
        for (i, j) in self.joints.iter_mut().enumerate() {
            j.inverse_bind = unm(gm(binds[i]).inverse());
        }
    }
}

// ── Linear Blend Skinning (the default) — render-only deformation ─────────────────────────────────────

/// LBS **position**: `p' = Σ wᵢ · (Mᵢ · p)` over the (≤4) influencing joints. `joints`/`weights` are the
/// vertex's `JOINTS_0`/`WEIGHTS_0`; `skin` is [`Skeleton::skinning_matrices`]. Out-of-range joint indices
/// contribute nothing (defensive).
#[must_use]
pub fn skin_position(pos: Vec3, joints: [u16; 4], weights: [f32; 4], skin: &[Mat4]) -> Vec3 {
    let p = gv(pos).extend(1.0);
    let mut acc = GVec3::ZERO;
    for k in 0..4 {
        let w = weights[k];
        if w == 0.0 {
            continue;
        }
        if let Some(m) = skin.get(joints[k] as usize) {
            acc += w * (gm(*m) * p).truncate();
        }
    }
    acc.to_array()
}

/// LBS **normal**: `n' = normalize(Σ wᵢ · (Nᵢ · n))` where `Nᵢ` is the **inverse-transpose** of the upper
/// 3×3 of the skinning matrix — so a non-uniformly-scaled joint doesn't skew the normal (the lighting-
/// under-scale guard). Falls back to identity if a joint matrix is singular.
#[must_use]
pub fn skin_normal(normal: Vec3, joints: [u16; 4], weights: [f32; 4], skin: &[Mat4]) -> Vec3 {
    let n = gv(normal);
    let mut acc = GVec3::ZERO;
    for k in 0..4 {
        let w = weights[k];
        if w == 0.0 {
            continue;
        }
        if let Some(m) = skin.get(joints[k] as usize) {
            let normal_mat = normal_matrix(gm(*m));
            acc += w * (normal_mat * n);
        }
    }
    acc.normalize_or_zero().to_array()
}

/// The normal matrix = inverse-transpose of a transform's upper 3×3 (rotation+scale+shear → correct normal
/// transform). Identity if the 3×3 is singular (degenerate scale).
fn normal_matrix(m: GMat4) -> Mat3 {
    let m3 = Mat3::from_mat4(m);
    if m3.determinant().abs() < 1e-12 {
        Mat3::IDENTITY
    } else {
        m3.inverse().transpose()
    }
}

// ── bone-scale policy (the genuinely hard bit, handled explicitly) ────────────────────────────────────

pub mod bone_scale {
    //! The non-uniform-bone-scale hazard, handled explicitly (deliverable 5). Default uniform; non-uniform
    //! is allowed only on the **LBS 4×4 + inverse-transpose-normals** path ([`super::skin_normal`]), and
    //! **DQS is gated off it** (DQS can't represent non-uniform scale or shear). Never silently push
    //! non-uniform scale through a quat-TRS path (it shears children) — [`super::Skeleton::globals`] keeps
    //! full 4×4, so it doesn't.

    use super::{Pose, Skeleton, Transform};

    /// Whether a TRS's scale is (near-)uniform — safe + DQS-compatible.
    #[must_use]
    pub fn is_uniform(t: &Transform, eps: f32) -> bool {
        let [x, y, z] = t.scale;
        (x - y).abs() <= eps && (y - z).abs() <= eps && (x - z).abs() <= eps
    }

    /// Whether **any** posed joint local carries non-uniform scale — the shear-risk flag (drives the G1
    /// precision-HUD warning + the DQS gate). Checks the effective (posed) local of every joint.
    #[must_use]
    pub fn has_non_uniform_scale(skel: &Skeleton, pose: &Pose, eps: f32) -> bool {
        (0..skel.joints.len()).any(|i| !is_uniform(&pose.local(i, skel), eps))
    }
}

/// Which skinning method to deform with. LBS is the universal default; DQS is the opt-in upgrade that fixes
/// the candy-wrapper twist — but **cannot represent non-uniform scale**, so [`resolve_skin_method`] gates it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SkinMethod {
    #[default]
    Lbs,
    Dqs,
}

/// Resolve the **effective** skin method given the user's request + the rig's scale: a `Dqs` request on a
/// rig with **non-uniform** bone scale is **refused** (it would corrupt the deform) and falls back to `Lbs`,
/// with a reason. `Lbs` always stands. This is the explicit DQS-off-the-non-uniform-path gate (deliverable
/// 4/5) — never silently apply DQS where it's invalid.
#[must_use]
pub fn resolve_skin_method(
    requested: SkinMethod,
    skel: &Skeleton,
    pose: &Pose,
) -> (SkinMethod, Option<&'static str>) {
    if requested == SkinMethod::Dqs && bone_scale::has_non_uniform_scale(skel, pose, 1e-4) {
        (
            SkinMethod::Lbs,
            Some("DQS can't represent non-uniform bone scale — using LBS (the safe 4×4 path)"),
        )
    } else {
        (requested, None)
    }
}

// ── Dual-Quaternion Skinning (the toggle) — the rigid path ────────────────────────────────────────────

/// A unit dual quaternion (rotation `real` + ½·translation⊗rotation `dual`) — the DQS blend primitive.
#[derive(Clone, Copy, Debug)]
struct DualQuat {
    real: GQuat,
    dual: GQuat,
}

impl DualQuat {
    /// From a **rigid** transform's rotation + translation (DQS ignores scale — gated to the uniform path).
    fn from_rotation_translation(r: GQuat, t: GVec3) -> Self {
        let real = r.normalize();
        // dual = 0.5 * (0, t) * real
        let tq = GQuat::from_xyzw(t.x, t.y, t.z, 0.0);
        let dual = mul_quat(tq, real) * 0.5;
        Self { real, dual }
    }
    fn transform_point(&self, p: GVec3) -> GVec3 {
        // r p r* + 2 (real_w * dual_v - dual_w * real_v + real_v × dual_v)
        let rv = self.real.xyz();
        let dv = self.dual.xyz();
        let t = 2.0 * (self.real.w * dv - self.dual.w * rv + rv.cross(dv));
        self.real * p + t
    }
}

fn mul_quat(a: GQuat, b: GQuat) -> GQuat {
    a * b
}

/// DQS **position** for the uniform-scale (rigid) path: blend each joint's rotation+translation as a dual
/// quaternion (with antipodality fixed against the first influence), normalize, transform. Use only when
/// [`resolve_skin_method`] returns `Dqs`. (Translation+rotation only — scale is the LBS path's job.)
#[must_use]
pub fn skin_position_dqs(pos: Vec3, joints: [u16; 4], weights: [f32; 4], skin: &[Mat4]) -> Vec3 {
    let mut acc = DualQuat {
        real: GQuat::from_xyzw(0.0, 0.0, 0.0, 0.0),
        dual: GQuat::from_xyzw(0.0, 0.0, 0.0, 0.0),
    };
    let mut pivot: Option<GQuat> = None;
    let mut any = false;
    for k in 0..4 {
        let w = weights[k];
        if w == 0.0 {
            continue;
        }
        let Some(m) = skin.get(joints[k] as usize) else {
            continue;
        };
        let gm4 = gm(*m);
        let (_s, r, t) = gm4.to_scale_rotation_translation();
        let dq = DualQuat::from_rotation_translation(r, t);
        // Antipodality: keep all influences in the same hemisphere as the first (else the blend cancels).
        let sign = if let Some(p0) = pivot {
            if p0.dot(dq.real) < 0.0 {
                -1.0
            } else {
                1.0
            }
        } else {
            pivot = Some(dq.real);
            1.0
        };
        acc.real += dq.real * (w * sign);
        acc.dual += dq.dual * (w * sign);
        any = true;
    }
    if !any {
        return pos;
    }
    let len = acc.real.length();
    if len < 1e-8 {
        return pos;
    }
    let norm = DualQuat {
        real: acc.real * (1.0 / len),
        dual: acc.dual * (1.0 / len),
    };
    norm.transform_point(gv(pos)).to_array()
}

#[cfg(test)]
mod tests;
