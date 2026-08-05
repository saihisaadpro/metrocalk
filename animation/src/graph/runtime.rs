#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::too_many_lines
)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{AnimValue, AnimationEvent, Binding, Direction, SequenceId, Tick};

use super::{
    CompiledAnimationGraph, CompiledCondition, CompiledNodeKind, CompiledStateMachine,
    CompiledWeight, GraphConditionTest, GraphEventPolicy, GraphInterruptionPolicy, GraphNodeId,
    GraphOutsideHullMode, GraphParameterId, GraphParameterKind, GraphParameterValue, GraphStateId,
    GraphTransitionCurve, GraphTransitionId, RationalRate, MAX_BLEND_2D_COORDINATE_ABS,
};

pub const DEFAULT_GRAPH_EVENT_LIMIT: usize = 256;
pub const MAX_GRAPH_EVENT_LIMIT: usize = 4_096;
pub const MAX_GRAPH_INPUT_TRACE: usize = 16_384;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphParameterSnapshot {
    pub id: GraphParameterId,
    pub value: GraphParameterValue,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphParameterError {
    UnknownParameter,
    TypeMismatch,
    NonFinite,
    NegativeWeight,
    TriggerRequiresFire,
}

impl std::fmt::Display for GraphParameterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UnknownParameter => "animation graph parameter does not exist",
            Self::TypeMismatch => "animation graph parameter type does not match",
            Self::NonFinite => "animation graph number parameter must be finite",
            Self::NegativeWeight => "a parameter used as a graph weight cannot be negative",
            Self::TriggerRequiresFire => "trigger parameters must be fired, not assigned",
        })
    }
}

impl std::error::Error for GraphParameterError {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphBindingValue {
    pub binding: Binding,
    pub value: AnimValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnimationGraphFrame {
    pub raw_tick: Tick,
    /// Stable binding order inherited from the compiled reference signature.
    pub values: Vec<GraphBindingValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphEventOccurrence {
    pub source_node: GraphNodeId,
    pub sequence_id: SequenceId,
    pub event: AnimationEvent,
    pub source_raw_tick: Tick,
    pub source_local_tick: Tick,
    pub direction: Direction,
    pub contribution_weight: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphNodeTrace {
    pub node_id: GraphNodeId,
    pub evaluated: bool,
    pub contribution_weight: f64,
    #[serde(default)]
    pub local_tick: Option<Tick>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphInputTrace {
    pub from: GraphNodeId,
    pub to: GraphNodeId,
    pub contribution_weight: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphTransitionTrace {
    pub id: GraphTransitionId,
    pub from: GraphStateId,
    pub to: GraphStateId,
    pub elapsed: Tick,
    pub duration: Tick,
    pub linear_progress: f64,
    pub curved_progress: f64,
    pub source_is_captured_composite: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphStateTrace {
    pub current_state: Option<GraphStateId>,
    pub state_elapsed: Tick,
    #[serde(default)]
    pub transition: Option<GraphTransitionTrace>,
    pub transitions_taken_this_update: usize,
    pub transition_limit_reached: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationGraphRebindPolicy {
    #[default]
    Reset,
    PreserveState,
}

#[derive(Clone, Debug)]
struct NodeBuffer {
    values: Vec<AnimValue>,
    clip_weights: Vec<f64>,
    input_weights: Vec<(usize, f64)>,
    local_tick: Option<Tick>,
}

#[derive(Clone, Debug)]
struct EvaluationWorkspace {
    nodes: Vec<NodeBuffer>,
}

impl EvaluationWorkspace {
    fn new(plan: &CompiledAnimationGraph) -> Self {
        Self {
            nodes: plan
                .nodes
                .iter()
                .map(|_| NodeBuffer {
                    values: plan.reference_values.clone(),
                    clip_weights: vec![0.0; plan.clip_nodes.len()],
                    input_weights: Vec::new(),
                    local_tick: None,
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug)]
enum TransitionSource {
    Dynamic { state: usize },
    Captured { values: Vec<AnimValue> },
}

#[derive(Clone, Debug)]
struct ActiveTransition {
    transition_id: GraphTransitionId,
    from_state: usize,
    to_state: usize,
    source: TransitionSource,
    elapsed: Tick,
    duration: Tick,
    curve: GraphTransitionCurve,
    interruption: GraphInterruptionPolicy,
}

#[derive(Clone, Debug)]
struct EventCandidate {
    source_node: GraphNodeId,
    sequence_id: SequenceId,
    occurrence: crate::EventOccurrence,
    traversal_direction: Direction,
    weight: f64,
}

#[derive(Clone, Copy, Debug)]
struct EventBranch {
    workspace: usize,
    root: usize,
    to: Tick,
    weight: f64,
}

#[derive(Clone, Debug)]
struct EventTraversal {
    sequence_id: SequenceId,
    from: Tick,
    to: Tick,
    direction: Direction,
    weight: f64,
}

/// One deterministic graph instance. It has no wall clock, renderer, ECS, or document mutation
/// seam: callers advance exact integer ticks and project the returned transient frame themselves.
#[derive(Clone, Debug)]
pub struct AnimationGraphRuntimeInstance {
    plan: CompiledAnimationGraph,
    parameters: Vec<GraphParameterValue>,
    parameter_lookup: BTreeMap<GraphParameterId, usize>,
    raw_tick: Tick,
    current_state: Option<usize>,
    state_elapsed: Vec<Tick>,
    transition: Option<ActiveTransition>,
    frame: AnimationGraphFrame,
    events: Vec<GraphEventOccurrence>,
    event_limit: usize,
    events_truncated: bool,
    node_trace: Vec<GraphNodeTrace>,
    input_trace: Vec<GraphInputTrace>,
    input_trace_truncated: bool,
    state_trace: GraphStateTrace,
    workspace_a: EvaluationWorkspace,
    workspace_b: EvaluationWorkspace,
    candidates: Vec<EventCandidate>,
    dominant_event_source: Option<GraphNodeId>,
}

impl AnimationGraphRuntimeInstance {
    #[must_use]
    pub fn new(plan: CompiledAnimationGraph) -> Self {
        Self::with_event_limit(plan, DEFAULT_GRAPH_EVENT_LIMIT)
    }

    #[must_use]
    pub fn with_event_limit(plan: CompiledAnimationGraph, event_limit: usize) -> Self {
        let parameters: Vec<_> = plan
            .parameters
            .iter()
            .map(|parameter| parameter.default.clone())
            .collect();
        let parameter_lookup = plan
            .parameters
            .iter()
            .enumerate()
            .map(|(index, parameter)| (parameter.id.clone(), index))
            .collect();
        let current_state = plan
            .state_machine
            .as_ref()
            .map(|machine| machine.initial_state);
        let state_count = plan
            .state_machine
            .as_ref()
            .map_or(0, |machine| machine.states.len());
        let workspace_a = EvaluationWorkspace::new(&plan);
        let workspace_b = EvaluationWorkspace::new(&plan);
        let frame = AnimationGraphFrame {
            raw_tick: Tick::ZERO,
            values: plan
                .bindings
                .iter()
                .zip(&plan.reference_values)
                .map(|(binding, value)| GraphBindingValue {
                    binding: binding.clone(),
                    value: value.clone(),
                })
                .collect(),
        };
        let mut instance = Self {
            plan,
            parameters,
            parameter_lookup,
            raw_tick: Tick::ZERO,
            current_state,
            state_elapsed: vec![Tick::ZERO; state_count],
            transition: None,
            frame,
            events: Vec::with_capacity(event_limit.min(MAX_GRAPH_EVENT_LIMIT)),
            event_limit: event_limit.min(MAX_GRAPH_EVENT_LIMIT),
            events_truncated: false,
            node_trace: Vec::new(),
            input_trace: Vec::new(),
            input_trace_truncated: false,
            state_trace: GraphStateTrace {
                current_state: None,
                state_elapsed: Tick::ZERO,
                transition: None,
                transitions_taken_this_update: 0,
                transition_limit_reached: false,
            },
            workspace_a,
            workspace_b,
            candidates: Vec::new(),
            dominant_event_source: None,
        };
        instance.sample_current(0, false);
        instance
    }

    #[must_use]
    pub const fn plan(&self) -> &CompiledAnimationGraph {
        &self.plan
    }

    #[must_use]
    pub const fn raw_tick(&self) -> Tick {
        self.raw_tick
    }

    #[must_use]
    pub fn frame(&self) -> &AnimationGraphFrame {
        &self.frame
    }

    #[must_use]
    pub fn events(&self) -> &[GraphEventOccurrence] {
        &self.events
    }

    #[must_use]
    pub const fn events_truncated(&self) -> bool {
        self.events_truncated
    }

    #[must_use]
    pub fn node_trace(&self) -> &[GraphNodeTrace] {
        &self.node_trace
    }

    #[must_use]
    pub fn input_trace(&self) -> &[GraphInputTrace] {
        &self.input_trace
    }

    /// True when the bounded edge trace was truncated or a contributing captured composite has no
    /// recoverable internal graph provenance.
    #[must_use]
    pub const fn input_trace_truncated(&self) -> bool {
        self.input_trace_truncated
    }

    #[must_use]
    pub const fn state_trace(&self) -> &GraphStateTrace {
        &self.state_trace
    }

    #[must_use]
    pub fn parameters(&self) -> Vec<GraphParameterSnapshot> {
        self.plan
            .parameters
            .iter()
            .zip(&self.parameters)
            .map(|(parameter, value)| GraphParameterSnapshot {
                id: parameter.id.clone(),
                value: value.clone(),
            })
            .collect()
    }

    pub fn set_event_limit(&mut self, event_limit: usize) {
        self.event_limit = event_limit.min(MAX_GRAPH_EVENT_LIMIT);
        if self.events.capacity() < self.event_limit {
            self.events
                .reserve(self.event_limit - self.events.capacity());
        }
    }

    pub fn set_parameter(
        &mut self,
        id: &GraphParameterId,
        value: GraphParameterValue,
    ) -> Result<(), GraphParameterError> {
        let index = self
            .parameter_lookup
            .get(id)
            .copied()
            .ok_or(GraphParameterError::UnknownParameter)?;
        let definition = &self.plan.parameters[index];
        if definition.kind == GraphParameterKind::Trigger {
            return Err(GraphParameterError::TriggerRequiresFire);
        }
        if definition.kind != value.kind() {
            return Err(GraphParameterError::TypeMismatch);
        }
        if !value.is_finite() {
            return Err(GraphParameterError::NonFinite);
        }
        if definition.used_as_weight
            && matches!(value, GraphParameterValue::Number(number) if number < 0.0)
        {
            return Err(GraphParameterError::NegativeWeight);
        }
        self.parameters[index] = value;
        self.sample_current(0, false);
        Ok(())
    }

    pub fn fire_trigger(&mut self, id: &GraphParameterId) -> Result<(), GraphParameterError> {
        let index = self
            .parameter_lookup
            .get(id)
            .copied()
            .ok_or(GraphParameterError::UnknownParameter)?;
        if self.plan.parameters[index].kind != GraphParameterKind::Trigger {
            return Err(GraphParameterError::TypeMismatch);
        }
        self.parameters[index] = GraphParameterValue::Trigger(true);
        Ok(())
    }

    /// Move to an exact graph tick without event dispatch or condition evaluation. State-machine
    /// local time is explicitly repositioned and any in-flight crossfade is cancelled.
    pub fn seek(&mut self, tick: Tick) -> &AnimationGraphFrame {
        self.raw_tick = tick;
        self.transition = None;
        if let Some(state) = self.current_state {
            self.state_elapsed[state] = tick;
        }
        self.events.clear();
        self.events_truncated = false;
        self.sample_current(0, false);
        &self.frame
    }

    /// Advance by an exact signed integer delta. Event collection is two-phase: bounded clip
    /// candidates are gathered first, then graph contribution policy filters the final output.
    pub fn advance(&mut self, delta: Tick) -> &AnimationGraphFrame {
        let previous_raw = self.raw_tick;
        self.raw_tick = self.raw_tick.saturating_add(delta);
        self.events.clear();
        self.events_truncated = false;
        self.candidates.clear();
        self.dominant_event_source = None;

        let old_elapsed = self.state_elapsed.clone();
        if let Some(active) = &self.transition {
            if let TransitionSource::Dynamic { state } = active.source {
                self.state_elapsed[state] = self.state_elapsed[state].saturating_add(delta);
            }
            self.state_elapsed[active.to_state] =
                self.state_elapsed[active.to_state].saturating_add(delta);
        } else if let Some(state) = self.current_state {
            self.state_elapsed[state] = self.state_elapsed[state].saturating_add(delta);
        }
        if let Some(active) = &mut self.transition {
            active.elapsed = Tick(active.elapsed.0.saturating_add(delta.0.abs()));
        }

        let branches = self.sample_current(0, true);
        self.collect_event_candidates(&branches, &old_elapsed, previous_raw);
        let transitions_taken = self.resolve_transitions();
        if transitions_taken > 0 {
            self.sample_current(transitions_taken, false);
        }
        self.filter_events();
        &self.frame
    }

    pub fn reset(&mut self) -> &AnimationGraphFrame {
        for (value, parameter) in self.parameters.iter_mut().zip(&self.plan.parameters) {
            *value = parameter.default.clone();
        }
        self.raw_tick = Tick::ZERO;
        self.transition = None;
        self.state_elapsed.fill(Tick::ZERO);
        self.current_state = self
            .plan
            .state_machine
            .as_ref()
            .map(|machine| machine.initial_state);
        self.events.clear();
        self.events_truncated = false;
        self.sample_current(0, false);
        &self.frame
    }

    /// Hot-swap an immutable compiled plan. `PreserveState` rebinds parameters and the active state
    /// by stable ID, retains its local/raw clock, and deliberately cancels an in-flight transition.
    pub fn rebind_plan(
        &mut self,
        plan: CompiledAnimationGraph,
        policy: AnimationGraphRebindPolicy,
    ) -> &AnimationGraphFrame {
        let old_parameters: BTreeMap<_, _> = self
            .plan
            .parameters
            .iter()
            .zip(&self.parameters)
            .map(|(definition, value)| (definition.id.clone(), (definition.kind, value.clone())))
            .collect();
        let old_state = self.current_state.and_then(|index| {
            self.plan
                .state_machine
                .as_ref()
                .map(|machine| (machine.states[index].id.clone(), self.state_elapsed[index]))
        });
        self.plan = plan;
        self.parameter_lookup = self
            .plan
            .parameters
            .iter()
            .enumerate()
            .map(|(index, parameter)| (parameter.id.clone(), index))
            .collect();
        self.parameters = self
            .plan
            .parameters
            .iter()
            .map(|definition| {
                if definition.kind == GraphParameterKind::Trigger {
                    // Triggers are one-shot input receipts, never durable runtime state. Carrying a
                    // fired bit across a compile install could replay an action against new code.
                    GraphParameterValue::Trigger(false)
                } else if policy == AnimationGraphRebindPolicy::PreserveState {
                    old_parameters
                        .get(&definition.id)
                        .filter(|(kind, value)| {
                            *kind == definition.kind
                                && (!definition.used_as_weight
                                    || !matches!(
                                        value,
                                        GraphParameterValue::Number(number) if *number < 0.0
                                    ))
                        })
                        .map_or_else(|| definition.default.clone(), |(_, value)| value.clone())
                } else {
                    definition.default.clone()
                }
            })
            .collect();
        let state_count = self
            .plan
            .state_machine
            .as_ref()
            .map_or(0, |machine| machine.states.len());
        self.state_elapsed = vec![Tick::ZERO; state_count];
        self.current_state = self.plan.state_machine.as_ref().map(|machine| {
            if policy == AnimationGraphRebindPolicy::PreserveState {
                if let Some((id, elapsed)) = &old_state {
                    if let Some(index) = machine.state_lookup.get(id).copied() {
                        self.state_elapsed[index] = *elapsed;
                        return index;
                    }
                }
            }
            machine.initial_state
        });
        if policy == AnimationGraphRebindPolicy::Reset {
            self.raw_tick = Tick::ZERO;
        }
        self.transition = None;
        self.workspace_a = EvaluationWorkspace::new(&self.plan);
        self.workspace_b = EvaluationWorkspace::new(&self.plan);
        self.frame.values = self
            .plan
            .bindings
            .iter()
            .zip(&self.plan.reference_values)
            .map(|(binding, value)| GraphBindingValue {
                binding: binding.clone(),
                value: value.clone(),
            })
            .collect();
        self.events.clear();
        self.sample_current(0, false);
        &self.frame
    }

    fn sample_current(&mut self, transitions_taken: usize, for_events: bool) -> Vec<EventBranch> {
        self.frame.raw_tick = self.raw_tick;
        let mut branches = Vec::with_capacity(2);
        let mut root_weights = Vec::with_capacity(2);
        if let Some(machine) = &self.plan.state_machine {
            let current = self.current_state.unwrap_or(machine.initial_state);
            if let Some(active) = &self.transition {
                let linear = transition_progress(active.elapsed, active.duration);
                let alpha = curve_progress(active.curve, linear);
                let target_tick = self.state_elapsed[active.to_state];
                evaluate_workspace(
                    &self.plan,
                    &self.parameters,
                    target_tick,
                    &mut self.workspace_b,
                );
                let target_root = machine.states[active.to_state].node;
                match &active.source {
                    TransitionSource::Dynamic { state } => {
                        let source_tick = self.state_elapsed[*state];
                        evaluate_workspace(
                            &self.plan,
                            &self.parameters,
                            source_tick,
                            &mut self.workspace_a,
                        );
                        let source_root = machine.states[*state].node;
                        blend_two_into_frame(
                            &self.plan,
                            &self.workspace_a.nodes[source_root].values,
                            &self.workspace_b.nodes[target_root].values,
                            alpha,
                            &mut self.frame,
                        );
                        root_weights.push((0, source_root, 1.0 - alpha));
                        if for_events {
                            branches.push(EventBranch {
                                workspace: 0,
                                root: source_root,
                                to: source_tick,
                                weight: 1.0 - alpha,
                            });
                        }
                    }
                    TransitionSource::Captured { values } => {
                        blend_two_into_frame(
                            &self.plan,
                            values,
                            &self.workspace_b.nodes[target_root].values,
                            alpha,
                            &mut self.frame,
                        );
                    }
                }
                root_weights.push((1, target_root, alpha));
                if for_events {
                    branches.push(EventBranch {
                        workspace: 1,
                        root: target_root,
                        to: target_tick,
                        weight: alpha,
                    });
                }
            } else {
                let tick = self.state_elapsed[current];
                evaluate_workspace(&self.plan, &self.parameters, tick, &mut self.workspace_a);
                let root = machine.states[current].node;
                write_values_into_frame(
                    &self.plan,
                    &self.workspace_a.nodes[root].values,
                    &mut self.frame,
                );
                root_weights.push((0, root, 1.0));
                if for_events {
                    branches.push(EventBranch {
                        workspace: 0,
                        root,
                        to: tick,
                        weight: 1.0,
                    });
                }
            }
        } else {
            evaluate_workspace(
                &self.plan,
                &self.parameters,
                self.raw_tick,
                &mut self.workspace_a,
            );
            write_values_into_frame(
                &self.plan,
                &self.workspace_a.nodes[self.plan.output_node].values,
                &mut self.frame,
            );
            root_weights.push((0, self.plan.output_node, 1.0));
            if for_events {
                branches.push(EventBranch {
                    workspace: 0,
                    root: self.plan.output_node,
                    to: self.raw_tick,
                    weight: 1.0,
                });
            }
        }
        self.refresh_trace(&root_weights, transitions_taken);
        branches
    }

    fn collect_event_candidates(
        &mut self,
        branches: &[EventBranch],
        old_elapsed: &[Tick],
        previous_raw: Tick,
    ) {
        let max = self.plan.max_event_candidates_per_update;
        let machine = self.plan.state_machine.as_ref();
        let active = self.transition.as_ref();
        let mut source_weights = BTreeMap::<GraphNodeId, f64>::new();
        let mut traversals = BTreeMap::<GraphNodeId, Vec<EventTraversal>>::new();
        for branch in branches {
            let workspace = if branch.workspace == 0 {
                &self.workspace_a
            } else {
                &self.workspace_b
            };
            let buffer = &workspace.nodes[branch.root];
            let (branch_from, branch_to) = if let Some(machine) = machine {
                let state = if let Some(active) = active {
                    if branch.workspace == 0 {
                        match active.source {
                            TransitionSource::Dynamic { state } => state,
                            TransitionSource::Captured { .. } => continue,
                        }
                    } else {
                        active.to_state
                    }
                } else {
                    self.current_state.unwrap_or(machine.initial_state)
                };
                (
                    old_elapsed.get(state).copied().unwrap_or(Tick::ZERO),
                    branch.to,
                )
            } else {
                (previous_raw, branch.to)
            };
            for (slot, clip_weight) in buffer.clip_weights.iter().copied().enumerate() {
                let weight = clip_weight * branch.weight;
                if weight <= 0.0 {
                    continue;
                }
                let node_index = self.plan.clip_nodes[slot];
                let node = &self.plan.nodes[node_index];
                *source_weights.entry(node.id.clone()).or_default() += weight;
                let CompiledNodeKind::Clip {
                    sequence,
                    rate,
                    start_tick,
                    ..
                } = &node.kind
                else {
                    continue;
                };
                if !self.plan.sequences.contains_key(sequence) {
                    continue;
                }
                let from = apply_rate(branch_from, *rate).saturating_add(*start_tick);
                let to = apply_rate(branch_to, *rate).saturating_add(*start_tick);
                traversals
                    .entry(node.id.clone())
                    .or_default()
                    .push(EventTraversal {
                        sequence_id: sequence.clone(),
                        from,
                        to,
                        direction: if to < from {
                            Direction::Reverse
                        } else {
                            Direction::Forward
                        },
                        weight,
                    });
            }
        }

        'sources: for (source_node, source_traversals) in traversals {
            let remaining = max.saturating_sub(self.candidates.len());
            if remaining == 0 {
                for traversal in source_traversals {
                    let Some(source) = self.plan.sequences.get(&traversal.sequence_id) else {
                        continue;
                    };
                    let batch = source.events_crossed_limited(traversal.from, traversal.to, 1);
                    if batch.truncated || !batch.occurrences.is_empty() {
                        self.events_truncated = true;
                        break 'sources;
                    }
                }
                continue;
            }

            // A sample has at most two event branches (crossfade source and destination). Fetch one
            // more than the remaining unique budget from each, merge occurrence identity, and only
            // then apply the global candidate bound. Duplicate routes therefore consume one slot,
            // with a strict 2 * (budget + 1) temporary upper bound per source.
            let per_traversal_limit = remaining.saturating_add(1);
            let mut merged: BTreeMap<(crate::EventId, Tick), EventCandidate> = BTreeMap::new();
            for traversal in source_traversals {
                let Some(source) = self.plan.sequences.get(&traversal.sequence_id) else {
                    continue;
                };
                let batch = source.events_crossed_limited(
                    traversal.from,
                    traversal.to,
                    per_traversal_limit,
                );
                self.events_truncated |= batch.truncated;
                for occurrence in batch.occurrences {
                    let key = (occurrence.event.id.clone(), occurrence.raw_tick);
                    if let Some(candidate) = merged.get_mut(&key) {
                        candidate.weight += traversal.weight;
                    } else {
                        merged.insert(
                            key,
                            EventCandidate {
                                source_node: source_node.clone(),
                                sequence_id: traversal.sequence_id.clone(),
                                occurrence,
                                traversal_direction: traversal.direction,
                                weight: traversal.weight,
                            },
                        );
                    }
                }
            }
            let mut merged: Vec<EventCandidate> = merged.into_values().collect();
            merged.sort_by(event_candidate_order);
            if merged.len() > remaining {
                merged.truncate(remaining);
                self.events_truncated = true;
            }
            self.candidates.extend(merged);
        }
        self.candidates.sort_by(event_candidate_order);
        self.dominant_event_source = source_weights
            .into_iter()
            .max_by(|(left_id, left_weight), (right_id, right_weight)| {
                left_weight
                    .total_cmp(right_weight)
                    .then_with(|| right_id.cmp(left_id))
            })
            .map(|(id, _)| id);
    }

    fn filter_events(&mut self) {
        for candidate in &self.candidates {
            let include = match self.plan.event_policy {
                GraphEventPolicy::All => candidate.weight > 0.0,
                GraphEventPolicy::Dominant => {
                    self.dominant_event_source.as_ref() == Some(&candidate.source_node)
                }
                GraphEventPolicy::Threshold { minimum_weight } => {
                    candidate.weight >= minimum_weight
                }
                GraphEventPolicy::None => false,
            };
            if !include {
                continue;
            }
            if self.events.len() == self.event_limit {
                self.events_truncated = true;
                break;
            }
            self.events.push(GraphEventOccurrence {
                source_node: candidate.source_node.clone(),
                sequence_id: candidate.sequence_id.clone(),
                event: candidate.occurrence.event.clone(),
                source_raw_tick: candidate.occurrence.raw_tick,
                source_local_tick: candidate.occurrence.local_tick,
                direction: candidate.occurrence.direction,
                contribution_weight: candidate.weight,
            });
        }
    }

    fn resolve_transitions(&mut self) -> usize {
        let Some(machine) = self.plan.state_machine.as_ref() else {
            return 0;
        };
        let mut taken = 0;
        while taken < self.plan.max_transitions_per_update {
            if let Some(active) = &self.transition {
                if active.duration.0 == 0 || active.elapsed >= active.duration {
                    self.current_state = Some(active.to_state);
                    self.transition = None;
                    continue;
                }
            }
            let candidate = select_transition(
                machine,
                self.current_state.unwrap_or(machine.initial_state),
                self.transition.as_ref(),
                &self.parameters,
            );
            let Some(candidate_index) = candidate else {
                break;
            };
            let candidate = &machine.transitions[candidate_index];
            for condition in &candidate.conditions {
                if matches!(condition.test, GraphConditionTest::Triggered) {
                    self.parameters[condition.parameter] = GraphParameterValue::Trigger(false);
                }
            }
            let captured = self.transition.as_ref().map(|_| {
                self.frame
                    .values
                    .iter()
                    .map(|entry| entry.value.clone())
                    .collect::<Vec<_>>()
            });
            let to_state = candidate.to;
            if machine.states[to_state].reset_on_entry {
                self.state_elapsed[to_state] = Tick::ZERO;
            }
            self.current_state = Some(to_state);
            self.transition = Some(ActiveTransition {
                transition_id: candidate.id.clone(),
                from_state: candidate.from,
                to_state,
                source: captured.map_or(
                    TransitionSource::Dynamic {
                        state: candidate.from,
                    },
                    |values| TransitionSource::Captured { values },
                ),
                elapsed: Tick::ZERO,
                duration: candidate.duration,
                curve: candidate.curve,
                interruption: candidate.interruption,
            });
            taken += 1;
            if candidate.duration.0 > 0 {
                break;
            }
        }
        taken
    }

    fn refresh_trace(&mut self, roots: &[(usize, usize, f64)], transitions_taken: usize) {
        let mut activations = vec![0.0; self.plan.nodes.len()];
        let mut input_activations = BTreeMap::new();
        self.input_trace_truncated = self.transition.as_ref().is_some_and(|active| {
            matches!(active.source, TransitionSource::Captured { .. })
                && 1.0
                    - curve_progress(
                        active.curve,
                        transition_progress(active.elapsed, active.duration),
                    )
                    > 0.0
        });
        let mut workspace_activations = vec![0.0; self.plan.nodes.len()];
        for workspace_index in 0..=1 {
            workspace_activations.fill(0.0);
            for (_, root, weight) in roots
                .iter()
                .filter(|(workspace, _, _)| *workspace == workspace_index)
            {
                workspace_activations[*root] += *weight;
            }
            let workspace = if workspace_index == 0 {
                &self.workspace_a
            } else {
                &self.workspace_b
            };
            propagate_activations(
                &self.plan,
                workspace,
                &mut workspace_activations,
                &mut activations,
                &mut input_activations,
                &mut self.input_trace_truncated,
            );
        }
        self.node_trace.clear();
        self.node_trace
            .extend(self.plan.nodes.iter().enumerate().map(|(index, node)| {
                GraphNodeTrace {
                    node_id: node.id.clone(),
                    evaluated: true,
                    contribution_weight: activations[index],
                    local_tick: self.workspace_a.nodes[index]
                        .local_tick
                        .or(self.workspace_b.nodes[index].local_tick),
                }
            }));
        self.input_trace.clear();
        self.input_trace.extend(input_activations.into_iter().map(
            |((from, to), contribution_weight)| GraphInputTrace {
                from,
                to,
                contribution_weight,
            },
        ));
        let (current_state, elapsed) =
            self.current_state
                .and_then(|index| {
                    self.plan.state_machine.as_ref().map(|machine| {
                        (machine.states[index].id.clone(), self.state_elapsed[index])
                    })
                })
                .map_or((None, Tick::ZERO), |(id, tick)| (Some(id), tick));
        let transition = self.transition.as_ref().and_then(|active| {
            let machine = self.plan.state_machine.as_ref()?;
            let linear = transition_progress(active.elapsed, active.duration);
            Some(GraphTransitionTrace {
                id: active.transition_id.clone(),
                from: machine.states[active.from_state].id.clone(),
                to: machine.states[active.to_state].id.clone(),
                elapsed: active.elapsed,
                duration: active.duration,
                linear_progress: linear,
                curved_progress: curve_progress(active.curve, linear),
                source_is_captured_composite: matches!(
                    active.source,
                    TransitionSource::Captured { .. }
                ),
            })
        });
        self.state_trace = GraphStateTrace {
            current_state,
            state_elapsed: elapsed,
            transition,
            transitions_taken_this_update: transitions_taken,
            transition_limit_reached: transitions_taken == self.plan.max_transitions_per_update,
        };
    }
}

fn select_transition(
    machine: &CompiledStateMachine,
    current_state: usize,
    active: Option<&ActiveTransition>,
    parameters: &[GraphParameterValue],
) -> Option<usize> {
    let allowed = |from: usize| match active {
        None => from == current_state,
        Some(active) => match active.interruption {
            GraphInterruptionPolicy::None => false,
            GraphInterruptionPolicy::Source => from == active.from_state,
            GraphInterruptionPolicy::Destination => from == active.to_state,
            GraphInterruptionPolicy::Both => from == active.from_state || from == active.to_state,
        },
    };
    machine
        .transitions
        .iter()
        .enumerate()
        .filter(|(_, transition)| {
            allowed(transition.from)
                && active.is_none_or(|active| transition.id != active.transition_id)
                && transition
                    .conditions
                    .iter()
                    .all(|condition| condition_matches(condition, parameters))
        })
        .max_by(|(_, left), (_, right)| {
            left.priority
                .cmp(&right.priority)
                .then_with(|| right.id.cmp(&left.id))
        })
        .map(|(index, _)| index)
}

fn event_candidate_order(left: &EventCandidate, right: &EventCandidate) -> std::cmp::Ordering {
    left.source_node
        .cmp(&right.source_node)
        .then_with(|| {
            let direction = direction_tag(left.traversal_direction)
                .cmp(&direction_tag(right.traversal_direction));
            if !direction.is_eq() {
                return direction;
            }
            let order = left.occurrence.raw_tick.cmp(&right.occurrence.raw_tick);
            if left.traversal_direction == Direction::Reverse {
                order.reverse()
            } else {
                order
            }
        })
        .then(left.occurrence.event.id.cmp(&right.occurrence.event.id))
        .then(
            direction_tag(left.occurrence.direction)
                .cmp(&direction_tag(right.occurrence.direction)),
        )
        .then(left.occurrence.local_tick.cmp(&right.occurrence.local_tick))
}

const fn direction_tag(direction: Direction) -> u8 {
    match direction {
        Direction::Forward => 0,
        Direction::Reverse => 1,
    }
}

fn condition_matches(condition: &CompiledCondition, parameters: &[GraphParameterValue]) -> bool {
    match (&parameters[condition.parameter], &condition.test) {
        (GraphParameterValue::Boolean(value), GraphConditionTest::BooleanEquals(expected)) => {
            value == expected
        }
        (GraphParameterValue::Number(value), GraphConditionTest::NumberEquals(expected)) => {
            value.total_cmp(expected).is_eq()
        }
        (GraphParameterValue::Number(value), GraphConditionTest::NumberNotEquals(expected)) => {
            !value.total_cmp(expected).is_eq()
        }
        (GraphParameterValue::Number(value), GraphConditionTest::NumberGreaterThan(threshold)) => {
            value > threshold
        }
        (
            GraphParameterValue::Number(value),
            GraphConditionTest::NumberGreaterOrEqual(threshold),
        ) => value >= threshold,
        (GraphParameterValue::Number(value), GraphConditionTest::NumberLessThan(threshold)) => {
            value < threshold
        }
        (GraphParameterValue::Number(value), GraphConditionTest::NumberLessOrEqual(threshold)) => {
            value <= threshold
        }
        (GraphParameterValue::Integer(value), GraphConditionTest::IntegerEquals(expected)) => {
            value == expected
        }
        (GraphParameterValue::Integer(value), GraphConditionTest::IntegerNotEquals(expected)) => {
            value != expected
        }
        (GraphParameterValue::Integer(value), GraphConditionTest::IntegerGreaterThan(expected)) => {
            value > expected
        }
        (
            GraphParameterValue::Integer(value),
            GraphConditionTest::IntegerGreaterOrEqual(expected),
        ) => value >= expected,
        (GraphParameterValue::Integer(value), GraphConditionTest::IntegerLessThan(expected)) => {
            value < expected
        }
        (GraphParameterValue::Integer(value), GraphConditionTest::IntegerLessOrEqual(expected)) => {
            value <= expected
        }
        (GraphParameterValue::Trigger(value), GraphConditionTest::Triggered) => *value,
        _ => false,
    }
}

fn evaluate_workspace(
    plan: &CompiledAnimationGraph,
    parameters: &[GraphParameterValue],
    tick: Tick,
    workspace: &mut EvaluationWorkspace,
) {
    for node_index in 0..plan.nodes.len() {
        let (dependencies, current_and_after) = workspace.nodes.split_at_mut(node_index);
        let output = &mut current_and_after[0];
        output.values.clone_from(&plan.reference_values);
        output.clip_weights.fill(0.0);
        output.input_weights.clear();
        output.local_tick = None;
        match &plan.nodes[node_index].kind {
            CompiledNodeKind::ReferencePose => {}
            CompiledNodeKind::Clip {
                sequence,
                rate,
                start_tick,
                clip_slot,
            } => {
                let local_tick = apply_rate(tick, *rate).saturating_add(*start_tick);
                output.local_tick = Some(local_tick);
                output.clip_weights[*clip_slot] = 1.0;
                if let Some(sequence) = plan.sequences.get(sequence) {
                    let evaluation = sequence.evaluate(local_tick);
                    for evaluated in evaluation.bindings {
                        if let Ok(index) = plan.bindings.binary_search(&evaluated.binding) {
                            output.values[index] = evaluated.value;
                        }
                    }
                }
            }
            CompiledNodeKind::Blend { mode, inputs } => {
                let mut weights: Vec<_> = inputs
                    .iter()
                    .map(|(input, weight)| (*input, resolve_weight(weight, parameters)))
                    .collect();
                normalize_blend_weights(*mode, &mut weights);
                mix_pose(
                    plan,
                    dependencies,
                    &weights,
                    direct_reference_weight(*mode, &weights),
                    output,
                );
            }
            CompiledNodeKind::Blend1d { parameter, points } => {
                let position = number_parameter(parameters, *parameter);
                let weights = blend_1d_weights(points, position);
                mix_pose(plan, dependencies, &weights, 0.0, output);
            }
            CompiledNodeKind::Blend2d {
                parameter_x,
                parameter_y,
                points,
                triangles,
                boundary_edges,
                outside_hull,
            } => {
                let position = [
                    bounded_blend_coordinate(number_parameter(parameters, *parameter_x)),
                    bounded_blend_coordinate(number_parameter(parameters, *parameter_y)),
                ];
                let weights =
                    blend_2d_weights(points, triangles, boundary_edges, *outside_hull, position);
                mix_pose(plan, dependencies, &weights, 0.0, output);
            }
            CompiledNodeKind::OverrideLayer {
                base,
                layer,
                weight,
                mask,
            } => {
                let weight = resolve_weight(weight, parameters).clamp(0.0, 1.0);
                let mask = mask.map_or(&[][..], |index| &plan.masks[index].weights);
                override_pose(
                    plan,
                    &dependencies[*base],
                    &dependencies[*layer],
                    weight,
                    mask,
                    output,
                );
                output.input_weights.push((*base, 1.0 - weight));
                output.input_weights.push((*layer, weight));
            }
            CompiledNodeKind::AdditiveLayer {
                base,
                layer,
                weight,
                mask,
            } => {
                let weight = resolve_weight(weight, parameters).clamp(0.0, 1.0);
                let mask = mask.map_or(&[][..], |index| &plan.masks[index].weights);
                additive_pose(
                    plan,
                    &dependencies[*base],
                    &dependencies[*layer],
                    weight,
                    mask,
                    output,
                );
                output.input_weights.push((*base, 1.0));
                output.input_weights.push((*layer, weight));
            }
        }
    }
}

fn resolve_weight(weight: &CompiledWeight, parameters: &[GraphParameterValue]) -> f64 {
    match weight {
        CompiledWeight::Constant(value) => *value,
        CompiledWeight::Parameter(index) => number_parameter(parameters, *index),
    }
}

fn number_parameter(parameters: &[GraphParameterValue], index: usize) -> f64 {
    match parameters.get(index) {
        Some(GraphParameterValue::Number(value)) => *value,
        _ => 0.0,
    }
}

fn normalize_blend_weights(mode: super::GraphBlendMode, weights: &mut [(usize, f64)]) {
    for (_, weight) in weights.iter_mut() {
        *weight = weight.max(0.0);
    }
    if mode == super::GraphBlendMode::Normalized {
        let sum: f64 = weights.iter().map(|(_, weight)| weight).sum();
        if sum > 1.0e-12 {
            for (_, weight) in weights {
                *weight /= sum;
            }
        } else {
            for (_, weight) in weights {
                *weight = 0.0;
            }
        }
    }
}

fn direct_reference_weight(mode: super::GraphBlendMode, weights: &[(usize, f64)]) -> f64 {
    if mode == super::GraphBlendMode::Direct {
        (1.0 - weights.iter().map(|(_, weight)| weight).sum::<f64>()).max(0.0)
    } else if weights.iter().all(|(_, weight)| *weight == 0.0) {
        1.0
    } else {
        0.0
    }
}

fn mix_pose(
    plan: &CompiledAnimationGraph,
    dependencies: &[NodeBuffer],
    weights: &[(usize, f64)],
    reference_weight: f64,
    output: &mut NodeBuffer,
) {
    for (binding_index, reference) in plan.reference_values.iter().enumerate() {
        let candidates: Vec<_> = weights
            .iter()
            .map(|(node, weight)| (&dependencies[*node].values[binding_index], *weight, *node))
            .collect();
        output.values[binding_index] = blend_anim_values(reference, reference_weight, &candidates);
    }
    for (node, weight) in weights {
        output.input_weights.push((*node, *weight));
        for (destination, source) in output
            .clip_weights
            .iter_mut()
            .zip(&dependencies[*node].clip_weights)
        {
            *destination += source * *weight;
        }
    }
}

fn override_pose(
    plan: &CompiledAnimationGraph,
    base: &NodeBuffer,
    layer: &NodeBuffer,
    weight: f64,
    mask: &[f64],
    output: &mut NodeBuffer,
) {
    for index in 0..plan.bindings.len() {
        let influence = weight * mask.get(index).copied().unwrap_or(1.0);
        output.values[index] = blend_anim_values(
            &plan.reference_values[index],
            0.0,
            &[
                (&base.values[index], 1.0 - influence, 0),
                (&layer.values[index], influence, 1),
            ],
        );
    }
    for index in 0..output.clip_weights.len() {
        output.clip_weights[index] =
            base.clip_weights[index] * (1.0 - weight) + layer.clip_weights[index] * weight;
    }
}

fn additive_pose(
    plan: &CompiledAnimationGraph,
    base: &NodeBuffer,
    layer: &NodeBuffer,
    weight: f64,
    mask: &[f64],
    output: &mut NodeBuffer,
) {
    for index in 0..plan.bindings.len() {
        let influence = weight * mask.get(index).copied().unwrap_or(1.0);
        output.values[index] = additive_value(
            &base.values[index],
            &layer.values[index],
            &plan.reference_values[index],
            influence,
        );
    }
    for index in 0..output.clip_weights.len() {
        output.clip_weights[index] = base.clip_weights[index] + layer.clip_weights[index] * weight;
    }
}

fn blend_anim_values(
    reference: &AnimValue,
    reference_weight: f64,
    candidates: &[(&AnimValue, f64, usize)],
) -> AnimValue {
    match reference {
        AnimValue::Number(reference) => AnimValue::Number(candidates.iter().fold(
            reference * reference_weight,
            |sum, (value, weight, _)| {
                if let AnimValue::Number(value) = value {
                    value.mul_add(*weight, sum)
                } else {
                    sum
                }
            },
        )),
        AnimValue::Integer(reference) => {
            let value = candidates.iter().fold(
                *reference as f64 * reference_weight,
                |sum, (value, weight, _)| {
                    if let AnimValue::Integer(value) = value {
                        (*value as f64).mul_add(*weight, sum)
                    } else {
                        sum
                    }
                },
            );
            AnimValue::Integer(value.round() as i64)
        }
        AnimValue::Boolean(_) | AnimValue::String(_) => {
            let mut selected = reference;
            let mut best_weight = reference_weight;
            let mut best_node = None;
            for (value, weight, node) in candidates {
                if *weight > best_weight
                    || (weight.total_cmp(&best_weight).is_eq()
                        && best_node.is_some_and(|best| *node < best))
                {
                    selected = value;
                    best_weight = *weight;
                    best_node = Some(*node);
                }
            }
            selected.clone()
        }
        AnimValue::Vec3(reference) => {
            let mut result = reference.map(|part| part * reference_weight);
            for (value, weight, _) in candidates {
                if let AnimValue::Vec3(value) = value {
                    for index in 0..3 {
                        result[index] = value[index].mul_add(*weight, result[index]);
                    }
                }
            }
            AnimValue::Vec3(result)
        }
        AnimValue::Vec4(reference) => {
            let mut result = reference.map(|part| part * reference_weight);
            for (value, weight, _) in candidates {
                if let AnimValue::Vec4(value) = value {
                    for index in 0..4 {
                        result[index] = value[index].mul_add(*weight, result[index]);
                    }
                }
            }
            AnimValue::Vec4(result)
        }
        AnimValue::Quaternion(reference) => {
            let mut result = reference.map(|part| part * reference_weight);
            for (value, weight, _) in candidates {
                if let AnimValue::Quaternion(value) = value {
                    let sign = if quaternion_dot(*reference, *value) < 0.0 {
                        -1.0
                    } else {
                        1.0
                    };
                    for index in 0..4 {
                        result[index] = (value[index] * sign).mul_add(*weight, result[index]);
                    }
                }
            }
            AnimValue::Quaternion(normalize_quaternion(result))
        }
        AnimValue::Weights(reference) => {
            let mut result: Vec<_> = reference
                .iter()
                .map(|value| value * reference_weight)
                .collect();
            for (value, weight, _) in candidates {
                if let AnimValue::Weights(value) = value {
                    for (destination, source) in result.iter_mut().zip(value) {
                        *destination = source.mul_add(*weight, *destination);
                    }
                }
            }
            AnimValue::Weights(result)
        }
    }
}

fn additive_value(
    base: &AnimValue,
    layer: &AnimValue,
    reference: &AnimValue,
    weight: f64,
) -> AnimValue {
    match (base, layer, reference) {
        (AnimValue::Number(base), AnimValue::Number(layer), AnimValue::Number(reference)) => {
            AnimValue::Number((layer - reference).mul_add(weight, *base))
        }
        (AnimValue::Integer(base), AnimValue::Integer(layer), AnimValue::Integer(reference)) => {
            AnimValue::Integer(
                ((*layer - *reference) as f64)
                    .mul_add(weight, *base as f64)
                    .round() as i64,
            )
        }
        (AnimValue::Vec3(base), AnimValue::Vec3(layer), AnimValue::Vec3(reference)) => {
            AnimValue::Vec3(std::array::from_fn(|index| {
                (layer[index] - reference[index]).mul_add(weight, base[index])
            }))
        }
        (AnimValue::Vec4(base), AnimValue::Vec4(layer), AnimValue::Vec4(reference)) => {
            AnimValue::Vec4(std::array::from_fn(|index| {
                (layer[index] - reference[index]).mul_add(weight, base[index])
            }))
        }
        (
            AnimValue::Quaternion(base),
            AnimValue::Quaternion(layer),
            AnimValue::Quaternion(reference),
        ) => {
            let delta = quaternion_multiply(*layer, quaternion_inverse(*reference));
            let scaled = nlerp_quaternion([0.0, 0.0, 0.0, 1.0], delta, weight);
            AnimValue::Quaternion(normalize_quaternion(quaternion_multiply(*base, scaled)))
        }
        (AnimValue::Weights(base), AnimValue::Weights(layer), AnimValue::Weights(reference)) => {
            AnimValue::Weights(
                base.iter()
                    .zip(layer)
                    .zip(reference)
                    .map(|((base, layer), reference)| (layer - reference).mul_add(weight, *base))
                    .collect(),
            )
        }
        _ => base.clone(),
    }
}

fn blend_1d_weights(points: &[super::CompiledBlend1dPoint], position: f64) -> Vec<(usize, f64)> {
    if position <= points[0].position {
        return vec![(points[0].node, 1.0)];
    }
    let last = points.len() - 1;
    if position >= points[last].position {
        return vec![(points[last].node, 1.0)];
    }
    let upper = points.partition_point(|point| point.position < position);
    let left = &points[upper - 1];
    let right = &points[upper];
    let alpha = (position - left.position) / (right.position - left.position);
    vec![(left.node, 1.0 - alpha), (right.node, alpha)]
}

fn blend_2d_weights(
    points: &[super::CompiledBlend2dPoint],
    triangles: &[super::CompiledTriangle],
    boundary_edges: &[[usize; 2]],
    outside_hull: GraphOutsideHullMode,
    position: [f64; 2],
) -> Vec<(usize, f64)> {
    for triangle in triangles {
        let vertices = triangle.indices.map(|index| points[index].position);
        if let Some(weights) = barycentric(position, vertices) {
            if weights.iter().all(|weight| *weight >= -1.0e-10) {
                return triangle
                    .indices
                    .iter()
                    .zip(weights)
                    .map(|(index, weight)| (points[*index].node, weight.max(0.0)))
                    .collect();
            }
        }
    }
    if outside_hull == GraphOutsideHullMode::NearestPoint {
        let nearest = points
            .iter()
            .min_by(|left, right| {
                distance_squared(left.position, position)
                    .total_cmp(&distance_squared(right.position, position))
                    .then(left.id.cmp(&right.id))
            })
            .expect("compiler requires 2D points");
        return vec![(nearest.node, 1.0)];
    }
    let mut best: Option<(f64, usize, f64)> = None;
    for (edge_index, [start, end]) in boundary_edges.iter().copied().enumerate() {
        let (distance, alpha) =
            closest_segment(position, points[start].position, points[end].position);
        if best.as_ref().is_none_or(|(best_distance, best_index, _)| {
            distance < *best_distance
                || (distance.total_cmp(best_distance).is_eq() && edge_index < *best_index)
        }) {
            best = Some((distance, edge_index, alpha));
        }
    }
    let (_, edge_index, alpha) = best.expect("compiler requires a 2D mesh boundary");
    let [start, end] = boundary_edges[edge_index];
    [(points[start].node, 1.0 - alpha), (points[end].node, alpha)]
        .into_iter()
        .filter(|(_, weight)| *weight > 0.0)
        .collect()
}

fn bounded_blend_coordinate(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(-MAX_BLEND_2D_COORDINATE_ABS, MAX_BLEND_2D_COORDINATE_ABS)
    } else {
        0.0
    }
}

fn barycentric(point: [f64; 2], triangle: [[f64; 2]; 3]) -> Option<[f64; 3]> {
    let [a, b, c] = triangle;
    let denominator = (b[1] - c[1]).mul_add(a[0] - c[0], (c[0] - b[0]) * (a[1] - c[1]));
    if !denominator.is_finite() || denominator.abs() <= 1.0e-12 {
        return None;
    }
    let first =
        ((b[1] - c[1]).mul_add(point[0] - c[0], (c[0] - b[0]) * (point[1] - c[1]))) / denominator;
    let second =
        ((c[1] - a[1]).mul_add(point[0] - c[0], (a[0] - c[0]) * (point[1] - c[1]))) / denominator;
    let weights = [first, second, 1.0 - first - second];
    weights
        .iter()
        .all(|weight| weight.is_finite())
        .then_some(weights)
}

fn closest_segment(point: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, f64) {
    let delta = [b[0] - a[0], b[1] - a[1]];
    let length_squared = delta[0].mul_add(delta[0], delta[1] * delta[1]);
    let alpha = if !length_squared.is_finite() || length_squared <= 1.0e-24 {
        0.0
    } else {
        let projection =
            (point[0] - a[0]).mul_add(delta[0], (point[1] - a[1]) * delta[1]) / length_squared;
        if projection.is_finite() {
            projection.clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    let closest = [delta[0].mul_add(alpha, a[0]), delta[1].mul_add(alpha, a[1])];
    (distance_squared(point, closest), alpha)
}

fn distance_squared(a: [f64; 2], b: [f64; 2]) -> f64 {
    let distance = (a[0] - b[0]).mul_add(a[0] - b[0], (a[1] - b[1]) * (a[1] - b[1]));
    if distance.is_finite() {
        distance
    } else {
        f64::MAX
    }
}

fn write_values_into_frame(
    plan: &CompiledAnimationGraph,
    values: &[AnimValue],
    frame: &mut AnimationGraphFrame,
) {
    for ((output, value), binding) in frame.values.iter_mut().zip(values).zip(&plan.bindings) {
        output.binding.clone_from(binding);
        output.value.clone_from(value);
    }
}

fn blend_two_into_frame(
    plan: &CompiledAnimationGraph,
    source: &[AnimValue],
    target: &[AnimValue],
    alpha: f64,
    frame: &mut AnimationGraphFrame,
) {
    for index in 0..plan.bindings.len() {
        frame.values[index]
            .binding
            .clone_from(&plan.bindings[index]);
        frame.values[index].value = blend_anim_values(
            &plan.reference_values[index],
            0.0,
            &[(&source[index], 1.0 - alpha, 0), (&target[index], alpha, 1)],
        );
    }
}

fn propagate_activations(
    plan: &CompiledAnimationGraph,
    workspace: &EvaluationWorkspace,
    workspace_activations: &mut [f64],
    activations: &mut [f64],
    input_activations: &mut BTreeMap<(GraphNodeId, GraphNodeId), f64>,
    input_trace_truncated: &mut bool,
) {
    // The compiled schedule is dependency-first. Walking it backwards guarantees every consumer's
    // complete global activation is known before that contribution is propagated to dependencies,
    // aggregating diamonds in O(V + E) instead of enumerating root-to-node paths.
    for node in (0..plan.nodes.len()).rev() {
        let weight = workspace_activations[node];
        activations[node] += weight;
        if weight <= 0.0 {
            continue;
        }
        for (dependency, dependency_weight) in &workspace.nodes[node].input_weights {
            if *dependency != node && *dependency_weight > 0.0 {
                let contribution = weight * dependency_weight;
                record_input_activation(
                    input_activations,
                    (
                        plan.nodes[*dependency].id.clone(),
                        plan.nodes[node].id.clone(),
                    ),
                    contribution,
                    input_trace_truncated,
                );
                workspace_activations[*dependency] += contribution;
            }
        }
    }
}

fn record_input_activation(
    input_activations: &mut BTreeMap<(GraphNodeId, GraphNodeId), f64>,
    edge: (GraphNodeId, GraphNodeId),
    contribution: f64,
    truncated: &mut bool,
) {
    if let Some(weight) = input_activations.get_mut(&edge) {
        *weight += contribution;
        return;
    }
    if input_activations.len() < MAX_GRAPH_INPUT_TRACE {
        input_activations.insert(edge, contribution);
        return;
    }
    *truncated = true;
    if input_activations
        .last_key_value()
        .is_some_and(|(last, _)| edge < *last)
    {
        input_activations.pop_last();
        input_activations.insert(edge, contribution);
    }
}

fn transition_progress(elapsed: Tick, duration: Tick) -> f64 {
    if duration.0 <= 0 {
        1.0
    } else {
        (elapsed.0 as f64 / duration.0 as f64).clamp(0.0, 1.0)
    }
}

fn curve_progress(curve: GraphTransitionCurve, value: f64) -> f64 {
    match curve {
        GraphTransitionCurve::Linear => value,
        GraphTransitionCurve::EaseIn => value * value,
        GraphTransitionCurve::EaseOut => 1.0 - (1.0 - value) * (1.0 - value),
        GraphTransitionCurve::EaseInOut => {
            if value < 0.5 {
                2.0 * value * value
            } else {
                1.0 - (-2.0 * value + 2.0).powi(2) / 2.0
            }
        }
        GraphTransitionCurve::Smoothstep => value * value * (3.0 - 2.0 * value),
    }
}

fn apply_rate(tick: Tick, rate: RationalRate) -> Tick {
    if rate.denominator == 0 {
        return Tick::ZERO;
    }
    let numerator = i128::from(tick.0) * i128::from(rate.numerator);
    let scaled = numerator / i128::from(rate.denominator);
    Tick(i64::try_from(scaled).unwrap_or(if scaled.is_negative() {
        i64::MIN
    } else {
        i64::MAX
    }))
}

fn quaternion_dot(left: [f64; 4], right: [f64; 4]) -> f64 {
    left[0].mul_add(
        right[0],
        left[1].mul_add(right[1], left[2].mul_add(right[2], left[3] * right[3])),
    )
}

fn normalize_quaternion(value: [f64; 4]) -> [f64; 4] {
    let length = quaternion_dot(value, value).sqrt();
    if length <= 1.0e-12 || !length.is_finite() {
        [0.0, 0.0, 0.0, 1.0]
    } else {
        value.map(|part| part / length)
    }
}

fn quaternion_inverse(value: [f64; 4]) -> [f64; 4] {
    [-value[0], -value[1], -value[2], value[3]]
}

fn quaternion_multiply(left: [f64; 4], right: [f64; 4]) -> [f64; 4] {
    [
        left[3].mul_add(
            right[0],
            left[0].mul_add(right[3], left[1] * right[2] - left[2] * right[1]),
        ),
        left[3].mul_add(
            right[1],
            (-left[0]).mul_add(right[2], left[1] * right[3] + left[2] * right[0]),
        ),
        left[3].mul_add(
            right[2],
            left[0].mul_add(right[1], -left[1] * right[0] + left[2] * right[3]),
        ),
        (-left[0]).mul_add(
            right[0],
            (-left[1]).mul_add(right[1], (-left[2]).mul_add(right[2], left[3] * right[3])),
        ),
    ]
}

fn nlerp_quaternion(from: [f64; 4], mut to: [f64; 4], alpha: f64) -> [f64; 4] {
    if quaternion_dot(from, to) < 0.0 {
        to = to.map(|part| -part);
    }
    normalize_quaternion(std::array::from_fn(|index| {
        (to[index] - from[index]).mul_add(alpha, from[index])
    }))
}
