#![allow(clippy::too_many_lines)]

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{AnimValue, Binding, CompiledSequence, SequenceId, Severity, Tick, ValueKind};

use super::{
    AnimationGraph, AnimationGraphId, GraphBindingSignature, GraphBlend1dPoint, GraphBlend2dPoint,
    GraphBlend2dTriangle, GraphBlendMode, GraphCondition, GraphConditionTest, GraphEventPolicy,
    GraphInterruptionPolicy, GraphMaskId, GraphNodeId, GraphNodeKind, GraphOutsideHullMode,
    GraphParameterId, GraphParameterKind, GraphParameterValue, GraphStateId, GraphTransitionCurve,
    GraphTransitionId, GraphTransitionMode, GraphWeight, RationalRate,
    ANIMATION_GRAPH_SCHEMA_VERSION, MAX_BLEND_2D_COORDINATE_ABS,
};

const ABSOLUTE_MAX_NODES: usize = 4_096;
const ABSOLUTE_MAX_POSE_EDGES: usize = 16_384;
const ABSOLUTE_MAX_PARAMETERS: usize = 1_024;
const ABSOLUTE_MAX_STATES: usize = 1_024;
const ABSOLUTE_MAX_TRANSITIONS: usize = 16_384;
const ABSOLUTE_MAX_TRANSITIONS_PER_UPDATE: usize = 64;
const ABSOLUTE_MAX_EVENT_CANDIDATES: usize = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationGraphIssueCode {
    UnsupportedSchemaVersion,
    EmptyStableId,
    DuplicateStableId,
    LimitExceeded,
    EmptyReferencePose,
    InvalidReferenceValue,
    DuplicateReferenceBinding,
    MissingReferenceBinding,
    ReferenceTypeMismatch,
    MissingNode,
    PoseCycle,
    MissingSequence,
    InvalidRate,
    InvalidParameter,
    ParameterTypeMismatch,
    InvalidWeight,
    InvalidMask,
    InvalidBlendSpace,
    InvalidTriangle,
    InvalidStateMachine,
    InvalidTransition,
    UnsupportedTransitionMode,
    UnsupportedAdditiveType,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimationGraphDiagnostic {
    pub severity: Severity,
    pub code: AnimationGraphIssueCode,
    pub location: String,
    pub message: String,
    pub recovery: String,
    #[serde(default)]
    pub node_id: Option<GraphNodeId>,
    /// Optional stable input/field name for port-addressable editor diagnostics.
    #[serde(default)]
    pub port: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimationGraphCompileError {
    pub diagnostics: Vec<AnimationGraphDiagnostic>,
}

impl std::fmt::Display for AnimationGraphCompileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
            .count();
        write!(
            formatter,
            "animation graph compilation failed with {count} error(s)"
        )
    }
}

impl std::error::Error for AnimationGraphCompileError {}

#[derive(Clone, Debug)]
pub struct CompiledAnimationGraph {
    pub id: AnimationGraphId,
    pub stable_hash: String,
    pub diagnostics: Vec<AnimationGraphDiagnostic>,
    pub(super) bindings: Vec<Binding>,
    pub(super) reference_values: Vec<AnimValue>,
    pub(super) parameters: Vec<CompiledParameter>,
    pub(super) nodes: Vec<CompiledNode>,
    pub(super) output_node: usize,
    pub(super) masks: Vec<CompiledMask>,
    pub(super) state_machine: Option<CompiledStateMachine>,
    pub(super) sequences: BTreeMap<SequenceId, CompiledSequence>,
    pub(super) clip_nodes: Vec<usize>,
    pub(super) event_policy: GraphEventPolicy,
    pub(super) max_pose_edges: usize,
    pub(super) max_transitions_per_update: usize,
    pub(super) max_event_candidates_per_update: usize,
}

#[derive(Clone, Debug)]
pub(super) struct CompiledParameter {
    pub id: GraphParameterId,
    pub kind: GraphParameterKind,
    pub default: GraphParameterValue,
    pub used_as_weight: bool,
}

#[derive(Clone, Debug)]
pub(super) struct CompiledMask {
    pub id: GraphMaskId,
    pub weights: Vec<f64>,
}

#[derive(Clone, Debug)]
pub(super) struct CompiledNode {
    pub id: GraphNodeId,
    pub kind: CompiledNodeKind,
}

#[derive(Clone, Debug)]
pub(super) enum CompiledWeight {
    Constant(f64),
    Parameter(usize),
}

#[derive(Clone, Debug)]
pub(super) struct CompiledBlend1dPoint {
    pub position: f64,
    pub node: usize,
}

#[derive(Clone, Debug)]
pub(super) struct CompiledBlend2dPoint {
    pub id: super::GraphBlendPointId,
    pub position: [f64; 2],
    pub node: usize,
}

#[derive(Clone, Debug)]
pub(super) struct CompiledTriangle {
    pub indices: [usize; 3],
}

#[derive(Clone, Debug)]
pub(super) enum CompiledNodeKind {
    ReferencePose,
    Clip {
        sequence: SequenceId,
        rate: RationalRate,
        start_tick: Tick,
        clip_slot: usize,
    },
    Blend {
        mode: GraphBlendMode,
        inputs: Vec<(usize, CompiledWeight)>,
    },
    Blend1d {
        parameter: usize,
        points: Vec<CompiledBlend1dPoint>,
    },
    Blend2d {
        parameter_x: usize,
        parameter_y: usize,
        points: Vec<CompiledBlend2dPoint>,
        triangles: Vec<CompiledTriangle>,
        boundary_edges: Vec<[usize; 2]>,
        outside_hull: GraphOutsideHullMode,
    },
    OverrideLayer {
        base: usize,
        layer: usize,
        weight: CompiledWeight,
        mask: Option<usize>,
    },
    AdditiveLayer {
        base: usize,
        layer: usize,
        weight: CompiledWeight,
        mask: Option<usize>,
    },
}

#[derive(Clone, Debug)]
pub(super) struct CompiledStateMachine {
    pub initial_state: usize,
    pub states: Vec<CompiledState>,
    pub state_lookup: BTreeMap<GraphStateId, usize>,
    pub transitions: Vec<CompiledTransition>,
}

#[derive(Clone, Debug)]
pub(super) struct CompiledState {
    pub id: GraphStateId,
    pub node: usize,
    pub reset_on_entry: bool,
}

#[derive(Clone, Debug)]
pub(super) struct CompiledTransition {
    pub id: GraphTransitionId,
    pub from: usize,
    pub to: usize,
    pub priority: i32,
    pub duration: Tick,
    pub curve: GraphTransitionCurve,
    pub interruption: GraphInterruptionPolicy,
    pub conditions: Vec<CompiledCondition>,
}

#[derive(Clone, Debug)]
pub(super) struct CompiledCondition {
    pub parameter: usize,
    pub test: GraphConditionTest,
}

impl CompiledAnimationGraph {
    /// Compile against an explicit immutable sequence catalog. Only referenced sequences are cloned
    /// into the resulting plan, keeping runtime resolution independent from an asset store.
    pub fn compile(
        graph: &AnimationGraph,
        sequences: &BTreeMap<SequenceId, CompiledSequence>,
    ) -> Result<Self, AnimationGraphCompileError> {
        Compiler::new(graph, sequences).compile()
    }

    #[must_use]
    pub fn referenced_sequence_ids(&self) -> Vec<SequenceId> {
        self.sequences.keys().cloned().collect()
    }

    #[must_use]
    pub fn binding_signatures(&self) -> Vec<GraphBindingSignature> {
        self.bindings
            .iter()
            .map(|binding| GraphBindingSignature {
                binding: binding.clone(),
                reference_kind: binding.value_kind,
            })
            .collect()
    }

    /// Stable topological pose schedule. Dependencies always precede their consumer.
    #[must_use]
    pub fn schedule(&self) -> Vec<GraphNodeId> {
        self.nodes.iter().map(|node| node.id.clone()).collect()
    }

    #[must_use]
    pub fn mask_ids(&self) -> Vec<GraphMaskId> {
        self.masks.iter().map(|mask| mask.id.clone()).collect()
    }

    #[must_use]
    pub fn parameter_defaults(&self) -> Vec<super::GraphParameterSnapshot> {
        self.parameters
            .iter()
            .map(|parameter| super::GraphParameterSnapshot {
                id: parameter.id.clone(),
                value: parameter.default.clone(),
            })
            .collect()
    }
}

struct Compiler<'a> {
    graph: &'a AnimationGraph,
    catalog: &'a BTreeMap<SequenceId, CompiledSequence>,
    diagnostics: Vec<AnimationGraphDiagnostic>,
}

impl<'a> Compiler<'a> {
    const fn new(
        graph: &'a AnimationGraph,
        catalog: &'a BTreeMap<SequenceId, CompiledSequence>,
    ) -> Self {
        Self {
            graph,
            catalog,
            diagnostics: Vec::new(),
        }
    }

    fn compile(mut self) -> Result<CompiledAnimationGraph, AnimationGraphCompileError> {
        self.validate_header_and_limits();
        let (bindings, reference_values) = self.compile_reference_pose();
        let (parameters, parameter_lookup) = self.compile_parameters();
        let (masks, mask_lookup) = self.compile_masks(&bindings);
        let authored_nodes = self.canonical_nodes();
        self.validate_dependencies(&authored_nodes);
        let schedule = self.topological_schedule(&authored_nodes);
        let node_lookup: BTreeMap<_, _> = schedule
            .iter()
            .enumerate()
            .map(|(index, id)| (id.clone(), index))
            .collect();
        let (nodes, sequences, clip_nodes, weight_parameters) = self.compile_nodes(
            &schedule,
            &authored_nodes,
            &node_lookup,
            &parameter_lookup,
            &mask_lookup,
            &masks,
            &bindings,
            &reference_values,
        );
        let mut parameters = parameters;
        for index in weight_parameters {
            if let Some(parameter) = parameters.get_mut(index) {
                parameter.used_as_weight = true;
            }
        }
        let output_node = node_lookup
            .get(&self.graph.output)
            .copied()
            .unwrap_or_else(|| {
                self.error(
                    AnimationGraphIssueCode::MissingNode,
                    "graph.output",
                    format!("output node '{}' does not exist", self.graph.output),
                    "Connect the graph output to a valid pose node.",
                    None,
                    Some("output"),
                );
                0
            });
        let state_machine = self.compile_state_machine(&node_lookup, &parameter_lookup);

        self.diagnostics.sort_by(|left, right| {
            left.location
                .cmp(&right.location)
                .then(left.code.cmp(&right.code))
                .then(left.message.cmp(&right.message))
        });
        if self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
        {
            return Err(AnimationGraphCompileError {
                diagnostics: self.diagnostics,
            });
        }

        let mut compiled = CompiledAnimationGraph {
            id: self.graph.id.clone(),
            stable_hash: String::new(),
            diagnostics: self.diagnostics,
            bindings,
            reference_values,
            parameters,
            nodes,
            output_node,
            masks,
            state_machine,
            sequences,
            clip_nodes,
            event_policy: self.graph.event_policy.clone(),
            max_pose_edges: self.graph.limits.max_pose_edges,
            max_transitions_per_update: self.graph.limits.max_transitions_per_update,
            max_event_candidates_per_update: self.graph.limits.max_event_candidates_per_update,
        };
        compiled.stable_hash = stable_hash(&compiled);
        Ok(compiled)
    }

    fn validate_header_and_limits(&mut self) {
        if self.graph.schema_version != ANIMATION_GRAPH_SCHEMA_VERSION {
            self.error(
                AnimationGraphIssueCode::UnsupportedSchemaVersion,
                "graph.schema_version",
                format!(
                    "schema {} is unsupported; expected {}",
                    self.graph.schema_version, ANIMATION_GRAPH_SCHEMA_VERSION
                ),
                "Migrate the authored graph before compiling it.",
                None,
                None,
            );
        }
        if self.graph.id.as_str().trim().is_empty() {
            self.error(
                AnimationGraphIssueCode::EmptyStableId,
                "graph.id",
                "graph ID is empty",
                "Assign a persistent graph ID.",
                None,
                None,
            );
        }
        let limits = self.graph.limits;
        let checks = [
            ("max_nodes", limits.max_nodes, ABSOLUTE_MAX_NODES),
            (
                "max_pose_edges",
                limits.max_pose_edges,
                ABSOLUTE_MAX_POSE_EDGES,
            ),
            (
                "max_parameters",
                limits.max_parameters,
                ABSOLUTE_MAX_PARAMETERS,
            ),
            ("max_states", limits.max_states, ABSOLUTE_MAX_STATES),
            (
                "max_transitions",
                limits.max_transitions,
                ABSOLUTE_MAX_TRANSITIONS,
            ),
            (
                "max_transitions_per_update",
                limits.max_transitions_per_update,
                ABSOLUTE_MAX_TRANSITIONS_PER_UPDATE,
            ),
            (
                "max_event_candidates_per_update",
                limits.max_event_candidates_per_update,
                ABSOLUTE_MAX_EVENT_CANDIDATES,
            ),
        ];
        for (name, value, hard_max) in checks {
            if value == 0 || value > hard_max {
                self.error(
                    AnimationGraphIssueCode::LimitExceeded,
                    format!("graph.limits.{name}"),
                    format!("limit {value} must be within 1..={hard_max}"),
                    "Choose a positive bounded compile/runtime limit.",
                    None,
                    None,
                );
            }
        }
        for (location, actual, configured) in [
            ("graph.nodes", self.graph.nodes.len(), limits.max_nodes),
            (
                "graph.pose_edges",
                self.graph.nodes.iter().fold(0_usize, |count, node| {
                    count.saturating_add(node.kind.dependencies().len())
                }),
                limits.max_pose_edges,
            ),
            (
                "graph.parameters",
                self.graph.parameters.len(),
                limits.max_parameters,
            ),
            (
                "graph.state_machine.states",
                self.graph
                    .state_machine
                    .as_ref()
                    .map_or(0, |machine| machine.states.len()),
                limits.max_states,
            ),
            (
                "graph.state_machine.transitions",
                self.graph
                    .state_machine
                    .as_ref()
                    .map_or(0, |machine| machine.transitions.len()),
                limits.max_transitions,
            ),
        ] {
            if actual > configured {
                self.error(
                    AnimationGraphIssueCode::LimitExceeded,
                    location,
                    format!("count {actual} exceeds authored limit {configured}"),
                    "Reduce the graph or raise its bounded limit.",
                    None,
                    None,
                );
            }
        }
        if let GraphEventPolicy::Threshold { minimum_weight } = self.graph.event_policy {
            if !unit_weight(minimum_weight) {
                self.error(
                    AnimationGraphIssueCode::InvalidWeight,
                    "graph.event_policy.minimum_weight",
                    "event threshold must be finite and within [0, 1]",
                    "Use a normalized event threshold.",
                    None,
                    None,
                );
            }
        }
    }

    fn compile_reference_pose(&mut self) -> (Vec<Binding>, Vec<AnimValue>) {
        if self.graph.reference_pose.is_empty() {
            self.error(
                AnimationGraphIssueCode::EmptyReferencePose,
                "graph.reference_pose",
                "reference pose is empty",
                "Author an explicit typed baseline for every animated property.",
                None,
                None,
            );
        }
        let mut references = self.graph.reference_pose.clone();
        references.sort_by(|left, right| left.binding.cmp(&right.binding));
        let mut seen_paths = BTreeMap::new();
        for reference in &mut references {
            let location = format!(
                "graph.reference_pose.{}",
                reference.binding.path.display_path()
            );
            if !reference.value.is_finite()
                || reference.value.kind() != reference.binding.value_kind
                || (matches!(reference.value, AnimValue::Quaternion(_))
                    && reference.value.normalized_quaternion().is_none())
            {
                self.error(
                    AnimationGraphIssueCode::InvalidReferenceValue,
                    &location,
                    "reference value is non-finite, zero-length, or disagrees with its binding type",
                    "Supply a finite value of the declared type; quaternions must be normalizable.",
                    None,
                    None,
                );
            }
            reference.value = reference.value.clone().canonicalize();
            if seen_paths
                .insert(reference.binding.path.clone(), reference.binding.value_kind)
                .is_some()
            {
                self.error(
                    AnimationGraphIssueCode::DuplicateReferenceBinding,
                    &location,
                    "reference pose contains the same property path more than once",
                    "Keep one typed baseline per property path.",
                    None,
                    None,
                );
            }
        }
        (
            references
                .iter()
                .map(|entry| entry.binding.clone())
                .collect(),
            references.into_iter().map(|entry| entry.value).collect(),
        )
    }

    fn compile_parameters(
        &mut self,
    ) -> (Vec<CompiledParameter>, BTreeMap<GraphParameterId, usize>) {
        let mut authored = self.graph.parameters.clone();
        authored.sort_by(|left, right| left.id.cmp(&right.id));
        let mut parameters = Vec::with_capacity(authored.len());
        let mut lookup = BTreeMap::new();
        for parameter in authored {
            let location = format!("graph.parameters.{}", parameter.id);
            if parameter.id.as_str().trim().is_empty() {
                self.error(
                    AnimationGraphIssueCode::EmptyStableId,
                    &location,
                    "parameter ID is empty",
                    "Assign a persistent parameter ID.",
                    None,
                    None,
                );
            }
            if parameter.kind != parameter.default.kind() || !parameter.default.is_finite() {
                self.error(
                    AnimationGraphIssueCode::InvalidParameter,
                    &location,
                    "parameter default is non-finite or disagrees with its declared kind",
                    "Use a finite default of the declared parameter kind.",
                    None,
                    None,
                );
            }
            if matches!(parameter.default, GraphParameterValue::Trigger(true)) {
                self.error(
                    AnimationGraphIssueCode::InvalidParameter,
                    &location,
                    "trigger parameters must default to false",
                    "Fire triggers explicitly on a runtime instance.",
                    None,
                    None,
                );
            }
            let index = parameters.len();
            if lookup.insert(parameter.id.clone(), index).is_some() {
                self.error(
                    AnimationGraphIssueCode::DuplicateStableId,
                    &location,
                    "parameter ID is duplicated",
                    "Give every parameter a unique stable ID.",
                    None,
                    None,
                );
            }
            parameters.push(CompiledParameter {
                id: parameter.id,
                kind: parameter.kind,
                default: parameter.default,
                used_as_weight: false,
            });
        }
        (parameters, lookup)
    }

    fn compile_masks(
        &mut self,
        bindings: &[Binding],
    ) -> (Vec<CompiledMask>, BTreeMap<GraphMaskId, usize>) {
        let binding_lookup: BTreeMap<_, _> = bindings
            .iter()
            .enumerate()
            .map(|(index, binding)| (binding.path.clone(), index))
            .collect();
        let mut authored = self.graph.masks.clone();
        authored.sort_by(|left, right| left.id.cmp(&right.id));
        let mut masks = Vec::with_capacity(authored.len());
        let mut lookup = BTreeMap::new();
        for mask in authored {
            let location = format!("graph.masks.{}", mask.id);
            if !unit_weight(mask.default_weight) {
                self.error(
                    AnimationGraphIssueCode::InvalidMask,
                    &location,
                    "mask default weight must be finite and within [0, 1]",
                    "Use normalized mask weights.",
                    None,
                    None,
                );
            }
            let mut weights = vec![mask.default_weight; bindings.len()];
            let mut entries = mask.entries;
            entries.sort_by(|left, right| left.path.cmp(&right.path));
            let mut seen = BTreeSet::new();
            for entry in entries {
                if !unit_weight(entry.weight) || !seen.insert(entry.path.clone()) {
                    self.error(
                        AnimationGraphIssueCode::InvalidMask,
                        &location,
                        format!(
                            "invalid or duplicate mask entry '{}''",
                            entry.path.display_path()
                        ),
                        "Keep one normalized entry per property path.",
                        None,
                        None,
                    );
                }
                let Some(index) = binding_lookup.get(&entry.path) else {
                    self.error(
                        AnimationGraphIssueCode::InvalidMask,
                        &location,
                        format!(
                            "mask path '{}' is absent from the reference pose",
                            entry.path.display_path()
                        ),
                        "Add the property to the reference pose or remove the stale mask entry.",
                        None,
                        None,
                    );
                    continue;
                };
                weights[*index] = entry.weight;
            }
            let index = masks.len();
            if lookup.insert(mask.id.clone(), index).is_some() {
                self.error(
                    AnimationGraphIssueCode::DuplicateStableId,
                    &location,
                    "mask ID is duplicated",
                    "Give every mask a unique stable ID.",
                    None,
                    None,
                );
            }
            masks.push(CompiledMask {
                id: mask.id,
                weights,
            });
        }
        (masks, lookup)
    }

    fn canonical_nodes(&mut self) -> BTreeMap<GraphNodeId, super::GraphNode> {
        let mut nodes = BTreeMap::new();
        for node in &self.graph.nodes {
            if node.id.as_str().trim().is_empty() {
                self.error(
                    AnimationGraphIssueCode::EmptyStableId,
                    "graph.nodes",
                    "node ID is empty",
                    "Assign a persistent node ID.",
                    Some(node.id.clone()),
                    None,
                );
            }
            if nodes.insert(node.id.clone(), node.clone()).is_some() {
                self.error(
                    AnimationGraphIssueCode::DuplicateStableId,
                    format!("graph.nodes.{}", node.id),
                    "node ID is duplicated",
                    "Give every node a unique stable ID.",
                    Some(node.id.clone()),
                    None,
                );
            }
        }
        nodes
    }

    fn validate_dependencies(&mut self, nodes: &BTreeMap<GraphNodeId, super::GraphNode>) {
        for node in nodes.values() {
            for dependency in node.kind.dependencies() {
                if !nodes.contains_key(dependency) {
                    self.error(
                        AnimationGraphIssueCode::MissingNode,
                        format!("graph.nodes.{}", node.id),
                        format!("node references missing input '{dependency}'"),
                        "Reconnect the input to an existing node.",
                        Some(node.id.clone()),
                        Some("input"),
                    );
                }
            }
        }
    }

    fn topological_schedule(
        &mut self,
        nodes: &BTreeMap<GraphNodeId, super::GraphNode>,
    ) -> Vec<GraphNodeId> {
        fn visit(
            id: &GraphNodeId,
            nodes: &BTreeMap<GraphNodeId, super::GraphNode>,
            temporary: &mut BTreeSet<GraphNodeId>,
            permanent: &mut BTreeSet<GraphNodeId>,
            schedule: &mut Vec<GraphNodeId>,
            cycles: &mut BTreeSet<GraphNodeId>,
        ) {
            if permanent.contains(id) {
                return;
            }
            if !temporary.insert(id.clone()) {
                cycles.insert(id.clone());
                return;
            }
            if let Some(node) = nodes.get(id) {
                let mut dependencies = node.kind.dependencies();
                dependencies.sort();
                dependencies.dedup();
                for dependency in dependencies {
                    if nodes.contains_key(dependency) {
                        visit(dependency, nodes, temporary, permanent, schedule, cycles);
                    }
                }
            }
            temporary.remove(id);
            if permanent.insert(id.clone()) {
                schedule.push(id.clone());
            }
        }
        let mut temporary = BTreeSet::new();
        let mut permanent = BTreeSet::new();
        let mut schedule = Vec::with_capacity(nodes.len());
        let mut cycles = BTreeSet::new();
        for id in nodes.keys() {
            visit(
                id,
                nodes,
                &mut temporary,
                &mut permanent,
                &mut schedule,
                &mut cycles,
            );
        }
        for id in cycles {
            self.error(
                AnimationGraphIssueCode::PoseCycle,
                format!("graph.nodes.{id}"),
                "pose dependency cycle detected",
                "Keep the pose DAG acyclic; state-machine transitions may contain cycles.",
                Some(id),
                Some("input"),
            );
        }
        schedule
    }

    #[allow(clippy::too_many_arguments)]
    fn compile_nodes(
        &mut self,
        schedule: &[GraphNodeId],
        authored_nodes: &BTreeMap<GraphNodeId, super::GraphNode>,
        node_lookup: &BTreeMap<GraphNodeId, usize>,
        parameter_lookup: &BTreeMap<GraphParameterId, usize>,
        mask_lookup: &BTreeMap<GraphMaskId, usize>,
        masks: &[CompiledMask],
        bindings: &[Binding],
        reference_values: &[AnimValue],
    ) -> (
        Vec<CompiledNode>,
        BTreeMap<SequenceId, CompiledSequence>,
        Vec<usize>,
        BTreeSet<usize>,
    ) {
        let reference_lookup: BTreeMap<_, _> = bindings
            .iter()
            .enumerate()
            .map(|(index, binding)| (binding.clone(), index))
            .collect();
        let mut nodes = Vec::with_capacity(schedule.len());
        let mut resolved_sequences = BTreeMap::new();
        let mut clip_nodes = Vec::new();
        let mut weight_parameters = BTreeSet::new();
        for id in schedule {
            let Some(authored) = authored_nodes.get(id) else {
                continue;
            };
            let location = format!("graph.nodes.{id}");
            let compiled_kind = match &authored.kind {
                GraphNodeKind::ReferencePose => CompiledNodeKind::ReferencePose,
                GraphNodeKind::Clip {
                    sequence,
                    rate,
                    start_tick,
                } => {
                    if rate.denominator == 0 {
                        self.error(
                            AnimationGraphIssueCode::InvalidRate,
                            &location,
                            "clip rate denominator is zero",
                            "Use a non-zero rational denominator.",
                            Some(id.clone()),
                            Some("rate"),
                        );
                    }
                    if let Some(source) = self.catalog.get(sequence) {
                        let evaluation = source.evaluate(Tick::ZERO);
                        for value in evaluation.bindings {
                            match reference_lookup.get(&value.binding) {
                                Some(index)
                                    if bindings[*index].value_kind == value.value.kind() => {
                                        if let (
                                            AnimValue::Weights(reference),
                                            AnimValue::Weights(source),
                                        ) = (&reference_values[*index], &value.value)
                                        {
                                            if reference.len() != source.len() {
                                                self.error(
                                                    AnimationGraphIssueCode::ReferenceTypeMismatch,
                                                    &location,
                                                    format!(
                                                        "sequence '{}' weights width {} disagrees with reference width {}",
                                                        sequence,
                                                        source.len(),
                                                        reference.len()
                                                    ),
                                                    "Use the same morph-weight width in source and reference pose.",
                                                    Some(id.clone()),
                                                    Some("sequence"),
                                                );
                                            }
                                        }
                                    }
                                Some(_) => self.error(
                                    AnimationGraphIssueCode::ReferenceTypeMismatch,
                                    &location,
                                    format!(
                                        "sequence '{}' binding '{}' disagrees with the reference type",
                                        sequence,
                                        value.binding.path.display_path()
                                    ),
                                    "Correct the reference value type or source binding.",
                                    Some(id.clone()),
                                    Some("sequence"),
                                ),
                                None => self.error(
                                    AnimationGraphIssueCode::MissingReferenceBinding,
                                    &location,
                                    format!(
                                        "sequence '{}' emits '{}' without an explicit reference value",
                                        sequence,
                                        value.binding.path.display_path()
                                    ),
                                    "Add the source binding to graph.reference_pose.",
                                    Some(id.clone()),
                                    Some("sequence"),
                                ),
                            }
                        }
                        resolved_sequences.insert(sequence.clone(), source.clone());
                    } else {
                        self.error(
                            AnimationGraphIssueCode::MissingSequence,
                            &location,
                            format!("sequence '{sequence}' is missing from the compile catalog"),
                            "Load the referenced compiled sequence before graph compilation.",
                            Some(id.clone()),
                            Some("sequence"),
                        );
                    }
                    let clip_slot = clip_nodes.len();
                    clip_nodes.push(nodes.len());
                    CompiledNodeKind::Clip {
                        sequence: sequence.clone(),
                        rate: *rate,
                        start_tick: *start_tick,
                        clip_slot,
                    }
                }
                GraphNodeKind::Blend { mode, inputs } => {
                    if inputs.is_empty() {
                        self.error(
                            AnimationGraphIssueCode::InvalidWeight,
                            &location,
                            "blend has no inputs",
                            "Connect at least one pose input.",
                            Some(id.clone()),
                            Some("inputs"),
                        );
                    }
                    let mut canonical = inputs.clone();
                    canonical.sort_by(|left, right| left.node.cmp(&right.node));
                    let inputs = canonical
                        .iter()
                        .filter_map(|input| {
                            let node = node_lookup.get(&input.node).copied()?;
                            let weight = self.compile_weight(
                                &input.weight,
                                parameter_lookup,
                                &location,
                                id,
                                &mut weight_parameters,
                            );
                            Some((node, weight))
                        })
                        .collect();
                    CompiledNodeKind::Blend {
                        mode: *mode,
                        inputs,
                    }
                }
                GraphNodeKind::Blend1d { parameter, points } => {
                    let parameter = self.number_parameter(
                        parameter,
                        parameter_lookup,
                        &location,
                        id,
                        "parameter",
                    );
                    let points = self.compile_blend_1d(points, node_lookup, &location, id);
                    CompiledNodeKind::Blend1d { parameter, points }
                }
                GraphNodeKind::Blend2d {
                    parameter_x,
                    parameter_y,
                    points,
                    triangles,
                    outside_hull,
                } => {
                    let parameter_x = self.number_parameter(
                        parameter_x,
                        parameter_lookup,
                        &location,
                        id,
                        "parameter_x",
                    );
                    let parameter_y = self.number_parameter(
                        parameter_y,
                        parameter_lookup,
                        &location,
                        id,
                        "parameter_y",
                    );
                    let (points, triangles, boundary_edges) =
                        self.compile_blend_2d(points, triangles, node_lookup, &location, id);
                    CompiledNodeKind::Blend2d {
                        parameter_x,
                        parameter_y,
                        points,
                        triangles,
                        boundary_edges,
                        outside_hull: *outside_hull,
                    }
                }
                GraphNodeKind::OverrideLayer {
                    base,
                    layer,
                    weight,
                    mask,
                }
                | GraphNodeKind::AdditiveLayer {
                    base,
                    layer,
                    weight,
                    mask,
                } => {
                    let base = node_lookup.get(base).copied().unwrap_or(0);
                    let layer = node_lookup.get(layer).copied().unwrap_or(0);
                    let weight = self.compile_weight(
                        weight,
                        parameter_lookup,
                        &location,
                        id,
                        &mut weight_parameters,
                    );
                    let mask = mask.as_ref().and_then(|mask_id| {
                        mask_lookup.get(mask_id).copied().or_else(|| {
                            self.error(
                                AnimationGraphIssueCode::InvalidMask,
                                &location,
                                format!("layer references missing mask '{mask_id}'"),
                                "Select an existing property mask.",
                                Some(id.clone()),
                                Some("mask"),
                            );
                            None
                        })
                    });
                    if matches!(authored.kind, GraphNodeKind::AdditiveLayer { .. }) {
                        for (binding_index, binding) in bindings.iter().enumerate() {
                            let influence = mask
                                .and_then(|mask_index| masks.get(mask_index))
                                .and_then(|mask| mask.weights.get(binding_index))
                                .copied()
                                .unwrap_or(1.0);
                            if matches!(binding.value_kind, ValueKind::Boolean | ValueKind::String)
                                && influence > 0.0
                            {
                                self.error(
                                    AnimationGraphIssueCode::UnsupportedAdditiveType,
                                    &location,
                                    format!(
                                        "additive node cannot operate on discrete reference binding '{}'",
                                        binding.path.display_path()
                                    ),
                                    "Separate discrete properties into an override-only graph.",
                                    Some(id.clone()),
                                    Some("layer"),
                                );
                            }
                        }
                        CompiledNodeKind::AdditiveLayer {
                            base,
                            layer,
                            weight,
                            mask,
                        }
                    } else {
                        CompiledNodeKind::OverrideLayer {
                            base,
                            layer,
                            weight,
                            mask,
                        }
                    }
                }
            };
            nodes.push(CompiledNode {
                id: id.clone(),
                kind: compiled_kind,
            });
        }
        (nodes, resolved_sequences, clip_nodes, weight_parameters)
    }

    fn compile_weight(
        &mut self,
        weight: &GraphWeight,
        parameter_lookup: &BTreeMap<GraphParameterId, usize>,
        location: &str,
        node_id: &GraphNodeId,
        weight_parameters: &mut BTreeSet<usize>,
    ) -> CompiledWeight {
        match weight {
            GraphWeight::Constant(value) => {
                if !value.is_finite() || *value < 0.0 {
                    self.error(
                        AnimationGraphIssueCode::InvalidWeight,
                        location,
                        "graph weight must be finite and non-negative",
                        "Use a non-negative constant or number parameter.",
                        Some(node_id.clone()),
                        Some("weight"),
                    );
                }
                CompiledWeight::Constant(*value)
            }
            GraphWeight::Parameter(parameter_id) => {
                let parameter = self.number_parameter(
                    parameter_id,
                    parameter_lookup,
                    location,
                    node_id,
                    "weight",
                );
                weight_parameters.insert(parameter);
                CompiledWeight::Parameter(parameter)
            }
        }
    }

    fn number_parameter(
        &mut self,
        parameter_id: &GraphParameterId,
        parameter_lookup: &BTreeMap<GraphParameterId, usize>,
        location: &str,
        node_id: &GraphNodeId,
        port: &str,
    ) -> usize {
        let Some(index) = parameter_lookup.get(parameter_id).copied() else {
            self.error(
                AnimationGraphIssueCode::InvalidParameter,
                location,
                format!("parameter '{parameter_id}' does not exist"),
                "Create the parameter or repair this node input.",
                Some(node_id.clone()),
                Some(port),
            );
            return 0;
        };
        if self
            .graph
            .parameters
            .iter()
            .find(|parameter| parameter.id == *parameter_id)
            .map(|parameter| parameter.kind)
            != Some(GraphParameterKind::Number)
        {
            self.error(
                AnimationGraphIssueCode::ParameterTypeMismatch,
                location,
                format!("parameter '{parameter_id}' must be a number"),
                "Bind a number parameter to this input.",
                Some(node_id.clone()),
                Some(port),
            );
        }
        index
    }

    fn compile_blend_1d(
        &mut self,
        points: &[GraphBlend1dPoint],
        node_lookup: &BTreeMap<GraphNodeId, usize>,
        location: &str,
        node_id: &GraphNodeId,
    ) -> Vec<CompiledBlend1dPoint> {
        let mut points = points.to_vec();
        points.sort_by(|left, right| {
            left.position
                .total_cmp(&right.position)
                .then(left.id.cmp(&right.id))
        });
        if points.len() < 2
            || points.iter().any(|point| !point.position.is_finite())
            || points
                .windows(2)
                .any(|pair| pair[0].position.total_cmp(&pair[1].position).is_eq())
        {
            self.error(
                AnimationGraphIssueCode::InvalidBlendSpace,
                location,
                "1D blend requires at least two finite, uniquely positioned points",
                "Author two or more points with distinct positions.",
                Some(node_id.clone()),
                Some("points"),
            );
        }
        let mut seen_ids = BTreeSet::new();
        points
            .into_iter()
            .filter_map(|point| {
                if !seen_ids.insert(point.id.clone()) {
                    self.error(
                        AnimationGraphIssueCode::DuplicateStableId,
                        location,
                        format!("blend point ID '{}' is duplicated", point.id),
                        "Give every point a unique stable ID.",
                        Some(node_id.clone()),
                        Some("points"),
                    );
                }
                Some(CompiledBlend1dPoint {
                    position: point.position,
                    node: node_lookup.get(&point.node).copied()?,
                })
            })
            .collect()
    }

    fn compile_blend_2d(
        &mut self,
        points: &[GraphBlend2dPoint],
        triangles: &[GraphBlend2dTriangle],
        node_lookup: &BTreeMap<GraphNodeId, usize>,
        location: &str,
        node_id: &GraphNodeId,
    ) -> (
        Vec<CompiledBlend2dPoint>,
        Vec<CompiledTriangle>,
        Vec<[usize; 2]>,
    ) {
        let mut points = points.to_vec();
        points.sort_by(|left, right| left.id.cmp(&right.id));
        let mut point_lookup = BTreeMap::new();
        let mut coordinate_lookup = BTreeMap::new();
        let mut compiled_points = Vec::with_capacity(points.len());
        for point in points {
            let finite = point.position.iter().all(|part| part.is_finite());
            let bounded = finite
                && point
                    .position
                    .iter()
                    .all(|part| part.abs() <= MAX_BLEND_2D_COORDINATE_ABS);
            if !finite {
                self.error(
                    AnimationGraphIssueCode::InvalidBlendSpace,
                    location,
                    "2D blend point has a non-finite coordinate",
                    "Use finite Cartesian coordinates.",
                    Some(node_id.clone()),
                    Some("points"),
                );
            } else if !bounded {
                self.error(
                    AnimationGraphIssueCode::InvalidBlendSpace,
                    location,
                    format!(
                        "2D blend coordinate exceeds the absolute geometry limit {MAX_BLEND_2D_COORDINATE_ABS:e}"
                    ),
                    "Scale the authored Cartesian blend space below the documented geometry limit.",
                    Some(node_id.clone()),
                    Some("points"),
                );
            }
            if bounded {
                let coordinate = point.position.map(canonical_coordinate_bits);
                if let Some(existing) = coordinate_lookup.insert(coordinate, point.id.clone()) {
                    self.error(
                        AnimationGraphIssueCode::InvalidBlendSpace,
                        location,
                        format!(
                            "blend points '{}' and '{}' have the same Cartesian coordinate",
                            existing, point.id
                        ),
                        "Give every 2D blend sample a unique Cartesian coordinate.",
                        Some(node_id.clone()),
                        Some("points"),
                    );
                }
            }
            let index = compiled_points.len();
            if point_lookup.contains_key(&point.id) {
                self.error(
                    AnimationGraphIssueCode::DuplicateStableId,
                    location,
                    format!("blend point ID '{}' is duplicated", point.id),
                    "Give every point a unique stable ID.",
                    Some(node_id.clone()),
                    Some("points"),
                );
            } else {
                point_lookup.insert(point.id.clone(), index);
            }
            compiled_points.push(CompiledBlend2dPoint {
                id: point.id,
                position: point.position,
                node: node_lookup.get(&point.node).copied().unwrap_or(0),
            });
        }
        if compiled_points.len() < 3 || triangles.is_empty() {
            self.error(
                AnimationGraphIssueCode::InvalidBlendSpace,
                location,
                "2D blend requires at least three points and one authored triangle",
                "Triangulate the authored Cartesian sample set.",
                Some(node_id.clone()),
                Some("triangles"),
            );
        }
        let mut authored_triangles = triangles.to_vec();
        for triangle in &mut authored_triangles {
            triangle.points.sort();
        }
        authored_triangles.sort_by(|left, right| left.points.cmp(&right.points));
        let mut compiled_triangles = Vec::with_capacity(authored_triangles.len());
        let mut seen_triangles = BTreeSet::new();
        for triangle in authored_triangles {
            if triangle.points[0] == triangle.points[1] || triangle.points[1] == triangle.points[2]
            {
                self.error(
                    AnimationGraphIssueCode::InvalidTriangle,
                    location,
                    "triangle repeats a blend point",
                    "Connect each triangle to three distinct stable point IDs.",
                    Some(node_id.clone()),
                    Some("triangles"),
                );
                continue;
            }
            if !seen_triangles.insert(triangle.points.clone()) {
                self.error(
                    AnimationGraphIssueCode::InvalidTriangle,
                    location,
                    "the same Cartesian triangle is authored more than once",
                    "Keep one triangle for each stable point-ID triplet.",
                    Some(node_id.clone()),
                    Some("triangles"),
                );
                continue;
            }
            let indices = triangle
                .points
                .map(|point| point_lookup.get(&point).copied());
            let Some(indices) = indices
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .and_then(|values| <[usize; 3]>::try_from(values).ok())
            else {
                self.error(
                    AnimationGraphIssueCode::InvalidTriangle,
                    location,
                    "triangle references a missing blend point",
                    "Repair the authored triangle point IDs.",
                    Some(node_id.clone()),
                    Some("triangles"),
                );
                continue;
            };
            let [a, b, c] = indices.map(|index| compiled_points[index].position);
            let area = triangle_area2(a, b, c);
            if !area.is_finite() || area.abs() <= 1.0e-12 {
                self.error(
                    AnimationGraphIssueCode::InvalidTriangle,
                    location,
                    "triangle has non-finite, degenerate or collinear derived geometry",
                    "Author a bounded non-zero-area Cartesian triangle.",
                    Some(node_id.clone()),
                    Some("triangles"),
                );
                continue;
            }
            compiled_triangles.push(CompiledTriangle { indices });
        }
        let boundary_edges =
            self.validate_blend_2d_mesh(&compiled_points, &compiled_triangles, location, node_id);
        (compiled_points, compiled_triangles, boundary_edges)
    }

    fn validate_blend_2d_mesh(
        &mut self,
        points: &[CompiledBlend2dPoint],
        triangles: &[CompiledTriangle],
        location: &str,
        node_id: &GraphNodeId,
    ) -> Vec<[usize; 2]> {
        if triangles.is_empty() {
            return Vec::new();
        }

        let mut used_points = vec![false; points.len()];
        let mut edge_owners = BTreeMap::<[usize; 2], Vec<usize>>::new();
        for (triangle_index, triangle) in triangles.iter().enumerate() {
            for point in triangle.indices {
                used_points[point] = true;
            }
            for edge in triangle_edges(triangle.indices) {
                edge_owners.entry(edge).or_default().push(triangle_index);
            }
        }

        let unused: Vec<_> = used_points
            .iter()
            .enumerate()
            .filter(|(_, used)| !**used)
            .map(|(index, _)| points[index].id.to_string())
            .collect();
        if !unused.is_empty() {
            self.error(
                AnimationGraphIssueCode::InvalidBlendSpace,
                location,
                format!(
                    "2D blend points are not covered by the authored triangle mesh: {}",
                    unused.join(", ")
                ),
                "Connect every authored sample to the one connected Cartesian mesh.",
                Some(node_id.clone()),
                Some("triangles"),
            );
        }

        for (edge, owners) in &edge_owners {
            if owners.len() > 2 {
                self.error(
                    AnimationGraphIssueCode::InvalidTriangle,
                    location,
                    format!(
                        "non-manifold edge '{}' to '{}' belongs to {} triangles",
                        points[edge[0]].id,
                        points[edge[1]].id,
                        owners.len()
                    ),
                    "Triangulate a manifold surface where every edge has at most two owners.",
                    Some(node_id.clone()),
                    Some("triangles"),
                );
            }
        }

        let mut adjacency = vec![Vec::new(); triangles.len()];
        for owners in edge_owners.values().filter(|owners| owners.len() == 2) {
            adjacency[owners[0]].push(owners[1]);
            adjacency[owners[1]].push(owners[0]);
        }
        let mut reached = vec![false; triangles.len()];
        let mut pending = vec![0];
        reached[0] = true;
        while let Some(current) = pending.pop() {
            for &next in &adjacency[current] {
                if !reached[next] {
                    reached[next] = true;
                    pending.push(next);
                }
            }
        }
        if reached.iter().any(|reached| !reached) {
            self.error(
                AnimationGraphIssueCode::InvalidBlendSpace,
                location,
                "authored 2D triangles do not form one edge-connected mesh",
                "Join every triangle through shared edges or split disconnected regions into separate blend nodes.",
                Some(node_id.clone()),
                Some("triangles"),
            );
        }

        'overlap: for left in 0..triangles.len() {
            for right in (left + 1)..triangles.len() {
                if triangles_overlap(points, &triangles[left], &triangles[right]) {
                    self.error(
                        AnimationGraphIssueCode::InvalidTriangle,
                        location,
                        format!(
                            "authored Cartesian triangles {left} and {right} overlap in their interiors"
                        ),
                        "Use a non-overlapping manifold triangulation of the sample space.",
                        Some(node_id.clone()),
                        Some("triangles"),
                    );
                    break 'overlap;
                }
            }
        }

        edge_owners
            .into_iter()
            .filter_map(|(edge, owners)| (owners.len() == 1).then_some(edge))
            .collect()
    }

    fn compile_state_machine(
        &mut self,
        node_lookup: &BTreeMap<GraphNodeId, usize>,
        parameter_lookup: &BTreeMap<GraphParameterId, usize>,
    ) -> Option<CompiledStateMachine> {
        let machine = self.graph.state_machine.as_ref()?;
        let mut states = machine.states.clone();
        states.sort_by(|left, right| left.id.cmp(&right.id));
        let mut state_lookup = BTreeMap::new();
        let mut compiled_states = Vec::with_capacity(states.len());
        for state in states {
            let location = format!("graph.state_machine.states.{}", state.id);
            let node = node_lookup.get(&state.node).copied().unwrap_or_else(|| {
                self.error(
                    AnimationGraphIssueCode::MissingNode,
                    &location,
                    format!("state references missing node '{}'", state.node),
                    "Connect the state to a valid pose node.",
                    None,
                    Some("node"),
                );
                0
            });
            let index = compiled_states.len();
            if state_lookup.insert(state.id.clone(), index).is_some() {
                self.error(
                    AnimationGraphIssueCode::DuplicateStableId,
                    &location,
                    "state ID is duplicated",
                    "Give every state a unique stable ID.",
                    None,
                    None,
                );
            }
            compiled_states.push(CompiledState {
                id: state.id,
                node,
                reset_on_entry: state.reset_on_entry,
            });
        }
        let initial_state = state_lookup
            .get(&machine.initial_state)
            .copied()
            .unwrap_or_else(|| {
                self.error(
                    AnimationGraphIssueCode::InvalidStateMachine,
                    "graph.state_machine.initial_state",
                    format!("initial state '{}' does not exist", machine.initial_state),
                    "Select an existing initial state.",
                    None,
                    None,
                );
                0
            });
        let mut authored_transitions = machine.transitions.clone();
        authored_transitions.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then(left.id.cmp(&right.id))
        });
        if let Some(mut cycle) =
            unconditional_zero_duration_cycle(&authored_transitions, &state_lookup)
        {
            cycle.sort();
            self.error(
                AnimationGraphIssueCode::InvalidTransition,
                "graph.state_machine.transitions",
                format!(
                    "unconditional zero-duration transitions form a cycle: {}",
                    cycle
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" -> ")
                ),
                "Add a condition or positive duration to break the automatic zero-time cycle.",
                None,
                Some("transitions"),
            );
        }
        let mut seen = BTreeSet::new();
        let mut transitions = Vec::with_capacity(authored_transitions.len());
        for transition in authored_transitions {
            let location = format!("graph.state_machine.transitions.{}", transition.id);
            if !seen.insert(transition.id.clone()) {
                self.error(
                    AnimationGraphIssueCode::DuplicateStableId,
                    &location,
                    "transition ID is duplicated",
                    "Give every transition a unique stable ID.",
                    None,
                    None,
                );
            }
            if transition.duration.0 < 0 {
                self.error(
                    AnimationGraphIssueCode::InvalidTransition,
                    &location,
                    "transition duration is negative",
                    "Use a zero or positive integer duration.",
                    None,
                    Some("duration"),
                );
            }
            if transition.mode != GraphTransitionMode::Crossfade {
                self.error(
                    AnimationGraphIssueCode::UnsupportedTransitionMode,
                    &location,
                    "marker synchronization and inertialization are declared future modes, not runtime fallbacks",
                    "Use crossfade until the selected mode has a deterministic implementation.",
                    None,
                    Some("mode"),
                );
            }
            let from = state_lookup.get(&transition.from).copied();
            let to = state_lookup.get(&transition.to).copied();
            if from.is_none() || to.is_none() {
                self.error(
                    AnimationGraphIssueCode::InvalidTransition,
                    &location,
                    "transition references a missing source or target state",
                    "Repair the stable state references.",
                    None,
                    None,
                );
            }
            let conditions = transition
                .conditions
                .iter()
                .filter_map(|condition| {
                    self.compile_condition(condition, parameter_lookup, &location)
                })
                .collect();
            transitions.push(CompiledTransition {
                id: transition.id,
                from: from.unwrap_or(0),
                to: to.unwrap_or(0),
                priority: transition.priority,
                duration: transition.duration,
                curve: transition.curve,
                interruption: transition.interruption,
                conditions,
            });
        }
        Some(CompiledStateMachine {
            initial_state,
            states: compiled_states,
            state_lookup,
            transitions,
        })
    }

    fn compile_condition(
        &mut self,
        condition: &GraphCondition,
        parameter_lookup: &BTreeMap<GraphParameterId, usize>,
        location: &str,
    ) -> Option<CompiledCondition> {
        let Some(parameter) = parameter_lookup.get(&condition.parameter).copied() else {
            self.error(
                AnimationGraphIssueCode::InvalidParameter,
                location,
                format!(
                    "condition parameter '{}' does not exist",
                    condition.parameter
                ),
                "Create the parameter or remove the condition.",
                None,
                Some("conditions"),
            );
            return None;
        };
        let actual = self
            .graph
            .parameters
            .iter()
            .find(|candidate| candidate.id == condition.parameter)
            .map(|candidate| candidate.kind);
        let expected = match condition.test {
            GraphConditionTest::BooleanEquals(_) => GraphParameterKind::Boolean,
            GraphConditionTest::NumberEquals(value)
            | GraphConditionTest::NumberNotEquals(value)
            | GraphConditionTest::NumberGreaterThan(value)
            | GraphConditionTest::NumberGreaterOrEqual(value)
            | GraphConditionTest::NumberLessThan(value)
            | GraphConditionTest::NumberLessOrEqual(value) => {
                if !value.is_finite() {
                    self.error(
                        AnimationGraphIssueCode::InvalidTransition,
                        location,
                        "condition threshold is non-finite",
                        "Use a finite threshold.",
                        None,
                        Some("conditions"),
                    );
                }
                GraphParameterKind::Number
            }
            GraphConditionTest::IntegerEquals(_)
            | GraphConditionTest::IntegerNotEquals(_)
            | GraphConditionTest::IntegerGreaterThan(_)
            | GraphConditionTest::IntegerGreaterOrEqual(_)
            | GraphConditionTest::IntegerLessThan(_)
            | GraphConditionTest::IntegerLessOrEqual(_) => GraphParameterKind::Integer,
            GraphConditionTest::Triggered => GraphParameterKind::Trigger,
        };
        if actual != Some(expected) {
            self.error(
                AnimationGraphIssueCode::ParameterTypeMismatch,
                location,
                format!("condition expects {expected:?}, found {actual:?}"),
                "Use a condition matching the parameter kind.",
                None,
                Some("conditions"),
            );
        }
        Some(CompiledCondition {
            parameter,
            test: condition.test.clone(),
        })
    }

    fn error(
        &mut self,
        code: AnimationGraphIssueCode,
        location: impl Into<String>,
        message: impl Into<String>,
        recovery: impl Into<String>,
        node_id: Option<GraphNodeId>,
        port: Option<&str>,
    ) {
        self.diagnostics.push(AnimationGraphDiagnostic {
            severity: Severity::Error,
            code,
            location: location.into(),
            message: message.into(),
            recovery: recovery.into(),
            node_id,
            port: port.map(ToOwned::to_owned),
        });
    }
}

fn unit_weight(value: f64) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

fn triangle_area2(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]).mul_add(c[1] - a[1], -(b[1] - a[1]) * (c[0] - a[0]))
}

fn canonical_coordinate_bits(value: f64) -> u64 {
    if value == 0.0 {
        0
    } else {
        value.to_bits()
    }
}

fn triangle_edges([a, b, c]: [usize; 3]) -> [[usize; 2]; 3] {
    [
        canonical_edge(a, b),
        canonical_edge(b, c),
        canonical_edge(c, a),
    ]
}

const fn canonical_edge(left: usize, right: usize) -> [usize; 2] {
    if left < right {
        [left, right]
    } else {
        [right, left]
    }
}

fn triangles_overlap(
    points: &[CompiledBlend2dPoint],
    left: &CompiledTriangle,
    right: &CompiledTriangle,
) -> bool {
    let shared: Vec<_> = left
        .indices
        .iter()
        .copied()
        .filter(|index| right.indices.contains(index))
        .collect();
    if shared.len() == 3 {
        return true;
    }
    if shared.len() == 2 {
        let left_opposite = left
            .indices
            .iter()
            .copied()
            .find(|index| !shared.contains(index))
            .expect("a validated triangle has one vertex opposite a shared edge");
        let right_opposite = right
            .indices
            .iter()
            .copied()
            .find(|index| !shared.contains(index))
            .expect("a validated triangle has one vertex opposite a shared edge");
        let edge_a = points[shared[0]].position;
        let edge_b = points[shared[1]].position;
        let left_side = triangle_area2(edge_a, edge_b, points[left_opposite].position);
        let right_side = triangle_area2(edge_a, edge_b, points[right_opposite].position);
        return (left_side > 1.0e-12 && right_side > 1.0e-12)
            || (left_side < -1.0e-12 && right_side < -1.0e-12);
    }

    for index in left.indices {
        if !shared.contains(&index)
            && point_strictly_inside_triangle(points[index].position, right, points)
        {
            return true;
        }
    }
    for index in right.indices {
        if !shared.contains(&index)
            && point_strictly_inside_triangle(points[index].position, left, points)
        {
            return true;
        }
    }
    for left_edge in triangle_edges(left.indices) {
        for right_edge in triangle_edges(right.indices) {
            if segments_have_forbidden_intersection(left_edge, right_edge, points) {
                return true;
            }
        }
    }
    false
}

fn point_strictly_inside_triangle(
    point: [f64; 2],
    triangle: &CompiledTriangle,
    points: &[CompiledBlend2dPoint],
) -> bool {
    let [a, b, c] = triangle.indices.map(|index| points[index].position);
    let sides = [
        triangle_area2(a, b, point),
        triangle_area2(b, c, point),
        triangle_area2(c, a, point),
    ];
    sides.iter().all(|side| *side > 1.0e-12) || sides.iter().all(|side| *side < -1.0e-12)
}

fn segments_have_forbidden_intersection(
    [a_index, b_index]: [usize; 2],
    [c_index, d_index]: [usize; 2],
    points: &[CompiledBlend2dPoint],
) -> bool {
    let a = points[a_index].position;
    let b = points[b_index].position;
    let c = points[c_index].position;
    let d = points[d_index].position;
    let abc = triangle_area2(a, b, c);
    let abd = triangle_area2(a, b, d);
    let cda = triangle_area2(c, d, a);
    let cdb = triangle_area2(c, d, b);
    if opposite_strict_signs(abc, abd) && opposite_strict_signs(cda, cdb) {
        return true;
    }

    for (point_index, point, side, segment) in [
        (c_index, c, abc, [a_index, b_index]),
        (d_index, d, abd, [a_index, b_index]),
        (a_index, a, cda, [c_index, d_index]),
        (b_index, b, cdb, [c_index, d_index]),
    ] {
        if side.abs() <= 1.0e-12
            && point_on_segment(
                point,
                points[segment[0]].position,
                points[segment[1]].position,
            )
            && !segment.contains(&point_index)
        {
            return true;
        }
    }
    false
}

fn opposite_strict_signs(left: f64, right: f64) -> bool {
    (left > 1.0e-12 && right < -1.0e-12) || (left < -1.0e-12 && right > 1.0e-12)
}

fn point_on_segment(point: [f64; 2], start: [f64; 2], end: [f64; 2]) -> bool {
    point[0] >= start[0].min(end[0]) - 1.0e-12
        && point[0] <= start[0].max(end[0]) + 1.0e-12
        && point[1] >= start[1].min(end[1]) - 1.0e-12
        && point[1] <= start[1].max(end[1]) + 1.0e-12
}

fn unconditional_zero_duration_cycle(
    transitions: &[super::GraphTransition],
    state_lookup: &BTreeMap<GraphStateId, usize>,
) -> Option<Vec<GraphTransitionId>> {
    fn visit(
        state: usize,
        adjacency: &[Vec<(usize, GraphTransitionId)>],
        colors: &mut [u8],
        state_stack: &mut Vec<usize>,
        edge_stack: &mut Vec<GraphTransitionId>,
    ) -> Option<Vec<GraphTransitionId>> {
        colors[state] = 1;
        state_stack.push(state);
        for (target, transition_id) in &adjacency[state] {
            if colors[*target] == 0 {
                edge_stack.push(transition_id.clone());
                if let Some(cycle) = visit(*target, adjacency, colors, state_stack, edge_stack) {
                    return Some(cycle);
                }
                edge_stack.pop();
            } else if colors[*target] == 1 {
                let start = state_stack
                    .iter()
                    .position(|candidate| candidate == target)
                    .expect("an active DFS target is present in the state stack");
                let mut cycle = edge_stack[start..].to_vec();
                cycle.push(transition_id.clone());
                return Some(cycle);
            }
        }
        state_stack.pop();
        colors[state] = 2;
        None
    }

    let state_count = state_lookup
        .values()
        .copied()
        .max()
        .map_or(0, |index| index.saturating_add(1));
    let mut adjacency = vec![Vec::<(usize, GraphTransitionId)>::new(); state_count];
    for transition in transitions
        .iter()
        .filter(|transition| transition.duration == Tick::ZERO && transition.conditions.is_empty())
    {
        let (Some(from), Some(to)) = (
            state_lookup.get(&transition.from).copied(),
            state_lookup.get(&transition.to).copied(),
        ) else {
            continue;
        };
        adjacency[from].push((to, transition.id.clone()));
    }
    for edges in &mut adjacency {
        edges.sort_by(|left, right| left.1.cmp(&right.1).then(left.0.cmp(&right.0)));
    }

    let mut colors = vec![0; adjacency.len()];
    let mut state_stack = Vec::new();
    let mut edge_stack = Vec::new();
    for state in 0..adjacency.len() {
        if colors[state] == 0 {
            if let Some(cycle) = visit(
                state,
                &adjacency,
                &mut colors,
                &mut state_stack,
                &mut edge_stack,
            ) {
                return Some(cycle);
            }
        }
    }
    None
}

fn stable_hash(graph: &CompiledAnimationGraph) -> String {
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    // Every contributing collection has already been canonically ordered. Debug formatting is used
    // only as a byte encoding for this internal FNV-1a revision fingerprint, never as persistence.
    let sequence_revisions: Vec<_> = graph
        .sequences
        .iter()
        .map(|(id, sequence)| (id, &sequence.stable_hash))
        .collect();
    let canonical = format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}|{}",
        graph.id,
        graph.bindings,
        graph.reference_values,
        graph.parameters,
        graph.nodes,
        graph.output_node,
        graph.masks,
        graph.state_machine,
        sequence_revisions,
        graph.event_policy,
        graph.max_pose_edges,
        graph.max_transitions_per_update,
        graph.max_event_candidates_per_update,
    );
    let mut hash = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d_u128;
    for byte in canonical.bytes() {
        hash ^= u128::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:032x}")
}
