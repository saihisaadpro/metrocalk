//! `metrocalk-skeleton` headless spine (M9.3 / G3) — FK matches a reference pose, skinning matrices are
//! identity at bind, LBS deforms correctly (with inverse-transpose normals under non-uniform scale), DQS
//! matches LBS on the rigid path, the bone-scale policy gates DQS off non-uniform scale, the 2-bone IK
//! reaches the target (single-pass + deterministic + full-extension-guarded), and a pose re-applies to a
//! selection of bones. Covers the prompt's success criteria + adversarial traps.

use super::*;
use crate::ik;
use metrocalk_gizmo::{axis_angle, Transform};

const EPS: f32 = 1e-4;

/// Build a skeleton from `(parent, local_bind)` pairs (topological order), computing each joint's
/// `inverseBindMatrix` from its bind-pose global (the standard glTF setup → skinning matrices are identity
/// at bind).
fn skeleton(specs: &[(Option<usize>, Transform)]) -> Skeleton {
    let joints: Vec<Joint> = specs
        .iter()
        .map(|(parent, local_bind)| Joint {
            parent: *parent,
            local_bind: *local_bind,
            inverse_bind: [[0.0; 4]; 4], // filled below
        })
        .collect();
    let mut skel = Skeleton { joints };
    skel.recompute_inverse_binds();
    skel
}

fn tf(t: [f32; 3]) -> Transform {
    Transform {
        translation: t,
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0],
    }
}

fn dist(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

// ── FK: descendants follow a parent (bone) edit ──────────────────────────────

#[test]
fn fk_composes_parent_local_so_descendants_follow() {
    // root → child at local (1,0,0) → grandchild at local (1,0,0): bind globals 0,1,2 along X.
    let skel = skeleton(&[
        (None, tf([0.0, 0.0, 0.0])),
        (Some(0), tf([1.0, 0.0, 0.0])),
        (Some(1), tf([1.0, 0.0, 0.0])),
    ]);
    let bind = skel.globals(&Pose::new());
    assert!(
        dist(skel.joint_position(&Pose::new(), 2), [2.0, 0.0, 0.0]) < EPS,
        "bind: end at x=2"
    );

    // Rotate the ROOT 90° about Z (local). The whole chain swings into +Y — descendants follow.
    let mut pose = Pose::new();
    let mut rl = skel.joints[0].local_bind;
    rl.rotation = axis_angle([0.0, 0.0, 1.0], std::f32::consts::FRAC_PI_2);
    pose.set(0, rl);
    let end = skel.joint_position(&pose, 2);
    assert!(
        dist(end, [0.0, 2.0, 0.0]) < 1e-3,
        "root rotation carried the end to +Y, got {end:?}"
    );
    let _ = bind;
}

// ── skinning matrices: identity at bind (a vertex is unmoved in the rest pose) ─

#[test]
fn skinning_matrix_is_identity_at_bind() {
    let skel = skeleton(&[(None, tf([0.0, 0.0, 0.0])), (Some(0), tf([2.0, 0.0, 0.0]))]);
    for m in skel.skinning_matrices(&Pose::new()) {
        let id = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        for c in 0..4 {
            for r in 0..4 {
                assert!(
                    (m[c][r] - id[c][r]).abs() < EPS,
                    "skinning matrix == identity at bind"
                );
            }
        }
    }
}

// ── LBS: a fully-weighted vertex follows its joint's skinning matrix ──────────

#[test]
fn lbs_position_matches_the_skinning_matrix() {
    let skel = skeleton(&[(None, tf([0.0, 0.0, 0.0])), (Some(0), tf([1.0, 0.0, 0.0]))]);
    // Pose: translate joint 1's local by +Y 3 → its skinning matrix moves a bound vertex +Y 3.
    let mut pose = Pose::new();
    pose.set(1, tf([1.0, 3.0, 0.0]));
    let skin = skel.skinning_matrices(&pose);
    // A vertex at the bind position of joint 1 (x=1), fully weighted to joint 1, moves to (1,3,0).
    let out = skin_position([1.0, 0.0, 0.0], [1, 0, 0, 0], [1.0, 0.0, 0.0, 0.0], &skin);
    assert!(
        dist(out, [1.0, 3.0, 0.0]) < 1e-3,
        "LBS moved the vertex with its joint, got {out:?}"
    );
}

#[test]
fn lbs_normal_uses_inverse_transpose_under_non_uniform_scale() {
    // ADVERSARIAL: "normals aren't inverse-transposed under scale → lighting wrong." A single root joint
    // scaled 2× in X. A diagonal normal (1,1,0): the CORRECT (inverse-transpose) transform shrinks X
    // (∝ 0.5,1,0 → x<y); the naive matrix would GROW X (x>y).
    // A single root joint, bind = identity (so invbind = identity → the skinning matrix == the pose scale).
    let skel = skeleton(&[(None, tf([0.0, 0.0, 0.0]))]);
    let mut pose = Pose::new();
    pose.set(
        0,
        Transform {
            scale: [2.0, 1.0, 1.0],
            ..tf([0.0, 0.0, 0.0])
        },
    );
    let skin = skel.skinning_matrices(&pose);
    let inv_sqrt2 = std::f32::consts::FRAC_1_SQRT_2;
    let n = skin_normal(
        [inv_sqrt2, inv_sqrt2, 0.0],
        [0, 0, 0, 0],
        [1.0, 0.0, 0.0, 0.0],
        &skin,
    );
    assert!(
        n[0] < n[1],
        "inverse-transpose: X shrinks under +X scale (got {n:?}), not the naive grow"
    );
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    assert!((len - 1.0).abs() < 1e-3, "normal stays unit length");
}

// ── DQS: matches LBS on the rigid (rotation+translation) path; gated off non-uniform scale ────────────

#[test]
fn dqs_matches_lbs_for_a_single_rigid_joint() {
    let skel = skeleton(&[(None, tf([0.0, 0.0, 0.0])), (Some(0), tf([1.0, 0.0, 0.0]))]);
    let mut pose = Pose::new();
    let mut rl = skel.joints[1].local_bind;
    rl.rotation = axis_angle([0.0, 0.0, 1.0], 0.7); // a pure rotation (rigid)
    pose.set(1, rl);
    let skin = skel.skinning_matrices(&pose);
    let p = [1.3, 0.2, 0.0];
    let lbs = skin_position(p, [1, 0, 0, 0], [1.0, 0.0, 0.0, 0.0], &skin);
    let dqs = skin_position_dqs(p, [1, 0, 0, 0], [1.0, 0.0, 0.0, 0.0], &skin);
    assert!(
        dist(lbs, dqs) < 1e-3,
        "DQS == LBS for a single rigid joint: {lbs:?} vs {dqs:?}"
    );
}

#[test]
fn dqs_is_gated_off_the_non_uniform_scale_path() {
    let skel = skeleton(&[(None, tf([0.0, 0.0, 0.0]))]);
    // Uniform pose: a DQS request stands.
    let uniform = Pose::new();
    assert_eq!(
        resolve_skin_method(SkinMethod::Dqs, &skel, &uniform).0,
        SkinMethod::Dqs
    );
    // Non-uniform scale: DQS is REFUSED (it can't represent it) and falls back to LBS, with a reason.
    let mut nonuniform = Pose::new();
    nonuniform.set(
        0,
        Transform {
            scale: [2.0, 1.0, 1.0],
            ..tf([0.0, 0.0, 0.0])
        },
    );
    let (method, reason) = resolve_skin_method(SkinMethod::Dqs, &skel, &nonuniform);
    assert_eq!(method, SkinMethod::Lbs, "DQS gated off non-uniform scale");
    assert!(reason.is_some(), "the refusal is explained");
    // LBS is never gated.
    assert_eq!(
        resolve_skin_method(SkinMethod::Lbs, &skel, &nonuniform).0,
        SkinMethod::Lbs
    );
    assert!(bone_scale::has_non_uniform_scale(&skel, &nonuniform, 1e-4));
    assert!(!bone_scale::has_non_uniform_scale(&skel, &uniform, 1e-4));
}

// ── non-uniform scale stays 4×4 (no quat-TRS round-trip that shears children) ──

#[test]
fn non_uniform_parent_scale_propagates_as_full_4x4_not_sheared() {
    // ADVERSARIAL: "non-uniform bone scale silently shears the children (quaternion-TRS path)." Our FK
    // keeps full 4×4 globals, so a child's global is EXACTLY parent_4x4 · child_local — never a lossy
    // quat decomposition.
    let skel = skeleton(&[(None, tf([0.0, 0.0, 0.0])), (Some(0), tf([1.0, 0.0, 0.0]))]);
    let mut pose = Pose::new();
    pose.set(
        0,
        Transform {
            scale: [3.0, 1.0, 1.0],
            ..tf([0.0, 0.0, 0.0])
        },
    );
    let g = skel.globals(&pose);
    let expected = mat_mul(g[0], skel.joints[1].local_bind.to_matrix());
    for c in 0..4 {
        for r in 0..4 {
            assert!(
                (g[1][c][r] - expected[c][r]).abs() < EPS,
                "child global == parent_4x4 · child_local"
            );
        }
    }
    // The child sits at x=3 (parent's 3× X scale carried the unit offset) — scale propagated correctly.
    assert!((skel.joint_position(&pose, 1)[0] - 3.0).abs() < 1e-3);
}

// ── 2-bone analytic IK: grab the foot, the leg solves ────────────────────────

#[test]
fn two_bone_ik_reaches_a_reachable_target() {
    // root → mid → end, straight along +X (bones length 1 each; reach < 2).
    let skel = skeleton(&[
        (None, tf([0.0, 0.0, 0.0])),
        (Some(0), tf([1.0, 0.0, 0.0])),
        (Some(1), tf([1.0, 0.0, 0.0])),
    ]);
    for target in [[1.0, 0.8, 0.0], [0.6, -0.9, 0.0], [1.4, 0.5, 0.0]] {
        let pole = [0.0, 1.0, 0.0];
        let posed = ik::apply_two_bone_ik(&skel, &Pose::new(), 0, 1, 2, target, pole);
        let end = skel.joint_position(&posed, 2);
        assert!(
            dist(end, target) < 1e-2,
            "IK end {end:?} reached target {target:?}"
        );
    }
}

#[test]
fn two_bone_ik_is_deterministic_and_single_pass() {
    let skel = skeleton(&[
        (None, tf([0.0, 0.0, 0.0])),
        (Some(0), tf([1.0, 0.0, 0.0])),
        (Some(1), tf([1.0, 0.0, 0.0])),
    ]);
    let target = [1.1, 0.6, 0.2];
    let pole = [0.0, 1.0, 0.0];
    let a = ik::apply_two_bone_ik(&skel, &Pose::new(), 0, 1, 2, target, pole);
    let b = ik::apply_two_bone_ik(&skel, &Pose::new(), 0, 1, 2, target, pole);
    assert_eq!(
        a, b,
        "same input → identical solve (deterministic; safe for gameplay, M8.1 boundary)"
    );
}

#[test]
fn two_bone_ik_guards_full_extension_no_nan() {
    // ADVERSARIAL: "IK NaNs at full extension." A target FAR beyond reach must straighten the limb, not NaN.
    let skel = skeleton(&[
        (None, tf([0.0, 0.0, 0.0])),
        (Some(0), tf([1.0, 0.0, 0.0])),
        (Some(1), tf([1.0, 0.0, 0.0])),
    ]);
    let target = [100.0, 0.0, 0.0]; // unreachable
    let posed = ik::apply_two_bone_ik(&skel, &Pose::new(), 0, 1, 2, target, [0.0, 1.0, 0.0]);
    let end = skel.joint_position(&posed, 2);
    assert!(
        end.iter().all(|v| v.is_finite()),
        "no NaN at full extension, got {end:?}"
    );
    // The limb straightens toward the target, reaching ~max extent (just under 2).
    assert!(
        end[0] > 1.9 && end[0] <= 2.0,
        "straightened toward the target (x≈2), got {end:?}"
    );
}

// ── pose = an override bundle re-applyable to a SELECTION of bones (G2 model) ──

#[test]
fn a_pose_applies_to_a_selection_of_bones_only() {
    let skel = skeleton(&[
        (None, tf([0.0, 0.0, 0.0])),
        (Some(0), tf([1.0, 0.0, 0.0])),
        (Some(1), tf([1.0, 0.0, 0.0])),
    ]);
    // A saved "pose" overrides ONLY joint 1 (the selection) — joints 0 and 2 keep their bind.
    let mut pose = Pose::new();
    let mut bent = skel.joints[1].local_bind;
    bent.rotation = axis_angle([0.0, 0.0, 1.0], std::f32::consts::FRAC_PI_2);
    pose.set(1, bent);
    assert_eq!(
        pose.locals.len(),
        1,
        "the pose is a SPARSE bundle — only the selected bone"
    );
    // Joint 1 bends (its child follows); joint 0 is unmoved at the origin.
    assert!(
        dist(skel.joint_position(&pose, 0), [0.0, 0.0, 0.0]) < EPS,
        "unselected root unchanged"
    );
    let end = skel.joint_position(&pose, 2);
    assert!(
        dist(end, [1.0, 1.0, 0.0]) < 1e-3,
        "the selected bend carried the end, got {end:?}"
    );
}
