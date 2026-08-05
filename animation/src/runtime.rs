//! Stateful, renderer-independent playback over a compiled animation sequence.
//!
//! The runtime owns clock state and reusable frame/event buffers. It never mutates authored data,
//! commits ECS fields, or knows about a renderer; consumers apply [`AnimationFramePatch`] writes at
//! their own projection boundary.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{
    AnimValue, CompiledSequence, Direction, EventOccurrence, PropertyPath, Tick, TrackId, ValueKind,
};

const NANOSECONDS_PER_SECOND: u128 = 1_000_000_000;

/// Conservative default that prevents a large seek/clock jump from flooding an event consumer.
pub const DEFAULT_RUNTIME_EVENT_LIMIT: usize = 256;

/// Hard output bound even when an untrusted caller requests an excessive per-update event limit.
pub const MAX_RUNTIME_EVENT_LIMIT: usize = 4_096;

/// How [`AnimationRuntimeInstance::update`] advances its raw sequence clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationUpdateMode {
    /// `update` holds the current tick. Call `seek` or `advance` from an editor/simulation clock.
    #[default]
    Manual,
    /// Every `update` advances by this exact integer amount. A negative step plays backwards.
    Fixed { ticks_per_update: Tick },
    /// `update` converts its caller-provided elapsed [`Duration`] through the sequence time base.
    Realtime,
}

/// What should happen to playback position when replacing the immutable compiled plan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationRebindPolicy {
    /// Start the replacement plan at raw tick zero.
    #[default]
    ResetToStart,
    /// Evaluate the replacement plan at the current unwrapped raw tick.
    PreserveRawTick,
}

/// One typed value write. `(path, track_id)` is the stable key, allowing multiple tracks to target
/// the same property without collapsing their identities at the pure runtime boundary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnimationBindingWrite {
    pub path: PropertyPath,
    pub track_id: TrackId,
    pub value_kind: ValueKind,
    pub value: AnimValue,
}

impl AnimationBindingWrite {
    #[must_use]
    pub fn matches(&self, path: &PropertyPath, track_id: &TrackId) -> bool {
        self.path == *path && self.track_id == *track_id
    }
}

/// Reusable sampled state for one raw tick. Event traversal is deliberately exposed separately:
/// this patch is therefore identical for the same compiled plan and raw tick in every clock mode.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnimationFramePatch {
    pub raw_tick: Tick,
    pub local_tick: Tick,
    pub iteration: i64,
    /// Direction of the local loop/ping-pong leg at `local_tick`.
    pub direction: Direction,
    /// Stable ordering inherited from compilation: `(binding, track_id)`.
    pub writes: Vec<AnimationBindingWrite>,
}

impl Default for AnimationFramePatch {
    fn default() -> Self {
        Self {
            raw_tick: Tick::ZERO,
            local_tick: Tick::ZERO,
            iteration: 0,
            direction: Direction::Forward,
            writes: Vec::new(),
        }
    }
}

impl AnimationFramePatch {
    /// Find a write by the complete stable runtime key.
    #[must_use]
    pub fn write(&self, path: &PropertyPath, track_id: &TrackId) -> Option<&AnimationBindingWrite> {
        self.writes
            .iter()
            .find(|write| write.matches(path, track_id))
    }
}

/// Playback state for one immutable compiled sequence.
#[derive(Clone, Debug)]
pub struct AnimationRuntimeInstance {
    plan: CompiledSequence,
    update_mode: AnimationUpdateMode,
    previous_raw_tick: Tick,
    current_raw_tick: Tick,
    realtime_remainder: u128,
    event_limit: usize,
    frame: AnimationFramePatch,
    events: Vec<EventOccurrence>,
    events_truncated: bool,
}

impl AnimationRuntimeInstance {
    /// Create an instance at raw tick zero with the default bounded event limit.
    #[must_use]
    pub fn new(plan: CompiledSequence, update_mode: AnimationUpdateMode) -> Self {
        Self::with_event_limit(plan, update_mode, DEFAULT_RUNTIME_EVENT_LIMIT)
    }

    /// Create an instance with an explicit event output bound. Values above
    /// [`MAX_RUNTIME_EVENT_LIMIT`] are clamped.
    #[must_use]
    pub fn with_event_limit(
        plan: CompiledSequence,
        update_mode: AnimationUpdateMode,
        event_limit: usize,
    ) -> Self {
        let mut instance = Self {
            frame: AnimationFramePatch {
                writes: Vec::with_capacity(plan.tracks_len()),
                ..AnimationFramePatch::default()
            },
            plan,
            update_mode,
            previous_raw_tick: Tick::ZERO,
            current_raw_tick: Tick::ZERO,
            realtime_remainder: 0,
            event_limit: event_limit.min(MAX_RUNTIME_EVENT_LIMIT),
            events: Vec::with_capacity(event_limit.min(MAX_RUNTIME_EVENT_LIMIT)),
            events_truncated: false,
        };
        instance.sample_transition(Tick::ZERO, false);
        instance
    }

    #[must_use]
    pub const fn update_mode(&self) -> AnimationUpdateMode {
        self.update_mode
    }

    /// Switch clocks without changing the raw tick. Fractional realtime carry is reset explicitly.
    pub fn set_update_mode(&mut self, update_mode: AnimationUpdateMode) {
        self.update_mode = update_mode;
        self.realtime_remainder = 0;
    }

    #[must_use]
    pub const fn previous_raw_tick(&self) -> Tick {
        self.previous_raw_tick
    }

    #[must_use]
    pub const fn current_raw_tick(&self) -> Tick {
        self.current_raw_tick
    }

    #[must_use]
    pub const fn event_limit(&self) -> usize {
        self.event_limit
    }

    /// Set the per-transition event output limit. Zero disables delivery while retaining a
    /// truncation signal when the traversed interval contained events.
    pub fn set_event_limit(&mut self, event_limit: usize) {
        self.event_limit = event_limit.min(MAX_RUNTIME_EVENT_LIMIT);
        if self.events.capacity() < self.event_limit {
            self.events
                .reserve(self.event_limit.saturating_sub(self.events.len()));
        }
    }

    #[must_use]
    pub const fn plan(&self) -> &CompiledSequence {
        &self.plan
    }

    #[must_use]
    pub fn frame(&self) -> &AnimationFramePatch {
        &self.frame
    }

    /// Events crossed by the most recent `advance`, Fixed update, or Realtime update.
    #[must_use]
    pub fn events(&self) -> &[EventOccurrence] {
        &self.events
    }

    #[must_use]
    pub const fn events_truncated(&self) -> bool {
        self.events_truncated
    }

    /// Advance according to the configured clock. Manual mode holds and refreshes the current tick;
    /// Fixed mode ignores `elapsed`; Realtime mode converts it using exact integer nanoseconds.
    pub fn update(&mut self, elapsed: Duration) -> &AnimationFramePatch {
        match self.update_mode {
            AnimationUpdateMode::Manual => self.sample_transition(self.current_raw_tick, false),
            AnimationUpdateMode::Fixed { ticks_per_update } => {
                self.realtime_remainder = 0;
                let next = self.current_raw_tick.saturating_add(ticks_per_update);
                self.sample_transition(next, true);
            }
            AnimationUpdateMode::Realtime => {
                let delta = self.realtime_delta(elapsed);
                let next = self.current_raw_tick.saturating_add(delta);
                self.sample_transition(next, true);
            }
        }
        &self.frame
    }

    /// Sample an arbitrary raw tick without dispatching traversal events. This is the editor scrub
    /// path; the old tick is retained as `previous_raw_tick` for inspection.
    pub fn seek(&mut self, raw_tick: Tick) -> &AnimationFramePatch {
        self.realtime_remainder = 0;
        self.sample_transition(raw_tick, false);
        &self.frame
    }

    /// Advance by an exact signed raw-tick delta and collect bounded crossing events.
    pub fn advance(&mut self, delta: Tick) -> &AnimationFramePatch {
        self.realtime_remainder = 0;
        let next = self.current_raw_tick.saturating_add(delta);
        self.sample_transition(next, true);
        &self.frame
    }

    /// Evaluate without changing playback state, event buffers, or the immutable compiled plan.
    #[must_use]
    pub fn preview(&self, raw_tick: Tick) -> AnimationFramePatch {
        let evaluation = self.plan.evaluate(raw_tick);
        patch_from_evaluation(evaluation)
    }

    /// Reset playback and transient buffers while retaining the plan, mode, and event limit.
    pub fn reset(&mut self) -> &AnimationFramePatch {
        self.previous_raw_tick = Tick::ZERO;
        self.current_raw_tick = Tick::ZERO;
        self.realtime_remainder = 0;
        self.sample_transition(Tick::ZERO, false);
        &self.frame
    }

    /// Replace the immutable plan with an explicit position policy. Rebinding never emits events.
    pub fn rebind_plan(
        &mut self,
        plan: CompiledSequence,
        policy: AnimationRebindPolicy,
    ) -> &AnimationFramePatch {
        let raw_tick = match policy {
            AnimationRebindPolicy::ResetToStart => Tick::ZERO,
            AnimationRebindPolicy::PreserveRawTick => self.current_raw_tick,
        };
        self.plan = plan;
        self.previous_raw_tick = raw_tick;
        self.current_raw_tick = raw_tick;
        self.realtime_remainder = 0;
        if self.frame.writes.capacity() < self.plan.tracks_len() {
            self.frame.writes.reserve(
                self.plan
                    .tracks_len()
                    .saturating_sub(self.frame.writes.len()),
            );
        }
        self.sample_transition(raw_tick, false);
        &self.frame
    }

    #[must_use]
    pub fn into_plan(self) -> CompiledSequence {
        self.plan
    }

    fn realtime_delta(&mut self, elapsed: Duration) -> Tick {
        let numerator = elapsed
            .as_nanos()
            .saturating_mul(u128::from(self.plan.time_base.ticks_per_second))
            .saturating_add(self.realtime_remainder);
        let whole_ticks = numerator / NANOSECONDS_PER_SECOND;
        self.realtime_remainder = numerator % NANOSECONDS_PER_SECOND;
        Tick(i64::try_from(whole_ticks).unwrap_or(i64::MAX))
    }

    fn sample_transition(&mut self, raw_tick: Tick, collect_events: bool) {
        let previous = self.current_raw_tick;
        let evaluation = self.plan.evaluate(raw_tick);
        let event_batch = collect_events.then(|| {
            self.plan
                .events_crossed_limited(previous, raw_tick, self.event_limit)
        });

        self.previous_raw_tick = previous;
        self.current_raw_tick = raw_tick;
        self.frame.raw_tick = evaluation.raw_tick;
        self.frame.local_tick = evaluation.local_tick;
        self.frame.iteration = evaluation.iteration;
        self.frame.direction = evaluation.direction;
        self.frame.writes.clear();
        self.frame
            .writes
            .extend(evaluation.bindings.into_iter().map(|evaluated| {
                debug_assert_eq!(evaluated.binding.value_kind, evaluated.value.kind());
                AnimationBindingWrite {
                    path: evaluated.binding.path,
                    track_id: evaluated.track_id,
                    value_kind: evaluated.binding.value_kind,
                    value: evaluated.value,
                }
            }));

        self.events.clear();
        if let Some(event_batch) = event_batch {
            self.events_truncated = event_batch.truncated;
            self.events.extend(event_batch.occurrences);
        } else {
            self.events_truncated = false;
        }
    }
}

fn patch_from_evaluation(evaluation: crate::Evaluation) -> AnimationFramePatch {
    AnimationFramePatch {
        raw_tick: evaluation.raw_tick,
        local_tick: evaluation.local_tick,
        iteration: evaluation.iteration,
        direction: evaluation.direction,
        writes: evaluation
            .bindings
            .into_iter()
            .map(|evaluated| AnimationBindingWrite {
                path: evaluated.binding.path,
                track_id: evaluated.track_id,
                value_kind: evaluated.binding.value_kind,
                value: evaluated.value,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AnimValue, AnimationEvent, Binding, EventId, Interpolation, Keyframe, LoopPolicy,
        PropertyPath, Sequence, TimeBase, Track,
    };

    fn sequence(policy: LoopPolicy, end_value: f64) -> Sequence {
        let mut sequence = Sequence::new("sequence:runtime", "Runtime", Tick(100));
        sequence.time_base = TimeBase::new(100);
        sequence.loop_policy = policy;
        sequence.tracks.push(Track {
            id: TrackId::new("track:x"),
            name: "Position X".into(),
            binding: Binding {
                path: PropertyPath::new("entity:hero", "Transform", "x"),
                value_kind: ValueKind::Number,
            },
            interpolation: Interpolation::Linear,
            keyframes: vec![
                Keyframe::new("key:start", Tick::ZERO, AnimValue::Number(0.0)),
                Keyframe::new("key:end", Tick(100), AnimValue::Number(end_value)),
            ],
            enabled: true,
        });
        sequence.events = vec![
            AnimationEvent {
                id: EventId::new("event:25"),
                name: "Quarter".into(),
                tick: Tick(25),
                payload: Some(AnimValue::Integer(25)),
            },
            AnimationEvent {
                id: EventId::new("event:75"),
                name: "Three quarters".into(),
                tick: Tick(75),
                payload: None,
            },
        ];
        sequence
    }

    fn compiled(policy: LoopPolicy) -> CompiledSequence {
        sequence(policy, 100.0).compile().unwrap()
    }

    #[test]
    fn equivalent_manual_fixed_and_realtime_ticks_produce_identical_patches() {
        let plan = compiled(LoopPolicy::Once);
        let mut manual = AnimationRuntimeInstance::new(plan.clone(), AnimationUpdateMode::Manual);
        let manual_patch = manual.seek(Tick(50)).clone();

        let mut fixed = AnimationRuntimeInstance::new(
            plan.clone(),
            AnimationUpdateMode::Fixed {
                ticks_per_update: Tick(10),
            },
        );
        for _ in 0..5 {
            fixed.update(Duration::ZERO);
        }

        let mut realtime = AnimationRuntimeInstance::new(plan, AnimationUpdateMode::Realtime);
        realtime.update(Duration::from_millis(500));

        assert_eq!(manual_patch, *fixed.frame());
        assert_eq!(manual_patch, *realtime.frame());
        assert_eq!(manual.current_raw_tick(), Tick(50));
        assert_eq!(fixed.previous_raw_tick(), Tick(40));
        assert_eq!(realtime.previous_raw_tick(), Tick::ZERO);
    }

    #[test]
    fn realtime_fractional_ticks_accumulate_without_frame_rounding_drift() {
        let plan = compiled(LoopPolicy::Once);
        let mut many = AnimationRuntimeInstance::new(plan.clone(), AnimationUpdateMode::Realtime);
        for _ in 0..100 {
            many.update(Duration::from_millis(1));
        }
        let mut one = AnimationRuntimeInstance::new(plan, AnimationUpdateMode::Realtime);
        one.update(Duration::from_millis(100));
        assert_eq!(many.current_raw_tick(), Tick(10));
        assert_eq!(many.frame(), one.frame());
    }

    #[test]
    fn typed_writes_use_property_path_and_track_id_as_the_complete_key() {
        let plan = compiled(LoopPolicy::Once);
        let runtime = AnimationRuntimeInstance::new(plan, AnimationUpdateMode::Manual);
        let path = PropertyPath::new("entity:hero", "Transform", "x");
        let write = runtime
            .frame()
            .write(&path, &TrackId::new("track:x"))
            .unwrap();
        assert_eq!(write.value_kind, ValueKind::Number);
        assert_eq!(write.value.kind(), write.value_kind);
        assert!(runtime
            .frame()
            .write(&path, &TrackId::new("track:other"))
            .is_none());
    }

    #[test]
    fn seek_and_preview_do_not_mutate_authored_content_or_plan_hash() {
        let authored = sequence(LoopPolicy::Loop, 100.0);
        let authored_before = authored.clone();
        let plan = authored.compile().unwrap();
        let stable_hash = plan.stable_hash.clone();
        let mut runtime = AnimationRuntimeInstance::new(plan, AnimationUpdateMode::Manual);
        let frame_before = runtime.frame().clone();

        let preview = runtime.preview(Tick(175));
        assert_eq!(runtime.frame(), &frame_before);
        assert_eq!(runtime.current_raw_tick(), Tick::ZERO);
        assert_eq!(preview.local_tick, Tick(75));
        runtime.seek(Tick(175));

        assert_eq!(runtime.plan().stable_hash, stable_hash);
        assert_eq!(authored, authored_before);
        assert_eq!(authored.compile().unwrap().stable_hash, stable_hash);
        assert!(
            runtime.events().is_empty(),
            "scrubbing must not dispatch events"
        );
    }

    #[test]
    fn once_events_cross_forward_and_reverse_with_half_open_endpoints() {
        let mut runtime =
            AnimationRuntimeInstance::new(compiled(LoopPolicy::Once), AnimationUpdateMode::Manual);
        runtime.advance(Tick(75));
        assert_eq!(
            runtime
                .events()
                .iter()
                .map(|event| (event.raw_tick, event.direction))
                .collect::<Vec<_>>(),
            vec![
                (Tick(25), Direction::Forward),
                (Tick(75), Direction::Forward)
            ]
        );
        runtime.advance(Tick(-50));
        assert_eq!(
            runtime
                .events()
                .iter()
                .map(|event| (event.raw_tick, event.direction))
                .collect::<Vec<_>>(),
            vec![(Tick(25), Direction::Reverse)]
        );
        runtime.advance(Tick(-1));
        assert!(
            runtime.events().is_empty(),
            "shared endpoint must not refire"
        );
    }

    #[test]
    fn loop_events_are_bounded_and_ordered_in_both_traversal_directions() {
        let mut runtime = AnimationRuntimeInstance::with_event_limit(
            compiled(LoopPolicy::Loop),
            AnimationUpdateMode::Manual,
            3,
        );
        runtime.advance(Tick(230));
        assert_eq!(
            runtime
                .events()
                .iter()
                .map(|event| event.raw_tick)
                .collect::<Vec<_>>(),
            vec![Tick(25), Tick(75), Tick(125)]
        );
        assert!(runtime.events_truncated());

        runtime.set_event_limit(MAX_RUNTIME_EVENT_LIMIT);
        runtime.advance(Tick(-230));
        assert_eq!(
            runtime
                .events()
                .iter()
                .map(|event| event.raw_tick)
                .collect::<Vec<_>>(),
            vec![Tick(225), Tick(175), Tick(125), Tick(75), Tick(25)]
        );
        assert!(runtime
            .events()
            .iter()
            .all(|event| event.direction == Direction::Reverse));
    }

    #[test]
    fn ping_pong_events_report_local_leg_direction_forward_and_reverse() {
        let mut runtime = AnimationRuntimeInstance::new(
            compiled(LoopPolicy::PingPong),
            AnimationUpdateMode::Manual,
        );
        runtime.advance(Tick(180));
        assert_eq!(
            runtime
                .events()
                .iter()
                .map(|event| (event.raw_tick, event.direction))
                .collect::<Vec<_>>(),
            vec![
                (Tick(25), Direction::Forward),
                (Tick(75), Direction::Forward),
                (Tick(125), Direction::Reverse),
                (Tick(175), Direction::Reverse),
            ]
        );
        runtime.advance(Tick(-180));
        assert_eq!(
            runtime
                .events()
                .iter()
                .map(|event| (event.raw_tick, event.direction))
                .collect::<Vec<_>>(),
            vec![
                (Tick(175), Direction::Forward),
                (Tick(125), Direction::Forward),
                (Tick(75), Direction::Reverse),
                (Tick(25), Direction::Reverse),
            ]
        );
    }

    #[test]
    fn reset_and_rebind_position_policy_are_explicit() {
        let first = compiled(LoopPolicy::Loop);
        let replacement = sequence(LoopPolicy::Loop, 200.0).compile().unwrap();
        let mut runtime = AnimationRuntimeInstance::new(first, AnimationUpdateMode::Manual);
        runtime.advance(Tick(75));
        runtime.rebind_plan(replacement.clone(), AnimationRebindPolicy::PreserveRawTick);
        assert_eq!(runtime.current_raw_tick(), Tick(75));
        assert_eq!(runtime.previous_raw_tick(), Tick(75));
        assert_eq!(runtime.frame().writes[0].value, AnimValue::Number(150.0));
        assert!(runtime.events().is_empty());

        runtime.rebind_plan(replacement, AnimationRebindPolicy::ResetToStart);
        assert_eq!(runtime.current_raw_tick(), Tick::ZERO);
        assert_eq!(runtime.previous_raw_tick(), Tick::ZERO);
        runtime.advance(Tick(25));
        runtime.reset();
        assert_eq!(runtime.current_raw_tick(), Tick::ZERO);
        assert_eq!(runtime.previous_raw_tick(), Tick::ZERO);
        assert!(runtime.events().is_empty());
    }

    #[test]
    fn frame_and_event_buffers_retain_capacity_across_updates() {
        let mut runtime = AnimationRuntimeInstance::with_event_limit(
            compiled(LoopPolicy::Loop),
            AnimationUpdateMode::Manual,
            8,
        );
        runtime.advance(Tick(380));
        let write_capacity = runtime.frame.writes.capacity();
        let event_capacity = runtime.events.capacity();
        runtime.advance(Tick(-350));
        assert_eq!(runtime.frame.writes.capacity(), write_capacity);
        assert_eq!(runtime.events.capacity(), event_capacity);
    }

    #[test]
    fn event_limit_is_always_hard_bounded() {
        let mut runtime = AnimationRuntimeInstance::with_event_limit(
            compiled(LoopPolicy::Loop),
            AnimationUpdateMode::Manual,
            usize::MAX,
        );
        assert_eq!(runtime.event_limit(), MAX_RUNTIME_EVENT_LIMIT);
        runtime.set_event_limit(1);
        runtime.advance(Tick(1_000));
        assert_eq!(runtime.events().len(), 1);
        assert!(runtime.events_truncated());
    }
}
