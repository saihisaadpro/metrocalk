// Full issue accumulation is intentionally kept in two audit-friendly passes rather than failing fast.
#![allow(clippy::too_many_lines)]

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    AnimValue, CompileError, CompiledSequence, Tick, TimeBase, TimeError, ValueKind,
    SEQUENCE_SCHEMA_VERSION,
};

macro_rules! stable_id {
    ($name:ident) => {
        #[derive(
            Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

stable_id!(SequenceId);
stable_id!(TrackId);
stable_id!(KeyId);
stable_id!(ClipId);
stable_id!(MarkerId);
stable_id!(EventId);

/// A stable, renderer/ECS-independent address. Segments are explicit rather than one magic string so
/// inspectors can present breadcrumbs and re-importers can rebind a target without parsing syntax.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PropertyPath {
    pub target: String,
    pub component: String,
    pub property: String,
    #[serde(default)]
    pub subpath: Vec<String>,
}

impl PropertyPath {
    #[must_use]
    pub fn new(
        target: impl Into<String>,
        component: impl Into<String>,
        property: impl Into<String>,
    ) -> Self {
        Self {
            target: target.into(),
            component: component.into(),
            property: property.into(),
            subpath: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_subpath(mut self, segments: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.subpath = segments.into_iter().map(Into::into).collect();
        self
    }

    #[must_use]
    pub fn display_path(&self) -> String {
        let mut result = format!("{}/{}/{}", self.target, self.component, self.property);
        for segment in &self.subpath {
            result.push('/');
            result.push_str(segment);
        }
        result
    }

    fn is_valid(&self) -> bool {
        [&self.target, &self.component, &self.property]
            .into_iter()
            .chain(self.subpath.iter())
            .all(|segment| !segment.trim().is_empty() && !segment.contains(['/', '\0']))
    }
}

/// A path plus its declared type. Compilation rejects every key that disagrees with this declaration.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Binding {
    pub path: PropertyPath,
    pub value_kind: ValueKind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    Step,
    #[default]
    Linear,
    Cubic,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    /// Stable CRDT/re-import identity. Same-tick conflicts resolve by lexical key ID, never arrival order.
    pub id: KeyId,
    pub tick: Tick,
    pub value: AnimValue,
    /// Incoming derivative in value-units per tick. Missing cubic tangents mean zero/ease tangents.
    #[serde(default)]
    pub in_tangent: Option<AnimValue>,
    /// Outgoing derivative in value-units per tick. Missing cubic tangents mean zero/ease tangents.
    #[serde(default)]
    pub out_tangent: Option<AnimValue>,
}

impl Keyframe {
    #[must_use]
    pub fn new(id: impl Into<KeyId>, tick: Tick, value: AnimValue) -> Self {
        Self {
            id: id.into(),
            tick,
            value,
            in_tangent: None,
            out_tangent: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub name: String,
    pub binding: Binding,
    pub interpolation: Interpolation,
    pub keyframes: Vec<Keyframe>,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

const fn enabled() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopPolicy {
    #[default]
    Once,
    Loop,
    PingPong,
}

/// A non-destructive timeline placement. Nested sequence resolution is intentionally a later orchestration
/// layer; the kernel validates placement and retains the stable source identity without owning an asset store.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    pub name: String,
    #[serde(default)]
    pub source_sequence: Option<SequenceId>,
    pub timeline_start: Tick,
    pub source_start: Tick,
    pub source_end: Tick,
    pub playback_rate: f64,
    pub loop_policy: LoopPolicy,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub id: MarkerId,
    pub name: String,
    pub tick: Tick,
    #[serde(default)]
    pub color: Option<[u8; 4]>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnimationEvent {
    pub id: EventId,
    pub name: String,
    pub tick: Tick,
    #[serde(default)]
    pub payload: Option<AnimValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sequence {
    pub schema_version: u32,
    pub id: SequenceId,
    pub name: String,
    pub time_base: TimeBase,
    pub duration: Tick,
    pub loop_policy: LoopPolicy,
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub clips: Vec<Clip>,
    #[serde(default)]
    pub markers: Vec<Marker>,
    #[serde(default)]
    pub events: Vec<AnimationEvent>,
}

impl Sequence {
    #[must_use]
    pub fn new(id: impl Into<SequenceId>, name: impl Into<String>, duration: Tick) -> Self {
        Self {
            schema_version: SEQUENCE_SCHEMA_VERSION,
            id: id.into(),
            name: name.into(),
            time_base: TimeBase::default(),
            duration,
            loop_policy: LoopPolicy::Once,
            tracks: Vec::new(),
            clips: Vec::new(),
            markers: Vec::new(),
            events: Vec::new(),
        }
    }

    /// Re-express the complete authored sequence in another integer clock while preserving wall time.
    /// Cubic derivatives are rescaled from value/source-tick to value/target-tick; values and stable
    /// identities remain byte-for-byte equivalent. A target clock that would collapse distinct keys,
    /// a positive duration, or a nested clip range fails closed instead of changing authored motion.
    pub fn retimed(&self, target_time_base: TimeBase) -> Result<Self, TimeError> {
        let source_time_base = self.time_base;
        if source_time_base.ticks_per_second == 0 || target_time_base.ticks_per_second == 0 {
            return Err(TimeError::ZeroRate);
        }
        if source_time_base == target_time_base {
            return Ok(self.clone());
        }

        let mut retimed = self.clone();
        retimed.time_base = target_time_base;
        retimed.duration = source_time_base.convert_tick(self.duration, target_time_base)?;
        if self.duration.0 > 0 && retimed.duration == Tick::ZERO {
            return Err(TimeError::PrecisionLoss);
        }
        for track in &mut retimed.tracks {
            let mut converted_ticks = BTreeMap::new();
            for key in &mut track.keyframes {
                let source_tick = key.tick;
                let target_tick = source_time_base.convert_tick(source_tick, target_time_base)?;
                if converted_ticks
                    .insert(target_tick, source_tick)
                    .is_some_and(|previous_source_tick| previous_source_tick != source_tick)
                {
                    return Err(TimeError::PrecisionLoss);
                }
                key.tick = target_tick;
                key.in_tangent = key
                    .in_tangent
                    .as_ref()
                    .map(|value| retime_derivative(value, source_time_base, target_time_base))
                    .transpose()?;
                key.out_tangent = key
                    .out_tangent
                    .as_ref()
                    .map(|value| retime_derivative(value, source_time_base, target_time_base))
                    .transpose()?;
            }
        }
        for clip in &mut retimed.clips {
            let timeline_start =
                source_time_base.convert_tick(clip.timeline_start, target_time_base)?;
            let source_start =
                source_time_base.convert_tick(clip.source_start, target_time_base)?;
            let source_end = source_time_base.convert_tick(clip.source_end, target_time_base)?;
            if clip.source_end > clip.source_start && source_end <= source_start {
                return Err(TimeError::PrecisionLoss);
            }
            clip.timeline_start = timeline_start;
            clip.source_start = source_start;
            clip.source_end = source_end;
        }
        let mut converted_marker_ticks = BTreeMap::new();
        for marker in &mut retimed.markers {
            let source_tick = marker.tick;
            let target_tick = source_time_base.convert_tick(source_tick, target_time_base)?;
            if converted_marker_ticks
                .insert(target_tick, source_tick)
                .is_some_and(|previous_source_tick| previous_source_tick != source_tick)
            {
                return Err(TimeError::PrecisionLoss);
            }
            marker.tick = target_tick;
        }
        let mut converted_event_ticks = BTreeMap::new();
        for event in &mut retimed.events {
            let source_tick = event.tick;
            let target_tick = source_time_base.convert_tick(source_tick, target_time_base)?;
            if converted_event_ticks
                .insert(target_tick, source_tick)
                .is_some_and(|previous_source_tick| previous_source_tick != source_tick)
            {
                return Err(TimeError::PrecisionLoss);
            }
            event.tick = target_tick;
        }
        Ok(retimed)
    }

    /// Returns all errors and warnings in deterministic location/code order.
    #[must_use]
    pub fn validate(&self) -> Vec<ValidationIssue> {
        let mut issues = Vec::new();
        if self.schema_version != SEQUENCE_SCHEMA_VERSION {
            issues.push(issue(
                Severity::Error,
                IssueCode::UnsupportedSchemaVersion,
                "sequence",
                format!(
                    "schema {} is unsupported; migrate to {}",
                    self.schema_version, SEQUENCE_SCHEMA_VERSION
                ),
                "Run the sequence schema migrator before compilation.",
            ));
        }
        if self.id.0.trim().is_empty() {
            issues.push(issue(
                Severity::Error,
                IssueCode::EmptyStableId,
                "sequence.id",
                "sequence ID is empty",
                "Assign a persistent, non-empty sequence ID.",
            ));
        }
        if self.time_base.ticks_per_second == 0 {
            issues.push(issue(
                Severity::Error,
                IssueCode::InvalidTimeBase,
                "sequence.time_base",
                "ticks_per_second is zero",
                "Use a non-zero integer clock such as 60,000 ticks per second.",
            ));
        }
        if self.duration.0 <= 0 {
            issues.push(issue(
                Severity::Error,
                IssueCode::InvalidDuration,
                "sequence.duration",
                "duration must be greater than zero",
                "Set duration to the last authored tick or later.",
            ));
        }

        validate_unique_ids(
            self.tracks.iter().map(|item| item.id.as_str()),
            "tracks",
            &mut issues,
        );
        validate_unique_ids(
            self.clips.iter().map(|item| item.id.as_str()),
            "clips",
            &mut issues,
        );
        validate_unique_ids(
            self.markers.iter().map(|item| item.id.as_str()),
            "markers",
            &mut issues,
        );
        validate_unique_ids(
            self.events.iter().map(|item| item.id.as_str()),
            "events",
            &mut issues,
        );

        for (track_index, track) in self.tracks.iter().enumerate() {
            validate_track(self, track, track_index, &mut issues);
        }
        validate_conflicting_bindings(&self.tracks, &mut issues);
        for (index, clip) in self.clips.iter().enumerate() {
            let location = format!("clips[{index}]");
            if clip.id.0.trim().is_empty() {
                issues.push(empty_id(&format!("{location}.id")));
            }
            if !clip.playback_rate.is_finite() || clip.playback_rate == 0.0 {
                issues.push(issue(
                    Severity::Error,
                    IssueCode::InvalidPlaybackRate,
                    &format!("{location}.playback_rate"),
                    "playback rate must be finite and non-zero",
                    "Use a finite positive or negative playback rate.",
                ));
            }
            if clip.source_start.0 < 0 || clip.source_end <= clip.source_start {
                issues.push(issue(
                    Severity::Error,
                    IssueCode::InvalidClipRange,
                    &location,
                    "clip source range is empty or begins before zero",
                    "Set source_start >= 0 and source_end > source_start.",
                ));
            }
            if clip.timeline_start.0 < 0 || clip.timeline_start > self.duration {
                issues.push(out_of_range(&format!("{location}.timeline_start")));
            }
        }
        for (index, marker) in self.markers.iter().enumerate() {
            if marker.id.0.trim().is_empty() {
                issues.push(empty_id(&format!("markers[{index}].id")));
            }
            if marker.tick.0 < 0 || marker.tick > self.duration {
                issues.push(out_of_range(&format!("markers[{index}].tick")));
            }
        }
        for (index, event) in self.events.iter().enumerate() {
            if event.id.0.trim().is_empty() {
                issues.push(empty_id(&format!("events[{index}].id")));
            }
            if event.name.trim().is_empty() {
                issues.push(issue(
                    Severity::Warning,
                    IssueCode::EmptyEventName,
                    &format!("events[{index}].name"),
                    "event name is empty and will be difficult to route",
                    "Give the event a stable semantic name.",
                ));
            }
            if event.tick.0 < 0 || event.tick > self.duration {
                issues.push(out_of_range(&format!("events[{index}].tick")));
            }
            if event
                .payload
                .as_ref()
                .is_some_and(|value| !value.is_finite())
            {
                issues.push(non_finite(&format!("events[{index}].payload")));
            }
        }
        issues.sort_by(|left, right| {
            left.location
                .cmp(&right.location)
                .then(left.code.cmp(&right.code))
        });
        issues
    }

    /// Validates and produces a stable, lookup-efficient representation.
    pub fn compile(&self) -> Result<CompiledSequence, CompileError> {
        CompiledSequence::compile(self)
    }
}

fn retime_derivative(
    value: &AnimValue,
    source_time_base: TimeBase,
    target_time_base: TimeBase,
) -> Result<AnimValue, TimeError> {
    let scale =
        f64::from(source_time_base.ticks_per_second) / f64::from(target_time_base.ticks_per_second);
    let scaled = |value: f64| {
        let result = value * scale;
        result
            .is_finite()
            .then_some(result)
            .ok_or(TimeError::OutOfRange)
    };
    Ok(match value {
        AnimValue::Number(value) => AnimValue::Number(scaled(*value)?),
        AnimValue::Integer(value) => {
            // Integer cubic derivatives cannot absorb a fractional unit-per-target-tick correction.
            // Reject rather than rounding a Hermite slope and changing the curve shape.
            let numerator = i128::from(*value) * i128::from(source_time_base.ticks_per_second);
            let denominator = i128::from(target_time_base.ticks_per_second);
            if numerator % denominator != 0 {
                return Err(TimeError::PrecisionLoss);
            }
            AnimValue::Integer(
                i64::try_from(numerator / denominator).map_err(|_| TimeError::OutOfRange)?,
            )
        }
        AnimValue::Boolean(value) => AnimValue::Boolean(*value),
        AnimValue::String(value) => AnimValue::String(value.clone()),
        AnimValue::Vec3(value) => {
            AnimValue::Vec3([scaled(value[0])?, scaled(value[1])?, scaled(value[2])?])
        }
        AnimValue::Vec4(value) => AnimValue::Vec4([
            scaled(value[0])?,
            scaled(value[1])?,
            scaled(value[2])?,
            scaled(value[3])?,
        ]),
        AnimValue::Quaternion(value) => AnimValue::Quaternion([
            scaled(value[0])?,
            scaled(value[1])?,
            scaled(value[2])?,
            scaled(value[3])?,
        ]),
        AnimValue::Weights(value) => AnimValue::Weights(
            value
                .iter()
                .map(|part| scaled(*part))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueCode {
    UnsupportedSchemaVersion,
    EmptyStableId,
    DuplicateStableId,
    InvalidTimeBase,
    InvalidDuration,
    InvalidPropertyPath,
    EmptyTrack,
    KeyOutOfRange,
    ValueTypeMismatch,
    NonFiniteValue,
    UnsupportedInterpolation,
    TangentTypeMismatch,
    InvalidQuaternion,
    QuaternionRenormalized,
    InconsistentWeightCount,
    DuplicateKeyTime,
    InvalidPlaybackRate,
    InvalidClipRange,
    EmptyEventName,
    ConflictingBinding,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub severity: Severity,
    pub code: IssueCode,
    pub location: String,
    pub message: String,
    pub remediation: String,
}

fn validate_unique_ids<'a>(
    ids: impl Iterator<Item = &'a str>,
    location: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    let mut observed = BTreeSet::new();
    for (index, id) in ids.enumerate() {
        if id.trim().is_empty() {
            issues.push(empty_id(&format!("{location}[{index}].id")));
        } else if !observed.insert(id) {
            issues.push(issue(
                Severity::Error,
                IssueCode::DuplicateStableId,
                &format!("{location}[{index}].id"),
                format!("stable ID '{id}' is duplicated"),
                "Generate a new persistent ID for one of the conflicting items.",
            ));
        }
    }
}

fn validate_conflicting_bindings(tracks: &[Track], issues: &mut Vec<ValidationIssue>) {
    let mut enabled_by_binding: BTreeMap<&Binding, Vec<&str>> = BTreeMap::new();
    for track in tracks
        .iter()
        .filter(|track| track.enabled && !track.keyframes.is_empty())
    {
        enabled_by_binding
            .entry(&track.binding)
            .or_default()
            .push(track.id.as_str());
    }

    for (binding, mut track_ids) in enabled_by_binding {
        if track_ids.len() < 2 {
            continue;
        }
        track_ids.sort_unstable();
        let display_ids = track_ids
            .iter()
            .map(|id| format!("'{id}'"))
            .collect::<Vec<_>>()
            .join(", ");
        issues.push(issue(
            Severity::Error,
            IssueCode::ConflictingBinding,
            &format!(
                "bindings[{}::{:?}]",
                binding.path.display_path(),
                binding.value_kind
            ),
            format!(
                "enabled flat-sequence tracks {display_ids} target the same binding and would have an implicit last-wins projection"
            ),
            "Disable all but one flat-sequence track, or route the tracks through an explicit animation graph/layer blend before compilation; disabled alternatives may coexist.",
        ));
    }
}

fn validate_track(
    sequence: &Sequence,
    track: &Track,
    track_index: usize,
    issues: &mut Vec<ValidationIssue>,
) {
    let location = format!("tracks[{track_index}]");
    if !track.binding.path.is_valid() {
        issues.push(issue(
            Severity::Error,
            IssueCode::InvalidPropertyPath,
            &format!("{location}.binding.path"),
            "path contains an empty segment, slash, or NUL",
            "Use separate non-empty target/component/property segments.",
        ));
    }
    if track.keyframes.is_empty() {
        issues.push(issue(
            Severity::Warning,
            IssueCode::EmptyTrack,
            &format!("{location}.keyframes"),
            "track has no keyframes and will not produce a value",
            "Add a keyframe or remove the track.",
        ));
        return;
    }
    if matches!(
        track.binding.value_kind,
        ValueKind::Boolean | ValueKind::String
    ) && track.interpolation != Interpolation::Step
    {
        issues.push(issue(
            Severity::Error,
            IssueCode::UnsupportedInterpolation,
            &format!("{location}.interpolation"),
            "boolean and string tracks support step interpolation only",
            "Change interpolation to step.",
        ));
    }

    let mut time_counts = BTreeMap::new();
    let mut key_ids = BTreeSet::new();
    let mut weight_count = None;
    for (key_index, key) in track.keyframes.iter().enumerate() {
        let key_location = format!("{location}.keyframes[{key_index}]");
        if key.id.0.trim().is_empty() {
            issues.push(empty_id(&format!("{key_location}.id")));
        } else if !key_ids.insert(key.id.as_str()) {
            issues.push(issue(
                Severity::Error,
                IssueCode::DuplicateStableId,
                &format!("{key_location}.id"),
                format!("key ID '{}' is duplicated within the track", key.id),
                "Assign a new persistent key ID to one of the conflicting keys.",
            ));
        }
        if key.tick.0 < 0 || key.tick > sequence.duration {
            issues.push(out_of_range(&format!("{key_location}.tick")));
        }
        *time_counts.entry(key.tick).or_insert(0usize) += 1;
        if key.value.kind() != track.binding.value_kind {
            issues.push(issue(
                Severity::Error,
                IssueCode::ValueTypeMismatch,
                &format!("{key_location}.value"),
                format!(
                    "key is {:?}, but binding declares {:?}",
                    key.value.kind(),
                    track.binding.value_kind
                ),
                "Convert the key value or correct the binding declaration.",
            ));
        }
        if !key.value.is_finite() {
            issues.push(non_finite(&format!("{key_location}.value")));
        }
        if let AnimValue::Quaternion(quaternion) = &key.value {
            let length_squared = quaternion.iter().map(|part| part * part).sum::<f64>();
            if !length_squared.is_finite() || length_squared <= 1.0e-24 {
                issues.push(issue(
                    Severity::Error,
                    IssueCode::InvalidQuaternion,
                    &format!("{key_location}.value"),
                    "quaternion has zero or non-finite length",
                    "Author a finite, non-zero rotation quaternion.",
                ));
            } else if (length_squared - 1.0).abs() > 1.0e-6 {
                issues.push(issue(
                    Severity::Warning,
                    IssueCode::QuaternionRenormalized,
                    &format!("{key_location}.value"),
                    "quaternion is not unit length and will be normalized during compilation",
                    "Normalize source rotations to avoid avoidable import drift.",
                ));
            }
        }
        if let AnimValue::Weights(weights) = &key.value {
            if let Some(expected) = weight_count {
                if weights.len() != expected {
                    issues.push(issue(
                        Severity::Error,
                        IssueCode::InconsistentWeightCount,
                        &format!("{key_location}.value"),
                        format!("expected {expected} morph weights, found {}", weights.len()),
                        "Keep a constant morph-target count across the track.",
                    ));
                }
            } else {
                weight_count = Some(weights.len());
            }
        }
        for (tangent_name, tangent) in [
            ("in_tangent", key.in_tangent.as_ref()),
            ("out_tangent", key.out_tangent.as_ref()),
        ] {
            let Some(tangent) = tangent else { continue };
            if tangent.kind() != track.binding.value_kind {
                issues.push(issue(
                    Severity::Error,
                    IssueCode::TangentTypeMismatch,
                    &format!("{key_location}.{tangent_name}"),
                    "tangent type differs from the binding type",
                    "Use a tangent with the same numeric shape as the key value.",
                ));
            }
            if !tangent.is_finite() {
                issues.push(non_finite(&format!("{key_location}.{tangent_name}")));
            }
            if let (AnimValue::Weights(value), Some(expected)) = (tangent, weight_count) {
                if value.len() != expected {
                    issues.push(issue(
                        Severity::Error,
                        IssueCode::InconsistentWeightCount,
                        &format!("{key_location}.{tangent_name}"),
                        "morph tangent width differs from the track",
                        "Keep tangent and key morph-target counts identical.",
                    ));
                }
            }
        }
    }
    for (tick, count) in time_counts {
        if count > 1 {
            issues.push(issue(
                Severity::Warning,
                IssueCode::DuplicateKeyTime,
                &format!("tracks[id={}].tick={}", track.id, tick.0),
                format!(
                    "{count} keys occur at tick {}; the lexically greatest key ID wins exactly at that tick",
                    tick.0
                ),
                "Remove the duplicate unless deterministic same-tick conflict resolution is intended.",
            ));
        }
    }
}

fn issue(
    severity: Severity,
    code: IssueCode,
    location: &str,
    message: impl Into<String>,
    remediation: impl Into<String>,
) -> ValidationIssue {
    ValidationIssue {
        severity,
        code,
        location: location.to_owned(),
        message: message.into(),
        remediation: remediation.into(),
    }
}

fn empty_id(location: &str) -> ValidationIssue {
    issue(
        Severity::Error,
        IssueCode::EmptyStableId,
        location,
        "stable ID is empty",
        "Assign a persistent, non-empty ID.",
    )
}

fn out_of_range(location: &str) -> ValidationIssue {
    issue(
        Severity::Error,
        IssueCode::KeyOutOfRange,
        location,
        "tick is outside the sequence range",
        "Move it into the inclusive range 0..=duration or extend the sequence.",
    )
}

fn non_finite(location: &str) -> ValidationIssue {
    issue(
        Severity::Error,
        IssueCode::NonFiniteValue,
        location,
        "value contains NaN or infinity",
        "Replace every non-finite component with an authored finite value.",
    )
}
