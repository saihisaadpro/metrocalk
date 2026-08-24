//! The rig panel's fixture is **the real serialized output**, and this test is what keeps it that way.
//!
//! WHY THIS TEST EXISTS. `editor/scripts/shots/scenes.tsx` carries a scar worth reading: a harness that
//! photographed a payload the real core cannot produce is "the C6 failure (green against the mock,
//! wrong against `/core`) reached through a screenshot instead of a test". A rig panel driven by a
//! hand-written JSON mock would be exactly that — it would keep painting a beautiful characterization
//! long after the Rust side stopped producing that shape.
//!
//! So the fixture the panel renders is committed at
//! `editor/src/panels/__fixtures__/rig-characterization.json`, and this test asserts that
//! [`characterize`] still produces it **byte for byte**. Change the Rust type and this test goes red
//! with the regeneration command in the failure message; it cannot go quietly stale.

use std::path::PathBuf;

use metrocalk_skeleton::characterize::characterize;
use metrocalk_skeleton::humanoid::{HumanBone as HB, RigConvention};
use metrocalk_skeleton::{Joint, Skeleton, Transform};

/// The fixture, relative to the workspace root.
fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("editor")
        .join("src")
        .join("panels")
        .join("__fixtures__")
        .join(name)
}

fn tf(t: [f32; 3]) -> Transform {
    Transform {
        translation: t,
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0; 3],
    }
}

/// The rig the panel is photographed against: a **Mixamo character with a tail**.
///
/// Chosen deliberately. Mixamo is the most common free source of characters, its `LeftUpLeg`/`LeftLeg`
/// spelling is the collision that defeats naive matching, and the tail is the bone Unity would silently
/// drop — so one fixture exercises the recognition, the trap and the preservation claim at once.
fn mixamo_character_with_a_tail() -> Skeleton {
    // (name, parent, local translation) — a minimal but complete humanoid, plus one non-humanoid chain.
    let spec: Vec<(&str, Option<usize>, [f32; 3])> = vec![
        ("mixamorig:Hips", None, [0.0, 1.0, 0.0]),
        ("mixamorig:Spine", Some(0), [0.0, 0.15, 0.0]),
        ("mixamorig:Spine1", Some(1), [0.0, 0.15, 0.0]),
        ("mixamorig:Neck", Some(2), [0.0, 0.15, 0.0]),
        ("mixamorig:Head", Some(3), [0.0, 0.10, 0.0]),
        ("mixamorig:LeftShoulder", Some(2), [0.05, 0.10, 0.0]),
        ("mixamorig:LeftArm", Some(5), [0.10, 0.0, 0.0]),
        ("mixamorig:LeftForeArm", Some(6), [0.25, 0.0, 0.0]),
        ("mixamorig:LeftHand", Some(7), [0.25, 0.0, 0.0]),
        ("mixamorig:RightShoulder", Some(2), [-0.05, 0.10, 0.0]),
        ("mixamorig:RightArm", Some(9), [-0.10, 0.0, 0.0]),
        ("mixamorig:RightForeArm", Some(10), [-0.25, 0.0, 0.0]),
        ("mixamorig:RightHand", Some(11), [-0.25, 0.0, 0.0]),
        ("mixamorig:LeftUpLeg", Some(0), [0.08, -0.05, 0.0]),
        ("mixamorig:LeftLeg", Some(13), [0.0, -0.40, 0.0]),
        ("mixamorig:LeftFoot", Some(14), [0.0, -0.40, 0.0]),
        ("mixamorig:LeftToeBase", Some(15), [0.0, -0.05, 0.10]),
        ("mixamorig:RightUpLeg", Some(0), [-0.08, -0.05, 0.0]),
        ("mixamorig:RightLeg", Some(17), [0.0, -0.40, 0.0]),
        ("mixamorig:RightFoot", Some(18), [0.0, -0.40, 0.0]),
        ("mixamorig:RightToeBase", Some(19), [0.0, -0.05, 0.10]),
        // The bones Unity's humanoid enum has no slot for, and therefore discards.
        ("Tail_01", Some(0), [0.0, 0.0, -0.12]),
        ("Tail_02", Some(21), [0.0, 0.0, -0.12]),
    ];

    let mut skel = Skeleton {
        joints: spec
            .into_iter()
            .map(|(name, parent, t)| Joint {
                name: name.to_string(),
                parent,
                local_bind: tf(t),
                inverse_bind: [[0.0; 4]; 4],
            })
            .collect(),
    };
    skel.recompute_inverse_binds();
    skel
}

/// The document the editor consumes. Kept small and explicit — the panel needs the mapping, the
/// evidence, the diagnostics and the joint names to render a row per bone, and nothing else.
fn panel_document(skeleton: &Skeleton) -> serde_json::Value {
    let ch = characterize(skeleton);

    serde_json::json!({
        "characterization": ch,
        "jointNames": skeleton.joints.iter().map(|j| j.name.clone()).collect::<Vec<_>>(),
        "retargetable": ch.is_retargetable(),
        "coverage": ch.coverage(),
        "extraJoints": ch.profile.extra_joints(ch.joint_count),
    })
}

/// The SECOND state the panel must be able to show: a character that cannot be animated yet.
///
/// A prop-style rig — the torso of the Mixamo character with every limb stripped — so the panel is
/// photographed in its blocking state carrying the real message and the real remediation rather than a
/// hand-written approximation of them. The failure state is the one that most needs to be legible, and
/// it is the one Unity renders as a red cross with no sentence attached.
fn a_character_that_cannot_be_animated_yet() -> Skeleton {
    let mut skel = mixamo_character_with_a_tail();
    skel.joints.truncate(5); // hips, spine, chest, neck, head — no limbs at all
    skel.recompute_inverse_binds();
    skel
}

/// Every fixture the rig panel renders: `(file name, the rig it is produced from)`.
fn fixtures() -> Vec<(&'static str, Skeleton)> {
    vec![
        ("rig-characterization.json", mixamo_character_with_a_tail()),
        (
            "rig-not-retargetable.json",
            a_character_that_cannot_be_animated_yet(),
        ),
    ]
}

/// The environment variable that lets a fixture be rewritten. **Nothing sets it in CI**, and that is
/// the whole point.
///
/// WHY THIS EXISTS AT ALL. The first version of this test regenerated the fixture the moment it
/// disagreed and then failed — which reads as strict, and is the opposite. A golden test that writes
/// its own expectation is green on the second run, always, whatever changed: run it once, watch it
/// fail, run it again, watch it pass, commit. The drift it was written to catch becomes a two-command
/// ritual, and the diff sails through review as "the fixture was regenerated". `<test_and_ci_discipline>`
/// 2 (no vacuous passes) and 4 (a flake is a failure) both land on it, and it was observed exactly that
/// way: a first run FAILED and an immediately-repeated run PASSED with nothing edited in between.
///
/// So the default path never writes. Blessing is a deliberate act with a name on it, and even then the
/// test still fails, because a run that rewrote its own expectation has not verified anything.
const BLESS: &str = "MTK_BLESS_FIXTURES";

/// The first line where two documents disagree, as `line N: expected … / produced …`.
///
/// A byte-count mismatch is not a diff. These documents are 60–180 lines of pretty-printed JSON and
/// the failure a reader needs to act on is one field, so the message names it.
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
fn the_committed_rig_fixtures_are_what_this_crate_actually_produces() {
    let bless = std::env::var_os(BLESS).is_some();
    let mut stale = Vec::new();
    for (name, skeleton) in fixtures() {
        let produced = serde_json::to_string_pretty(&panel_document(&skeleton)).expect("serialize");
        let path = fixture_path(name);
        // A MISSING fixture is a failure, not an empty string to be filled in. The panel renders a
        // committed document; if it is not there, what the scene photographs does not exist.
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        if committed.trim() != produced.trim() {
            if bless {
                std::fs::create_dir_all(path.parent().unwrap()).ok();
                std::fs::write(&path, format!("{produced}\n")).expect("write the fixture");
            }
            stale.push(format!(
                "{}\n  {}",
                path.display(),
                first_difference(committed.trim(), produced.trim())
            ));
        }
    }
    assert!(
        stale.is_empty(),
        "the rig panel's fixture(s) no longer match what `characterize` produces:\n  {}\n\n{}",
        stale.join("\n  "),
        if bless {
            format!(
                "{BLESS} was set, so they HAVE been rewritten. Review the diff (the panel renders \
                 these documents) and commit it. This run still fails: a run that rewrote its own \
                 expectation has verified nothing."
            )
        } else {
            format!(
                "Nothing was written. If the new output is the intended one, re-run with \
                 {BLESS}=1 to regenerate, then review and commit the diff."
            )
        }
    );
}

#[test]
fn the_blocking_fixture_really_is_blocking_and_says_what_to_do() {
    // A "failure state" fixture that quietly became a success state would photograph a green panel
    // under a caption promising a red one — the caption-nothing-evaluates failure this repo gates for.
    let ch = characterize(&a_character_that_cannot_be_animated_yet());
    assert!(!ch.is_retargetable());
    let blocking = ch
        .diagnostics
        .iter()
        .find(|d| d.code.is_blocking())
        .expect("the panel's failure state needs a blocking diagnostic to show");
    assert!(
        blocking.message.contains("Left Upper Arm"),
        "the missing bones must be NAMED: {}",
        blocking.message
    );
    assert!(
        blocking.remediation.contains("rig panel"),
        "the remediation must point somewhere: {}",
        blocking.remediation
    );
}

#[test]
fn the_fixture_actually_exercises_the_claims_the_panel_shows() {
    // A fixture that drifted into something trivial would keep this contract green while proving
    // nothing — so the fixture's own content is asserted, not just its stability.
    let skeleton = mixamo_character_with_a_tail();
    let ch = characterize(&skeleton);

    assert!(
        ch.is_retargetable(),
        "the panel's headline state must be the happy one"
    );
    assert_eq!(
        ch.profile.convention.map(RigConvention::label),
        Some("Mixamo"),
        "the fixture must exercise convention recognition"
    );
    // The trap: `LeftLeg` is the LOWER leg.
    assert_eq!(
        skeleton.joints[ch.profile.joint(HB::LeftLowerLeg).unwrap()].name,
        "mixamorig:LeftLeg"
    );
    assert_eq!(
        skeleton.joints[ch.profile.joint(HB::LeftUpperLeg).unwrap()].name,
        "mixamorig:LeftUpLeg"
    );
    // The preservation claim: the tail survives and is counted.
    assert_eq!(
        ch.profile.extra_joints(ch.joint_count).len(),
        2,
        "both tail joints must be reported as kept"
    );
}

#[test]
fn every_serialized_bone_name_round_trips_through_its_vrm_spelling() {
    // The panel keys its rows by the serialized bone name. If serde's `camelCase` rename ever stopped
    // agreeing with `HumanBone::as_str`, the panel would render rows it cannot label — silently.
    for bone in HB::ALL {
        let wire = serde_json::to_string(&bone).expect("serialize");
        let unquoted = wire.trim_matches('"');
        assert_eq!(
            unquoted,
            bone.as_str(),
            "serde and `as_str` must spell {bone:?} the same way"
        );
        let back: HB = serde_json::from_str(&wire).expect("deserialize");
        assert_eq!(back, bone);
    }
}
