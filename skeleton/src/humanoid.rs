//! The **canonical humanoid vocabulary** — the one thing neither Unreal nor Unity has, and the reason
//! retargeting in both of them is a per-pair authoring chore instead of a pure function.
//!
//! WHY THIS FILE EXISTS AT ALL. A rig arrives as a bag of strings: `mixamorig:LeftForeArm`,
//! `lowerarm_l`, `forearm.L`, `Bip01 L Forearm`, `CC_Base_L_Forearm`, `lForearmBend`. Every one of
//! those is the same bone, and nothing in glTF, FBX or USD says so — glTF 2.0 has no humanoid concept
//! whatsoever. So each engine invents a private one and makes the USER restate it:
//!
//!   * **Unreal** wants an *IK Rig per skeleton* (retarget chains + a retarget root) and then an *IK
//!     Retargeter per pair*. Three authored assets before one frame of animation moves, and the
//!     Retarget Pose that reconciles an A-pose source against a T-pose target is authored by hand, in
//!     a space where a shoulder correction propagates down the hierarchy and wrecks the fingers.
//!   * **Unity** has a fixed humanoid enum and *silently drops every bone outside it* — tails, capes,
//!     skirts, hair chains, prop sockets — then clamps what remains into muscle space.
//!
//! Both mistakes come from the same place: the characterization is modelled as an artifact ABOUT a
//! pair of skeletons rather than as a PROPERTY OF ONE skeleton. Make it a property, and retargeting
//! stops being an asset and becomes `f(source_characterization, target_characterization)` — which is
//! this repository's standing shape (the terrain lesson: a recipe in the document, everything else
//! derived by a pure function).
//!
//! WHY *THIS* LIST, AND NOT ONE WE INVENTED. These are the 55 human bones of **VRM 1.0**, which is
//! also — bone for bone — Godot 4's `SkeletonProfileHumanoid` minus its synthetic `Root`. Picking the
//! published list rather than a private one is the whole point: a vocabulary only pays off if other
//! tools already speak it. VRM additionally specifies the exact normalized-space quaternion math that
//! [`crate::retarget`] implements, so adopting the list buys the algorithm with it.
//!
//! WHAT THIS DELIBERATELY DOES NOT DO. It does not claim a rig has *only* these bones. A joint that
//! maps to no [`HumanBone`] is not dropped, not clamped, and not renamed — it keeps its own name and
//! its own local TRS, and retargeting leaves it exactly alone. That is the direct answer to Unity's
//! "where did the character's tail go".

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Which side of the body a bone is on. `None` for the centre column (hips, spine, head…).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Side {
    Left,
    Right,
}

/// One slot of the canonical humanoid skeleton — the **VRM 1.0** human-bone list (55 bones), which is
/// also Godot's `SkeletonProfileHumanoid` without its synthetic `Root`.
///
/// The ordering of the variants is the ordering of the spec, and [`HumanBone::ALL`] preserves it, so a
/// characterization printed for a human reads top-down (hips → spine → head, then legs, arms, fingers)
/// rather than alphabetically.
/// Serialized as its VRM 1.0 name (`leftUpperArm`), so the document the editor reads is the same
/// vocabulary any VRM-aware tool reads — not a private integer that means nothing outside this build.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HumanBone {
    // ── torso + head (the centre column) ──
    Hips,
    Spine,
    Chest,
    UpperChest,
    Neck,
    Head,
    LeftEye,
    RightEye,
    Jaw,
    // ── legs ──
    LeftUpperLeg,
    LeftLowerLeg,
    LeftFoot,
    LeftToes,
    RightUpperLeg,
    RightLowerLeg,
    RightFoot,
    RightToes,
    // ── arms ──
    LeftShoulder,
    LeftUpperArm,
    LeftLowerArm,
    LeftHand,
    RightShoulder,
    RightUpperArm,
    RightLowerArm,
    RightHand,
    // ── left hand ──
    LeftThumbMetacarpal,
    LeftThumbProximal,
    LeftThumbDistal,
    LeftIndexProximal,
    LeftIndexIntermediate,
    LeftIndexDistal,
    LeftMiddleProximal,
    LeftMiddleIntermediate,
    LeftMiddleDistal,
    LeftRingProximal,
    LeftRingIntermediate,
    LeftRingDistal,
    LeftLittleProximal,
    LeftLittleIntermediate,
    LeftLittleDistal,
    // ── right hand ──
    RightThumbMetacarpal,
    RightThumbProximal,
    RightThumbDistal,
    RightIndexProximal,
    RightIndexIntermediate,
    RightIndexDistal,
    RightMiddleProximal,
    RightMiddleIntermediate,
    RightMiddleDistal,
    RightRingProximal,
    RightRingIntermediate,
    RightRingDistal,
    RightLittleProximal,
    RightLittleIntermediate,
    RightLittleDistal,
}

use HumanBone as B;

impl HumanBone {
    /// Every human bone, in spec order.
    pub const ALL: [Self; 55] = [
        B::Hips,
        B::Spine,
        B::Chest,
        B::UpperChest,
        B::Neck,
        B::Head,
        B::LeftEye,
        B::RightEye,
        B::Jaw,
        B::LeftUpperLeg,
        B::LeftLowerLeg,
        B::LeftFoot,
        B::LeftToes,
        B::RightUpperLeg,
        B::RightLowerLeg,
        B::RightFoot,
        B::RightToes,
        B::LeftShoulder,
        B::LeftUpperArm,
        B::LeftLowerArm,
        B::LeftHand,
        B::RightShoulder,
        B::RightUpperArm,
        B::RightLowerArm,
        B::RightHand,
        B::LeftThumbMetacarpal,
        B::LeftThumbProximal,
        B::LeftThumbDistal,
        B::LeftIndexProximal,
        B::LeftIndexIntermediate,
        B::LeftIndexDistal,
        B::LeftMiddleProximal,
        B::LeftMiddleIntermediate,
        B::LeftMiddleDistal,
        B::LeftRingProximal,
        B::LeftRingIntermediate,
        B::LeftRingDistal,
        B::LeftLittleProximal,
        B::LeftLittleIntermediate,
        B::LeftLittleDistal,
        B::RightThumbMetacarpal,
        B::RightThumbProximal,
        B::RightThumbDistal,
        B::RightIndexProximal,
        B::RightIndexIntermediate,
        B::RightIndexDistal,
        B::RightMiddleProximal,
        B::RightMiddleIntermediate,
        B::RightMiddleDistal,
        B::RightRingProximal,
        B::RightRingIntermediate,
        B::RightRingDistal,
        B::RightLittleProximal,
        B::RightLittleIntermediate,
        B::RightLittleDistal,
    ];

    /// The 15 bones **VRM 1.0 requires** of every humanoid. A characterization missing any of these is
    /// not a humanoid, and [`crate::characterize`] says so with the specific bone named rather than
    /// failing with a cross in a corner (Unity's Avatar configuration screen, whose only failure
    /// indicator is exactly that).
    pub const REQUIRED: [Self; 15] = [
        B::Hips,
        B::Spine,
        B::Head,
        B::LeftUpperLeg,
        B::LeftLowerLeg,
        B::LeftFoot,
        B::RightUpperLeg,
        B::RightLowerLeg,
        B::RightFoot,
        B::LeftUpperArm,
        B::LeftLowerArm,
        B::LeftHand,
        B::RightUpperArm,
        B::RightLowerArm,
        B::RightHand,
    ];

    /// Whether VRM 1.0 requires this bone.
    #[must_use]
    pub fn is_required(self) -> bool {
        Self::REQUIRED.contains(&self)
    }

    /// The bone's side, or `None` for the centre column.
    #[must_use]
    pub fn side(self) -> Option<Side> {
        let n = self.as_str();
        if n.starts_with("left") {
            Some(Side::Left)
        } else if n.starts_with("right") {
            Some(Side::Right)
        } else {
            None
        }
    }

    /// The **canonical humanoid parent** — the hierarchy the spec defines, which is NOT necessarily the
    /// concrete rig's parent chain (a rig may interpose twist bones, an extra spine segment, or nothing
    /// at all between two humanoid bones).
    ///
    /// Retargeting walks THIS chain rather than the source rig's, which is what makes a 5-segment
    /// Unreal spine and a 2-segment Mixamo spine interchangeable without the user describing either.
    /// `Hips` returns `None`: it is the root of the humanoid.
    // The spec's hierarchy, written out group by group. Several groups legitimately share a parent
    // (`Spine` and both upper legs hang off `Hips`; `Neck` and both shoulders off `UpperChest`), and
    // merging those arms to satisfy `match_same_arms` would scatter one anatomical group across two
    // places and make the table harder to check against VRM 1.0 — which is the only thing it is for.
    #[allow(clippy::match_same_arms)]
    #[must_use]
    pub fn canonical_parent(self) -> Option<Self> {
        Some(match self {
            B::Hips => return None,
            B::Spine => B::Hips,
            B::Chest => B::Spine,
            B::UpperChest => B::Chest,
            B::Neck => B::UpperChest,
            B::Head => B::Neck,
            B::LeftEye | B::RightEye | B::Jaw => B::Head,

            B::LeftUpperLeg | B::RightUpperLeg => B::Hips,
            B::LeftLowerLeg => B::LeftUpperLeg,
            B::LeftFoot => B::LeftLowerLeg,
            B::LeftToes => B::LeftFoot,
            B::RightLowerLeg => B::RightUpperLeg,
            B::RightFoot => B::RightLowerLeg,
            B::RightToes => B::RightFoot,

            B::LeftShoulder | B::RightShoulder => B::UpperChest,
            B::LeftUpperArm => B::LeftShoulder,
            B::LeftLowerArm => B::LeftUpperArm,
            B::LeftHand => B::LeftLowerArm,
            B::RightUpperArm => B::RightShoulder,
            B::RightLowerArm => B::RightUpperArm,
            B::RightHand => B::RightLowerArm,

            B::LeftThumbMetacarpal
            | B::LeftIndexProximal
            | B::LeftMiddleProximal
            | B::LeftRingProximal
            | B::LeftLittleProximal => B::LeftHand,
            B::LeftThumbProximal => B::LeftThumbMetacarpal,
            B::LeftThumbDistal => B::LeftThumbProximal,
            B::LeftIndexIntermediate => B::LeftIndexProximal,
            B::LeftIndexDistal => B::LeftIndexIntermediate,
            B::LeftMiddleIntermediate => B::LeftMiddleProximal,
            B::LeftMiddleDistal => B::LeftMiddleIntermediate,
            B::LeftRingIntermediate => B::LeftRingProximal,
            B::LeftRingDistal => B::LeftRingIntermediate,
            B::LeftLittleIntermediate => B::LeftLittleProximal,
            B::LeftLittleDistal => B::LeftLittleIntermediate,

            B::RightThumbMetacarpal
            | B::RightIndexProximal
            | B::RightMiddleProximal
            | B::RightRingProximal
            | B::RightLittleProximal => B::RightHand,
            B::RightThumbProximal => B::RightThumbMetacarpal,
            B::RightThumbDistal => B::RightThumbProximal,
            B::RightIndexIntermediate => B::RightIndexProximal,
            B::RightIndexDistal => B::RightIndexIntermediate,
            B::RightMiddleIntermediate => B::RightMiddleProximal,
            B::RightMiddleDistal => B::RightMiddleIntermediate,
            B::RightRingIntermediate => B::RightRingProximal,
            B::RightRingDistal => B::RightRingIntermediate,
            B::RightLittleIntermediate => B::RightLittleProximal,
            B::RightLittleDistal => B::RightLittleIntermediate,
        })
    }

    /// The VRM 1.0 spelling (`leftUpperArm`) — the wire name, so a characterization serialized here and
    /// read by any VRM-aware tool means the same thing.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        VRM_CAMEL[self as usize]
    }

    /// Parse a VRM 1.0 spelling back. Case-insensitive so a hand-edited document is forgiving.
    #[must_use]
    pub fn from_vrm_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|b| b.as_str().eq_ignore_ascii_case(name))
    }

    /// A human-facing label (`Left Upper Arm`) for the editor's rig panel.
    #[must_use]
    pub fn label(self) -> String {
        let raw = self.as_str();
        let mut out = String::with_capacity(raw.len() + 8);
        for (i, ch) in raw.chars().enumerate() {
            if i == 0 {
                out.extend(ch.to_uppercase());
            } else {
                if ch.is_ascii_uppercase() {
                    out.push(' ');
                }
                out.push(ch);
            }
        }
        out
    }
}

/// The VRM camelCase spelling of every bone, indexed by `HumanBone as usize` — so [`HumanBone::as_str`]
/// is one index rather than a 55-arm match that has to be kept in the same order as [`HumanBone::ALL`]
/// by hand. `vrm_names_match_the_variant_order` pins the two together.
static VRM_CAMEL: [&str; 55] = [
    "hips",
    "spine",
    "chest",
    "upperChest",
    "neck",
    "head",
    "leftEye",
    "rightEye",
    "jaw",
    "leftUpperLeg",
    "leftLowerLeg",
    "leftFoot",
    "leftToes",
    "rightUpperLeg",
    "rightLowerLeg",
    "rightFoot",
    "rightToes",
    "leftShoulder",
    "leftUpperArm",
    "leftLowerArm",
    "leftHand",
    "rightShoulder",
    "rightUpperArm",
    "rightLowerArm",
    "rightHand",
    "leftThumbMetacarpal",
    "leftThumbProximal",
    "leftThumbDistal",
    "leftIndexProximal",
    "leftIndexIntermediate",
    "leftIndexDistal",
    "leftMiddleProximal",
    "leftMiddleIntermediate",
    "leftMiddleDistal",
    "leftRingProximal",
    "leftRingIntermediate",
    "leftRingDistal",
    "leftLittleProximal",
    "leftLittleIntermediate",
    "leftLittleDistal",
    "rightThumbMetacarpal",
    "rightThumbProximal",
    "rightThumbDistal",
    "rightIndexProximal",
    "rightIndexIntermediate",
    "rightIndexDistal",
    "rightMiddleProximal",
    "rightMiddleIntermediate",
    "rightMiddleDistal",
    "rightRingProximal",
    "rightRingIntermediate",
    "rightRingDistal",
    "rightLittleProximal",
    "rightLittleIntermediate",
    "rightLittleDistal",
];

/// Which authoring tool produced a rig, recognized from its bone names.
///
/// WHY RECOGNIZE THE CONVENTION INSTEAD OF FUZZY-MATCHING EVERY NAME. Because the conventions
/// genuinely **collide**, and a single fuzzy table gets them wrong in the one place it matters most:
/// Mixamo's `LeftLeg` is the **lower** leg (its upper leg is `LeftUpLeg`), while Rigify's `thigh.L` is
/// an **upper** leg and its `shin.L` is the lower one. A matcher that scores the token `leg` in
/// isolation must guess. Recognizing `mixamorig:` first removes the guess — and gives the user
/// something worth reading ("this is a Mixamo rig"), which is evidence rather than a confidence
/// percentage nobody can act on.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RigConvention {
    /// `mixamorig:LeftForeArm` — Adobe Mixamo, the most common free source of characters and clips.
    Mixamo,
    /// `lowerarm_l`, `thigh_l`, `ball_l` — the Unreal Engine 4/5 Mannequin.
    UnrealMannequin,
    /// `forearm.L`, `shin.L`, `f_index.01.L` — Blender's Rigify (and Blender rigs that follow it).
    Rigify,
    /// `LeftLowerArm` — the VRM / Unity-humanoid spelling, i.e. already canonical.
    VrmHumanoid,
    /// `Bip01 L Forearm` — 3ds Max Biped / Character Studio.
    MaxBiped,
    /// `lForearmBend`, `lShin`, `abdomenUpper` — Daz Genesis.
    DazGenesis,
    /// `CC_Base_L_Forearm` — Reallusion Character Creator / iClone.
    CharacterCreator,
}

impl RigConvention {
    /// Every convention that has an explicit name table.
    pub const ALL: [Self; 7] = [
        Self::Mixamo,
        Self::UnrealMannequin,
        Self::Rigify,
        Self::VrmHumanoid,
        Self::MaxBiped,
        Self::DazGenesis,
        Self::CharacterCreator,
    ];

    /// A short human label for the rig panel.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Mixamo => "Mixamo",
            Self::UnrealMannequin => "Unreal Mannequin",
            Self::Rigify => "Blender Rigify",
            Self::VrmHumanoid => "VRM / Unity humanoid",
            Self::MaxBiped => "3ds Max Biped",
            Self::DazGenesis => "Daz Genesis",
            Self::CharacterCreator => "Character Creator",
        }
    }

    /// This convention's name table: `(bone, the exact normalized stem this convention uses)`.
    ///
    /// Stems are given already **normalized** by [`normalize`] — lowercase, separators and the
    /// convention's own namespace prefix removed — so each entry states the distinguishing stem and
    /// nothing else.
    #[must_use]
    pub fn table(self) -> Vec<(HumanBone, &'static str)> {
        match self {
            Self::Mixamo => MIXAMO.to_vec(),
            Self::UnrealMannequin => UNREAL.to_vec(),
            Self::Rigify => RIGIFY.to_vec(),
            Self::VrmHumanoid => HumanBone::ALL
                .into_iter()
                .map(|b| (b, VRM_STEM[b as usize]))
                .collect(),
            Self::MaxBiped => MAX_BIPED.to_vec(),
            Self::DazGenesis => DAZ.to_vec(),
            Self::CharacterCreator => CHARACTER_CREATOR.to_vec(),
        }
    }
}

/// The VRM spelling of every bone with separators removed and lowercased — the `VrmHumanoid` table.
static VRM_STEM: [&str; 55] = [
    "hips",
    "spine",
    "chest",
    "upperchest",
    "neck",
    "head",
    "lefteye",
    "righteye",
    "jaw",
    "leftupperleg",
    "leftlowerleg",
    "leftfoot",
    "lefttoes",
    "rightupperleg",
    "rightlowerleg",
    "rightfoot",
    "righttoes",
    "leftshoulder",
    "leftupperarm",
    "leftlowerarm",
    "lefthand",
    "rightshoulder",
    "rightupperarm",
    "rightlowerarm",
    "righthand",
    "leftthumbmetacarpal",
    "leftthumbproximal",
    "leftthumbdistal",
    "leftindexproximal",
    "leftindexintermediate",
    "leftindexdistal",
    "leftmiddleproximal",
    "leftmiddleintermediate",
    "leftmiddledistal",
    "leftringproximal",
    "leftringintermediate",
    "leftringdistal",
    "leftlittleproximal",
    "leftlittleintermediate",
    "leftlittledistal",
    "rightthumbmetacarpal",
    "rightthumbproximal",
    "rightthumbdistal",
    "rightindexproximal",
    "rightindexintermediate",
    "rightindexdistal",
    "rightmiddleproximal",
    "rightmiddleintermediate",
    "rightmiddledistal",
    "rightringproximal",
    "rightringintermediate",
    "rightringdistal",
    "rightlittleproximal",
    "rightlittleintermediate",
    "rightlittledistal",
];

static MIXAMO: [(HumanBone, &str); 52] = [
    (B::Hips, "hips"),
    (B::Spine, "spine"),
    (B::Chest, "spine1"),
    (B::UpperChest, "spine2"),
    (B::Neck, "neck"),
    (B::Head, "head"),
    (B::LeftUpperLeg, "leftupleg"),
    (B::LeftLowerLeg, "leftleg"),
    (B::LeftFoot, "leftfoot"),
    (B::LeftToes, "lefttoebase"),
    (B::RightUpperLeg, "rightupleg"),
    (B::RightLowerLeg, "rightleg"),
    (B::RightFoot, "rightfoot"),
    (B::RightToes, "righttoebase"),
    (B::LeftShoulder, "leftshoulder"),
    (B::LeftUpperArm, "leftarm"),
    (B::LeftLowerArm, "leftforearm"),
    (B::LeftHand, "lefthand"),
    (B::RightShoulder, "rightshoulder"),
    (B::RightUpperArm, "rightarm"),
    (B::RightLowerArm, "rightforearm"),
    (B::RightHand, "righthand"),
    (B::LeftThumbMetacarpal, "lefthandthumb1"),
    (B::LeftThumbProximal, "lefthandthumb2"),
    (B::LeftThumbDistal, "lefthandthumb3"),
    (B::LeftIndexProximal, "lefthandindex1"),
    (B::LeftIndexIntermediate, "lefthandindex2"),
    (B::LeftIndexDistal, "lefthandindex3"),
    (B::LeftMiddleProximal, "lefthandmiddle1"),
    (B::LeftMiddleIntermediate, "lefthandmiddle2"),
    (B::LeftMiddleDistal, "lefthandmiddle3"),
    (B::LeftRingProximal, "lefthandring1"),
    (B::LeftRingIntermediate, "lefthandring2"),
    (B::LeftRingDistal, "lefthandring3"),
    (B::LeftLittleProximal, "lefthandpinky1"),
    (B::LeftLittleIntermediate, "lefthandpinky2"),
    (B::LeftLittleDistal, "lefthandpinky3"),
    (B::RightThumbMetacarpal, "righthandthumb1"),
    (B::RightThumbProximal, "righthandthumb2"),
    (B::RightThumbDistal, "righthandthumb3"),
    (B::RightIndexProximal, "righthandindex1"),
    (B::RightIndexIntermediate, "righthandindex2"),
    (B::RightIndexDistal, "righthandindex3"),
    (B::RightMiddleProximal, "righthandmiddle1"),
    (B::RightMiddleIntermediate, "righthandmiddle2"),
    (B::RightMiddleDistal, "righthandmiddle3"),
    (B::RightRingProximal, "righthandring1"),
    (B::RightRingIntermediate, "righthandring2"),
    (B::RightRingDistal, "righthandring3"),
    (B::RightLittleProximal, "righthandpinky1"),
    (B::RightLittleIntermediate, "righthandpinky2"),
    (B::RightLittleDistal, "righthandpinky3"),
];

static UNREAL: [(HumanBone, &str); 52] = [
    (B::Hips, "pelvis"),
    (B::Spine, "spine01"),
    (B::Chest, "spine02"),
    (B::UpperChest, "spine03"),
    (B::Neck, "neck01"),
    (B::Head, "head"),
    (B::LeftUpperLeg, "thighl"),
    (B::LeftLowerLeg, "calfl"),
    (B::LeftFoot, "footl"),
    (B::LeftToes, "balll"),
    (B::RightUpperLeg, "thighr"),
    (B::RightLowerLeg, "calfr"),
    (B::RightFoot, "footr"),
    (B::RightToes, "ballr"),
    (B::LeftShoulder, "claviclel"),
    (B::LeftUpperArm, "upperarml"),
    (B::LeftLowerArm, "lowerarml"),
    (B::LeftHand, "handl"),
    (B::RightShoulder, "clavicler"),
    (B::RightUpperArm, "upperarmr"),
    (B::RightLowerArm, "lowerarmr"),
    (B::RightHand, "handr"),
    (B::LeftThumbMetacarpal, "thumb01l"),
    (B::LeftThumbProximal, "thumb02l"),
    (B::LeftThumbDistal, "thumb03l"),
    (B::LeftIndexProximal, "index01l"),
    (B::LeftIndexIntermediate, "index02l"),
    (B::LeftIndexDistal, "index03l"),
    (B::LeftMiddleProximal, "middle01l"),
    (B::LeftMiddleIntermediate, "middle02l"),
    (B::LeftMiddleDistal, "middle03l"),
    (B::LeftRingProximal, "ring01l"),
    (B::LeftRingIntermediate, "ring02l"),
    (B::LeftRingDistal, "ring03l"),
    (B::LeftLittleProximal, "pinky01l"),
    (B::LeftLittleIntermediate, "pinky02l"),
    (B::LeftLittleDistal, "pinky03l"),
    (B::RightThumbMetacarpal, "thumb01r"),
    (B::RightThumbProximal, "thumb02r"),
    (B::RightThumbDistal, "thumb03r"),
    (B::RightIndexProximal, "index01r"),
    (B::RightIndexIntermediate, "index02r"),
    (B::RightIndexDistal, "index03r"),
    (B::RightMiddleProximal, "middle01r"),
    (B::RightMiddleIntermediate, "middle02r"),
    (B::RightMiddleDistal, "middle03r"),
    (B::RightRingProximal, "ring01r"),
    (B::RightRingIntermediate, "ring02r"),
    (B::RightRingDistal, "ring03r"),
    (B::RightLittleProximal, "pinky01r"),
    (B::RightLittleIntermediate, "pinky02r"),
    (B::RightLittleDistal, "pinky03r"),
];

static RIGIFY: [(HumanBone, &str); 52] = [
    (B::Hips, "spine"),
    (B::Spine, "spine001"),
    (B::Chest, "spine002"),
    (B::UpperChest, "spine003"),
    (B::Neck, "spine005"),
    (B::Head, "spine006"),
    (B::LeftUpperLeg, "thighl"),
    (B::LeftLowerLeg, "shinl"),
    (B::LeftFoot, "footl"),
    (B::LeftToes, "toel"),
    (B::RightUpperLeg, "thighr"),
    (B::RightLowerLeg, "shinr"),
    (B::RightFoot, "footr"),
    (B::RightToes, "toer"),
    (B::LeftShoulder, "shoulderl"),
    (B::LeftUpperArm, "upperarml"),
    (B::LeftLowerArm, "forearml"),
    (B::LeftHand, "handl"),
    (B::RightShoulder, "shoulderr"),
    (B::RightUpperArm, "upperarmr"),
    (B::RightLowerArm, "forearmr"),
    (B::RightHand, "handr"),
    (B::LeftThumbMetacarpal, "thumb01l"),
    (B::LeftThumbProximal, "thumb02l"),
    (B::LeftThumbDistal, "thumb03l"),
    (B::LeftIndexProximal, "findex01l"),
    (B::LeftIndexIntermediate, "findex02l"),
    (B::LeftIndexDistal, "findex03l"),
    (B::LeftMiddleProximal, "fmiddle01l"),
    (B::LeftMiddleIntermediate, "fmiddle02l"),
    (B::LeftMiddleDistal, "fmiddle03l"),
    (B::LeftRingProximal, "fring01l"),
    (B::LeftRingIntermediate, "fring02l"),
    (B::LeftRingDistal, "fring03l"),
    (B::LeftLittleProximal, "fpinky01l"),
    (B::LeftLittleIntermediate, "fpinky02l"),
    (B::LeftLittleDistal, "fpinky03l"),
    (B::RightThumbMetacarpal, "thumb01r"),
    (B::RightThumbProximal, "thumb02r"),
    (B::RightThumbDistal, "thumb03r"),
    (B::RightIndexProximal, "findex01r"),
    (B::RightIndexIntermediate, "findex02r"),
    (B::RightIndexDistal, "findex03r"),
    (B::RightMiddleProximal, "fmiddle01r"),
    (B::RightMiddleIntermediate, "fmiddle02r"),
    (B::RightMiddleDistal, "fmiddle03r"),
    (B::RightRingProximal, "fring01r"),
    (B::RightRingIntermediate, "fring02r"),
    (B::RightRingDistal, "fring03r"),
    (B::RightLittleProximal, "fpinky01r"),
    (B::RightLittleIntermediate, "fpinky02r"),
    (B::RightLittleDistal, "fpinky03r"),
];

static MAX_BIPED: [(HumanBone, &str); 22] = [
    (B::Hips, "pelvis"),
    (B::Spine, "spine"),
    (B::Chest, "spine1"),
    (B::UpperChest, "spine2"),
    (B::Neck, "neck"),
    (B::Head, "head"),
    (B::LeftUpperLeg, "lthigh"),
    (B::LeftLowerLeg, "lcalf"),
    (B::LeftFoot, "lfoot"),
    (B::LeftToes, "ltoe0"),
    (B::RightUpperLeg, "rthigh"),
    (B::RightLowerLeg, "rcalf"),
    (B::RightFoot, "rfoot"),
    (B::RightToes, "rtoe0"),
    (B::LeftShoulder, "lclavicle"),
    (B::LeftUpperArm, "lupperarm"),
    (B::LeftLowerArm, "lforearm"),
    (B::LeftHand, "lhand"),
    (B::RightShoulder, "rclavicle"),
    (B::RightUpperArm, "rupperarm"),
    (B::RightLowerArm, "rforearm"),
    (B::RightHand, "rhand"),
];

static DAZ: [(HumanBone, &str); 22] = [
    (B::Hips, "hip"),
    (B::Spine, "abdomenlower"),
    (B::Chest, "abdomenupper"),
    (B::UpperChest, "chestlower"),
    (B::Neck, "necklower"),
    (B::Head, "head"),
    (B::LeftUpperLeg, "lthighbend"),
    (B::LeftLowerLeg, "lshin"),
    (B::LeftFoot, "lfoot"),
    (B::LeftToes, "ltoe"),
    (B::RightUpperLeg, "rthighbend"),
    (B::RightLowerLeg, "rshin"),
    (B::RightFoot, "rfoot"),
    (B::RightToes, "rtoe"),
    (B::LeftShoulder, "lcollar"),
    (B::LeftUpperArm, "lshldrbend"),
    (B::LeftLowerArm, "lforearmbend"),
    (B::LeftHand, "lhand"),
    (B::RightShoulder, "rcollar"),
    (B::RightUpperArm, "rshldrbend"),
    (B::RightLowerArm, "rforearmbend"),
    (B::RightHand, "rhand"),
];

static CHARACTER_CREATOR: [(HumanBone, &str); 22] = [
    (B::Hips, "hip"),
    (B::Spine, "waist"),
    (B::Chest, "spine01"),
    (B::UpperChest, "spine02"),
    (B::Neck, "necktwist01"),
    (B::Head, "head"),
    (B::LeftUpperLeg, "lthigh"),
    (B::LeftLowerLeg, "lcalf"),
    (B::LeftFoot, "lfoot"),
    (B::LeftToes, "ltoebase"),
    (B::RightUpperLeg, "rthigh"),
    (B::RightLowerLeg, "rcalf"),
    (B::RightFoot, "rfoot"),
    (B::RightToes, "rtoebase"),
    (B::LeftShoulder, "lclavicle"),
    (B::LeftUpperArm, "lupperarm"),
    (B::LeftLowerArm, "lforearm"),
    (B::LeftHand, "lhand"),
    (B::RightShoulder, "rclavicle"),
    (B::RightUpperArm, "rupperarm"),
    (B::RightLowerArm, "rforearm"),
    (B::RightHand, "rhand"),
];

/// The namespace prefixes a rig decorates its bones with, stripped before matching. Longest first, so
/// `mixamorig1:` is not half-eaten by `mixamorig:`.
const PREFIXES: [&str; 9] = [
    "mixamorig1:",
    "mixamorig:",
    "cc_base_",
    "bip001",
    "bip01",
    "armature|",
    "armature_",
    "root|",
    "genesis8_",
];

/// Reduce a rig's bone name to the stem the tables are written in: drop the namespace prefix, drop
/// every separator, lowercase.
///
/// Deliberately NOT a fuzzy / edit-distance match. A near-miss that silently binds the wrong bone is
/// precisely the failure this module exists to prevent: an unmatched bone gets reported and costs the
/// user one glance, whereas a wrongly-matched one produces a character that moves subtly incorrectly
/// forever, with nothing on screen suggesting why.
#[must_use]
pub fn normalize(raw: &str) -> String {
    let lowered = raw.trim().to_ascii_lowercase();
    let stem = PREFIXES
        .iter()
        .find_map(|p| lowered.strip_prefix(p))
        .unwrap_or(&lowered);
    stem.chars().filter(char::is_ascii_alphanumeric).collect()
}

/// A characterization: which joint of a concrete [`crate::Skeleton`] fills each humanoid slot, plus
/// which convention the names were recognized as.
///
/// This is the artifact Unreal spreads across an IK Rig per skeleton **and** an IK Retargeter per pair,
/// and that Unity hides inside a binary Avatar. Here it is one plain, inspectable value attached to ONE
/// skeleton — which is what lets [`crate::retarget`] be a pure function of two of them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanoidProfile {
    /// humanoid slot → joint index in the concrete skeleton.
    pub bones: BTreeMap<HumanBone, usize>,
    /// Which convention the names were recognized as, if any.
    pub convention: Option<RigConvention>,
}

impl HumanoidProfile {
    /// The joint index filling a humanoid slot, if the rig has one.
    #[must_use]
    pub fn joint(&self, bone: HumanBone) -> Option<usize> {
        self.bones.get(&bone).copied()
    }

    /// Whether every VRM-required bone is mapped — i.e. whether this rig can be retargeted at all.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        HumanBone::REQUIRED
            .iter()
            .all(|b| self.bones.contains_key(b))
    }

    /// The required bones this rig is missing, in spec order — the actionable half of "not a humanoid".
    #[must_use]
    pub fn missing_required(&self) -> Vec<HumanBone> {
        HumanBone::REQUIRED
            .into_iter()
            .filter(|b| !self.bones.contains_key(b))
            .collect()
    }

    /// Joint indices that fill NO humanoid slot — the tail, cape, skirt, hair chain, prop socket and
    /// twist bones. Named explicitly, and carried rather than discarded, because Unity's silent drop of
    /// exactly this set is one of the loudest complaints about its humanoid pipeline.
    #[must_use]
    pub fn extra_joints(&self, joint_count: usize) -> Vec<usize> {
        let mapped: std::collections::BTreeSet<usize> = self.bones.values().copied().collect();
        (0..joint_count).filter(|i| !mapped.contains(i)).collect()
    }
}
