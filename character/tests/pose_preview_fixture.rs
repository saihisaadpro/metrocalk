//! Generates the **pose-preview fixture** the editor photographs — the visual proof that one clip,
//! keyed to the human body rather than to a skeleton, plays on rigs it was never authored for.
//!
//! WHY A FIXTURE AND NOT A MOCK. Same reason as `skeleton/tests/rig_contract.rs`: a preview drawn from
//! hand-written coordinates would keep painting a convincing walk cycle long after the sampler stopped
//! producing one. Every number below comes out of `bind_sequence` → `sample` → `Skeleton::globals`, and
//! this test fails if the committed JSON and the live computation ever disagree.
//!
//! WHAT THE PREVIEW IS FOR, BEYOND EVIDENCE. In Unreal and Unity the only way to find out whether a
//! retarget worked is to press play and look at the character — which is why "it T-posed" is the
//! universal bug report. A still preview of the resulting pose, beside the source it came from, answers
//! the question before playback exists.

#![allow(clippy::cast_possible_truncation)]

use std::path::PathBuf;

use metrocalk_animation::{
    AnimValue, Binding, Interpolation, KeyId, Keyframe, Sequence, Tick, TimeBase, Track, TrackId,
    ValueKind,
};
use metrocalk_character::{bind_sequence, bone_path, BoneKey, Channel};
use metrocalk_skeleton::characterize::{characterize, HumanoidCharacterization};
use metrocalk_skeleton::humanoid::{HumanBone as HB, RigConvention};
use metrocalk_skeleton::retarget::retarget_pose;
use metrocalk_skeleton::{Joint, Pose, Skeleton, Transform};

const TARGET: &str = "hero";

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("editor")
        .join("src")
        .join("panels")
        .join("__fixtures__")
        .join("pose-preview.json")
}

fn tf(t: [f32; 3], rot: [f32; 4]) -> Transform {
    Transform {
        translation: t,
        rotation: rot,
        scale: [1.0; 3],
    }
}

fn rot_z(deg: f32) -> [f32; 4] {
    let h = deg.to_radians() * 0.5;
    [0.0, 0.0, h.sin(), h.cos()]
}

const IDENT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// A biped whose bones lie in the XY plane, so a preview is an honest orthographic front view rather
/// than a projection that could hide an error on the axis it drops.
///
/// * `droop_deg` — the rest pose's arm droop: `0` is a T-pose, `-50` an A-pose.
/// * `scale` — proportions, so "the same clip on a taller character" is visibly true.
fn biped(
    convention: RigConvention,
    droop_deg: f32,
    scale: f32,
) -> (Skeleton, HumanoidCharacterization) {
    let spec: Vec<(HB, Option<usize>, [f32; 3])> = vec![
        (HB::Hips, None, [0.0, 1.0, 0.0]),
        (HB::Spine, Some(0), [0.0, 0.15, 0.0]),
        (HB::Chest, Some(1), [0.0, 0.15, 0.0]),
        (HB::Neck, Some(2), [0.0, 0.12, 0.0]),
        (HB::Head, Some(3), [0.0, 0.14, 0.0]),
        (HB::LeftShoulder, Some(2), [0.06, 0.10, 0.0]),
        (HB::LeftUpperArm, Some(5), [0.10, 0.0, 0.0]),
        (HB::LeftLowerArm, Some(6), [0.26, 0.0, 0.0]),
        (HB::LeftHand, Some(7), [0.24, 0.0, 0.0]),
        (HB::RightShoulder, Some(2), [-0.06, 0.10, 0.0]),
        (HB::RightUpperArm, Some(9), [-0.10, 0.0, 0.0]),
        (HB::RightLowerArm, Some(10), [-0.26, 0.0, 0.0]),
        (HB::RightHand, Some(11), [-0.24, 0.0, 0.0]),
        (HB::LeftUpperLeg, Some(0), [0.09, -0.06, 0.0]),
        (HB::LeftLowerLeg, Some(13), [0.0, -0.42, 0.0]),
        (HB::LeftFoot, Some(14), [0.0, -0.42, 0.0]),
        (HB::RightUpperLeg, Some(0), [-0.09, -0.06, 0.0]),
        (HB::RightLowerLeg, Some(16), [0.0, -0.42, 0.0]),
        (HB::RightFoot, Some(17), [0.0, -0.42, 0.0]),
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

    let joints: Vec<Joint> = spec
        .iter()
        .map(|(b, parent, t)| {
            let rot = match b {
                HB::LeftShoulder => rot_z(droop_deg),
                HB::RightShoulder => rot_z(-droop_deg),
                _ => IDENT,
            };
            Joint {
                name: name(*b),
                parent: *parent,
                local_bind: tf([t[0] * scale, t[1] * scale, t[2] * scale], rot),
                inverse_bind: [[0.0; 4]; 4],
            }
        })
        .collect();

    let mut skel = Skeleton { joints };
    skel.recompute_inverse_binds();
    let ch = characterize(&skel);
    (skel, ch)
}

/// The clip: raise both upper arms and bend both elbows — keyed to **humanoid bones**, so it belongs to
/// no rig at all.
fn raise_arms() -> Sequence {
    let base = TimeBase::GAME_60;
    let end = base.from_seconds(1.0).expect("one second");
    let mut seq = Sequence::new("raise", "Raise arms", end);
    seq.time_base = base;

    for (i, (bone, deg)) in [
        (HB::LeftUpperArm, 55.0_f64),
        (HB::RightUpperArm, -55.0),
        (HB::LeftLowerArm, 40.0),
        (HB::RightLowerArm, -40.0),
    ]
    .into_iter()
    .enumerate()
    {
        let q = |d: f64| {
            let h = d.to_radians() * 0.5;
            AnimValue::Quaternion([0.0, 0.0, h.sin(), h.cos()])
        };
        seq.tracks.push(Track {
            id: TrackId::new(format!("t{i}")),
            name: bone.as_str().into(),
            binding: Binding {
                path: bone_path(TARGET, &BoneKey::Humanoid(bone), Channel::Rotation),
                value_kind: ValueKind::Quaternion,
            },
            interpolation: Interpolation::Linear,
            keyframes: vec![
                Keyframe {
                    id: KeyId::new(format!("k{i}a")),
                    tick: Tick(0),
                    value: q(0.0),
                    in_tangent: None,
                    out_tangent: None,
                },
                Keyframe {
                    id: KeyId::new(format!("k{i}b")),
                    tick: end,
                    value: q(deg),
                    in_tangent: None,
                    out_tangent: None,
                },
            ],
            enabled: true,
        });
    }
    seq
}

/// One drawable figure: every bone as a segment from its parent's origin to its own, in world XY,
/// plus **which side of the body it is on**.
///
/// THE SIDE IS COMPUTED HERE, FROM THE CHARACTERIZATION, AND THAT IS THE WHOLE POINT. The first
/// version of the preview inferred it in TypeScript with a regex over the joint name — and the capture
/// showed the bug immediately: the pattern matched Unreal's `upperarm_l` and could not match Mixamo's
/// `LeftArm`, so one rig was tinted and the other was not, under a caption promising both. Guessing a
/// bone's side from its spelling is precisely the problem `metrocalk_skeleton::humanoid` exists to
/// solve; re-solving it downstream, worse, in another language, was the error. `HumanBone::side()`
/// already knows, so it answers.
fn figure(
    caption: &str,
    detail: &str,
    skeleton: &Skeleton,
    characterization: &HumanoidCharacterization,
    pose: &Pose,
) -> serde_json::Value {
    // joint index → the humanoid slot it fills, so a side can be looked up per bone.
    let side_of: std::collections::BTreeMap<usize, &'static str> = characterization
        .profile
        .bones
        .iter()
        .filter_map(|(bone, joint)| {
            bone.side().map(|s| {
                (
                    *joint,
                    match s {
                        metrocalk_skeleton::humanoid::Side::Left => "left",
                        metrocalk_skeleton::humanoid::Side::Right => "right",
                    },
                )
            })
        })
        .collect();

    let globals = skeleton.globals(pose);
    let segments: Vec<serde_json::Value> = skeleton
        .joints
        .iter()
        .enumerate()
        .filter_map(|(i, joint)| {
            let p = joint.parent?;
            let a = globals[p];
            let b = globals[i];
            Some(serde_json::json!({
                "name": joint.name,
                // A bone belongs to the side of the joint it MOVES (its own end), not its parent's —
                // the left clavicle hangs off the centre chest and is still a left bone.
                "side": side_of.get(&i).copied(),
                "x1": round(a[3][0]), "y1": round(a[3][1]),
                "x2": round(b[3][0]), "y2": round(b[3][1]),
            }))
        })
        .collect();
    serde_json::json!({ "caption": caption, "detail": detail, "segments": segments })
}

/// Three decimals is far more than a 300 px preview can show, and it keeps the fixture diff readable
/// when something genuinely changes.
fn round(v: f32) -> f64 {
    (f64::from(v) * 1000.0).round() / 1000.0
}

fn preview_document() -> serde_json::Value {
    let seq = raise_arms().compile().expect("the clip compiles");
    let end = seq.duration;

    // Two characters that share no bone name, are not the same height, and do not even rest in the same
    // pose: a short T-posed Mixamo rig and a tall A-posed Unreal Mannequin.
    let (mixamo, mixamo_ch) = biped(RigConvention::Mixamo, 0.0, 1.0);
    let (unreal, unreal_ch) = biped(RigConvention::UnrealMannequin, -50.0, 1.45);

    let mixamo_binding = bind_sequence(&seq, &mixamo, &mixamo_ch);
    let unreal_binding = bind_sequence(&seq, &unreal, &unreal_ch);
    let mixamo_posed = mixamo_binding.sample(&seq, end, &mixamo);
    let unreal_posed = unreal_binding.sample(&seq, end, &unreal);

    // And the retargeter, moving the Mixamo character's finished pose onto the Unreal rig — the second
    // route to the same place, for a clip that is NOT humanoid-keyed.
    let (retargeted, report) =
        retarget_pose(&mixamo, &mixamo_ch, &mixamo_posed, &unreal, &unreal_ch)
            .expect("both rigs are retargetable");

    serde_json::json!({
        "clip": {
            "name": "Raise arms",
            "channels": seq.evaluate(Tick(0)).bindings.len(),
            "keyedBy": "humanoid",
        },
        "figures": [
            figure(
                "Mixamo · rest",
                "the character as authored — a T-pose",
                &mixamo,
                &mixamo_ch,
                &Pose::new(),
            ),
            figure(
                "Mixamo · the clip",
                "bound with 0 diagnostics",
                &mixamo,
                &mixamo_ch,
                &mixamo_posed,
            ),
            figure(
                "Unreal Mannequin · rest",
                "a different rig: 1.45x taller, A-posed, no bone name in common",
                &unreal,
                &unreal_ch,
                &Pose::new(),
            ),
            figure(
                "Unreal Mannequin · the SAME clip",
                "no retarget asset, no mapping, no user step",
                &unreal,
                &unreal_ch,
                &unreal_posed,
            ),
            figure(
                "Unreal Mannequin · retargeted",
                &report.summary(),
                &unreal,
                &unreal_ch,
                &retargeted,
            ),
        ],
        "boundChannels": {
            "mixamo": mixamo_binding.bound_count(),
            "unreal": unreal_binding.bound_count(),
        },
        "diagnostics": mixamo_binding.diagnostics.len() + unreal_binding.diagnostics.len(),
        // WHETHER THE TWO ROUTES LANDED IN THE SAME PLACE — **measured, and shown**.
        //
        // Figures 4 and 5 are pixel-identical, and until this field existed the panel said nothing
        // about that: a reader saw one picture drawn twice under two different captions and had no way
        // to tell the point of the whole preview from a render that had silently failed. It IS the
        // point — a clip addressed to the human body needs no retarget onto a characterized rig, so
        // the two routes coincide — but a claim the user has to already believe in order to read the
        // evidence for it is not evidence.
        //
        // Computed, never asserted true. If a change to either route ever separates them this goes
        // false, the panel's sentence disappears, and the scene's `text_present` reds — which is the
        // only arrangement where the sentence on screen and the geometry beneath it cannot disagree.
        "routesAgree": routes_agree(&unreal_posed, &retargeted, &unreal),
    })
}

/// Do the two routes put every joint in the same place? (Within 1e-4 — these are f32 rotations
/// composed in different orders, so exact equality would be a coincidence, not a proof.)
fn routes_agree(direct: &Pose, retargeted: &Pose, rig: &Skeleton) -> bool {
    let a = rig.globals(direct);
    let b = rig.globals(retargeted);
    a.len() == b.len()
        && a.iter().zip(b.iter()).all(|(x, y)| {
            x.iter()
                .flatten()
                .zip(y.iter().flatten())
                .all(|(p, q)| (p - q).abs() <= 1e-4)
        })
}

/// The environment variable that lets the fixture be rewritten. **Nothing sets it in CI.**
///
/// The first version of this test rewrote the fixture the instant it disagreed and then panicked,
/// which makes it green on every second run whatever changed — run, fail, run, pass, commit. That is a
/// golden test blessing its own expectation, and it was observed doing exactly that (one run FAILED,
/// the immediately-repeated run PASSED with nothing edited between them): `<test_and_ci_discipline>` 2
/// and 4. The default path now writes nothing, and a blessing run still fails, because a run that
/// rewrote what it was checking against has verified nothing.
const BLESS: &str = "MTK_BLESS_FIXTURES";

/// The first line where two documents disagree. This fixture is ~760 lines of joint positions; "they
/// differ" is not something a reader can act on, and the field that moved is.
fn first_difference(committed: &str, produced: &str) -> String {
    let mut left = committed.lines();
    let mut right = produced.lines();
    let mut line = 1usize;
    loop {
        match (left.next(), right.next()) {
            (None, None) => return "the documents are identical".into(),
            (a, b) if a == b => line += 1,
            (a, b) => {
                return format!(
                    "line {line}:\n    committed: {}\n    produced:  {}",
                    a.unwrap_or("<end of file>").trim(),
                    b.unwrap_or("<end of file>").trim()
                )
            }
        }
    }
}

#[test]
fn the_committed_pose_preview_is_what_the_sampler_actually_produces() {
    let produced = serde_json::to_string_pretty(&preview_document()).expect("serialize");
    let path = fixture_path();
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    if committed.trim() == produced.trim() {
        return;
    }
    let bless = std::env::var_os(BLESS).is_some();
    if bless {
        std::fs::create_dir_all(path.parent().unwrap()).ok();
        std::fs::write(&path, format!("{produced}\n")).expect("write the fixture");
    }
    panic!(
        "the pose-preview fixture no longer matches what the sampler produces ({}):\n  {}\n\n{}",
        path.display(),
        first_difference(committed.trim(), produced.trim()),
        if bless {
            format!("{BLESS} was set, so it HAS been rewritten. Review the diff (the editor draws this \
                 document) and commit it. This run still fails on purpose.")
        } else {
            format!(
                "Nothing was written. If the new output is the intended one, re-run with \
                 {BLESS}=1 to regenerate, then review and commit the diff."
            )
        }
    );
}

#[test]
fn the_preview_shows_a_clip_that_actually_moved_both_characters() {
    // A preview whose "posed" figures quietly equalled their rest figures would photograph a pair of
    // identical stick figures under a caption promising an animation — the caption-nothing-evaluates
    // failure. So the fixture's own content is asserted, not just its stability.
    let doc = preview_document();
    let figures = doc["figures"].as_array().expect("figures");
    assert_eq!(figures.len(), 5);

    for (rest, posed) in [(0, 1), (2, 3)] {
        assert_ne!(
            figures[rest]["segments"], figures[posed]["segments"],
            "figure {posed} must differ from its rest pose — otherwise the clip did nothing"
        );
    }
    // Both rigs bound all four channels, and nothing needed saying about either.
    assert_eq!(doc["boundChannels"]["mixamo"], 4);
    assert_eq!(doc["boundChannels"]["unreal"], 4);
    assert_eq!(doc["diagnostics"], 0);

    // THE TWO ROUTES COINCIDE, AND THAT IS THE CLAIM — so it is asserted, not left to be noticed.
    //
    // Figures 4 and 5 are the same pose reached two ways: bound straight from the humanoid-keyed clip,
    // and carried across by the retargeter from the Mixamo character's finished pose. They come out
    // pixel-identical, which is exactly what "a clip addressed to the body needs no retarget onto a
    // characterized rig" MEANS. Nothing asserted it, so the panel was showing one picture twice under
    // two captions and a reader could not tell the thesis from a render that had failed.
    //
    // Two ways for this to be a lie, and both are covered: the routes silently diverging (this
    // assertion), and BOTH routes collapsing to the rest pose, which would also make them agree — the
    // `(2, 3)` differ-from-rest pair above is what stops that, and it has to stay for this to mean
    // anything.
    assert_eq!(
        figures[3]["segments"], figures[4]["segments"],
        "the humanoid-bound pose and the retargeted pose must land in the same place — that IS the \
         claim the preview exists to show. If this ever separates, the panel's sentence has to go \
         with it, not be kept as decoration."
    );
    assert_eq!(
        doc["routesAgree"], true,
        "`routesAgree` is what the panel reads to decide whether to say so; it must agree with the \
         geometry asserted directly above, or the sentence on screen and the picture under it are \
         two statements of one fact that can drift"
    );

    // EVERY figure must carry left/right sides, on BOTH conventions. The first preview inferred the
    // side from the joint name in TypeScript, which matched `upperarm_l` and not `LeftArm` — so the
    // Mixamo figures were drawn entirely untinted beneath a caption promising otherwise. Only a human
    // reading the PNG caught it; this is the assertion that would have.
    for f in figures {
        let sides = f["segments"]
            .as_array()
            .expect("segments")
            .iter()
            .filter(|s| !s["side"].is_null())
            .count();
        assert!(
            sides >= 8,
            "{} has only {sides} sided bones — the left/right tint would be missing",
            f["caption"]
        );
    }
}
