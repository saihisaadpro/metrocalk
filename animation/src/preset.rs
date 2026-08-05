//! Editable, deterministic starting points for common flat-sequence animation tasks.
//!
//! Presets only author ordinary [`Sequence`] data. They do not install runtime behavior, blending,
//! transitions, or editor state. Graph, state-machine, and locomotion presets are intentionally absent
//! until those concepts have a first-class authored graph model; presenting them as flat tracks would
//! hide the blending and transition semantics that those workflows require.

use serde::{Deserialize, Serialize};

use crate::{
    AnimValue, Binding, Interpolation, KeyId, Keyframe, LoopPolicy, PropertyPath, Sequence,
    SequenceId, Tick, Track, TrackId, ValueKind,
};

const SECOND: i64 = 60_000;

/// The bounded set of data-only sequence templates supported by the v1 animation kernel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationPreset {
    SpriteLoop,
    Fade,
    UiHover,
    UiClick,
    LoadingPulse,
    RigidCameraMove,
}

impl AnimationPreset {
    pub const ALL: [Self; 6] = [
        Self::SpriteLoop,
        Self::Fade,
        Self::UiHover,
        Self::UiClick,
        Self::LoadingPulse,
        Self::RigidCameraMove,
    ];

    /// Builds fresh, fully editable authored data. `sequence_id` is the caller-owned persistent identity;
    /// track and key IDs are readable, deterministic descendants of it and never depend on key time/value.
    #[must_use]
    pub fn instantiate(
        self,
        sequence_id: impl Into<String>,
        target: impl Into<String>,
    ) -> Sequence {
        let sequence_id = sequence_id.into();
        let target = target.into();
        match self {
            Self::SpriteLoop => sprite_loop(sequence_id, target),
            Self::Fade => fade(sequence_id, target),
            Self::UiHover => ui_hover(sequence_id, target),
            Self::UiClick => ui_click(sequence_id, target),
            Self::LoadingPulse => loading_pulse(sequence_id, target),
            Self::RigidCameraMove => rigid_camera_move(sequence_id, target),
        }
    }

    const fn slug(self) -> &'static str {
        match self {
            Self::SpriteLoop => "sprite-loop",
            Self::Fade => "fade",
            Self::UiHover => "ui-hover",
            Self::UiClick => "ui-click",
            Self::LoadingPulse => "loading-pulse",
            Self::RigidCameraMove => "rigid-camera-move",
        }
    }
}

/// Eight-frame, eight-frames-per-second sprite cycle using exact step keys.
#[must_use]
pub fn sprite_loop(sequence_id: impl Into<String>, target: impl Into<String>) -> Sequence {
    let kind = AnimationPreset::SpriteLoop;
    let target = target.into();
    let mut sequence = base_sequence(sequence_id, "Sprite loop", Tick(SECOND), LoopPolicy::Loop);
    sequence.tracks.push(authored_track(
        &sequence.id,
        kind,
        "frame",
        &target,
        "Sprite",
        "frame",
        ValueKind::Integer,
        Interpolation::Step,
        (0_i64..8)
            .map(|frame| (Tick(frame * (SECOND / 8)), AnimValue::Integer(frame)))
            .collect(),
    ));
    sequence
}

/// A 300 ms eased sprite fade-in. Reverse the resulting clip for a fade-out.
#[must_use]
pub fn fade(sequence_id: impl Into<String>, target: impl Into<String>) -> Sequence {
    let kind = AnimationPreset::Fade;
    let target = target.into();
    let duration = Tick(18_000);
    let mut sequence = base_sequence(sequence_id, "Fade in", duration, LoopPolicy::Once);
    sequence.tracks.push(authored_track(
        &sequence.id,
        kind,
        "opacity",
        &target,
        "Sprite",
        "opacity",
        ValueKind::Number,
        Interpolation::Cubic,
        vec![
            (Tick::ZERO, AnimValue::Number(0.0)),
            (duration, AnimValue::Number(1.0)),
        ],
    ));
    sequence
}

/// A restrained 120 ms UI hover response with independent scale and opacity channels.
#[must_use]
pub fn ui_hover(sequence_id: impl Into<String>, target: impl Into<String>) -> Sequence {
    let kind = AnimationPreset::UiHover;
    let target = target.into();
    let duration = Tick(7_200);
    let mut sequence = base_sequence(sequence_id, "UI hover", duration, LoopPolicy::Once);
    sequence.tracks.extend([
        authored_track(
            &sequence.id,
            kind,
            "scale",
            &target,
            "UiStyle",
            "scale",
            ValueKind::Number,
            Interpolation::Cubic,
            vec![
                (Tick::ZERO, AnimValue::Number(1.0)),
                (duration, AnimValue::Number(1.04)),
            ],
        ),
        authored_track(
            &sequence.id,
            kind,
            "opacity",
            &target,
            "UiStyle",
            "opacity",
            ValueKind::Number,
            Interpolation::Cubic,
            vec![
                (Tick::ZERO, AnimValue::Number(0.92)),
                (duration, AnimValue::Number(1.0)),
            ],
        ),
    ]);
    sequence
}

/// A short, reversible 90 ms pressed-state response for buttons and other interactive UI elements.
#[must_use]
pub fn ui_click(sequence_id: impl Into<String>, target: impl Into<String>) -> Sequence {
    let kind = AnimationPreset::UiClick;
    let target = target.into();
    let duration = Tick(5_400);
    let mut sequence = base_sequence(sequence_id, "UI click", duration, LoopPolicy::Once);
    sequence.tracks.extend([
        authored_track(
            &sequence.id,
            kind,
            "scale",
            &target,
            "UiStyle",
            "scale",
            ValueKind::Number,
            Interpolation::Cubic,
            vec![
                (Tick::ZERO, AnimValue::Number(1.0)),
                (duration, AnimValue::Number(0.96)),
            ],
        ),
        authored_track(
            &sequence.id,
            kind,
            "opacity",
            &target,
            "UiStyle",
            "opacity",
            ValueKind::Number,
            Interpolation::Cubic,
            vec![
                (Tick::ZERO, AnimValue::Number(1.0)),
                (duration, AnimValue::Number(0.9)),
            ],
        ),
    ]);
    sequence
}

/// A seamless 900 ms loading pulse that returns to its exact initial pose each cycle.
#[must_use]
pub fn loading_pulse(sequence_id: impl Into<String>, target: impl Into<String>) -> Sequence {
    let kind = AnimationPreset::LoadingPulse;
    let target = target.into();
    let duration = Tick(54_000);
    let midpoint = Tick(duration.0 / 2);
    let mut sequence = base_sequence(sequence_id, "Loading pulse", duration, LoopPolicy::Loop);
    sequence.tracks.extend([
        authored_track(
            &sequence.id,
            kind,
            "scale",
            &target,
            "UiStyle",
            "scale",
            ValueKind::Number,
            Interpolation::Cubic,
            vec![
                (Tick::ZERO, AnimValue::Number(0.96)),
                (midpoint, AnimValue::Number(1.04)),
                (duration, AnimValue::Number(0.96)),
            ],
        ),
        authored_track(
            &sequence.id,
            kind,
            "opacity",
            &target,
            "UiStyle",
            "opacity",
            ValueKind::Number,
            Interpolation::Cubic,
            vec![
                (Tick::ZERO, AnimValue::Number(0.55)),
                (midpoint, AnimValue::Number(1.0)),
                (duration, AnimValue::Number(0.55)),
            ],
        ),
    ]);
    sequence
}

/// A 1.5 s linear camera transform with typed translation and quaternion rotation channels.
#[must_use]
pub fn rigid_camera_move(sequence_id: impl Into<String>, target: impl Into<String>) -> Sequence {
    let kind = AnimationPreset::RigidCameraMove;
    let target = target.into();
    let duration = Tick(90_000);
    let mut sequence = base_sequence(sequence_id, "Rigid camera move", duration, LoopPolicy::Once);
    sequence.tracks.extend([
        authored_track(
            &sequence.id,
            kind,
            "translation",
            &target,
            "Transform",
            "translation",
            ValueKind::Vec3,
            Interpolation::Linear,
            vec![
                (Tick::ZERO, AnimValue::Vec3([0.0, 1.6, 5.0])),
                (duration, AnimValue::Vec3([0.0, 2.2, 3.5])),
            ],
        ),
        authored_track(
            &sequence.id,
            kind,
            "rotation",
            &target,
            "Transform",
            "rotation",
            ValueKind::Quaternion,
            Interpolation::Linear,
            vec![
                (Tick::ZERO, AnimValue::Quaternion([0.0, 0.0, 0.0, 1.0])),
                (
                    duration,
                    AnimValue::Quaternion([
                        -0.087_155_742_747_658_17,
                        0.0,
                        0.0,
                        0.996_194_698_091_745_5,
                    ]),
                ),
            ],
        ),
    ]);
    sequence
}

fn base_sequence(
    sequence_id: impl Into<String>,
    name: &str,
    duration: Tick,
    loop_policy: LoopPolicy,
) -> Sequence {
    let mut sequence = Sequence::new(SequenceId::new(sequence_id), name, duration);
    sequence.loop_policy = loop_policy;
    sequence
}

#[allow(clippy::too_many_arguments)]
fn authored_track(
    sequence_id: &SequenceId,
    kind: AnimationPreset,
    channel: &str,
    target: &str,
    component: &str,
    property: &str,
    value_kind: ValueKind,
    interpolation: Interpolation,
    values: Vec<(Tick, AnimValue)>,
) -> Track {
    let track_id = TrackId::new(format!(
        "{}::preset::{}::track::{channel}",
        sequence_id,
        kind.slug()
    ));
    let keyframes = values
        .into_iter()
        .enumerate()
        .map(|(index, (tick, value))| Keyframe {
            id: KeyId::new(format!("{track_id}::key::{index:02}")),
            tick,
            value,
            in_tangent: None,
            out_tangent: None,
        })
        .collect();
    Track {
        id: track_id,
        name: channel.replace('-', " "),
        binding: Binding {
            path: PropertyPath::new(target, component, property),
            value_kind,
        },
        interpolation,
        keyframes,
        enabled: true,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn every_preset_is_deterministic_editable_and_round_trips() {
        for kind in AnimationPreset::ALL {
            let first = kind.instantiate("sequence:preset", "entity:preview");
            let second = kind.instantiate("sequence:preset", "entity:preview");
            assert_eq!(first, second);
            assert!(first.validate().is_empty(), "{kind:?} must start valid");

            let track_ids: BTreeSet<_> = first.tracks.iter().map(|track| &track.id).collect();
            assert_eq!(track_ids.len(), first.tracks.len());
            for track in &first.tracks {
                let key_ids: BTreeSet<_> = track.keyframes.iter().map(|key| &key.id).collect();
                assert_eq!(key_ids.len(), track.keyframes.len());
            }

            let compiled = first.compile().expect("preset compiles");
            let authored_json = serde_json::to_string(&first).expect("serialize authored preset");
            let authored_round_trip: Sequence =
                serde_json::from_str(&authored_json).expect("deserialize authored preset");
            assert_eq!(first, authored_round_trip);
            assert_eq!(
                compiled.stable_hash,
                authored_round_trip
                    .compile()
                    .expect("round-tripped preset compiles")
                    .stable_hash
            );

            let compiled_bytes = bincode::serialize(&compiled).expect("serialize compiled preset");
            let compiled_round_trip =
                bincode::deserialize(&compiled_bytes).expect("deserialize compiled preset");
            assert_eq!(compiled, compiled_round_trip);
        }
    }

    #[test]
    fn presets_expose_their_expected_typed_flat_channels() {
        let sprite = sprite_loop("sequence:sprite", "entity:sprite");
        assert_eq!(sprite.loop_policy, LoopPolicy::Loop);
        assert_eq!(sprite.tracks[0].binding.value_kind, ValueKind::Integer);
        assert_eq!(sprite.tracks[0].interpolation, Interpolation::Step);
        assert_eq!(sprite.tracks[0].keyframes.len(), 8);

        let fade_in = fade("sequence:fade", "entity:sprite");
        assert_eq!(fade_in.tracks[0].binding.path.property, "opacity");
        assert_eq!(fade_in.tracks[0].binding.value_kind, ValueKind::Number);

        for preset in [
            ui_hover("sequence:hover", "entity:button"),
            ui_click("sequence:click", "entity:button"),
            loading_pulse("sequence:pulse", "entity:spinner"),
        ] {
            assert_eq!(preset.tracks.len(), 2);
            assert!(preset
                .tracks
                .iter()
                .all(|track| track.binding.path.component == "UiStyle"));
        }

        let camera = rigid_camera_move("sequence:camera", "entity:camera");
        assert_eq!(
            camera
                .tracks
                .iter()
                .map(|track| track.binding.value_kind)
                .collect::<Vec<_>>(),
            vec![ValueKind::Vec3, ValueKind::Quaternion]
        );
        assert!(camera
            .validate()
            .iter()
            .all(|issue| issue.code != crate::IssueCode::ConflictingBinding));
    }
}
