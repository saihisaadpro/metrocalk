//! 2-bone analytic IK — "grab the foot, the leg solves" (deliverable 3). A hip-knee-ankle solver done
//! **geometrically**: place the new knee on the circle where the two bone-length spheres intersect (the
//! pole/hint picks which point), then read off the two bone rotations as point-to-point alignments. Exact,
//! **single-pass, deterministic** (no iteration → no GPU/vendor divergence; safe for gameplay-affecting
//! solves, the M8.1 boundary), and **NaN-guarded at full extension** (the target is clamped into reach).
//! CCD/FABRIK for longer arbitrary chains (spine/tail) is the noted out-of-scope seam.

use crate::{gv, Mat4, Pose, Quat, Skeleton, Vec3};
use glam::{Mat4 as GMat4, Quat as GQuat, Vec3 as GVec3, Vec4 as GVec4};

const EPS: f32 = 1e-6;

fn position(m: &Mat4) -> GVec3 {
    (GMat4::from_cols_array_2d(m) * GVec4::new(0.0, 0.0, 0.0, 1.0)).truncate()
}
fn rotation(m: &Mat4) -> GQuat {
    GMat4::from_cols_array_2d(m)
        .to_scale_rotation_translation()
        .1
        .normalize()
}
/// Any unit vector perpendicular to `d` (for a degenerate/straight-chain pole).
fn any_perp(d: GVec3) -> GVec3 {
    let c = if d.x.abs() < 0.9 { GVec3::X } else { GVec3::Y };
    d.cross(c).normalize_or(GVec3::Y)
}

/// The new **knee (mid-joint) position** for a 2-bone chain: root at `a`, bones `l_ab`/`l_cb`, reaching
/// toward `target` with the `pole` hint picking the bend direction. The returned `(mid, reached_target)`
/// satisfy the bone lengths exactly; `reached_target` is `target` clamped into `[ε, l_ab+l_cb)` so a
/// past-reach pull straightens the limb instead of NaN-ing at full extension.
#[must_use]
pub fn solve_knee(a: Vec3, target: Vec3, l_ab: f32, l_cb: f32, pole: Vec3) -> (Vec3, Vec3) {
    let a = gv(a);
    let (tgt, pole) = (gv(target), gv(pole));
    let dir = tgt - a;
    let dist = dir.length();
    let reach = (l_ab + l_cb) * (1.0 - 1e-4); // never fully extend (the singularity guard)
    let clamped = dist.clamp(EPS, reach.max(EPS));
    let axis_dir = if dist > EPS {
        dir / dist
    } else {
        GVec3::Y // target on the root → pick a stable reach direction
    };
    let reached = a + axis_dir * clamped;
    let l_at = clamped;
    // Projection of the knee onto the a→target axis + its off-axis radius (sphere-sphere intersection).
    let h = ((l_ab * l_ab - l_cb * l_cb) / l_at + l_at) * 0.5;
    let r = (l_ab * l_ab - h * h).max(0.0).sqrt();
    // Off-axis direction = the pole component perpendicular to the limb axis (else any perpendicular).
    let pole_vec = pole - a;
    let perp = pole_vec - axis_dir * pole_vec.dot(axis_dir);
    let perp_dir = if perp.length() > EPS {
        perp.normalize()
    } else {
        any_perp(axis_dir)
    };
    let mid = a + axis_dir * h + perp_dir * r;
    (mid.to_array(), reached.to_array())
}

/// Solve the two **world** bone rotations to apply (at the root, then the mid) so the chain
/// root(`a`)→mid(`b`)→end(`c`) reaches `target` with the `pole` hint — point-to-point alignments off
/// [`solve_knee`]. Returns `(root_world_rot, mid_world_rot_after_root)`; `apply_two_bone_ik` converts these
/// to local pose updates. Pure, deterministic, guarded.
#[must_use]
pub fn solve_two_bone(a: Vec3, b: Vec3, c: Vec3, target: Vec3, pole: Vec3) -> (Quat, Quat) {
    let (ga, gb, gc) = (gv(a), gv(b), gv(c));
    let l_ab = (gb - ga).length();
    let l_cb = (gc - gb).length();
    if l_ab < EPS || l_cb < EPS {
        return ([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0, 1.0]);
    }
    let (mid, reached) = solve_knee(a, target, l_ab, l_cb, pole);
    let (gmid, gtgt) = (gv(mid), gv(reached));
    // Root: rotate the root bone direction (a→b) onto (a→mid). It carries the whole chain.
    let q_root = GQuat::from_rotation_arc(
        (gb - ga).normalize_or_zero(),
        (gmid - ga).normalize_or_zero(),
    );
    // After the root rotation, the end is carried to here; bend the mid bone to land it on the target.
    let c_carried = gmid + q_root * (gc - gb);
    let q_mid = GQuat::from_rotation_arc(
        (c_carried - gmid).normalize_or_zero(),
        (gtgt - gmid).normalize_or_zero(),
    );
    (q_root.to_array(), q_mid.to_array())
}

/// Apply 2-bone IK to a [`Pose`]: solve for the chain `root`→`mid`→`end` to reach `target` (with the
/// `pole` hint) and write the two joints' new **local** rotations (translation + scale untouched —
/// FK-then-IK is rotation-only, the rest of the rig is unchanged). Returns the updated pose; FK over it
/// places `end` at the (reach-clamped) target. One commit's worth of joint-TRS overrides (a G2 bundle).
#[must_use]
pub fn apply_two_bone_ik(
    skel: &Skeleton,
    pose: &Pose,
    root: usize,
    mid: usize,
    end: usize,
    target: Vec3,
    pole: Vec3,
) -> Pose {
    let g = skel.globals(pose);
    let a = position(&g[root]).to_array();
    let b = position(&g[mid]).to_array();
    let c = position(&g[end]).to_array();
    let (q_root_a, q_mid_a) = solve_two_bone(a, b, c, target, pole);
    let q_root = GQuat::from_array(q_root_a);
    let q_mid = GQuat::from_array(q_mid_a);

    let root_w = rotation(&g[root]);
    let mid_w = rotation(&g[mid]);
    let root_w_new = q_root * root_w;
    let mid_w_new = q_mid * q_root * mid_w; // the mid inherits the root correction, then bends

    let root_parent_w = skel.joints[root]
        .parent
        .map_or(GQuat::IDENTITY, |p| rotation(&g[p]));
    let root_local = (root_parent_w.inverse() * root_w_new).normalize();
    let mid_local = (root_w_new.inverse() * mid_w_new).normalize();

    let mut out = pose.clone();
    let mut rl = pose.local(root, skel);
    rl.rotation = root_local.to_array();
    out.set(root, rl);
    let mut ml = pose.local(mid, skel);
    ml.rotation = mid_local.to_array();
    out.set(mid, ml);
    out
}
