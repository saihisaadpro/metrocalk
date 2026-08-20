//! **Pose algebra** — blending, masking and additive layering over [`Pose`].
//!
//! WHY THIS DID NOT EXIST, AND WHY NOTHING WORKS WITHOUT IT. [`Pose::set`] is a `BTreeMap::insert`. That
//! is enough to *hold* a pose and nothing else: there was no way to be half-way between two poses, no
//! way to play an animation on the legs while another plays on the arms, and no way to add a lean on
//! top of a walk. Every one of those is table stakes, and each is one function here.
//!
//! THE MASK IS THE PART THAT IS BETTER THAN THE INCUMBENTS. In Unity a layer mask is an Avatar Mask
//! asset, and in the engine's own animation kernel a mask is an explicit entry **per property path** —
//! so "the left arm" means enumerating every bone in the left arm, and re-enumerating it whenever the
//! rig changes. [`BoneMask::subtree`] takes a joint and means *that joint and everything under it*,
//! resolved from the hierarchy at call time. [`BoneMask::humanoid_subtree`] goes one better: it takes a
//! [`HumanBone`], so "mask the left arm" is written once and applies to every rig, whatever that rig
//! calls its bones and however many twist bones it interposes.
//!
//! SPARSENESS IS PRESERVED THROUGHOUT. A [`Pose`] is *the joints that changed*; a joint absent from it
//! is at its bind local. Every operation here keeps that invariant — a blend between two poses that
//! both leave the tail alone leaves the tail alone, rather than writing an identical value for it and
//! quietly turning a 4-joint pose into a 200-joint one.

use std::collections::BTreeMap;

use glam::{Quat as GQuat, Vec3 as GVec3};

use crate::characterize::HumanoidCharacterization;
use crate::humanoid::HumanBone;
use crate::{Pose, Skeleton, Transform};

fn gq(q: [f32; 4]) -> GQuat {
    GQuat::from_xyzw(q[0], q[1], q[2], q[3])
}

fn gv(v: [f32; 3]) -> GVec3 {
    GVec3::from_array(v)
}

/// Which joints an operation is allowed to touch — a per-joint bitmap, built from the hierarchy rather
/// than enumerated by hand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoneMask {
    included: Vec<bool>,
}

impl BoneMask {
    /// Every joint.
    #[must_use]
    pub fn all(skeleton: &Skeleton) -> Self {
        Self {
            included: vec![true; skeleton.joints.len()],
        }
    }

    /// No joint.
    #[must_use]
    pub fn none(skeleton: &Skeleton) -> Self {
        Self {
            included: vec![false; skeleton.joints.len()],
        }
    }

    /// `root` **and every joint beneath it** — the hierarchical mask.
    ///
    /// One call replaces the enumeration Unity's Avatar Mask and the property-path masks in this
    /// engine's own animation kernel both require, and it cannot go stale: a rig that gains a finger
    /// gains it inside the hand's mask automatically, because the answer is computed from the
    /// hierarchy rather than stored beside it.
    #[must_use]
    pub fn subtree(skeleton: &Skeleton, root: usize) -> Self {
        let n = skeleton.joints.len();
        let mut included = vec![false; n];
        if root >= n {
            return Self { included };
        }
        included[root] = true;
        // Joints are topologically ordered (parent < child), so ONE forward pass is enough: by the time
        // a child is visited its parent's answer is final.
        for i in (root + 1)..n {
            if let Some(p) = skeleton.joints[i].parent {
                if included[p] {
                    included[i] = true;
                }
            }
        }
        Self { included }
    }

    /// The subtree under a **humanoid** bone — "the left arm", written once, correct on every rig.
    ///
    /// Returns [`BoneMask::none`] if this rig has no such bone, which is the honest answer: masking to a
    /// bone a character does not have should affect nothing, not everything.
    #[must_use]
    pub fn humanoid_subtree(
        skeleton: &Skeleton,
        characterization: &HumanoidCharacterization,
        bone: HumanBone,
    ) -> Self {
        characterization
            .profile
            .joint(bone)
            .map_or_else(|| Self::none(skeleton), |j| Self::subtree(skeleton, j))
    }

    /// Whether a joint is in the mask. Out-of-range indices are out.
    #[must_use]
    pub fn contains(&self, joint: usize) -> bool {
        self.included.get(joint).copied().unwrap_or(false)
    }

    /// How many joints the mask covers.
    #[must_use]
    pub fn count(&self) -> usize {
        self.included.iter().filter(|b| **b).count()
    }

    /// Everything in either mask.
    #[must_use]
    pub fn union(mut self, other: &Self) -> Self {
        for (i, v) in self.included.iter_mut().enumerate() {
            *v = *v || other.contains(i);
        }
        self
    }

    /// Everything NOT in this mask — "the body except the left arm".
    #[must_use]
    pub fn inverted(mut self) -> Self {
        for v in &mut self.included {
            *v = !*v;
        }
        self
    }
}

/// Interpolate one joint's local TRS: slerp the rotation (shortest arc), lerp translation and scale.
fn lerp_local(a: Transform, b: Transform, t: f32) -> Transform {
    Transform {
        translation: gv(a.translation).lerp(gv(b.translation), t).to_array(),
        // `slerp` on glam quaternions already takes the shortest path, so a blend never spins the long
        // way round — the artifact that makes a naive component-wise lerp unusable for rotation.
        rotation: gq(a.rotation)
            .normalize()
            .slerp(gq(b.rotation).normalize(), t)
            .to_array(),
        scale: gv(a.scale).lerp(gv(b.scale), t).to_array(),
    }
}

/// Cross-fade `a` into `b` by `t` ∈ [0, 1]. `t = 0` is `a`, `t = 1` is `b`.
///
/// Joints missing from either pose are taken at their **bind local**, so blending a sparse pose against
/// an empty one fades it toward the rest pose — which is what "blend out" means — rather than treating
/// the absent side as identity and snapping.
#[must_use]
pub fn blend(skeleton: &Skeleton, a: &Pose, b: &Pose, t: f32) -> Pose {
    blend_masked(skeleton, a, b, t, None)
}

/// [`blend`], restricted to a mask: joints outside it keep `a` untouched.
///
/// This is the whole of layered animation — "play the reload on the arms while the legs keep running"
/// is `blend_masked(base, reload, 1.0, &BoneMask::humanoid_subtree(.., LeftShoulder))`.
#[must_use]
pub fn blend_masked(
    skeleton: &Skeleton,
    a: &Pose,
    b: &Pose,
    t: f32,
    mask: Option<&BoneMask>,
) -> Pose {
    let t = t.clamp(0.0, 1.0);
    let mut out = Pose::new();

    // Only joints named by ONE OF THE TWO poses can differ from bind — everything else is bind on both
    // sides and blends to bind, i.e. stays absent. That is what keeps a blend sparse.
    let touched: std::collections::BTreeSet<usize> =
        a.locals.keys().chain(b.locals.keys()).copied().collect();

    for joint in touched {
        if joint >= skeleton.joints.len() {
            continue;
        }
        let from = a.local(joint, skeleton);
        if mask.is_some_and(|m| !m.contains(joint)) {
            // Outside the mask: `a` passes through unchanged, and only if `a` actually said something.
            if let Some(v) = a.locals.get(&joint) {
                out.set(joint, *v);
            }
            continue;
        }
        let to = b.local(joint, skeleton);
        let blended = lerp_local(from, to, t);
        // A joint that lands back on its bind local is not a change — dropping it keeps the pose sparse
        // and keeps `Pose` equality meaningful (two poses that describe the same body compare equal).
        if !near_bind(&blended, &skeleton.joints[joint].local_bind) {
            out.set(joint, blended);
        }
    }
    out
}

/// Whether a local TRS is (numerically) the joint's bind local.
fn near_bind(t: &Transform, bind: &Transform) -> bool {
    const EPS: f32 = 1e-6;
    let dt = gv(t.translation).distance_squared(gv(bind.translation));
    let ds = gv(t.scale).distance_squared(gv(bind.scale));
    // Quaternion closeness via |dot|, which treats q and -q as the same rotation.
    let dr = gq(t.rotation)
        .normalize()
        .dot(gq(bind.rotation).normalize())
        .abs();
    dt <= EPS && ds <= EPS && (1.0 - dr) <= EPS
}

/// Layer `additive` on top of `base` at `weight`, where `additive` is read **relative to `reference`**.
///
/// WHY THE REFERENCE POSE IS AN ARGUMENT AND NOT A CONVENTION. "Additive" only means anything relative
/// to the pose the additive clip was authored against — a lean authored from a T-pose and the same lean
/// authored from an idle are different deltas. Unreal and Unity both make this a per-asset setting that
/// is easy to get wrong and silent when it is; requiring it here means an additive layer cannot be
/// applied without saying what it is additive TO.
///
/// The delta is composed in each joint's **local** space (`Δ = layer · reference⁻¹`, applied as
/// `Δ · base`), so an additive lean on the spine carries the arms with it, exactly as a real lean does.
#[must_use]
pub fn additive(
    skeleton: &Skeleton,
    base: &Pose,
    layer: &Pose,
    reference: &Pose,
    weight: f32,
    mask: Option<&BoneMask>,
) -> Pose {
    let weight = weight.clamp(0.0, 1.0);
    let mut out = base.clone();

    for joint in layer.locals.keys().copied() {
        if joint >= skeleton.joints.len() || mask.is_some_and(|m| !m.contains(joint)) {
            continue;
        }
        let layer_local = layer.local(joint, skeleton);
        let ref_local = reference.local(joint, skeleton);
        let base_local = base.local(joint, skeleton);

        // The delta, scaled by weight: a rotation delta slerped from identity, a translation/scale
        // delta lerped from zero/one.
        let d_rot = (gq(layer_local.rotation).normalize()
            * gq(ref_local.rotation).normalize().inverse())
        .normalize();
        let d_rot = GQuat::IDENTITY.slerp(d_rot, weight);
        let d_trans = (gv(layer_local.translation) - gv(ref_local.translation)) * weight;
        let d_scale = GVec3::ONE.lerp(
            gv(layer_local.scale) / gv(ref_local.scale).max(GVec3::splat(1e-6)),
            weight,
        );

        let composed = Transform {
            translation: (gv(base_local.translation) + d_trans).to_array(),
            rotation: (d_rot * gq(base_local.rotation).normalize())
                .normalize()
                .to_array(),
            scale: (gv(base_local.scale) * d_scale).to_array(),
        };
        if near_bind(&composed, &skeleton.joints[joint].local_bind) {
            out.locals.remove(&joint);
        } else {
            out.set(joint, composed);
        }
    }
    out
}

/// Restrict a pose to a mask — everything outside it falls back to bind.
#[must_use]
pub fn masked(pose: &Pose, mask: &BoneMask) -> Pose {
    Pose {
        locals: pose
            .locals
            .iter()
            .filter(|(j, _)| mask.contains(**j))
            .map(|(j, t)| (*j, *t))
            .collect::<BTreeMap<_, _>>(),
    }
}
