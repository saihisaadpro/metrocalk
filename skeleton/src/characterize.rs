//! **Infer** a [`HumanoidProfile`] from a rig, instead of asking the user to author one.
//!
//! THE PRODUCT CLAIM THIS FILE IS. In Unreal, characterizing a skeleton is an authoring task: you open
//! the IK Rig editor and assign retarget chains bone by bone, then nominate a retarget root, and you do
//! it once per skeleton before any pair of them can talk. In Unity it is the Avatar configuration
//! screen — nine steps, and when it fails the only indicator is a red cross with no sentence attached.
//! Both are asking the human for something the data already says: `mixamorig:LeftForeArm` is not
//! ambiguous to a person, and it should not be ambiguous to the engine.
//!
//! So the engine reads it. Import a character and the characterization is already there — recognized,
//! scored, and explained. The user's job shrinks from "map 15-55 bones" to "glance at what we found,
//! and correct it if we got one wrong". That is the entire ease-of-use difference, and it is available
//! *because* [`crate::humanoid`] made the characterization a property of one skeleton.
//!
//! HOW IT DECIDES, IN ORDER, AND WHY THAT ORDER.
//!
//!   1. **Recognize the convention, then apply its exact table.** Conventions collide on the tokens
//!      that matter — Mixamo's `LeftLeg` is the LOWER leg while Rigify's `shin.L` is — so a single
//!      fuzzy table has to guess exactly where guessing is most expensive. Scoring whole conventions
//!      first removes the guess and, as a bonus, produces a sentence worth reading ("this is a Mixamo
//!      rig") rather than a percentage nobody can act on.
//!   2. **Fall back to topology** when no convention scores. A rig with bones named `bone_001`… still
//!      has a shape: one joint whose subtree contains five long chains is a hips; the two chains that
//!      end lowest are legs; the two that branch off highest are arms. This is weaker evidence and is
//!      reported as such — it never silently claims the confidence of a name match.
//!
//! WHAT IT REFUSES TO DO. It does not fuzzy-match, and it does not fill a required bone by guessing.
//! An unmatched bone costs the user one glance at a panel; a *wrongly* matched one produces a character
//! that moves subtly incorrectly forever with nothing on screen suggesting why. Every decision this
//! module makes carries its [`Evidence`], so the panel can show the user what the engine saw.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::humanoid::{normalize, HumanBone, HumanoidProfile, RigConvention};
use crate::Skeleton;

/// Why one bone was assigned the joint it was assigned. Carried per bone so the rig panel can explain
/// itself; this repository does not ship a confidence number with no story behind it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Evidence {
    /// The joint's name matched this convention's table entry exactly (after normalization).
    NameMatch {
        convention: RigConvention,
        matched: String,
    },
    /// No name matched; the joint was identified by the skeleton's shape. Owned rather than
    /// `&'static str` because a characterization is a DOCUMENT — it round-trips through the transport
    /// to the rig panel, and a borrowed reason cannot be deserialized back.
    Topology { reason: String },
    /// A human overrode the inference. Never produced by [`characterize`] — only by
    /// [`HumanoidCharacterization::override_bone`] — and it always wins.
    Manual,
}

impl Evidence {
    /// A short sentence for the rig panel's evidence column.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::NameMatch {
                convention,
                matched,
            } => format!("named `{matched}` ({})", convention.label()),
            Self::Topology { reason } => format!("by shape — {reason}"),
            Self::Manual => "set by hand".to_string(),
        }
    }

    /// Whether this evidence came from a name (the strong kind).
    #[must_use]
    pub fn is_name_match(&self) -> bool {
        matches!(self, Self::NameMatch { .. })
    }
}

/// Something the characterizer wants to tell the user, in this repository's never-silent style: what is
/// wrong, where, and **what to do about it**. A diagnostic without a remediation is a complaint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RigDiagnostic {
    pub code: RigDiagnosticCode,
    pub message: String,
    pub remediation: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RigDiagnosticCode {
    /// A bone VRM 1.0 requires could not be identified. The rig cannot be retargeted.
    MissingRequiredBone,
    /// No naming convention was recognized; the mapping came from the skeleton's shape alone.
    UnrecognizedConvention,
    /// Two joints claimed the same humanoid slot; the first in topological order won.
    DuplicateSlot,
    /// The rig has joints outside the humanoid set. Purely informational — they are kept.
    ExtraBonesKept,
    /// The skeleton has no named joints at all, so only topology was available.
    NoNames,
}

impl RigDiagnosticCode {
    /// Whether this diagnostic blocks retargeting.
    #[must_use]
    pub fn is_blocking(self) -> bool {
        matches!(self, Self::MissingRequiredBone)
    }
}

/// The full result of characterizing one rig: the profile, the per-bone evidence, and everything the
/// engine wants to say about it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HumanoidCharacterization {
    pub profile: HumanoidProfile,
    /// Per-bone evidence, parallel to `profile.bones`.
    pub evidence: BTreeMap<HumanBone, Evidence>,
    pub diagnostics: Vec<RigDiagnostic>,
    /// How many joints the source skeleton had — so `extra_joints` can be computed without it.
    pub joint_count: usize,
}

impl HumanoidCharacterization {
    /// Whether this rig can be retargeted (every required bone present).
    #[must_use]
    pub fn is_retargetable(&self) -> bool {
        self.profile.is_complete()
    }

    /// The fraction of the 55 humanoid slots that were filled — a coverage number, deliberately NOT
    /// called "confidence". It says how much of the humanoid this rig expresses (a rig with no fingers
    /// scores lower and is still perfectly retargetable), not how sure we are.
    #[must_use]
    pub fn coverage(&self) -> f32 {
        self.profile.bones.len() as f32 / HumanBone::ALL.len() as f32
    }

    /// Correct one bone by hand. The override is recorded as [`Evidence::Manual`] and outranks anything
    /// inference decided — the one-screen correction that replaces Unreal's per-pair chain authoring.
    ///
    /// Passing `None` unsets the slot (the honest way to say "this rig has no such bone").
    pub fn override_bone(&mut self, bone: HumanBone, joint: Option<usize>) {
        if let Some(j) = joint {
            self.profile.bones.insert(bone, j);
            self.evidence.insert(bone, Evidence::Manual);
        } else {
            self.profile.bones.remove(&bone);
            self.evidence.remove(&bone);
        }
        self.diagnostics = diagnose(&self.profile, &self.evidence, self.joint_count);
    }
}

/// Score one convention against a skeleton's names: how many of its table entries match a joint, and
/// the `bone → joint` assignments that produced the score.
fn score_convention(
    convention: RigConvention,
    by_stem: &BTreeMap<String, usize>,
) -> (usize, BTreeMap<HumanBone, (usize, String)>) {
    let mut found = BTreeMap::new();
    for (bone, stem) in convention.table() {
        if let Some(&joint) = by_stem.get(stem) {
            found.insert(bone, (joint, stem.to_string()));
        }
    }
    // Required bones count double: a convention that finds 30 fingers but no hips has not recognized
    // the rig, and a rig with no fingers at all is still unambiguously a Mixamo rig.
    let weight = found
        .keys()
        .map(|b| if b.is_required() { 2 } else { 1 })
        .sum();
    (weight, found)
}

/// Infer a humanoid characterization from a rig — the entry point.
///
/// Deterministic and total: every skeleton produces a characterization, and one that identified nothing
/// says so in `diagnostics` rather than returning an error. That is deliberate — "this is not a
/// humanoid" is a normal, displayable answer about a crate or a car, not a failure.
#[must_use]
pub fn characterize(skeleton: &Skeleton) -> HumanoidCharacterization {
    let joint_count = skeleton.joints.len();

    // stem → FIRST joint with that stem (topological order, so a parent wins over a child that
    // normalizes the same way). Recorded once; every convention is scored against the same index.
    let mut by_stem: BTreeMap<String, usize> = BTreeMap::new();
    let mut named = 0usize;
    for (i, joint) in skeleton.joints.iter().enumerate() {
        if joint.name.trim().is_empty() {
            continue;
        }
        named += 1;
        by_stem.entry(normalize(&joint.name)).or_insert(i);
    }

    let mut profile = HumanoidProfile::default();
    let mut evidence: BTreeMap<HumanBone, Evidence> = BTreeMap::new();

    if named > 0 {
        // Pick the single best-scoring convention. Ties break by `RigConvention::ALL` order, which is
        // stable, so the same rig always characterizes the same way (the determinism this repository
        // gates for everywhere else).
        let best = RigConvention::ALL
            .into_iter()
            .map(|c| {
                let (score, found) = score_convention(c, &by_stem);
                (score, c, found)
            })
            .max_by_key(|(score, _, _)| *score);

        if let Some((score, convention, found)) = best {
            if score > 0 {
                profile.convention = Some(convention);
                for (bone, (joint, matched)) in found {
                    profile.bones.insert(bone, joint);
                    evidence.insert(
                        bone,
                        Evidence::NameMatch {
                            convention,
                            matched,
                        },
                    );
                }
            }
        }
    }

    // Topology fills only what names could not. It never overwrites a name match: a name is direct
    // testimony from the authoring tool, and shape is an inference about it.
    if !profile.is_complete() {
        infer_from_topology(skeleton, &mut profile, &mut evidence);
    }

    let diagnostics = diagnose(&profile, &evidence, joint_count);
    HumanoidCharacterization {
        profile,
        evidence,
        diagnostics,
        joint_count,
    }
}

/// The shape-based fallback, for a rig whose names say nothing (`bone_001`, `Object_47`, or a rig
/// exported with names stripped).
///
/// WHAT SHAPE ACTUALLY TELLS YOU. A biped's skeleton has a distinctive silhouette: one joint (the hips)
/// whose subtree contains every limb; two long chains descending from near it (the legs); two chains
/// branching much further up (the arms); one chain ending at a single leaf above everything (the
/// head). That is enough to place the 15 required bones and nothing else, which is exactly what this
/// does — it does not pretend to find fingers by shape.
fn infer_from_topology(
    skeleton: &Skeleton,
    profile: &mut HumanoidProfile,
    evidence: &mut BTreeMap<HumanBone, Evidence>,
) {
    let n = skeleton.joints.len();
    if n < 6 {
        return;
    }

    // children[i] = the joints whose parent is i.
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut roots: Vec<usize> = Vec::new();
    for (i, j) in skeleton.joints.iter().enumerate() {
        match j.parent {
            Some(p) if p < n => children[p].push(i),
            _ => roots.push(i),
        }
    }

    // The hips = the deepest joint from which THREE OR MORE independent chains still descend (two legs
    // + the spine). Walking down from the root past single-child links skips whatever pelvis-parent or
    // "root motion" bone the exporter added, which is the same bone Unreal makes you nominate by hand
    // as the "retarget root" and warns is "almost never the actual skeleton root".
    let Some(&root) = roots.first() else {
        return;
    };
    let mut hips = root;
    loop {
        let kids = &children[hips];
        if kids.len() >= 3 {
            break;
        }
        if kids.len() == 1 {
            hips = kids[0];
            continue;
        }
        break;
    }

    let bind = skeleton.globals(&crate::Pose::new());
    let height_of = |i: usize| bind[i][3][1];

    // A chain's tip = follow first-children to a leaf; its length = joints traversed.
    let tip_of = |mut i: usize| -> (usize, usize) {
        let mut len = 1;
        while let Some(&c) = children[i].first() {
            i = c;
            len += 1;
        }
        (i, len)
    };

    let mut limbs: Vec<(usize, usize, f32)> = children[hips]
        .iter()
        .map(|&c| {
            let (tip, len) = tip_of(c);
            (c, len, height_of(tip))
        })
        .collect();
    if limbs.len() < 3 {
        return;
    }
    // The two chains whose tips end LOWEST are the legs; the highest is the spine.
    limbs.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));

    let mut set = |bone: HumanBone, joint: usize, reason: &'static str| {
        if let std::collections::btree_map::Entry::Vacant(slot) = profile.bones.entry(bone) {
            slot.insert(joint);
            evidence.insert(
                bone,
                Evidence::Topology {
                    reason: reason.to_string(),
                },
            );
        }
    };

    set(HumanBone::Hips, hips, "the joint every limb descends from");

    // Left/right by the sign of the tip's X — the only geometric fact that distinguishes them. A rig
    // mirrored about X is the standing convention; a rig that is not gets this pair swapped, which is
    // why the panel lets the user swap them back in one click.
    let leg_a = limbs[0].0;
    let leg_b = limbs[1].0;
    let (left_leg, right_leg) = if bind[tip_of(leg_a).0][3][0] <= bind[tip_of(leg_b).0][3][0] {
        (leg_a, leg_b)
    } else {
        (leg_b, leg_a)
    };
    for (upper, lower, foot, chain) in [
        (
            HumanBone::LeftUpperLeg,
            HumanBone::LeftLowerLeg,
            HumanBone::LeftFoot,
            left_leg,
        ),
        (
            HumanBone::RightUpperLeg,
            HumanBone::RightLowerLeg,
            HumanBone::RightFoot,
            right_leg,
        ),
    ] {
        set(upper, chain, "a chain from the hips ending lowest");
        if let Some(&knee) = children[chain].first() {
            set(lower, knee, "the joint below the upper leg");
            if let Some(&ankle) = children[knee].first() {
                set(foot, ankle, "the joint below the lower leg");
            }
        }
    }

    // The spine chain is the remaining limb; the head is its highest leaf.
    let spine = limbs[limbs.len() - 1].0;
    set(
        HumanBone::Spine,
        spine,
        "the chain from the hips rising highest",
    );
    set(
        HumanBone::Head,
        highest_in_subtree(spine, &children, &height_of),
        "the highest joint above the spine",
    );
}

/// The highest (largest world Y) joint anywhere under `root` — the head of a spine chain, found without
/// assuming the chain is straight or that the head is its last joint (a rig with jaw, eye or hair
/// children hanging off the head has neither property).
fn highest_in_subtree(
    root: usize,
    children: &[Vec<usize>],
    height_of: &impl Fn(usize) -> f32,
) -> usize {
    let mut highest = root;
    let mut stack = vec![root];
    let mut seen = BTreeSet::new();
    while let Some(i) = stack.pop() {
        if !seen.insert(i) {
            continue;
        }
        if height_of(i) > height_of(highest) {
            highest = i;
        }
        stack.extend(children[i].iter().copied());
    }
    highest
}

/// Everything the engine wants to say about a characterization — computed from the profile alone, so a
/// hand-corrected profile re-diagnoses identically to an inferred one.
fn diagnose(
    profile: &HumanoidProfile,
    evidence: &BTreeMap<HumanBone, Evidence>,
    joint_count: usize,
) -> Vec<RigDiagnostic> {
    let mut out = Vec::new();

    let missing = profile.missing_required();
    if !missing.is_empty() {
        let names: Vec<String> = missing.iter().map(|b| b.label()).collect();
        out.push(RigDiagnostic {
            code: RigDiagnosticCode::MissingRequiredBone,
            message: format!(
                "{} required bone(s) could not be identified: {}",
                missing.len(),
                names.join(", ")
            ),
            remediation:
                "Assign each one in the rig panel — pick the joint from the skeleton tree. Animation \
                 can be retargeted onto this character as soon as all 15 required bones are set."
                    .to_string(),
        });
    }

    if profile.convention.is_none() {
        if profile.bones.is_empty() {
            out.push(RigDiagnostic {
                code: RigDiagnosticCode::NoNames,
                message:
                    "this skeleton's joints have no usable names, and its shape did not resolve \
                          to a biped"
                        .to_string(),
                remediation:
                    "Re-export the character with bone names kept, or map the 15 required \
                              bones by hand in the rig panel."
                        .to_string(),
            });
        } else {
            out.push(RigDiagnostic {
                code: RigDiagnosticCode::UnrecognizedConvention,
                message: "no known naming convention matched, so bones were identified from the \
                          skeleton's shape alone"
                    .to_string(),
                remediation:
                    "Check the mapping in the rig panel before retargeting — shape is weaker \
                              evidence than a name, and a mirrored rig can swap left for right."
                        .to_string(),
            });
        }
    }

    // Two slots pointing at one joint is a real ambiguity and always worth saying out loud.
    let mut seen: BTreeMap<usize, HumanBone> = BTreeMap::new();
    for (&bone, &joint) in &profile.bones {
        if let Some(&first) = seen.get(&joint) {
            out.push(RigDiagnostic {
                code: RigDiagnosticCode::DuplicateSlot,
                message: format!(
                    "joint {joint} fills both {} and {}",
                    first.label(),
                    bone.label()
                ),
                remediation: format!(
                    "Clear one of them in the rig panel. If this rig genuinely has no separate {}, \
                     leaving it unset is the honest answer.",
                    bone.label()
                ),
            });
        } else {
            seen.insert(joint, bone);
        }
    }

    let extra = profile.extra_joints(joint_count).len();
    if extra > 0 {
        out.push(RigDiagnostic {
            code: RigDiagnosticCode::ExtraBonesKept,
            message: format!(
                "{extra} joint(s) are outside the humanoid set (twist bones, hair, cape, props)"
            ),
            remediation: "Nothing to do — they are kept, animated and skinned as authored. \
                          Retargeting leaves them exactly as the character's own rig poses them."
                .to_string(),
        });
    }

    let _ = evidence;
    out
}
