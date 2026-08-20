//! `metrocalk-character` — **the bridge that lets a clip drive a skeleton.**
//!
//! THE GAP THIS CLOSES. This repository had two good halves of character animation that had never been
//! introduced. `metrocalk-animation` is a deterministic, content-hashed property kernel whose only
//! address is a four-part string path — it has no joint, bone or pose type anywhere in it.
//! `metrocalk-skeleton` is a correct FK/IK/skinning runtime that cannot sample a clip. So there was no
//! expression anywhere in the codebase for "play this animation on that character", and skeletal clips
//! were, quite reasonably, refused at import.
//!
//! Neither crate should absorb the other. The kernel is deliberately dependency-free so one validated
//! asset can drive the editor, the runtime, a headless server and `wasm32` alike; the skeleton crate is
//! the low-level statement of what a body *is*. The bridge belongs above both, and this is it.
//!
//! ─────────────────────────────────────────────────────────────────────────────────────────────────
//!
//! **THE IDEA WORTH KEEPING: a channel may be addressed by CANONICAL HUMANOID BONE, not only by the
//! rig's own joint name.**
//!
//! A clip whose channels say `leftLowerArm` is not authored against a skeleton at all — it is authored
//! against *the human body*. Bind it to any characterized rig and it plays, whatever that rig calls its
//! bones and however many twist or spine segments it interposes. There is no retarget step, because
//! there is nothing rig-specific to retarget FROM.
//!
//! That is the second half of the bet [`metrocalk_skeleton::humanoid`] makes. Unreal binds an animation
//! to *a Skeleton asset*, which is why a clip from the wrong skeleton needs an IK Retargeter and three
//! authored assets before it plays at all; Unity binds to an Avatar and silently T-poses when a
//! Generic-authored clip is marked Humanoid. Addressing the body instead of the rig removes the
//! question rather than answering it.
//!
//! Rig-specific channels ([`BoneKey::Joint`]) are still supported and still resolve by name — a clip
//! authored for one specific character is a legitimate thing to want, and [`retarget_pose`] exists for
//! moving it. The point is that it is no longer the only option.
//!
//! ─────────────────────────────────────────────────────────────────────────────────────────────────
//!
//! **BINDING IS SEPARATE FROM SAMPLING, AND IT IS WHERE EVERY DIAGNOSTIC LIVES.** [`bind_sequence`]
//! resolves every channel once and reports — by name, with a remediation — anything it could not place.
//! [`PoseBinding::sample`] is then a pure, allocation-light function of `(binding, tick)`. This split
//! is deliberate: "the character T-posed" is Unreal's universal silent-failure symptom with at least
//! four unrelated causes and no message attached to any of them. Here an unbound channel is a sentence
//! at bind time, before a single frame plays.
//!
//! [`retarget_pose`]: metrocalk_skeleton::retarget::retarget_pose

use std::collections::BTreeMap;

use metrocalk_animation::{AnimValue, CompiledSequence, PropertyPath, Tick};
use metrocalk_skeleton::characterize::HumanoidCharacterization;
use metrocalk_skeleton::humanoid::HumanBone;
use metrocalk_skeleton::{Pose, Skeleton, Transform};

/// The component segment a skeletal channel uses. A path whose `component` is anything else is not a
/// bone channel and is left entirely alone — this crate never claims a path it does not understand.
pub const SKELETON_COMPONENT: &str = "Skeleton";

/// Which joint a channel addresses.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BoneKey {
    /// A canonical humanoid slot (`leftLowerArm`) — **rig-independent**. Resolved through the target's
    /// [`HumanoidCharacterization`], so the same clip plays on every characterized character.
    Humanoid(HumanBone),
    /// This rig's own joint name (`mixamorig:LeftForeArm`) — rig-specific, resolved by exact match.
    Joint(String),
}

impl BoneKey {
    /// Read a channel's bone key from the path's `property` segment.
    ///
    /// A segment that spells a VRM human bone IS one; anything else is a raw joint name. The two
    /// namespaces cannot collide in practice — no rig names a bone exactly `leftUpperArm` unless it is
    /// already speaking the canonical vocabulary, in which case treating it as canonical is right.
    #[must_use]
    pub fn parse(property: &str) -> Self {
        HumanBone::from_vrm_name(property)
            .map_or_else(|| Self::Joint(property.to_string()), Self::Humanoid)
    }

    /// How this key reads in a diagnostic.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Humanoid(b) => format!("the humanoid bone `{}`", b.as_str()),
            Self::Joint(n) => format!("the joint named `{n}`"),
        }
    }
}

/// Which part of a joint's local TRS a channel drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Channel {
    Translation,
    Rotation,
    Scale,
}

impl Channel {
    /// Read the channel from the path's first subpath segment. glTF's own spelling.
    #[must_use]
    pub fn parse(segment: &str) -> Option<Self> {
        match segment {
            "translation" => Some(Self::Translation),
            "rotation" => Some(Self::Rotation),
            "scale" => Some(Self::Scale),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Translation => "translation",
            Self::Rotation => "rotation",
            Self::Scale => "scale",
        }
    }
}

/// A channel that could not be bound, and what to do about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindDiagnostic {
    pub code: BindDiagnosticCode,
    pub path: String,
    pub message: String,
    pub remediation: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BindDiagnosticCode {
    /// The clip drives a humanoid bone this character does not have.
    UnmappedHumanoidBone,
    /// The clip drives a joint name this rig does not contain.
    UnknownJoint,
    /// The path's subpath does not name translation/rotation/scale.
    UnknownChannel,
    /// The channel's value kind cannot drive the channel it addresses (a Vec3 on a rotation).
    WrongValueKind,
}

impl BindDiagnosticCode {
    /// Whether this stops the channel from playing at all. All of them do — a channel that cannot be
    /// bound cannot be sampled — but the code says which repair is needed.
    #[must_use]
    pub fn is_blocking(self) -> bool {
        true
    }
}

/// One resolved channel: which joint it drives and which part of its TRS.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct BoundChannel {
    path: PropertyPath,
    joint: usize,
    channel: Channel,
}

/// A sequence resolved against one skeleton — the reusable half of playback.
///
/// Built once per (clip, character) pair and then sampled every frame. Holds no clock and no mutable
/// state, so one binding can drive many instances at different times, and sampling is a pure function.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PoseBinding {
    channels: Vec<BoundChannel>,
    /// Channels that resolved to nothing, kept so a UI can show them after binding.
    pub diagnostics: Vec<BindDiagnostic>,
}

impl PoseBinding {
    /// How many channels actually drive a joint.
    #[must_use]
    pub fn bound_count(&self) -> usize {
        self.channels.len()
    }

    /// The distinct joints this binding writes.
    #[must_use]
    pub fn driven_joints(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.channels.iter().map(|c| c.joint).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Whether anything at all was bound. A binding with no channels would play a character's bind pose
    /// forever, which is exactly the silent "it T-posed" symptom — so callers are given a cheap way to
    /// refuse before it happens.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }

    /// Sample the sequence at `tick` into a [`Pose`].
    ///
    /// Pure: same binding, same sequence, same tick, same pose. A joint whose channels leave it at its
    /// bind local is **not written**, so the pose stays sparse — the invariant the whole
    /// [`metrocalk_skeleton::pose_ops`] algebra depends on.
    #[must_use]
    pub fn sample(&self, sequence: &CompiledSequence, tick: Tick, skeleton: &Skeleton) -> Pose {
        let evaluation = sequence.evaluate(tick);

        // The kernel returns values keyed by binding path; index them so each bound channel is one
        // lookup rather than a scan per channel.
        let by_path: BTreeMap<&PropertyPath, &AnimValue> = evaluation
            .bindings
            .iter()
            .map(|b| (&b.binding.path, &b.value))
            .collect();

        // Start every driven joint from its BIND local, not from identity: a clip that animates only
        // rotation must leave the bone's authored offset alone, or every limb collapses onto its
        // parent's origin. (The naive "compose from identity" is why a partially-keyed clip explodes.)
        let mut locals: BTreeMap<usize, Transform> = BTreeMap::new();
        for channel in &self.channels {
            let Some(value) = by_path.get(&channel.path) else {
                continue;
            };
            let entry = locals
                .entry(channel.joint)
                .or_insert_with(|| skeleton.joints[channel.joint].local_bind);
            apply(entry, channel.channel, value);
        }

        let mut pose = Pose::new();
        for (joint, local) in locals {
            if local != skeleton.joints[joint].local_bind {
                pose.set(joint, local);
            }
        }
        pose
    }
}

/// Write one sampled value into the part of a local TRS it addresses. A value of the wrong shape is
/// rejected at bind time, so anything reaching here is already the right kind.
///
/// THE f64 → f32 NARROWING IS THE POINT OF THIS FUNCTION, not an oversight. The kernel samples in `f64`
/// deliberately — that is what makes its evaluation bit-reproducible across platforms — while a pose is
/// `f32` because the skeleton, the gizmo and the GPU all are. Somewhere the two have to meet, and this
/// is the one place, so the narrowing is visible here instead of scattered through the callers.
#[allow(clippy::cast_possible_truncation)]
fn apply(local: &mut Transform, channel: Channel, value: &AnimValue) {
    match (channel, value) {
        (Channel::Translation, AnimValue::Vec3(v)) => {
            local.translation = [v[0] as f32, v[1] as f32, v[2] as f32];
        }
        (Channel::Scale, AnimValue::Vec3(v)) => {
            local.scale = [v[0] as f32, v[1] as f32, v[2] as f32];
        }
        (Channel::Rotation, AnimValue::Quaternion(q)) => {
            local.rotation = [q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32];
        }
        _ => {}
    }
}

/// Whether a value kind can drive a channel — the check that makes [`apply`] total.
fn kind_fits(channel: Channel, value: &AnimValue) -> bool {
    matches!(
        (channel, value),
        (Channel::Translation | Channel::Scale, AnimValue::Vec3(_))
            | (Channel::Rotation, AnimValue::Quaternion(_))
    )
}

/// Resolve every skeletal channel of `sequence` against `skeleton`, reporting what could not be placed.
///
/// Non-skeletal channels (any path whose `component` is not [`SKELETON_COMPONENT`]) are ignored
/// silently and deliberately: one sequence may animate a character's bones AND a material parameter,
/// and this crate has no business claiming the second.
#[must_use]
pub fn bind_sequence(
    sequence: &CompiledSequence,
    skeleton: &Skeleton,
    characterization: &HumanoidCharacterization,
) -> PoseBinding {
    // Joint name → index, first wins (topological order, so a parent beats a same-named child).
    let mut by_name: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, joint) in skeleton.joints.iter().enumerate() {
        if !joint.name.is_empty() {
            by_name.entry(joint.name.as_str()).or_insert(i);
        }
    }

    let mut binding = PoseBinding::default();

    // Sample at tick 0 purely to learn each channel's VALUE KIND — the kernel guarantees a track's kind
    // is constant, so one evaluation is enough to type-check every channel before any frame plays.
    let probe = sequence.evaluate(Tick(0));
    let kinds: BTreeMap<&PropertyPath, &AnimValue> = probe
        .bindings
        .iter()
        .map(|b| (&b.binding.path, &b.value))
        .collect();

    for evaluated in &probe.bindings {
        let path = &evaluated.binding.path;
        if path.component != SKELETON_COMPONENT {
            continue;
        }

        let display = path.display_path();
        let Some(channel) = path.subpath.first().and_then(|s| Channel::parse(s)) else {
            binding.diagnostics.push(BindDiagnostic {
                code: BindDiagnosticCode::UnknownChannel,
                path: display,
                message: format!(
                    "the channel `{}` is not translation, rotation or scale",
                    path.subpath.first().map_or("(none)", String::as_str)
                ),
                remediation: "A bone channel's first subpath segment must be `translation`, \
                              `rotation` or `scale` — the glTF spelling."
                    .to_string(),
            });
            continue;
        };

        let key = BoneKey::parse(&path.property);
        let joint = match &key {
            BoneKey::Humanoid(bone) => characterization.profile.joint(*bone),
            BoneKey::Joint(name) => by_name.get(name.as_str()).copied(),
        };

        let Some(joint) = joint else {
            binding.diagnostics.push(match &key {
                BoneKey::Humanoid(bone) => BindDiagnostic {
                    code: BindDiagnosticCode::UnmappedHumanoidBone,
                    path: display,
                    message: format!(
                        "this clip animates {}, which this character's rig has no bone for",
                        key.describe()
                    ),
                    remediation: format!(
                        "Assign {} in the character's rig panel, or accept that this channel does \
                         nothing — the rest of the clip still plays.",
                        bone.label()
                    ),
                },
                BoneKey::Joint(name) => BindDiagnostic {
                    code: BindDiagnosticCode::UnknownJoint,
                    path: display,
                    message: format!("this rig has no joint named `{name}`"),
                    remediation: "This clip was authored against a different rig. Re-key its \
                                  channels to canonical humanoid bones so it plays on any \
                                  character, or retarget it from the rig it was authored for."
                        .to_string(),
                },
            });
            continue;
        };

        if let Some(value) = kinds.get(path) {
            if !kind_fits(channel, value) {
                binding.diagnostics.push(BindDiagnostic {
                    code: BindDiagnosticCode::WrongValueKind,
                    path: display,
                    message: format!(
                        "a {} channel cannot be driven by a {:?} value",
                        channel.as_str(),
                        evaluated.binding.value_kind
                    ),
                    remediation:
                        "Rotation channels need a quaternion; translation and scale need a \
                                  vec3."
                            .to_string(),
                });
                continue;
            }
        }

        binding.channels.push(BoundChannel {
            path: path.clone(),
            joint,
            channel,
        });
    }

    // A STABLE ORDER, so a binding is comparable and a sample is reproducible regardless of the order
    // the kernel happened to return its bindings in.
    binding.channels.sort();
    binding
}

/// The [`PropertyPath`] a skeletal channel uses — so authoring code and this binder cannot disagree
/// about the spelling.
///
/// `target` is the character (an entity id or asset name); `property` is the bone key, which is either
/// a VRM human-bone name (rig-independent) or the rig's own joint name.
#[must_use]
pub fn bone_path(target: &str, bone: &BoneKey, channel: Channel) -> PropertyPath {
    let property = match bone {
        BoneKey::Humanoid(b) => b.as_str().to_string(),
        BoneKey::Joint(n) => n.clone(),
    };
    PropertyPath::new(target, SKELETON_COMPONENT, property).with_subpath([channel.as_str()])
}
