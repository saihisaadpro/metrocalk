//! **Retargeting as a pure function** — `f(source characterization, target characterization) → Pose`.
//!
//! WHAT THIS REPLACES. In Unreal, moving one animation onto one other character costs a minimum of
//! three authored assets (an IK Rig for the source skeleton, an IK Rig for the target, and an IK
//! Retargeter for the pair) plus a hand-authored Retarget Pose to reconcile an A-pose source against a
//! T-pose target — and as of UE 5.8 the retargeter's internals are a stack of ~15 operations plus 8 IK
//! sub-operations, two of them experimental. In Unity it costs a binary Avatar per rig and silently
//! discards every bone outside the fixed humanoid enum.
//!
//! Here it costs nothing. Both characterizations already exist as a property of their own skeleton
//! ([`crate::characterize`] infers them at import), so this is a function call with no assets, no
//! per-pair artifact, and no user step at all.
//!
//! THE MATH, AND WHY IT IS REST-POSE INDEPENDENT. This is VRM 1.0's normalized-space formulation. For
//! each humanoid bone the source's **world-space rotation delta from its own rest pose** is
//!
//! ```text
//!     D = W_pose · W_rest⁻¹
//! ```
//!
//! and that delta — not the source's local rotation — is what transfers. The target then reconstructs
//! its own world rotation from *its* rest pose, `W'_pose = D · W'_rest`, and only at the very end is
//! that converted back into a local TRS against the target's own parent chain.
//!
//! Two consequences fall straight out of that shape, and they are the two loudest retargeting
//! complaints in both engines:
//!
//!   * **An A-pose source drives a T-pose target correctly, with no retarget pose to author.** Each
//!     side is measured against its OWN rest, so the difference between the two rests cancels. This is
//!     the setting Unreal calls "the single most consequential in the workflow" and leaves to hand.
//!   * **A correction cannot compound down the hierarchy.** Every bone's world orientation is built
//!     from its own rest and its own delta, independently — so a shoulder that is off by 15° moves the
//!     shoulder by 15° and does not touch the fingers. In Unreal, editing the retarget pose propagates
//!     rotation down the chain and canonically destroys the hands.
//!
//! WHAT IS DELIBERATELY NOT TRANSFERRED. **Bone translations, for every bone except the hips.** Limb
//! lengths belong to the target character, not to the animation; copying the source's translations is
//! what stretches a short character's arms to a tall one's proportions. The hips are the exception
//! because hip translation IS the motion — and it is scaled by the ratio of the two rigs' hip heights,
//! so a child-sized character walking takes child-sized steps instead of sliding.
//!
//! WHAT IS LEFT ALONE. Every joint outside the humanoid set — twist bones, hair, capes, skirts, tails,
//! prop sockets — keeps its own bind local, untouched. They are not dropped (Unity), not clamped, and
//! not renamed.

use std::collections::BTreeMap;

use glam::{Mat4 as GMat4, Quat as GQuat, Vec3 as GVec3};

use crate::characterize::HumanoidCharacterization;
use crate::humanoid::HumanBone;
use crate::{Pose, Skeleton, Transform};

/// What a retarget actually did — never-silent, so "the character T-posed" can never be the whole
/// story the user gets. (That symptom has at least four unrelated causes in Unreal and no message
/// attached to any of them.)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RetargetReport {
    /// Humanoid bones present in BOTH rigs and therefore driven.
    pub driven: Vec<HumanBone>,
    /// Bones the source animates that the target has no slot for — motion that had nowhere to go.
    pub unmatched_source: Vec<HumanBone>,
    /// Bones the target has that the source could not drive — they hold their bind pose.
    pub unmatched_target: Vec<HumanBone>,
    /// Target joints outside the humanoid set, left exactly as authored.
    pub preserved_extras: usize,
    /// The hip-height ratio applied to root translation (`target_hip_height / source_hip_height`).
    pub hip_scale: f32,
}

impl RetargetReport {
    /// A one-line summary for the editor's status row.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} bones driven · {} target bones held at bind · {} non-humanoid joints preserved · \
             root motion scaled ×{:.3}",
            self.driven.len(),
            self.unmatched_target.len(),
            self.preserved_extras,
            self.hip_scale
        )
    }
}

/// Why a retarget could not be performed at all. Both arms name the offending side, because "it did not
/// work" is not an actionable sentence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetargetError {
    /// A rig is missing one or more VRM-required bones.
    NotRetargetable {
        which: &'static str,
        missing: Vec<HumanBone>,
    },
}

impl RetargetError {
    /// The message + the fix, in this repository's diagnostic style.
    #[must_use]
    pub fn describe(&self) -> (String, String) {
        match self {
            Self::NotRetargetable { which, missing } => {
                let names: Vec<String> = missing.iter().map(|b| b.label()).collect();
                (
                    format!(
                        "the {which} rig is missing {} required bone(s): {}",
                        missing.len(),
                        names.join(", ")
                    ),
                    format!(
                        "Open the {which} character's rig panel and assign the missing bone(s) from \
                         its skeleton tree. Retargeting needs all 15 required humanoid bones on both \
                         sides."
                    ),
                )
            }
        }
    }
}

fn gq(q: [f32; 4]) -> GQuat {
    GQuat::from_xyzw(q[0], q[1], q[2], q[3])
}

/// The rotation part of a column-major world matrix, orthonormalized (scale removed) — the only part of
/// a bone's world transform that retargets.
fn world_rotation(m: [[f32; 4]; 4]) -> GQuat {
    let (_s, r, _t) = GMat4::from_cols_array_2d(&m).to_scale_rotation_translation();
    r.normalize()
}

fn world_translation(m: [[f32; 4]; 4]) -> GVec3 {
    GVec3::new(m[3][0], m[3][1], m[3][2])
}

/// The rig's hip height in its own rest pose — the scalar that makes root motion proportional rather
/// than absolute. Measured from the lowest foot to the hips, not from the world origin, so a rig
/// authored with its feet above `y = 0` is not silently mis-scaled.
fn hip_height(skeleton: &Skeleton, ch: &HumanoidCharacterization, rest: &[[[f32; 4]; 4]]) -> f32 {
    let Some(hips) = ch.profile.joint(HumanBone::Hips) else {
        return 1.0;
    };
    let hips_y = rest[hips][3][1];
    let foot_y = [HumanBone::LeftFoot, HumanBone::RightFoot]
        .into_iter()
        .filter_map(|b| ch.profile.joint(b))
        .map(|j| rest[j][3][1])
        .fold(f32::INFINITY, f32::min);
    let _ = skeleton;
    if foot_y.is_finite() {
        (hips_y - foot_y).abs().max(1e-6)
    } else {
        hips_y.abs().max(1e-6)
    }
}

/// Retarget one pose from a source rig onto a target rig.
///
/// Pure and deterministic: same inputs, same [`Pose`], every time — no cached state, no per-pair asset,
/// nothing to keep in sync. Returns the target's pose plus a [`RetargetReport`] saying exactly what was
/// driven and what was not.
///
/// # Errors
/// [`RetargetError::NotRetargetable`] if either rig is missing a VRM-required bone, naming which side
/// and which bones — a rig that cannot be retargeted says so before producing a wrong answer.
pub fn retarget_pose(
    source: &Skeleton,
    source_ch: &HumanoidCharacterization,
    source_pose: &Pose,
    target: &Skeleton,
    target_ch: &HumanoidCharacterization,
) -> Result<(Pose, RetargetReport), RetargetError> {
    if !source_ch.is_retargetable() {
        return Err(RetargetError::NotRetargetable {
            which: "source",
            missing: source_ch.profile.missing_required(),
        });
    }
    if !target_ch.is_retargetable() {
        return Err(RetargetError::NotRetargetable {
            which: "target",
            missing: target_ch.profile.missing_required(),
        });
    }

    let src_rest = source.globals(&Pose::new());
    let src_posed = source.globals(source_pose);
    let dst_rest = target.globals(&Pose::new());

    // The world-space delta per TARGET joint. Bones the source does not have simply never appear here,
    // and their target joints therefore hold their bind local — the explicit, reported outcome rather
    // than an implicit zero rotation that would snap a limb to the rest pose.
    let mut delta: BTreeMap<usize, GQuat> = BTreeMap::new();
    let mut report = RetargetReport {
        hip_scale: 1.0,
        ..RetargetReport::default()
    };

    for bone in HumanBone::ALL {
        let s = source_ch.profile.joint(bone);
        let d = target_ch.profile.joint(bone);
        match (s, d) {
            (Some(s), Some(d)) => {
                let w_pose = world_rotation(src_posed[s]);
                let w_rest = world_rotation(src_rest[s]);
                delta.insert(d, (w_pose * w_rest.inverse()).normalize());
                report.driven.push(bone);
            }
            (Some(_), None) => report.unmatched_source.push(bone),
            (None, Some(_)) => report.unmatched_target.push(bone),
            (None, None) => {}
        }
    }

    // Walk the target in topological order (parent < child is the Skeleton invariant), rebuilding each
    // joint's world rotation and converting it back to a local TRS against the parent we just computed.
    let n = target.joints.len();
    let mut out_world: Vec<GQuat> = Vec::with_capacity(n);
    let mut pose = Pose::new();

    for (i, joint) in target.joints.iter().enumerate() {
        let parent_world = joint.parent.map_or(GQuat::IDENTITY, |p| out_world[p]);
        let bind_local = joint.local_bind;

        let local_rotation = if let Some(&d) = delta.get(&i) {
            // W' = D · W'_rest, then localize: L' = parent_world⁻¹ · W'.
            let w_new = (d * world_rotation(dst_rest[i])).normalize();
            let local = (parent_world.inverse() * w_new).normalize();
            pose.set(
                i,
                Transform {
                    translation: bind_local.translation,
                    rotation: local.to_array(),
                    scale: bind_local.scale,
                },
            );
            local
        } else {
            // Untouched: a twist bone, a cape, a hair chain, a prop socket — or a humanoid bone the
            // source could not drive. It keeps the target's own bind local, and is NOT written into the
            // pose, so `Pose` stays sparse (a pose is the set of joints that CHANGED).
            gq(bind_local.rotation).normalize()
        };
        out_world.push((parent_world * local_rotation).normalize());
    }

    // ── root motion: the one translation that transfers, scaled by the height ratio ──
    if let (Some(s_hips), Some(d_hips)) = (
        source_ch.profile.joint(HumanBone::Hips),
        target_ch.profile.joint(HumanBone::Hips),
    ) {
        let s_h = hip_height(source, source_ch, &src_rest);
        let d_h = hip_height(target, target_ch, &dst_rest);
        let scale = if s_h > 1e-6 { d_h / s_h } else { 1.0 };
        report.hip_scale = scale;

        // The source's hip displacement from its own rest, in world space, scaled to the target's size.
        let moved =
            (world_translation(src_posed[s_hips]) - world_translation(src_rest[s_hips])) * scale;

        // Express it in the target hips' PARENT space, so a rig with a root/reference bone above the
        // hips is offset correctly rather than by a world-space amount its parent then re-rotates.
        let parent_rot = target.joints[d_hips]
            .parent
            .map_or(GQuat::IDENTITY, |p| world_rotation(dst_rest[p]));
        let local_offset = parent_rot.inverse() * moved;

        let bind_local = target.joints[d_hips].local_bind;
        let rotation = pose
            .locals
            .get(&d_hips)
            .map_or(bind_local.rotation, |t| t.rotation);
        pose.set(
            d_hips,
            Transform {
                translation: [
                    bind_local.translation[0] + local_offset.x,
                    bind_local.translation[1] + local_offset.y,
                    bind_local.translation[2] + local_offset.z,
                ],
                rotation,
                scale: bind_local.scale,
            },
        );
    }

    report.preserved_extras = target_ch.profile.extra_joints(n).len();
    Ok((pose, report))
}
