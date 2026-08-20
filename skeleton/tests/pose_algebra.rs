//! Blending, masking and additive layering — the operations without which a `Pose` can only be *set*,
//! never combined, and therefore without which layered character animation does not exist.
//!
//! The claim these tests are written against is the mask: in Unity a layer mask is an authored Avatar
//! Mask asset, and in this engine's own animation kernel a mask is an explicit entry **per property
//! path**. Here "the left arm" is one call that resolves from the hierarchy — correct on every rig,
//! and incapable of going stale when a rig gains a bone.

// Math-heavy like the crate it tests; short names in the oracle are the canonical ones.
#![allow(clippy::many_single_char_names, clippy::cast_precision_loss)]

use metrocalk_skeleton::characterize::{characterize, HumanoidCharacterization};
use metrocalk_skeleton::humanoid::{HumanBone as HB, RigConvention};
use metrocalk_skeleton::pose_ops::{additive, blend, blend_masked, masked, BoneMask};
use metrocalk_skeleton::{Joint, Pose, Skeleton, Transform};

const EPS: f32 = 1e-4;

fn tf(t: [f32; 3]) -> Transform {
    Transform {
        translation: t,
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0; 3],
    }
}

fn rot_z(deg: f32) -> [f32; 4] {
    let h = deg.to_radians() * 0.5;
    [0.0, 0.0, h.sin(), h.cos()]
}

/// θ from `q = [0, 0, sin(θ/2), cos(θ/2)]`.
fn angle_z(q: [f32; 4]) -> f32 {
    2.0 * q[2].atan2(q[3]).to_degrees()
}

/// The same minimal biped in either convention, plus a tail — enough hierarchy for a subtree mask to
/// mean something.
fn biped(convention: RigConvention) -> (Skeleton, HumanoidCharacterization) {
    let spec: Vec<(HB, Option<usize>, [f32; 3])> = vec![
        (HB::Hips, None, [0.0, 1.0, 0.0]),
        (HB::Spine, Some(0), [0.0, 0.15, 0.0]),
        (HB::Chest, Some(1), [0.0, 0.15, 0.0]),
        (HB::Neck, Some(2), [0.0, 0.15, 0.0]),
        (HB::Head, Some(3), [0.0, 0.10, 0.0]),
        (HB::LeftShoulder, Some(2), [0.05, 0.10, 0.0]),
        (HB::LeftUpperArm, Some(5), [0.10, 0.0, 0.0]),
        (HB::LeftLowerArm, Some(6), [0.25, 0.0, 0.0]),
        (HB::LeftHand, Some(7), [0.25, 0.0, 0.0]),
        (HB::RightShoulder, Some(2), [-0.05, 0.10, 0.0]),
        (HB::RightUpperArm, Some(9), [-0.10, 0.0, 0.0]),
        (HB::RightLowerArm, Some(10), [-0.25, 0.0, 0.0]),
        (HB::RightHand, Some(11), [-0.25, 0.0, 0.0]),
        (HB::LeftUpperLeg, Some(0), [0.08, -0.05, 0.0]),
        (HB::LeftLowerLeg, Some(13), [0.0, -0.40, 0.0]),
        (HB::LeftFoot, Some(14), [0.0, -0.40, 0.0]),
        (HB::RightUpperLeg, Some(0), [-0.08, -0.05, 0.0]),
        (HB::RightLowerLeg, Some(16), [0.0, -0.40, 0.0]),
        (HB::RightFoot, Some(17), [0.0, -0.40, 0.0]),
    ];

    let name = |b: HB| -> String {
        match convention {
            RigConvention::Mixamo => format!(
                "mixamorig:{}",
                match b {
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
                    HB::LeftUpperLeg => "LeftUpLeg",
                    HB::LeftLowerLeg => "LeftLeg",
                    HB::LeftFoot => "LeftFoot",
                    HB::RightUpperLeg => "RightUpLeg",
                    HB::RightLowerLeg => "RightLeg",
                    HB::RightFoot => "RightFoot",
                    _ => "Unused",
                }
            ),
            _ => match b {
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
        }
    };

    let mut joints: Vec<Joint> = spec
        .iter()
        .map(|(b, parent, t)| Joint {
            name: name(*b),
            parent: *parent,
            local_bind: tf(*t),
            inverse_bind: [[0.0; 4]; 4],
        })
        .collect();
    joints.push(Joint {
        name: "Tail_01".into(),
        parent: Some(0),
        local_bind: tf([0.0, 0.0, -0.12]),
        inverse_bind: [[0.0; 4]; 4],
    });

    let mut skel = Skeleton { joints };
    skel.recompute_inverse_binds();
    let ch = characterize(&skel);
    (skel, ch)
}

/// A pose that rotates one humanoid bone about Z.
fn bent(skel: &Skeleton, ch: &HumanoidCharacterization, bone: HB, deg: f32) -> Pose {
    let j = ch.profile.joint(bone).expect("bone present");
    let mut p = Pose::new();
    p.set(
        j,
        Transform {
            rotation: rot_z(deg),
            ..skel.joints[j].local_bind
        },
    );
    p
}

// ── blending ─────────────────────────────────────────────────────────────────────────────────────

#[test]
fn a_blend_lands_on_each_end_at_the_extremes_and_halfway_in_between() {
    let (skel, ch) = biped(RigConvention::Mixamo);
    let a = bent(&skel, &ch, HB::LeftUpperArm, 10.0);
    let b = bent(&skel, &ch, HB::LeftUpperArm, 60.0);
    let j = ch.profile.joint(HB::LeftUpperArm).unwrap();

    assert_eq!(blend(&skel, &a, &b, 0.0), a, "t=0 is the first pose");
    assert_eq!(blend(&skel, &a, &b, 1.0), b, "t=1 is the second");

    let mid = blend(&skel, &a, &b, 0.5);
    let got = angle_z(mid.locals[&j].rotation);
    assert!(
        (got - 35.0).abs() < 0.01,
        "half-way between 10° and 60° is 35°, got {got}"
    );
}

#[test]
fn a_blend_returns_a_pose_in_canonical_sparse_form() {
    // A `Pose` can be BUILT holding an entry that happens to equal the joint's bind local — nothing
    // stops a caller doing that. A blend normalizes it away, so `blend(a, b, 0.0)` is `a` only up to
    // that normalization. Worth pinning rather than leaving as a surprise: it is what makes two poses
    // that describe the same body compare equal, which every test above relies on.
    let (skel, ch) = biped(RigConvention::Mixamo);
    let j = ch.profile.joint(HB::LeftUpperArm).unwrap();
    let redundant = bent(&skel, &ch, HB::LeftUpperArm, 0.0); // an explicit bind-valued entry
    assert_eq!(redundant.locals.len(), 1, "built with a redundant entry");

    let normalized = blend(&skel, &redundant, &redundant, 0.0);
    assert!(
        normalized.locals.is_empty(),
        "a bind-valued entry is not a change, so it is dropped; got {:?}",
        normalized.locals
    );
    let _ = j;
}

#[test]
fn a_blend_takes_the_short_way_round() {
    // A component-wise lerp between 170° and -170° travels 340° the wrong way — the artifact that makes
    // naive rotation blending unusable. Slerp goes the 20° way.
    let (skel, ch) = biped(RigConvention::Mixamo);
    let a = bent(&skel, &ch, HB::LeftUpperArm, 170.0);
    let b = bent(&skel, &ch, HB::LeftUpperArm, -170.0);
    let j = ch.profile.joint(HB::LeftUpperArm).unwrap();

    let mid = blend(&skel, &a, &b, 0.5);
    let got = angle_z(mid.locals[&j].rotation).abs();
    assert!(
        (got - 180.0).abs() < 0.5,
        "the short arc through ±180° should land at 180°, got {got}"
    );
}

#[test]
fn a_blend_only_writes_joints_one_of_the_two_poses_named() {
    // The sparseness invariant: everything else is bind on both sides, so it blends to bind and stays
    // absent. A blend that materialized all 20 joints would turn every later layer into a full-body
    // override without anyone asking for one.
    let (skel, ch) = biped(RigConvention::Mixamo);
    let a = bent(&skel, &ch, HB::LeftUpperArm, 20.0);
    let b = bent(&skel, &ch, HB::RightUpperArm, 20.0);

    let mid = blend(&skel, &a, &b, 0.5);
    assert_eq!(
        mid.locals.len(),
        2,
        "only the two named arms, got {:?}",
        mid.locals
    );
}

#[test]
fn blending_toward_an_empty_pose_fades_toward_the_rest_pose() {
    // "Blend out" is blending against nothing, and nothing MUST mean the bind pose — not identity. A
    // sampler that read the absent side as identity would snap every bone's offset to zero.
    let (skel, ch) = biped(RigConvention::Mixamo);
    let a = bent(&skel, &ch, HB::LeftUpperArm, 80.0);
    let j = ch.profile.joint(HB::LeftUpperArm).unwrap();

    let out = blend(&skel, &a, &Pose::new(), 0.5);
    let got = angle_z(out.locals[&j].rotation);
    assert!(
        (got - 40.0).abs() < 0.01,
        "half-way out of 80° is 40°, got {got}"
    );

    // All the way out is exactly bind — and therefore an EMPTY pose, not a pose full of bind values.
    let fully_out = blend(&skel, &a, &Pose::new(), 1.0);
    assert!(
        fully_out.locals.is_empty(),
        "a pose equal to bind is the empty pose, got {:?}",
        fully_out.locals
    );
}

// ── masking ──────────────────────────────────────────────────────────────────────────────────────

#[test]
fn a_subtree_mask_covers_every_descendant_and_nothing_else() {
    let (skel, ch) = biped(RigConvention::Mixamo);
    let shoulder = ch.profile.joint(HB::LeftShoulder).unwrap();
    let mask = BoneMask::subtree(&skel, shoulder);

    for bone in [
        HB::LeftShoulder,
        HB::LeftUpperArm,
        HB::LeftLowerArm,
        HB::LeftHand,
    ] {
        let j = ch.profile.joint(bone).unwrap();
        assert!(mask.contains(j), "{bone:?} is under the left shoulder");
    }
    for bone in [HB::RightUpperArm, HB::Head, HB::LeftFoot, HB::Hips] {
        let j = ch.profile.joint(bone).unwrap();
        assert!(!mask.contains(j), "{bone:?} is NOT under the left shoulder");
    }
    assert_eq!(mask.count(), 4);
}

#[test]
fn the_left_arm_is_written_once_and_is_correct_on_rigs_that_share_no_bone_names() {
    // THE POINT OF THE HUMANOID MASK. Two rigs, no name in common, one expression — and it selects the
    // same four anatomical bones in both. In Unity this is an authored Avatar Mask asset per rig.
    for convention in [RigConvention::Mixamo, RigConvention::UnrealMannequin] {
        let (skel, ch) = biped(convention);
        let mask = BoneMask::humanoid_subtree(&skel, &ch, HB::LeftShoulder);
        assert_eq!(
            mask.count(),
            4,
            "{}: the left arm is 4 bones",
            convention.label()
        );
        let hand = ch.profile.joint(HB::LeftHand).unwrap();
        assert!(
            mask.contains(hand),
            "{}: the hand is in the arm",
            convention.label()
        );
    }
}

#[test]
fn masking_to_a_bone_the_rig_lacks_selects_nothing_rather_than_everything() {
    // The dangerous default. "This character has no toes, so mask everything" would silently apply a
    // foot layer to the whole body.
    let (skel, ch) = biped(RigConvention::Mixamo);
    assert!(
        ch.profile.joint(HB::LeftToes).is_none(),
        "the test rig has no toes"
    );
    let mask = BoneMask::humanoid_subtree(&skel, &ch, HB::LeftToes);
    assert_eq!(mask.count(), 0);
}

#[test]
fn a_masked_blend_plays_one_animation_on_the_arms_while_the_legs_keep_the_other() {
    // The whole of layered animation, in one call.
    let (skel, ch) = biped(RigConvention::Mixamo);

    let mut run = Pose::new();
    for bone in [HB::LeftUpperLeg, HB::LeftUpperArm] {
        let j = ch.profile.joint(bone).unwrap();
        run.set(
            j,
            Transform {
                rotation: rot_z(15.0),
                ..skel.joints[j].local_bind
            },
        );
    }
    let reload = bent(&skel, &ch, HB::LeftUpperArm, 75.0);
    let arms = BoneMask::humanoid_subtree(&skel, &ch, HB::LeftShoulder);

    let out = blend_masked(&skel, &run, &reload, 1.0, Some(&arms));

    let arm = ch.profile.joint(HB::LeftUpperArm).unwrap();
    let leg = ch.profile.joint(HB::LeftUpperLeg).unwrap();
    assert!(
        (angle_z(out.locals[&arm].rotation) - 75.0).abs() < 0.01,
        "the arm takes the reload"
    );
    assert!(
        (angle_z(out.locals[&leg].rotation) - 15.0).abs() < 0.01,
        "the leg keeps running — it is outside the mask"
    );
}

#[test]
fn masked_drops_everything_outside_the_mask() {
    let (skel, ch) = biped(RigConvention::Mixamo);
    let mut p = bent(&skel, &ch, HB::LeftUpperArm, 20.0);
    let leg = ch.profile.joint(HB::LeftUpperLeg).unwrap();
    p.set(
        leg,
        Transform {
            rotation: rot_z(10.0),
            ..skel.joints[leg].local_bind
        },
    );

    let arms = BoneMask::humanoid_subtree(&skel, &ch, HB::LeftShoulder);
    let only_arms = masked(&p, &arms);
    assert_eq!(only_arms.locals.len(), 1);
    assert!(!only_arms.locals.contains_key(&leg));
}

#[test]
fn an_inverted_mask_is_the_rest_of_the_body() {
    let (skel, ch) = biped(RigConvention::Mixamo);
    let arms = BoneMask::humanoid_subtree(&skel, &ch, HB::LeftShoulder);
    let n = skel.joints.len();
    let rest = arms.clone().inverted();
    assert_eq!(arms.count() + rest.count(), n);
    let hand = ch.profile.joint(HB::LeftHand).unwrap();
    assert!(!rest.contains(hand));
}

// ── additive ─────────────────────────────────────────────────────────────────────────────────────

#[test]
fn an_additive_layer_adds_its_delta_from_the_reference_pose() {
    let (skel, ch) = biped(RigConvention::Mixamo);
    let j = ch.profile.joint(HB::Spine).unwrap();

    let base = bent(&skel, &ch, HB::Spine, 10.0);
    let reference = bent(&skel, &ch, HB::Spine, 0.0); // the pose the lean was authored FROM
    let lean = bent(&skel, &ch, HB::Spine, 25.0); // the authored lean

    let out = additive(&skel, &base, &lean, &reference, 1.0, None);
    let got = angle_z(out.locals[&j].rotation);
    assert!(
        (got - 35.0).abs() < 0.01,
        "10° of base plus a 25° delta is 35°, got {got}"
    );
}

#[test]
fn an_additive_layer_at_half_weight_adds_half_the_delta() {
    let (skel, ch) = biped(RigConvention::Mixamo);
    let j = ch.profile.joint(HB::Spine).unwrap();
    let base = bent(&skel, &ch, HB::Spine, 10.0);
    let reference = bent(&skel, &ch, HB::Spine, 0.0);
    let lean = bent(&skel, &ch, HB::Spine, 40.0);

    let out = additive(&skel, &base, &lean, &reference, 0.5, None);
    let got = angle_z(out.locals[&j].rotation);
    assert!(
        (got - 30.0).abs() < 0.01,
        "10° + half of 40° is 30°, got {got}"
    );
}

#[test]
fn the_reference_pose_changes_the_answer_which_is_why_it_is_required() {
    // The same authored lean means different things depending on what it was authored FROM. Unreal and
    // Unity both make this a per-asset setting that is silent when wrong; making it an argument means
    // an additive layer cannot be applied without saying what it is additive to.
    let (skel, ch) = biped(RigConvention::Mixamo);
    let j = ch.profile.joint(HB::Spine).unwrap();
    let base = bent(&skel, &ch, HB::Spine, 0.0);
    let lean = bent(&skel, &ch, HB::Spine, 30.0);

    let from_rest = additive(
        &skel,
        &base,
        &lean,
        &bent(&skel, &ch, HB::Spine, 0.0),
        1.0,
        None,
    );
    let from_idle = additive(
        &skel,
        &base,
        &lean,
        &bent(&skel, &ch, HB::Spine, 20.0),
        1.0,
        None,
    );

    let a = angle_z(from_rest.locals[&j].rotation);
    let b = from_idle
        .locals
        .get(&j)
        .map_or(0.0, |t| angle_z(t.rotation));
    assert!(
        (a - 30.0).abs() < 0.01,
        "authored from rest, the delta is the whole 30°"
    );
    assert!(
        (b - 10.0).abs() < 0.01,
        "authored from a 20° idle, the delta is only 10°"
    );
}

#[test]
fn an_additive_layer_respects_its_mask() {
    let (skel, ch) = biped(RigConvention::Mixamo);
    let spine = ch.profile.joint(HB::Spine).unwrap();
    let base = Pose::new();
    let reference = Pose::new();

    let mut layer = bent(&skel, &ch, HB::Spine, 30.0);
    let arm = ch.profile.joint(HB::LeftUpperArm).unwrap();
    layer.set(
        arm,
        Transform {
            rotation: rot_z(45.0),
            ..skel.joints[arm].local_bind
        },
    );

    let arms = BoneMask::humanoid_subtree(&skel, &ch, HB::LeftShoulder);
    let out = additive(&skel, &base, &layer, &reference, 1.0, Some(&arms));

    assert!(out.locals.contains_key(&arm), "the arm is in the mask");
    assert!(!out.locals.contains_key(&spine), "the spine is not");
}

#[test]
fn an_additive_layer_that_cancels_the_base_leaves_the_pose_empty() {
    // Sparseness again, on the additive path: a joint that lands back on bind is removed, not written
    // with a bind-valued entry that would make two equal poses compare unequal.
    let (skel, ch) = biped(RigConvention::Mixamo);
    let base = bent(&skel, &ch, HB::Spine, 20.0);
    let reference = bent(&skel, &ch, HB::Spine, 20.0);
    let layer = bent(&skel, &ch, HB::Spine, 0.0);

    let out = additive(&skel, &base, &layer, &reference, 1.0, None);
    assert!(
        out.locals.is_empty(),
        "a −20° delta on a +20° base is bind, so the pose is empty; got {:?}",
        out.locals
    );
}

#[test]
fn every_operation_is_deterministic() {
    let (skel, ch) = biped(RigConvention::Mixamo);
    let a = bent(&skel, &ch, HB::LeftUpperArm, 12.5);
    let b = bent(&skel, &ch, HB::LeftUpperArm, 47.5);
    let mask = BoneMask::humanoid_subtree(&skel, &ch, HB::LeftShoulder);

    assert_eq!(blend(&skel, &a, &b, 0.3), blend(&skel, &a, &b, 0.3));
    assert_eq!(
        blend_masked(&skel, &a, &b, 0.3, Some(&mask)),
        blend_masked(&skel, &a, &b, 0.3, Some(&mask))
    );
    assert_eq!(
        additive(&skel, &a, &b, &Pose::new(), 0.7, None),
        additive(&skel, &a, &b, &Pose::new(), 0.7, None)
    );
    let _ = EPS;
}
