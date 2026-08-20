//! The claim: **a clip keyed by canonical humanoid bone plays on a rig it was never authored for**,
//! with no retarget step, no per-pair asset and no user action.
//!
//! Unreal binds an animation to a Skeleton asset, so a clip from the wrong skeleton needs an IK Rig for
//! each side plus an IK Retargeter before it plays at all. Unity binds to an Avatar and silently
//! T-poses a Generic-authored clip that was marked Humanoid. The tests below are what it looks like for
//! that question not to arise: one sequence, two unrelated rigs, both posed correctly.

// Math-heavy like the crates it bridges; the test oracle's short names are the canonical ones.
#![allow(clippy::many_single_char_names, clippy::cast_precision_loss)]

use metrocalk_animation::{
    AnimValue, Binding, Interpolation, Keyframe, Sequence, Tick, TimeBase, Track, ValueKind,
};
use metrocalk_character::{
    bind_sequence, bone_path, BindDiagnosticCode, BoneKey, Channel, SKELETON_COMPONENT,
};
use metrocalk_skeleton::characterize::{characterize, HumanoidCharacterization};
use metrocalk_skeleton::humanoid::{HumanBone as HB, RigConvention};
use metrocalk_skeleton::{Joint, Pose, Skeleton, Transform};

const EPS: f32 = 1e-4;
const TARGET: &str = "hero";

// ── rigs ─────────────────────────────────────────────────────────────────────────────────────────

fn tf(t: [f32; 3]) -> Transform {
    Transform {
        translation: t,
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0; 3],
    }
}

/// A minimal biped in one of two naming conventions, with deliberately DIFFERENT proportions and a
/// non-humanoid tail — so "it played on both" cannot be an accident of them being the same rig.
fn biped(convention: RigConvention, scale: f32) -> (Skeleton, HumanoidCharacterization) {
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
            local_bind: tf([t[0] * scale, t[1] * scale, t[2] * scale]),
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

// ── clips ────────────────────────────────────────────────────────────────────────────────────────

fn rot_z(deg: f64) -> AnimValue {
    let h = deg.to_radians() * 0.5;
    AnimValue::Quaternion([0.0, 0.0, h.sin(), h.cos()])
}

/// A one-track clip driving `bone`'s rotation from 0° to `deg` over one second.
fn wave_clip(bone: &BoneKey, deg: f64) -> Sequence {
    let base = TimeBase::GAME_60;
    let end = base.from_seconds(1.0).expect("one second");
    let mut seq = Sequence::new("wave", "Wave", end);
    seq.time_base = base;
    seq.tracks.push(Track {
        id: "arm".into(),
        name: "arm".into(),
        binding: Binding {
            path: bone_path(TARGET, bone, Channel::Rotation),
            value_kind: ValueKind::Quaternion,
        },
        interpolation: Interpolation::Linear,
        keyframes: vec![
            Keyframe {
                id: "k0".into(),
                tick: Tick(0),
                value: rot_z(0.0),
                in_tangent: None,
                out_tangent: None,
            },
            Keyframe {
                id: "k1".into(),
                tick: end,
                value: rot_z(deg),
                in_tangent: None,
                out_tangent: None,
            },
        ],
        enabled: true,
    });
    seq
}

fn quat_angle_z(q: [f32; 4]) -> f32 {
    // The clip only rotates about Z, so `q = [0, 0, sin(θ/2), cos(θ/2)]` and the signed angle is
    // recoverable from (z, w) alone: `atan2(z, w)` is the HALF angle, so θ = 2·atan2(z, w). (The first
    // version of this helper wrote `2.0 * … / 2.0`, which cancels — it reported every rotation at half
    // strength and failed the one test that checks an actual number. The oracle was wrong, not the
    // sampler.)
    let (z, w) = (q[2], q[3]);
    2.0 * z.atan2(w).to_degrees()
}

// ── the claim ────────────────────────────────────────────────────────────────────────────────────

#[test]
fn one_humanoid_keyed_clip_plays_on_two_unrelated_rigs_with_no_retarget_step() {
    // ONE clip. It names `leftUpperArm` — the human body, not a skeleton.
    let seq = wave_clip(&BoneKey::Humanoid(HB::LeftUpperArm), 40.0)
        .compile()
        .expect("compiles");

    // Two rigs that share no bone name and are not even the same size.
    let (mixamo, mixamo_ch) = biped(RigConvention::Mixamo, 1.0);
    let (unreal, unreal_ch) = biped(RigConvention::UnrealMannequin, 1.7);

    for (label, skel, ch) in [
        ("mixamo", &mixamo, &mixamo_ch),
        ("unreal", &unreal, &unreal_ch),
    ] {
        let binding = bind_sequence(&seq, skel, ch);
        assert!(
            binding.diagnostics.is_empty(),
            "{label}: nothing should need saying, got {:?}",
            binding.diagnostics
        );
        assert_eq!(
            binding.bound_count(),
            1,
            "{label}: the one channel must bind"
        );

        let end = seq.duration;
        let pose = binding.sample(&seq, end, skel);
        let joint = ch.profile.joint(HB::LeftUpperArm).expect("left upper arm");
        let local = pose.locals.get(&joint).expect("the arm must be posed");
        assert!(
            (quat_angle_z(local.rotation) - 40.0).abs() < 0.01,
            "{label}: expected 40° on the upper arm, got {}",
            quat_angle_z(local.rotation)
        );
    }
}

#[test]
fn a_partially_keyed_clip_leaves_the_bones_own_offset_alone() {
    // THE COLLAPSE THIS PREVENTS. A clip that keys only ROTATION must start from the joint's BIND
    // local. Composing from identity instead zeroes the translation and every limb snaps onto its
    // parent's origin — a character folded into a point, which is a spectacular and very common bug.
    let seq = wave_clip(&BoneKey::Humanoid(HB::LeftLowerArm), 25.0)
        .compile()
        .expect("compiles");
    let (skel, ch) = biped(RigConvention::Mixamo, 1.0);
    let binding = bind_sequence(&seq, &skel, &ch);

    let joint = ch.profile.joint(HB::LeftLowerArm).unwrap();
    let pose = binding.sample(&seq, seq.duration, &skel);
    let local = pose.locals.get(&joint).expect("posed");

    let bind = skel.joints[joint].local_bind;
    for k in 0..3 {
        assert!(
            (local.translation[k] - bind.translation[k]).abs() < EPS,
            "the bone's authored offset must survive a rotation-only clip: {:?} vs {:?}",
            local.translation,
            bind.translation
        );
        assert!((local.scale[k] - bind.scale[k]).abs() < EPS);
    }
}

#[test]
fn sampling_writes_only_the_joints_the_clip_actually_drives() {
    // A `Pose` is the set of joints that CHANGED — the invariant every blend, mask and additive
    // operation in `pose_ops` depends on. A sampler that wrote all 20 joints would silently turn every
    // downstream layer into a full-body override.
    let seq = wave_clip(&BoneKey::Humanoid(HB::LeftUpperArm), 30.0)
        .compile()
        .expect("compiles");
    let (skel, ch) = biped(RigConvention::Mixamo, 1.0);
    let binding = bind_sequence(&seq, &skel, &ch);

    let pose = binding.sample(&seq, seq.duration, &skel);
    assert_eq!(pose.locals.len(), 1, "exactly one joint is driven");
    assert_eq!(
        pose.locals.keys().next(),
        ch.profile.joint(HB::LeftUpperArm).as_ref()
    );

    // And at tick 0 the clip's value IS the bind rotation, so nothing changed and the pose is empty.
    let at_rest = binding.sample(&seq, Tick(0), &skel);
    assert!(
        at_rest.locals.is_empty(),
        "a pose identical to bind must be empty, got {:?}",
        at_rest.locals
    );
}

#[test]
fn a_clip_authored_for_another_rig_says_so_by_name_instead_of_t_posing() {
    // THE UNITY FAILURE, INVERTED. A Generic-authored clip marked Humanoid silently T-poses with no
    // error anywhere. Here a rig-specific clip pointed at the wrong rig produces a sentence naming the
    // joint it wanted and what to do — at BIND time, before a frame plays.
    let seq = wave_clip(&BoneKey::Joint("mixamorig:LeftArm".into()), 40.0)
        .compile()
        .expect("compiles");
    let (unreal, unreal_ch) = biped(RigConvention::UnrealMannequin, 1.0);

    let binding = bind_sequence(&seq, &unreal, &unreal_ch);
    assert!(binding.is_empty(), "nothing should have bound");
    assert_eq!(binding.diagnostics.len(), 1);
    let d = &binding.diagnostics[0];
    assert_eq!(d.code, BindDiagnosticCode::UnknownJoint);
    assert!(d.message.contains("mixamorig:LeftArm"), "{}", d.message);
    assert!(
        d.remediation.contains("canonical humanoid bones"),
        "the remediation must name the way out: {}",
        d.remediation
    );
}

#[test]
fn a_clip_driving_a_bone_this_character_lacks_names_the_bone_and_keeps_playing_the_rest() {
    // Partial success is the honest outcome: a clip that waves a tail on a character with no tail
    // should still wave the arm. Unity's answer is to drop the channel silently; ours is to drop it
    // loudly and carry on.
    let mut seq = wave_clip(&BoneKey::Humanoid(HB::LeftUpperArm), 40.0);
    // Add a second track for a bone the minimal test rig genuinely does not have.
    let mut toes = seq.tracks[0].clone();
    toes.id = "toes".into();
    toes.binding.path = bone_path(TARGET, &BoneKey::Humanoid(HB::LeftToes), Channel::Rotation);
    seq.tracks.push(toes);
    let seq = seq.compile().expect("compiles");

    let (skel, ch) = biped(RigConvention::Mixamo, 1.0);
    let binding = bind_sequence(&seq, &skel, &ch);

    assert_eq!(binding.bound_count(), 1, "the arm still plays");
    assert_eq!(binding.diagnostics.len(), 1);
    let d = &binding.diagnostics[0];
    assert_eq!(d.code, BindDiagnosticCode::UnmappedHumanoidBone);
    assert!(d.message.contains("leftToes"), "{}", d.message);
    assert!(d.remediation.contains("rig panel"), "{}", d.remediation);
}

#[test]
fn channels_that_are_not_bones_are_left_entirely_alone() {
    // One sequence may animate a character's bones AND a material parameter. This crate must claim the
    // first and not so much as mention the second — a bridge that reported the material as an error
    // would make every real clip noisy.
    let mut seq = wave_clip(&BoneKey::Humanoid(HB::LeftUpperArm), 40.0);
    let mut material = seq.tracks[0].clone();
    material.id = "tint".into();
    material.binding.path =
        metrocalk_animation::PropertyPath::new(TARGET, "Material", "tint").with_subpath(["rgb"]);
    material.binding.value_kind = ValueKind::Vec3;
    material.keyframes = vec![
        Keyframe {
            id: "m0".into(),
            tick: Tick(0),
            value: AnimValue::Vec3([0.0, 0.0, 0.0]),
            in_tangent: None,
            out_tangent: None,
        },
        Keyframe {
            id: "m1".into(),
            tick: seq.duration,
            value: AnimValue::Vec3([1.0, 1.0, 1.0]),
            in_tangent: None,
            out_tangent: None,
        },
    ];
    seq.tracks.push(material);
    let seq = seq.compile().expect("compiles");

    let (skel, ch) = biped(RigConvention::Mixamo, 1.0);
    let binding = bind_sequence(&seq, &skel, &ch);

    assert_eq!(binding.bound_count(), 1, "only the bone channel is ours");
    assert!(
        binding.diagnostics.is_empty(),
        "a non-skeletal path is not an error: {:?}",
        binding.diagnostics
    );
}

#[test]
fn a_rotation_channel_fed_a_vec3_is_refused_at_bind_time() {
    let mut seq = wave_clip(&BoneKey::Humanoid(HB::LeftUpperArm), 40.0);
    seq.tracks[0].binding.value_kind = ValueKind::Vec3;
    seq.tracks[0].keyframes = vec![
        Keyframe {
            id: "k0".into(),
            tick: Tick(0),
            value: AnimValue::Vec3([0.0, 0.0, 0.0]),
            in_tangent: None,
            out_tangent: None,
        },
        Keyframe {
            id: "k1".into(),
            tick: seq.duration,
            value: AnimValue::Vec3([1.0, 0.0, 0.0]),
            in_tangent: None,
            out_tangent: None,
        },
    ];
    let seq = seq.compile().expect("compiles");

    let (skel, ch) = biped(RigConvention::Mixamo, 1.0);
    let binding = bind_sequence(&seq, &skel, &ch);
    assert!(binding.is_empty());
    assert_eq!(
        binding.diagnostics[0].code,
        BindDiagnosticCode::WrongValueKind
    );
}

#[test]
fn binding_and_sampling_are_both_deterministic() {
    let seq = wave_clip(&BoneKey::Humanoid(HB::LeftUpperArm), 33.0)
        .compile()
        .expect("compiles");
    let (skel, ch) = biped(RigConvention::Mixamo, 1.0);

    let a = bind_sequence(&seq, &skel, &ch);
    let b = bind_sequence(&seq, &skel, &ch);
    assert_eq!(a, b, "the same clip and rig must bind identically");

    let mid = Tick(seq.duration.0 / 2);
    assert_eq!(
        a.sample(&seq, mid, &skel),
        b.sample(&seq, mid, &skel),
        "the same binding and tick must sample identically"
    );
}

#[test]
fn the_bone_path_spelling_round_trips() {
    // Authoring code writes a path with `bone_path`; the binder reads it back apart. If those two ever
    // disagreed the clip would bind to nothing and the character would stand still — silently.
    for key in [
        BoneKey::Humanoid(HB::LeftUpperArm),
        BoneKey::Joint("mixamorig:LeftArm".into()),
    ] {
        for channel in [Channel::Translation, Channel::Rotation, Channel::Scale] {
            let path = bone_path(TARGET, &key, channel);
            assert_eq!(path.component, SKELETON_COMPONENT);
            assert_eq!(BoneKey::parse(&path.property), key);
            assert_eq!(
                Channel::parse(&path.subpath[0]),
                Some(channel),
                "{path:?} does not read back as {channel:?}"
            );
        }
    }
}

#[test]
fn an_unposed_skeleton_and_a_tick_zero_sample_agree() {
    // A sanity anchor for the whole bridge: at the clip's first key the character stands exactly in its
    // bind pose, so FK over the sampled pose equals FK over an empty one.
    let seq = wave_clip(&BoneKey::Humanoid(HB::LeftUpperArm), 40.0)
        .compile()
        .expect("compiles");
    let (skel, ch) = biped(RigConvention::Mixamo, 1.0);
    let binding = bind_sequence(&seq, &skel, &ch);

    let sampled = binding.sample(&seq, Tick(0), &skel);
    assert_eq!(skel.globals(&sampled), skel.globals(&Pose::new()));
    let _ = ch;
}
