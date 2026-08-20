//! What the character pipeline CLAIMS, as executable assertions.
//!
//! Each test here is one of the promises made against a named Unreal or Unity defect. They are written
//! as claims about behaviour ("an A-pose source drives a T-pose target with no authored retarget pose")
//! rather than as unit tests of internals, because the internals are allowed to change and the promises
//! are not.

// Math-heavy, like the crate it tests: the matrix→quaternion oracle's short names (m/n/x/y/z/s/tr) are
// the canonical ones from Shepperd's method, and joint counts converted to f32 lose nothing at these
// sizes. Same reasoning, and the same two allows, as `skeleton/src/lib.rs`.
#![allow(clippy::many_single_char_names, clippy::cast_precision_loss)]

use std::collections::BTreeMap;

use metrocalk_skeleton::characterize::{characterize, Evidence, RigDiagnosticCode};
use metrocalk_skeleton::humanoid::{HumanBone as HB, HumanoidProfile, RigConvention};
use metrocalk_skeleton::retarget::{retarget_pose, RetargetError};
use metrocalk_skeleton::{Joint, Pose, Skeleton, Transform};

const EPS: f32 = 1e-4;

// ── rig construction ─────────────────────────────────────────────────────────────────────────────

fn tf(t: [f32; 3], rot: [f32; 4]) -> Transform {
    Transform {
        translation: t,
        rotation: rot,
        scale: [1.0; 3],
    }
}

/// A quaternion for a rotation of `deg` about Z (the axis an arm droops around in a front-facing rig).
fn rot_z(deg: f32) -> [f32; 4] {
    let h = deg.to_radians() * 0.5;
    [0.0, 0.0, h.sin(), h.cos()]
}

const IDENT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// The bones of the test biped, in topological order, as `(bone, parent bone, local translation)`.
/// Deliberately a *minimal* humanoid: the 15 required bones plus shoulders, chest and neck — enough to
/// be retargetable, small enough to reason about by hand.
fn layout() -> Vec<(HB, Option<HB>, [f32; 3])> {
    vec![
        (HB::Hips, None, [0.0, 1.0, 0.0]),
        (HB::Spine, Some(HB::Hips), [0.0, 0.15, 0.0]),
        (HB::Chest, Some(HB::Spine), [0.0, 0.15, 0.0]),
        (HB::Neck, Some(HB::Chest), [0.0, 0.15, 0.0]),
        (HB::Head, Some(HB::Neck), [0.0, 0.10, 0.0]),
        (HB::LeftShoulder, Some(HB::Chest), [0.05, 0.10, 0.0]),
        (HB::LeftUpperArm, Some(HB::LeftShoulder), [0.10, 0.0, 0.0]),
        (HB::LeftLowerArm, Some(HB::LeftUpperArm), [0.25, 0.0, 0.0]),
        (HB::LeftHand, Some(HB::LeftLowerArm), [0.25, 0.0, 0.0]),
        (HB::RightShoulder, Some(HB::Chest), [-0.05, 0.10, 0.0]),
        (
            HB::RightUpperArm,
            Some(HB::RightShoulder),
            [-0.10, 0.0, 0.0],
        ),
        (
            HB::RightLowerArm,
            Some(HB::RightUpperArm),
            [-0.25, 0.0, 0.0],
        ),
        (HB::RightHand, Some(HB::RightLowerArm), [-0.25, 0.0, 0.0]),
        (HB::LeftUpperLeg, Some(HB::Hips), [0.08, -0.05, 0.0]),
        (HB::LeftLowerLeg, Some(HB::LeftUpperLeg), [0.0, -0.40, 0.0]),
        (HB::LeftFoot, Some(HB::LeftLowerLeg), [0.0, -0.40, 0.0]),
        (HB::RightUpperLeg, Some(HB::Hips), [-0.08, -0.05, 0.0]),
        (
            HB::RightLowerLeg,
            Some(HB::RightUpperLeg),
            [0.0, -0.40, 0.0],
        ),
        (HB::RightFoot, Some(HB::RightLowerLeg), [0.0, -0.40, 0.0]),
    ]
}

/// Raw, *un-normalized* bone names exactly as each tool spells them — so the tests exercise
/// [`metrocalk_skeleton::humanoid::normalize`] and the prefix stripping, not a pre-cleaned string.
fn raw_name(convention: RigConvention, bone: HB) -> String {
    match convention {
        RigConvention::Mixamo => {
            let stem = match bone {
                HB::Hips => "Hips",
                HB::Spine => "Spine",
                HB::Chest => "Spine1",
                HB::Neck => "Neck",
                HB::Head => "Head",
                HB::LeftShoulder => "LeftShoulder",
                HB::LeftUpperArm => "LeftArm",
                HB::LeftLowerArm => "LeftForeArm",
                HB::LeftHand => "LeftHand",
                HB::RightShoulder => "RightShoulder",
                HB::RightUpperArm => "RightArm",
                HB::RightLowerArm => "RightForeArm",
                HB::RightHand => "RightHand",
                // The trap: Mixamo's `LeftUpLeg` is the UPPER leg and `LeftLeg` is the LOWER one.
                HB::LeftUpperLeg => "LeftUpLeg",
                HB::LeftLowerLeg => "LeftLeg",
                HB::LeftFoot => "LeftFoot",
                HB::RightUpperLeg => "RightUpLeg",
                HB::RightLowerLeg => "RightLeg",
                HB::RightFoot => "RightFoot",
                _ => "Unused",
            };
            format!("mixamorig:{stem}")
        }
        RigConvention::UnrealMannequin => match bone {
            HB::Hips => "pelvis",
            HB::Spine => "spine_01",
            HB::Chest => "spine_02",
            HB::Neck => "neck_01",
            HB::Head => "head",
            HB::LeftShoulder => "clavicle_l",
            HB::LeftUpperArm => "upperarm_l",
            HB::LeftLowerArm => "lowerarm_l",
            HB::LeftHand => "hand_l",
            HB::RightShoulder => "clavicle_r",
            HB::RightUpperArm => "upperarm_r",
            HB::RightLowerArm => "lowerarm_r",
            HB::RightHand => "hand_r",
            HB::LeftUpperLeg => "thigh_l",
            HB::LeftLowerLeg => "calf_l",
            HB::LeftFoot => "foot_l",
            HB::RightUpperLeg => "thigh_r",
            HB::RightLowerLeg => "calf_r",
            HB::RightFoot => "foot_r",
            _ => "unused",
        }
        .to_string(),
        RigConvention::Rigify => match bone {
            HB::Hips => "spine",
            HB::Spine => "spine.001",
            HB::Chest => "spine.002",
            HB::Neck => "spine.005",
            HB::Head => "spine.006",
            HB::LeftShoulder => "shoulder.L",
            HB::LeftUpperArm => "upper_arm.L",
            HB::LeftLowerArm => "forearm.L",
            HB::LeftHand => "hand.L",
            HB::RightShoulder => "shoulder.R",
            HB::RightUpperArm => "upper_arm.R",
            HB::RightLowerArm => "forearm.R",
            HB::RightHand => "hand.R",
            HB::LeftUpperLeg => "thigh.L",
            HB::LeftLowerLeg => "shin.L",
            HB::LeftFoot => "foot.L",
            HB::RightUpperLeg => "thigh.R",
            HB::RightLowerLeg => "shin.R",
            HB::RightFoot => "foot.R",
            _ => "unused",
        }
        .to_string(),
        _ => bone.as_str().to_string(),
    }
}

/// Build a test biped.
///
/// * `convention` — how its bones are spelled.
/// * `droop_deg` — the arm droop of the **rest pose**: `0.0` is a T-pose, `-50.0` an A-pose. This is
///   the difference Unreal makes you reconcile by hand with an authored Retarget Pose.
/// * `scale` — overall proportions, so a child-sized rig can be retargeted from an adult-sized one.
/// * `extras` — additional non-humanoid joints (a tail, a cape) hung off the hips.
fn biped(
    convention: RigConvention,
    droop_deg: f32,
    scale: f32,
    extras: &[&str],
) -> (Skeleton, Vec<HB>) {
    let spec = layout();
    let index_of: BTreeMap<HB, usize> = spec
        .iter()
        .enumerate()
        .map(|(i, (b, _, _))| (*b, i))
        .collect();

    let mut joints: Vec<Joint> = Vec::new();
    for (bone, parent, t) in &spec {
        // The droop lives on the shoulders, so the whole arm chain below rotates with it — which is
        // exactly how a real A-pose rig is authored, and why the source and target rest poses differ.
        let rot = match bone {
            HB::LeftShoulder => rot_z(droop_deg),
            HB::RightShoulder => rot_z(-droop_deg),
            _ => IDENT,
        };
        joints.push(Joint {
            name: raw_name(convention, *bone),
            parent: parent.map(|p| index_of[&p]),
            local_bind: tf([t[0] * scale, t[1] * scale, t[2] * scale], rot),
            inverse_bind: [[0.0; 4]; 4],
        });
    }
    let hips = index_of[&HB::Hips];
    for (k, name) in extras.iter().enumerate() {
        joints.push(Joint {
            name: (*name).to_string(),
            parent: Some(hips),
            local_bind: tf([0.0, -0.05 * (k as f32 + 1.0), -0.1], IDENT),
            inverse_bind: [[0.0; 4]; 4],
        });
    }

    let mut skel = Skeleton { joints };
    skel.recompute_inverse_binds();
    let bones = spec.iter().map(|(b, _, _)| *b).collect();
    (skel, bones)
}

/// The world-space rotation of a joint under a pose, as a glam-free quaternion `[x,y,z,w]`.
fn world_rot(skel: &Skeleton, pose: &Pose, joint: usize) -> [f32; 4] {
    let m = skel.globals(pose)[joint];
    // Gram-Schmidt the upper 3x3 → the rotation, matching what the retargeter extracts.
    let (c0, c1, c2) = (
        [m[0][0], m[0][1], m[0][2]],
        [m[1][0], m[1][1], m[1][2]],
        [m[2][0], m[2][1], m[2][2]],
    );
    let n = |v: [f32; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / l, v[1] / l, v[2] / l]
    };
    let (x, y, z) = (n(c0), n(c1), n(c2));
    // matrix → quaternion (Shepperd), enough for a test oracle.
    let tr = x[0] + y[1] + z[2];
    if tr > 0.0 {
        let s = (tr + 1.0).sqrt() * 2.0;
        [
            (y[2] - z[1]) / s,
            (z[0] - x[2]) / s,
            (x[1] - y[0]) / s,
            0.25 * s,
        ]
    } else if x[0] > y[1] && x[0] > z[2] {
        let s = (1.0 + x[0] - y[1] - z[2]).sqrt() * 2.0;
        [
            0.25 * s,
            (y[0] + x[1]) / s,
            (z[0] + x[2]) / s,
            (y[2] - z[1]) / s,
        ]
    } else if y[1] > z[2] {
        let s = (1.0 + y[1] - x[0] - z[2]).sqrt() * 2.0;
        [
            (y[0] + x[1]) / s,
            0.25 * s,
            (z[1] + y[2]) / s,
            (z[0] - x[2]) / s,
        ]
    } else {
        let s = (1.0 + z[2] - x[0] - y[1]).sqrt() * 2.0;
        [
            (z[0] + x[2]) / s,
            (z[1] + y[2]) / s,
            0.25 * s,
            (x[1] - y[0]) / s,
        ]
    }
}

/// `a · b⁻¹` for unit quaternions, canonicalized to the positive-w hemisphere so `q` and `-q` — the
/// same rotation — compare equal.
fn delta(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let bi = [-b[0], -b[1], -b[2], b[3]];
    let q = [
        a[3] * bi[0] + a[0] * bi[3] + a[1] * bi[2] - a[2] * bi[1],
        a[3] * bi[1] - a[0] * bi[2] + a[1] * bi[3] + a[2] * bi[0],
        a[3] * bi[2] + a[0] * bi[1] - a[1] * bi[0] + a[2] * bi[3],
        a[3] * bi[3] - a[0] * bi[0] - a[1] * bi[1] - a[2] * bi[2],
    ];
    if q[3] < 0.0 {
        [-q[0], -q[1], -q[2], -q[3]]
    } else {
        q
    }
}

fn assert_quat_eq(got: [f32; 4], want: [f32; 4], what: &str) {
    let g = if got[3] < 0.0 {
        [-got[0], -got[1], -got[2], -got[3]]
    } else {
        got
    };
    let w = if want[3] < 0.0 {
        [-want[0], -want[1], -want[2], -want[3]]
    } else {
        want
    };
    for k in 0..4 {
        assert!(
            (g[k] - w[k]).abs() < EPS,
            "{what}: component {k} — got {g:?}, want {w:?}"
        );
    }
}

// ── characterization: the engine reads the rig instead of asking ─────────────────────────────────

#[test]
fn a_mixamo_rig_is_recognized_without_the_user_mapping_a_single_bone() {
    let (skel, _) = biped(RigConvention::Mixamo, 0.0, 1.0, &[]);
    let ch = characterize(&skel);

    assert_eq!(ch.profile.convention, Some(RigConvention::Mixamo));
    assert!(
        ch.is_retargetable(),
        "all 15 required bones should be identified; missing {:?}",
        ch.profile.missing_required()
    );
    // The whole point: no diagnostic asks the user to do anything, because there is nothing to do.
    assert!(
        !ch.diagnostics.iter().any(|d| d.code.is_blocking()),
        "a clean Mixamo rig must import with no blocking diagnostic, got {:?}",
        ch.diagnostics
    );
}

#[test]
fn mixamos_left_leg_is_the_lower_leg_and_left_up_leg_the_upper_one() {
    // THE COLLISION THAT DEFEATS FUZZY MATCHING. `LeftLeg` reads like a thigh and is a shin. A matcher
    // that scored the token `leg` in isolation would put the knee where the hip goes — and produce a
    // character that walks subtly wrongly forever, with nothing on screen to say why.
    let (skel, _) = biped(RigConvention::Mixamo, 0.0, 1.0, &[]);
    let ch = characterize(&skel);

    let upper = ch.profile.joint(HB::LeftUpperLeg).expect("upper leg");
    let lower = ch.profile.joint(HB::LeftLowerLeg).expect("lower leg");
    assert_eq!(skel.joints[upper].name, "mixamorig:LeftUpLeg");
    assert_eq!(skel.joints[lower].name, "mixamorig:LeftLeg");
    assert_eq!(
        skel.joints[lower].parent,
        Some(upper),
        "the lower leg must be the upper leg's child, or the chain is inverted"
    );
}

#[test]
fn the_unreal_mannequin_and_a_rigify_rig_are_both_recognized_as_themselves() {
    for (convention, sample) in [
        (RigConvention::UnrealMannequin, "lowerarm_l"),
        (RigConvention::Rigify, "forearm.L"),
    ] {
        let (skel, _) = biped(convention, 0.0, 1.0, &[]);
        let ch = characterize(&skel);
        assert_eq!(
            ch.profile.convention,
            Some(convention),
            "{} should be recognized as {}",
            sample,
            convention.label()
        );
        assert!(
            ch.is_retargetable(),
            "{} should be retargetable",
            convention.label()
        );
        let arm = ch.profile.joint(HB::LeftLowerArm).expect("left lower arm");
        assert_eq!(skel.joints[arm].name, sample);
    }
}

#[test]
fn every_identified_bone_carries_the_evidence_that_identified_it() {
    // A confidence number with no story behind it is not something a user can act on. Every bone the
    // engine claims must be able to say WHY, in a sentence naming the actual string it matched.
    let (skel, _) = biped(RigConvention::Mixamo, 0.0, 1.0, &[]);
    let ch = characterize(&skel);

    for bone in ch.profile.bones.keys() {
        let ev = ch
            .evidence
            .get(bone)
            .expect("evidence for every mapped bone");
        assert!(
            ev.is_name_match(),
            "{bone:?} should be a name match, got {ev:?}"
        );
        let text = ev.describe();
        assert!(
            text.contains("Mixamo"),
            "evidence should name the convention: {text}"
        );
    }
    let hips_evidence = ch.evidence[&HB::Hips].describe();
    assert_eq!(hips_evidence, "named `hips` (Mixamo)");
}

#[test]
fn an_unnamed_rig_falls_back_to_shape_and_says_so_instead_of_claiming_a_name_match() {
    let (mut skel, _) = biped(RigConvention::Mixamo, 0.0, 1.0, &[]);
    for j in &mut skel.joints {
        j.name = String::new();
    }
    let ch = characterize(&skel);

    assert_eq!(ch.profile.convention, None);
    // Shape alone must still find the spine and the legs — the 5-chain silhouette of a biped.
    assert!(
        ch.profile.joint(HB::Hips).is_some(),
        "the hips are findable by shape"
    );
    assert!(matches!(ch.evidence[&HB::Hips], Evidence::Topology { .. }));
    // And it must be honest that this is the weaker kind of evidence.
    assert!(
        ch.diagnostics.iter().any(|d| matches!(
            d.code,
            RigDiagnosticCode::UnrecognizedConvention | RigDiagnosticCode::NoNames
        )),
        "a shape-only match must say so, got {:?}",
        ch.diagnostics
    );
}

#[test]
fn a_rig_that_is_not_a_humanoid_names_the_missing_bones_and_how_to_fix_them() {
    // Unity's Avatar screen fails with a red cross and no sentence. This must fail with both.
    let (mut skel, _) = biped(RigConvention::Mixamo, 0.0, 1.0, &[]);
    skel.joints.truncate(5); // hips, spine, chest, neck, head — no limbs at all
    let ch = characterize(&skel);

    assert!(!ch.is_retargetable());
    let blocking = ch
        .diagnostics
        .iter()
        .find(|d| d.code == RigDiagnosticCode::MissingRequiredBone)
        .expect("a blocking diagnostic");
    assert!(
        blocking.message.contains("Left Upper Arm"),
        "{}",
        blocking.message
    );
    assert!(
        !blocking.remediation.is_empty(),
        "a diagnostic without a remediation is a complaint"
    );
}

#[test]
fn non_humanoid_joints_are_reported_as_kept_not_dropped() {
    let (skel, _) = biped(RigConvention::Mixamo, 0.0, 1.0, &["Tail_01", "Cape_02"]);
    let ch = characterize(&skel);

    let extras = ch.profile.extra_joints(ch.joint_count);
    assert_eq!(extras.len(), 2, "the tail and the cape must both survive");
    let d = ch
        .diagnostics
        .iter()
        .find(|d| d.code == RigDiagnosticCode::ExtraBonesKept)
        .expect("extras are worth a sentence");
    assert!(d.remediation.contains("Nothing to do"), "{}", d.remediation);
}

#[test]
fn characterization_is_deterministic() {
    let (skel, _) = biped(RigConvention::UnrealMannequin, 0.0, 1.0, &["Tail"]);
    let a = characterize(&skel);
    let b = characterize(&skel);
    assert_eq!(
        a, b,
        "the same rig must characterize identically every time"
    );
}

#[test]
fn a_bone_can_be_corrected_by_hand_and_the_correction_outranks_inference() {
    let (skel, _) = biped(RigConvention::Mixamo, 0.0, 1.0, &["Tail_01"]);
    let mut ch = characterize(&skel);
    let tail = ch.joint_count - 1;

    ch.override_bone(HB::Head, Some(tail));
    assert_eq!(ch.profile.joint(HB::Head), Some(tail));
    assert_eq!(ch.evidence[&HB::Head], Evidence::Manual);

    // Unsetting is allowed, and it is the honest way to say "this rig has no such bone".
    ch.override_bone(HB::Head, None);
    assert_eq!(ch.profile.joint(HB::Head), None);
    assert!(
        !ch.is_retargetable(),
        "Head is required — dropping it must block"
    );
}

// ── retargeting: the pure function ───────────────────────────────────────────────────────────────

#[test]
fn an_a_pose_source_drives_a_t_pose_target_with_no_authored_retarget_pose() {
    // THE HEADLINE CLAIM. Unreal calls the retarget pose "the single most consequential setting in the
    // workflow" and leaves it to hand-authoring. Here the two rests cancel, so there is nothing to
    // author: a source standing in ITS rest pose must put the target in ITS OWN rest pose, exactly —
    // an A-pose source must NOT bend a T-pose target's arms down.
    let (src, _) = biped(RigConvention::Mixamo, -50.0, 1.0, &[]);
    let (dst, _) = biped(RigConvention::UnrealMannequin, 0.0, 1.0, &[]);
    let (sch, dch) = (characterize(&src), characterize(&dst));

    let (pose, report) = retarget_pose(&src, &sch, &Pose::new(), &dst, &dch).expect("retargetable");

    for bone in &report.driven {
        let j = dch.profile.joint(*bone).unwrap();
        assert_quat_eq(
            world_rot(&dst, &pose, j),
            world_rot(&dst, &Pose::new(), j),
            &format!("{bone:?} should be at the TARGET's own rest orientation"),
        );
    }
}

#[test]
fn a_rotation_applied_to_the_source_arrives_on_the_target_undiluted() {
    let (src, _) = biped(RigConvention::Mixamo, -50.0, 1.0, &[]);
    let (dst, _) = biped(RigConvention::UnrealMannequin, 0.0, 1.0, &[]);
    let (sch, dch) = (characterize(&src), characterize(&dst));

    // Lift the source's left upper arm 30° — in the SOURCE's A-pose frame.
    let s_arm = sch.profile.joint(HB::LeftUpperArm).unwrap();
    let mut sp = Pose::new();
    let bind = src.joints[s_arm].local_bind;
    sp.set(
        s_arm,
        Transform {
            rotation: rot_z(30.0),
            ..bind
        },
    );

    let (pose, _) = retarget_pose(&src, &sch, &sp, &dst, &dch).expect("retargetable");

    // The source's world delta from its own rest…
    let s_delta = delta(
        world_rot(&src, &sp, s_arm),
        world_rot(&src, &Pose::new(), s_arm),
    );
    // …must equal the target's world delta from ITS own rest.
    let d_arm = dch.profile.joint(HB::LeftUpperArm).unwrap();
    let d_delta = delta(
        world_rot(&dst, &pose, d_arm),
        world_rot(&dst, &Pose::new(), d_arm),
    );
    assert_quat_eq(
        d_delta,
        s_delta,
        "the arm's world-space delta must transfer exactly",
    );
}

#[test]
fn a_difference_at_the_shoulder_cannot_reach_the_fingers() {
    // THE UNREAL DEFECT, AS A STANDING ASSERTION. Editing Unreal's retarget pose propagates rotation
    // down the hierarchy and canonically destroys the hands. Here every bone's world orientation is
    // rebuilt from its OWN rest and its OWN delta, so error cannot accumulate along a chain.
    //
    // The rigs differ at the shoulder by 50° AND differ in proportion, which is exactly the setup that
    // makes a naive local-rotation copy drift further at every joint down the arm.
    let (src, _) = biped(RigConvention::Mixamo, -50.0, 1.0, &[]);
    let (dst, _) = biped(RigConvention::Rigify, 0.0, 1.7, &[]);
    let (sch, dch) = (characterize(&src), characterize(&dst));

    let mut sp = Pose::new();
    for (bone, deg) in [
        (HB::LeftShoulder, 20.0),
        (HB::LeftUpperArm, 35.0),
        (HB::LeftLowerArm, -25.0),
        (HB::LeftHand, 15.0),
    ] {
        let j = sch.profile.joint(bone).unwrap();
        let bind = src.joints[j].local_bind;
        sp.set(
            j,
            Transform {
                rotation: rot_z(deg),
                ..bind
            },
        );
    }

    let (pose, report) = retarget_pose(&src, &sch, &sp, &dst, &dch).expect("retargetable");

    // The invariant, stated over EVERY driven bone — proximal and distal alike. If corrections
    // compounded, the error would grow monotonically down the arm and the hand would fail first.
    for bone in &report.driven {
        let s = sch.profile.joint(*bone).unwrap();
        let d = dch.profile.joint(*bone).unwrap();
        assert_quat_eq(
            delta(world_rot(&dst, &pose, d), world_rot(&dst, &Pose::new(), d)),
            delta(world_rot(&src, &sp, s), world_rot(&src, &Pose::new(), s)),
            &format!("{bone:?} accumulated error from an ancestor"),
        );
    }
}

#[test]
fn a_tail_and_a_cape_survive_retargeting_untouched() {
    // Unity silently drops every bone outside its humanoid enum. These must come through unchanged —
    // still in the skeleton, still at their authored bind, and counted in the report.
    let (src, _) = biped(RigConvention::Mixamo, -50.0, 1.0, &[]);
    let (dst, _) = biped(RigConvention::Mixamo, 0.0, 1.0, &["Tail_01", "Cape_02"]);
    let (sch, dch) = (characterize(&src), characterize(&dst));

    let mut sp = Pose::new();
    let s_arm = sch.profile.joint(HB::LeftUpperArm).unwrap();
    let bind = src.joints[s_arm].local_bind;
    sp.set(
        s_arm,
        Transform {
            rotation: rot_z(40.0),
            ..bind
        },
    );

    let (pose, report) = retarget_pose(&src, &sch, &sp, &dst, &dch).expect("retargetable");

    assert_eq!(report.preserved_extras, 2, "{}", report.summary());
    let tail = dst.joints.len() - 2;
    let cape = dst.joints.len() - 1;
    assert_eq!(dst.joints[tail].name, "Tail_01");
    // Untouched means literally absent from the pose — a Pose is the set of joints that CHANGED.
    assert!(
        !pose.locals.contains_key(&tail),
        "the tail must not be written"
    );
    assert!(
        !pose.locals.contains_key(&cape),
        "the cape must not be written"
    );
    assert_quat_eq(
        world_rot(&dst, &pose, tail),
        world_rot(&dst, &Pose::new(), tail),
        "the tail must be exactly where the rig authored it",
    );
}

#[test]
fn root_motion_is_scaled_to_the_target_so_a_small_character_takes_small_steps() {
    // Unreal makes "root translation vs stride length" a per-animation either/or the user must choose,
    // and choosing wrong causes sliding. Scaling by the hip-height ratio is the answer that needs no
    // choice: displacement is proportional to the body that is moving.
    let (src, _) = biped(RigConvention::Mixamo, 0.0, 1.0, &[]);
    let (dst, _) = biped(RigConvention::Mixamo, 0.0, 0.5, &[]); // a half-height character
    let (sch, dch) = (characterize(&src), characterize(&dst));

    let s_hips = sch.profile.joint(HB::Hips).unwrap();
    let mut sp = Pose::new();
    let bind = src.joints[s_hips].local_bind;
    sp.set(
        s_hips,
        Transform {
            translation: [
                bind.translation[0],
                bind.translation[1],
                bind.translation[2] + 1.0,
            ],
            ..bind
        },
    );

    let (pose, report) = retarget_pose(&src, &sch, &sp, &dst, &dch).expect("retargetable");

    assert!(
        (report.hip_scale - 0.5).abs() < EPS,
        "a half-height rig should scale root motion by 0.5, got {}",
        report.hip_scale
    );
    let d_hips = dch.profile.joint(HB::Hips).unwrap();
    let moved = pose.locals[&d_hips].translation[2] - dst.joints[d_hips].local_bind.translation[2];
    assert!(
        (moved - 0.5).abs() < EPS,
        "1.0 of source travel should become 0.5 on a half-height character, got {moved}"
    );
}

#[test]
fn retargeting_is_deterministic() {
    let (src, _) = biped(RigConvention::Mixamo, -50.0, 1.0, &[]);
    let (dst, _) = biped(RigConvention::Rigify, 0.0, 1.3, &["Tail"]);
    let (sch, dch) = (characterize(&src), characterize(&dst));

    let s_arm = sch.profile.joint(HB::LeftLowerArm).unwrap();
    let mut sp = Pose::new();
    let bind = src.joints[s_arm].local_bind;
    sp.set(
        s_arm,
        Transform {
            rotation: rot_z(22.5),
            ..bind
        },
    );

    let a = retarget_pose(&src, &sch, &sp, &dst, &dch).expect("retargetable");
    let b = retarget_pose(&src, &sch, &sp, &dst, &dch).expect("retargetable");
    assert_eq!(a.0, b.0, "the same inputs must produce the same pose");
    assert_eq!(a.1, b.1, "…and the same report");
}

#[test]
fn a_rig_that_cannot_be_retargeted_says_which_side_and_which_bones() {
    let (src, _) = biped(RigConvention::Mixamo, 0.0, 1.0, &[]);
    let (mut dst, _) = biped(RigConvention::Mixamo, 0.0, 1.0, &[]);
    dst.joints.truncate(5); // strip the limbs off the TARGET
    let (sch, dch) = (characterize(&src), characterize(&dst));

    let err = retarget_pose(&src, &sch, &Pose::new(), &dst, &dch).expect_err("must refuse");
    let RetargetError::NotRetargetable { which, missing } = &err;
    assert_eq!(*which, "target", "the failing side must be named");
    assert!(missing.contains(&HB::LeftUpperArm));

    let (message, remediation) = err.describe();
    assert!(message.contains("target"), "{message}");
    assert!(remediation.contains("rig panel"), "{remediation}");
}

// ── the vocabulary itself ────────────────────────────────────────────────────────────────────────

#[test]
fn the_vrm_name_table_stays_aligned_with_the_variant_order() {
    // `HumanBone::as_str` indexes a static table by `self as usize`. If a variant is ever inserted in
    // the middle without the table being updated, every bone after it silently renames itself — the
    // exact class of drift this repository gates for everywhere else.
    for bone in HB::ALL {
        assert_eq!(
            HB::from_vrm_name(bone.as_str()),
            Some(bone),
            "{bone:?} does not round-trip through its VRM name {:?}",
            bone.as_str()
        );
    }
    assert_eq!(HB::ALL.len(), 55, "VRM 1.0 defines exactly 55 human bones");
    assert_eq!(HB::Hips.as_str(), "hips");
    assert_eq!(HB::LeftUpperArm.as_str(), "leftUpperArm");
    assert_eq!(HB::LeftUpperArm.label(), "Left Upper Arm");
}

#[test]
fn the_canonical_parent_chain_reaches_the_hips_from_every_bone() {
    // Retargeting walks the canonical hierarchy rather than a concrete rig's, so it must be a tree
    // rooted at the hips — no cycles, no orphans.
    for bone in HB::ALL {
        let mut hops = 0;
        let mut cur = bone;
        while let Some(parent) = cur.canonical_parent() {
            cur = parent;
            hops += 1;
            assert!(
                hops < 20,
                "{bone:?} does not terminate — a cycle in the canonical parents"
            );
        }
        assert_eq!(cur, HB::Hips, "{bone:?} does not descend from the hips");
    }
}

#[test]
fn an_empty_profile_is_honest_about_everything_it_is_missing() {
    let p = HumanoidProfile::default();
    assert!(!p.is_complete());
    assert_eq!(p.missing_required().len(), 15);
    assert_eq!(p.extra_joints(3), vec![0, 1, 2]);
}
