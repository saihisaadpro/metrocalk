// Segment interpolation intentionally converts validated integer ticks and integer property values to f64;
// integer outputs use the documented nearest-integer policy.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    AnimValue, AnimationEvent, Binding, BindingSignature, Clip, Interpolation, Keyframe,
    LoopPolicy, Marker, Sequence, SequenceId, Severity, Tick, TimeBase, TrackId, ValidationIssue,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Forward,
    Reverse,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluatedBinding {
    pub track_id: TrackId,
    pub binding: Binding,
    pub value: AnimValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Evaluation {
    pub raw_tick: Tick,
    pub local_tick: Tick,
    pub iteration: i64,
    pub direction: Direction,
    /// Stable ordering by `(binding, track_id)`.
    pub bindings: Vec<EvaluatedBinding>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventOccurrence {
    pub event: AnimationEvent,
    /// Unwrapped sequence tick at which this occurrence was crossed.
    pub raw_tick: Tick,
    pub local_tick: Tick,
    /// Local sequence motion while the caller traverses the requested interval.
    pub direction: Direction,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventBatch {
    pub occurrences: Vec<EventOccurrence>,
    /// True when `events_crossed_limited` stopped at the caller-provided safety bound.
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompiledSequence {
    pub id: SequenceId,
    pub time_base: TimeBase,
    pub duration: Tick,
    pub loop_policy: LoopPolicy,
    pub stable_hash: String,
    /// Warnings retained for UX surfaces after successful compilation.
    pub diagnostics: Vec<ValidationIssue>,
    tracks: Vec<CompiledTrack>,
    clips: Vec<Clip>,
    markers: Vec<Marker>,
    events: Vec<AnimationEvent>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct CompiledTrack {
    id: TrackId,
    binding: Binding,
    interpolation: Interpolation,
    keys: Vec<Keyframe>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompileError {
    pub issues: Vec<ValidationIssue>,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let errors = self
            .issues
            .iter()
            .filter(|issue| issue.severity == Severity::Error)
            .count();
        write!(
            formatter,
            "animation compilation failed with {errors} error(s)"
        )
    }
}

impl std::error::Error for CompileError {}

impl CompiledSequence {
    pub(crate) fn compile(sequence: &Sequence) -> Result<Self, CompileError> {
        let diagnostics = sequence.validate();
        if diagnostics
            .iter()
            .any(|issue| issue.severity == Severity::Error)
        {
            return Err(CompileError {
                issues: diagnostics,
            });
        }

        let mut tracks: Vec<_> = sequence
            .tracks
            .iter()
            .filter(|track| track.enabled && !track.keyframes.is_empty())
            .map(|track| {
                let mut keys = track.keyframes.clone();
                for key in &mut keys {
                    key.value = key.value.clone().canonicalize();
                }
                // KeyId is the CRDT convergence tie-break for same-tick keys.
                keys.sort_by(|left, right| left.tick.cmp(&right.tick).then(left.id.cmp(&right.id)));
                CompiledTrack {
                    id: track.id.clone(),
                    binding: track.binding.clone(),
                    interpolation: track.interpolation,
                    keys,
                }
            })
            .collect();
        tracks.sort_by(|left, right| {
            left.binding
                .cmp(&right.binding)
                .then(left.id.cmp(&right.id))
        });

        let mut clips: Vec<_> = sequence
            .clips
            .iter()
            .filter(|clip| clip.enabled)
            .cloned()
            .collect();
        clips.sort_by(|left, right| {
            left.timeline_start
                .cmp(&right.timeline_start)
                .then(left.id.cmp(&right.id))
        });
        let mut markers = sequence.markers.clone();
        markers.sort_by(|left, right| left.tick.cmp(&right.tick).then(left.id.cmp(&right.id)));
        let mut events = sequence.events.clone();
        for event in &mut events {
            if let Some(payload) = event.payload.take() {
                event.payload = Some(payload.canonicalize());
            }
        }
        events.sort_by(|left, right| left.tick.cmp(&right.tick).then(left.id.cmp(&right.id)));

        let mut compiled = Self {
            id: sequence.id.clone(),
            time_base: sequence.time_base,
            duration: sequence.duration,
            loop_policy: sequence.loop_policy,
            stable_hash: String::new(),
            diagnostics,
            tracks,
            clips,
            markers,
            events,
        };
        compiled.stable_hash = compiled.compute_hash();
        Ok(compiled)
    }

    #[must_use]
    pub fn tracks_len(&self) -> usize {
        self.tracks.len()
    }

    /// Typed binding catalog retained by this immutable plan, without sampling at an arbitrary tick.
    #[must_use]
    pub fn binding_signature(&self) -> BindingSignature {
        BindingSignature::with_element_counts(
            self.tracks.iter().map(|track| track.binding.clone()),
            self.tracks
                .iter()
                .filter_map(|track| {
                    track.keys.first().and_then(|key| match &key.value {
                        AnimValue::Weights(values) => u32::try_from(values.len())
                            .ok()
                            .map(|count| (track.binding.clone(), count)),
                        _ => None,
                    })
                })
                .collect(),
        )
    }

    #[must_use]
    pub fn clips(&self) -> &[Clip] {
        &self.clips
    }

    #[must_use]
    pub fn markers(&self) -> &[Marker] {
        &self.markers
    }

    #[must_use]
    pub fn events(&self) -> &[AnimationEvent] {
        &self.events
    }

    /// Samples all enabled non-empty tracks. The result order is invariant to authored vector order.
    #[must_use]
    pub fn evaluate(&self, raw_tick: Tick) -> Evaluation {
        let (local_tick, iteration, direction) = self.map_tick(raw_tick);
        let bindings = self
            .tracks
            .iter()
            .filter_map(|track| {
                sample_track(track, local_tick).map(|value| EvaluatedBinding {
                    track_id: track.id.clone(),
                    binding: track.binding.clone(),
                    value,
                })
            })
            .collect();
        Evaluation {
            raw_tick,
            local_tick,
            iteration,
            direction,
            bindings,
        }
    }

    /// Events crossed over a half-open traversal interval: forward `(from, to]`, reverse `[to, from)`.
    /// This convention prevents double firing when adjacent frames share an endpoint.
    #[must_use]
    pub fn events_crossed(&self, from: Tick, to: Tick) -> Vec<EventOccurrence> {
        self.events_crossed_limited(from, to, usize::MAX)
            .occurrences
    }

    /// Bounded event collection for untrusted or extremely large timeline jumps.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn events_crossed_limited(&self, from: Tick, to: Tick, limit: usize) -> EventBatch {
        if from == to || limit == 0 {
            return EventBatch {
                occurrences: Vec::new(),
                truncated: limit == 0 && from != to && !self.events.is_empty(),
            };
        }
        let traversal = if to > from {
            Direction::Forward
        } else {
            Direction::Reverse
        };
        // One extra candidate per periodic series proves truncation without enumerating an untrusted jump.
        let series_limit = limit.saturating_add(1);
        let mut candidates = Vec::new();
        let mut observed = BTreeSet::new();
        for event in &self.events {
            match self.loop_policy {
                LoopPolicy::Once => {
                    if crossed(
                        i128::from(from.0),
                        i128::from(to.0),
                        i128::from(event.tick.0),
                    ) {
                        push_occurrence(
                            &mut candidates,
                            &mut observed,
                            event,
                            event.tick.0,
                            event.tick,
                            traversal,
                        );
                    }
                }
                LoopPolicy::Loop => {
                    add_periodic_occurrences(
                        &mut candidates,
                        &mut observed,
                        event,
                        from,
                        to,
                        self.duration.0,
                        event.tick.0,
                        event.tick,
                        traversal,
                        series_limit,
                    );
                }
                LoopPolicy::PingPong => {
                    let period = i128::from(self.duration.0) * 2;
                    let forward_offset = i128::from(event.tick.0);
                    let reverse_offset = period - forward_offset;
                    add_periodic_occurrences_i128(
                        &mut candidates,
                        &mut observed,
                        event,
                        from,
                        to,
                        period,
                        forward_offset,
                        event.tick,
                        if traversal == Direction::Forward {
                            Direction::Forward
                        } else {
                            Direction::Reverse
                        },
                        series_limit,
                    );
                    if reverse_offset != forward_offset && reverse_offset != period {
                        add_periodic_occurrences_i128(
                            &mut candidates,
                            &mut observed,
                            event,
                            from,
                            to,
                            period,
                            reverse_offset,
                            event.tick,
                            if traversal == Direction::Forward {
                                Direction::Reverse
                            } else {
                                Direction::Forward
                            },
                            series_limit,
                        );
                    }
                }
            }
        }
        candidates.sort_by(|left, right| {
            let tick_order = left.raw_tick.cmp(&right.raw_tick);
            let tick_order = if traversal == Direction::Forward {
                tick_order
            } else {
                tick_order.reverse()
            };
            tick_order.then(left.event.id.cmp(&right.event.id))
        });
        let truncated = candidates.len() > limit;
        candidates.truncate(limit);
        EventBatch {
            occurrences: candidates,
            truncated,
        }
    }

    fn map_tick(&self, raw_tick: Tick) -> (Tick, i64, Direction) {
        let duration = self.duration.0;
        match self.loop_policy {
            LoopPolicy::Once => (Tick(raw_tick.0.clamp(0, duration)), 0, Direction::Forward),
            LoopPolicy::Loop => (
                Tick(raw_tick.0.rem_euclid(duration)),
                raw_tick.0.div_euclid(duration),
                Direction::Forward,
            ),
            LoopPolicy::PingPong => {
                let period = duration.saturating_mul(2);
                let phase = raw_tick.0.rem_euclid(period);
                let iteration = raw_tick.0.div_euclid(period);
                if phase < duration {
                    (Tick(phase), iteration, Direction::Forward)
                } else {
                    (Tick(period - phase), iteration, Direction::Reverse)
                }
            }
        }
    }

    fn compute_hash(&self) -> String {
        let mut hash = StableHasher::new();
        hash.string(self.id.as_str());
        hash.u64(u64::from(self.time_base.ticks_per_second));
        hash.i64(self.duration.0);
        hash.u8(loop_tag(self.loop_policy));
        hash.u64(self.tracks.len() as u64);
        for track in &self.tracks {
            hash.string(track.id.as_str());
            hash.string(&track.binding.path.display_path());
            hash.u8(track.binding.value_kind as u8);
            hash.u8(interpolation_tag(track.interpolation));
            hash.u64(track.keys.len() as u64);
            for key in &track.keys {
                hash.string(key.id.as_str());
                hash.i64(key.tick.0);
                hash.value(&key.value);
                hash.optional_value(key.in_tangent.as_ref());
                hash.optional_value(key.out_tangent.as_ref());
            }
        }
        hash.u64(self.clips.len() as u64);
        for clip in &self.clips {
            hash.string(clip.id.as_str());
            hash.i64(clip.timeline_start.0);
            hash.i64(clip.source_start.0);
            hash.i64(clip.source_end.0);
            hash.f64(clip.playback_rate);
            hash.u8(loop_tag(clip.loop_policy));
            hash.string(clip.source_sequence.as_ref().map_or("", SequenceId::as_str));
        }
        hash.u64(self.markers.len() as u64);
        for marker in &self.markers {
            hash.string(marker.id.as_str());
            hash.i64(marker.tick.0);
            hash.string(&marker.name);
        }
        hash.u64(self.events.len() as u64);
        for event in &self.events {
            hash.string(event.id.as_str());
            hash.i64(event.tick.0);
            hash.string(&event.name);
            hash.optional_value(event.payload.as_ref());
        }
        format!("mtkanim:{:032x}", hash.finish())
    }
}

fn sample_track(track: &CompiledTrack, tick: Tick) -> Option<AnimValue> {
    let keys = &track.keys;
    if keys.is_empty() {
        return None;
    }
    // upper_bound: same-tick keys are sorted by KeyId, so the lexically greatest key wins.
    let upper = keys.partition_point(|key| key.tick <= tick);
    if upper == 0 {
        return Some(keys[0].value.clone());
    }
    let left = &keys[upper - 1];
    let Some(right) = keys.get(upper) else {
        return Some(left.value.clone());
    };
    if left.tick == tick || right.tick == left.tick || track.interpolation == Interpolation::Step {
        return Some(left.value.clone());
    }
    let span = (right.tick.0 - left.tick.0) as f64;
    let factor = (tick.0 - left.tick.0) as f64 / span;
    Some(match track.interpolation {
        Interpolation::Step => left.value.clone(),
        Interpolation::Linear => interpolate_linear(&left.value, &right.value, factor),
        Interpolation::Cubic => interpolate_cubic(left, right, factor, span),
    })
}

fn interpolate_linear(left: &AnimValue, right: &AnimValue, factor: f64) -> AnimValue {
    match (left, right) {
        (AnimValue::Number(a), AnimValue::Number(b)) => AnimValue::Number(lerp(*a, *b, factor)),
        (AnimValue::Integer(a), AnimValue::Integer(b)) => {
            AnimValue::Integer(lerp(*a as f64, *b as f64, factor).round() as i64)
        }
        (AnimValue::Vec3(a), AnimValue::Vec3(b)) => AnimValue::Vec3(std::array::from_fn(|index| {
            lerp(a[index], b[index], factor)
        })),
        (AnimValue::Vec4(a), AnimValue::Vec4(b)) => AnimValue::Vec4(std::array::from_fn(|index| {
            lerp(a[index], b[index], factor)
        })),
        (AnimValue::Quaternion(a), AnimValue::Quaternion(b)) => {
            AnimValue::Quaternion(quaternion_slerp(*a, *b, factor))
        }
        (AnimValue::Weights(a), AnimValue::Weights(b)) => AnimValue::Weights(
            a.iter()
                .zip(b)
                .map(|(left, right)| lerp(*left, *right, factor))
                .collect(),
        ),
        _ => left.clone(),
    }
}

fn interpolate_cubic(left: &Keyframe, right: &Keyframe, factor: f64, span: f64) -> AnimValue {
    let h00 = 2.0 * factor.powi(3) - 3.0 * factor.powi(2) + 1.0;
    let h10 = factor.powi(3) - 2.0 * factor.powi(2) + factor;
    let h01 = -2.0 * factor.powi(3) + 3.0 * factor.powi(2);
    let h11 = factor.powi(3) - factor.powi(2);
    let scalar = |p0: f64, p1: f64, m0: f64, m1: f64| {
        h00 * p0 + h10 * span * m0 + h01 * p1 + h11 * span * m1
    };
    match (&left.value, &right.value) {
        (AnimValue::Number(a), AnimValue::Number(b)) => AnimValue::Number(scalar(
            *a,
            *b,
            number_tangent(left.out_tangent.as_ref()),
            number_tangent(right.in_tangent.as_ref()),
        )),
        (AnimValue::Integer(a), AnimValue::Integer(b)) => AnimValue::Integer(
            scalar(
                *a as f64,
                *b as f64,
                integer_tangent(left.out_tangent.as_ref()),
                integer_tangent(right.in_tangent.as_ref()),
            )
            .round() as i64,
        ),
        (AnimValue::Vec3(a), AnimValue::Vec3(b)) => {
            let m0 = vec3_tangent(left.out_tangent.as_ref());
            let m1 = vec3_tangent(right.in_tangent.as_ref());
            AnimValue::Vec3(std::array::from_fn(|index| {
                scalar(a[index], b[index], m0[index], m1[index])
            }))
        }
        (AnimValue::Vec4(a), AnimValue::Vec4(b)) => {
            let m0 = vec4_tangent(left.out_tangent.as_ref());
            let m1 = vec4_tangent(right.in_tangent.as_ref());
            AnimValue::Vec4(std::array::from_fn(|index| {
                scalar(a[index], b[index], m0[index], m1[index])
            }))
        }
        (AnimValue::Quaternion(a), AnimValue::Quaternion(b)) => {
            // glTF CUBICSPLINE is component-wise Hermite over the AUTHORED quaternion components,
            // followed by normalization. Unlike LINEAR quaternion interpolation, endpoint hemisphere
            // flipping is forbidden here: changing an endpoint sign (or its tangent sign) changes the
            // authored polynomial curve even though q and -q represent the same endpoint orientation.
            let incoming = vec4_tangent(right.in_tangent.as_ref());
            let outgoing = vec4_tangent(left.out_tangent.as_ref());
            let value = std::array::from_fn(|index| {
                scalar(a[index], b[index], outgoing[index], incoming[index])
            });
            AnimValue::Quaternion(normalize4(value))
        }
        (AnimValue::Weights(a), AnimValue::Weights(b)) => {
            let m0 = weights_tangent(left.out_tangent.as_ref(), a.len());
            let m1 = weights_tangent(right.in_tangent.as_ref(), a.len());
            AnimValue::Weights(
                a.iter()
                    .zip(b)
                    .enumerate()
                    .map(|(index, (a, b))| scalar(*a, *b, m0[index], m1[index]))
                    .collect(),
            )
        }
        _ => left.value.clone(),
    }
}

const fn lerp(left: f64, right: f64, factor: f64) -> f64 {
    left + (right - left) * factor
}

fn dot4(left: [f64; 4], right: [f64; 4]) -> f64 {
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}

fn normalize4(value: [f64; 4]) -> [f64; 4] {
    let length = value.iter().map(|part| part * part).sum::<f64>().sqrt();
    if length <= 1.0e-12 {
        [0.0, 0.0, 0.0, 1.0]
    } else {
        value.map(|part| part / length)
    }
}

/// Shortest-arc spherical interpolation for generic LINEAR quaternion tracks. The near-parallel branch
/// avoids dividing by a vanishing sine and remains normalized; both endpoints are normalized defensively.
fn quaternion_slerp(left: [f64; 4], mut right: [f64; 4], factor: f64) -> [f64; 4] {
    let left = normalize4(left);
    right = normalize4(right);
    let mut cosine = dot4(left, right).clamp(-1.0, 1.0);
    if cosine < 0.0 {
        right = right.map(|part| -part);
        cosine = -cosine;
    }
    if cosine > 0.999_5 {
        return normalize4(std::array::from_fn(|index| {
            lerp(left[index], right[index], factor)
        }));
    }
    let angle = cosine.acos();
    let sine = angle.sin();
    if sine.abs() <= 1.0e-12 {
        return normalize4(std::array::from_fn(|index| {
            lerp(left[index], right[index], factor)
        }));
    }
    let left_weight = ((1.0 - factor) * angle).sin() / sine;
    let right_weight = (factor * angle).sin() / sine;
    normalize4(std::array::from_fn(|index| {
        left[index] * left_weight + right[index] * right_weight
    }))
}

fn number_tangent(value: Option<&AnimValue>) -> f64 {
    match value {
        Some(AnimValue::Number(value)) => *value,
        _ => 0.0,
    }
}

fn integer_tangent(value: Option<&AnimValue>) -> f64 {
    match value {
        Some(AnimValue::Integer(value)) => *value as f64,
        _ => 0.0,
    }
}

fn vec3_tangent(value: Option<&AnimValue>) -> [f64; 3] {
    match value {
        Some(AnimValue::Vec3(value)) => *value,
        _ => [0.0; 3],
    }
}

fn vec4_tangent(value: Option<&AnimValue>) -> [f64; 4] {
    match value {
        Some(AnimValue::Vec4(value) | AnimValue::Quaternion(value)) => *value,
        _ => [0.0; 4],
    }
}

fn weights_tangent(value: Option<&AnimValue>, length: usize) -> Vec<f64> {
    match value {
        Some(AnimValue::Weights(value)) => value.clone(),
        _ => vec![0.0; length],
    }
}

fn crossed(from: i128, to: i128, occurrence: i128) -> bool {
    if to > from {
        from < occurrence && occurrence <= to
    } else {
        to <= occurrence && occurrence < from
    }
}

#[allow(clippy::too_many_arguments)]
fn add_periodic_occurrences(
    candidates: &mut Vec<EventOccurrence>,
    observed: &mut BTreeSet<(String, i64)>,
    event: &AnimationEvent,
    from: Tick,
    to: Tick,
    period: i64,
    offset: i64,
    local_tick: Tick,
    direction: Direction,
    series_limit: usize,
) {
    add_periodic_occurrences_i128(
        candidates,
        observed,
        event,
        from,
        to,
        i128::from(period),
        i128::from(offset),
        local_tick,
        direction,
        series_limit,
    );
}

#[allow(clippy::too_many_arguments)]
fn add_periodic_occurrences_i128(
    candidates: &mut Vec<EventOccurrence>,
    observed: &mut BTreeSet<(String, i64)>,
    event: &AnimationEvent,
    from: Tick,
    to: Tick,
    period: i128,
    offset: i128,
    local_tick: Tick,
    direction: Direction,
    series_limit: usize,
) {
    let low = i128::from(from.0.min(to.0));
    let high = i128::from(from.0.max(to.0));
    let first = ceil_div(low - offset, period);
    let last = (high - offset).div_euclid(period);
    let visit = |cycle: i128,
                 candidates: &mut Vec<EventOccurrence>,
                 observed: &mut BTreeSet<(String, i64)>| {
        let occurrence = cycle * period + offset;
        if crossed(i128::from(from.0), i128::from(to.0), occurrence) {
            if let Ok(raw_tick) = i64::try_from(occurrence) {
                push_occurrence(candidates, observed, event, raw_tick, local_tick, direction);
            }
        }
    };
    if to > from {
        for cycle in (first..=last).take(series_limit) {
            visit(cycle, candidates, observed);
        }
    } else {
        for cycle in (first..=last).rev().take(series_limit) {
            visit(cycle, candidates, observed);
        }
    }
}

fn ceil_div(numerator: i128, denominator: i128) -> i128 {
    -(-numerator).div_euclid(denominator)
}

fn push_occurrence(
    candidates: &mut Vec<EventOccurrence>,
    observed: &mut BTreeSet<(String, i64)>,
    event: &AnimationEvent,
    raw_tick: i64,
    local_tick: Tick,
    direction: Direction,
) {
    if observed.insert((event.id.0.clone(), raw_tick)) {
        candidates.push(EventOccurrence {
            event: event.clone(),
            raw_tick: Tick(raw_tick),
            local_tick,
            direction,
        });
    }
}

const fn loop_tag(policy: LoopPolicy) -> u8 {
    match policy {
        LoopPolicy::Once => 0,
        LoopPolicy::Loop => 1,
        LoopPolicy::PingPong => 2,
    }
}

const fn interpolation_tag(interpolation: Interpolation) -> u8 {
    match interpolation {
        Interpolation::Step => 0,
        Interpolation::Linear => 1,
        Interpolation::Cubic => 2,
    }
}

struct StableHasher(u128);

impl StableHasher {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

    const fn new() -> Self {
        Self(Self::OFFSET)
    }

    const fn finish(self) -> u128 {
        self.0
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u128::from(*byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    fn u8(&mut self, value: u8) {
        self.bytes(&[value]);
    }

    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn i64(&mut self, value: i64) {
        self.bytes(&value.to_le_bytes());
    }

    fn f64(&mut self, value: f64) {
        let canonical = if value == 0.0 { 0.0 } else { value };
        self.u64(canonical.to_bits());
    }

    fn string(&mut self, value: &str) {
        self.u64(value.len() as u64);
        self.bytes(value.as_bytes());
    }

    fn optional_value(&mut self, value: Option<&AnimValue>) {
        self.u8(u8::from(value.is_some()));
        if let Some(value) = value {
            self.value(value);
        }
    }

    fn value(&mut self, value: &AnimValue) {
        self.u8(value.kind() as u8);
        match value {
            AnimValue::Number(value) => self.f64(*value),
            AnimValue::Integer(value) => self.i64(*value),
            AnimValue::Boolean(value) => self.u8(u8::from(*value)),
            AnimValue::String(value) => self.string(value),
            AnimValue::Vec3(value) => {
                for value in value {
                    self.f64(*value);
                }
            }
            AnimValue::Vec4(value) | AnimValue::Quaternion(value) => {
                for value in value {
                    self.f64(*value);
                }
            }
            AnimValue::Weights(value) => {
                self.u64(value.len() as u64);
                for value in value {
                    self.f64(*value);
                }
            }
        }
    }
}
