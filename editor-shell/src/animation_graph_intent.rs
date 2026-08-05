//! Versioned animation-graph authoring over the authoritative ECS/Loro op-stream.
//!
//! The editor submits one complete draft so native validation has a coherent view. Persistence remains
//! granular: graph metadata, nodes, node layout, edges, parameters, machines, states and transitions use
//! stable independent component fields. One save is one undoable transaction; deleting an element writes
//! an explicit tombstone instead of replacing an opaque graph blob.
#![expect(
    clippy::single_match_else,
    clippy::unnecessary_wraps,
    clippy::items_after_statements,
    reason = "shape lints over a validation module whose arms are deliberately parallel: every field               reader has the same match-and-diagnose form, and rewriting some of them as `if let` would               make the odd ones out look meaningful when they are not"
)]
#![expect(
    clippy::float_cmp,
    reason = "authored keyframe TIMES are compared for exact equality on purpose - two keys at the same               instant is the error being detected, and an epsilon would silently accept a document that               names one instant twice"
)]
#![expect(
    clippy::too_many_lines,
    reason = "each of these is ONE linear pass over a versioned authoring document - validate, migrate,               adapt - and the order of the checks is the contract. Splitting a pass into helpers hides               which field is read before which, which is exactly what a reader of a migration needs to see."
)]
#![expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "authored times and weights cross an i64/f64 wire boundary; the truncation is the intended               quantisation to the document's own units, not an accident"
)]
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use metrocalk_animation as animation;
use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::FlecsWorld;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub const ANIMATION_GRAPH: &str = "AnimationGraph";
pub const ANIMATION_GRAPH_SCHEMA_VERSION: u32 = 2;

const GRAPH_PREFIX: &str = "graph";
const NODE_PREFIX: &str = "node";
const EDGE_PREFIX: &str = "edge";
const PARAMETER_PREFIX: &str = "parameter";
const MACHINE_PREFIX: &str = "machine";
const STATE_PREFIX: &str = "state";
const TRANSITION_PREFIX: &str = "transition";

const MAX_NODES: usize = 512;
const MAX_EDGES: usize = 2_048;
const MAX_PARAMETERS: usize = 128;
const MAX_STATE_MACHINES: usize = 1;
const MAX_STATES: usize = 128;
const MAX_TRANSITIONS: usize = 1_024;
const MAX_CONDITIONS_PER_TRANSITION: usize = 32;
const MAX_ID_BYTES: usize = 128;
const MAX_NAME_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationGraphParameterKind {
    Float,
    Integer,
    Boolean,
    Trigger,
    Vec2,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnimationGraphValue {
    Number(f64),
    Boolean(bool),
    Vec2([f64; 2]),
}

impl AnimationGraphValue {
    #[must_use]
    pub fn is_finite(&self) -> bool {
        match self {
            Self::Number(value) => value.is_finite(),
            Self::Boolean(_) => true,
            Self::Vec2(value) => value.iter().all(|part| part.is_finite()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationGraphNodeKind {
    ReferencePose,
    Sequence,
    BlendNormalized,
    BlendDirect,
    #[serde(rename = "blend_1d")]
    Blend1d,
    #[serde(rename = "blend_2d_cartesian")]
    Blend2dCartesian,
    LayerOverride,
    LayerAdditive,
    StateMachine,
    Output,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationGraphCurve {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    Smoothstep,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationGraphInterruption {
    None,
    Source,
    Destination,
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationGraphConditionOperator {
    Greater,
    GreaterEqual,
    Less,
    LessEqual,
    Equal,
    NotEqual,
    IsTrue,
    IsFalse,
    Triggered,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationGraphPosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationGraphBlendSample {
    pub id: String,
    pub edge_id: String,
    pub position: [f64; 2],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationGraphNode {
    pub id: String,
    pub kind: AnimationGraphNodeKind,
    pub name: String,
    pub position: AnimationGraphPosition,
    pub enabled: bool,
    pub source_id: Option<String>,
    #[serde(default)]
    pub parameter_ids: Vec<String>,
    /// Durable keyed Blend1D/Cartesian Blend2D samples.
    #[serde(default)]
    pub samples: Vec<AnimationGraphBlendSample>,
    /// Legacy positional schema-v1 migration input. Canonical documents persist this empty.
    #[serde(default)]
    pub thresholds: Vec<f64>,
    #[serde(default)]
    pub triangles: Vec<[String; 3]>,
    #[serde(default)]
    pub mask_bindings: Vec<String>,
    pub state_machine_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationGraphEdge {
    pub id: String,
    pub from_node_id: String,
    pub from_port_id: String,
    pub to_node_id: String,
    pub to_port_id: String,
    pub enabled: bool,
    pub weight: Option<f64>,
    #[serde(default)]
    pub weight_parameter_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationGraphParameter {
    pub id: String,
    pub name: String,
    pub kind: AnimationGraphParameterKind,
    pub default_value: AnimationGraphValue,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationGraphCondition {
    pub id: String,
    pub parameter_id: String,
    pub operator: AnimationGraphConditionOperator,
    pub value: Option<AnimationGraphValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationGraphState {
    pub id: String,
    pub name: String,
    pub pose_node_id: String,
    pub reset_on_entry: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationGraphTransition {
    pub id: String,
    pub from_state_id: String,
    pub to_state_id: String,
    pub priority: i32,
    pub duration_tick: i64,
    pub curve: AnimationGraphCurve,
    pub interruption: AnimationGraphInterruption,
    #[serde(default)]
    pub conditions: Vec<AnimationGraphCondition>,
    pub exit_time: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationGraphStateMachine {
    pub id: String,
    pub name: String,
    pub entry_state_id: String,
    #[serde(default)]
    pub states: Vec<AnimationGraphState>,
    #[serde(default)]
    pub transitions: Vec<AnimationGraphTransition>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationGraphDocument {
    pub schema_version: u32,
    pub id: String,
    pub sequence_id: String,
    pub name: String,
    pub output_node_id: String,
    #[serde(default)]
    pub nodes: Vec<AnimationGraphNode>,
    #[serde(default)]
    pub edges: Vec<AnimationGraphEdge>,
    #[serde(default)]
    pub parameters: Vec<AnimationGraphParameter>,
    #[serde(default)]
    pub state_machines: Vec<AnimationGraphStateMachine>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationGraphDiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationGraphDiagnostic {
    pub id: String,
    pub severity: AnimationGraphDiagnosticSeverity,
    pub code: String,
    pub message: String,
    pub fix: Option<String>,
    pub node_id: Option<String>,
    pub edge_id: Option<String>,
    pub port_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationGraphLoad {
    pub document: Option<AnimationGraphDocument>,
    pub revision: String,
    pub diagnostics: Vec<AnimationGraphDiagnostic>,
    pub controller_id: Option<String>,
}

/// One editor parameter's mapping into the pure runtime graph. Vec2 parameters expand to two stable
/// scalar runtime parameters; native preview uses this table to split one transient editor value without
/// changing authored data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationGraphParameterRoute {
    pub editor_parameter_id: String,
    pub runtime_parameter_ids: Vec<String>,
    pub kind: AnimationGraphParameterKind,
}

/// Stable edge provenance retained outside the pure evaluator. The core graph embeds dependencies in
/// nodes, so this table maps a compiler port diagnostic or live dependency weight back to an editor edge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationGraphEdgeProvenance {
    pub edge_id: String,
    pub from_node_id: String,
    pub to_node_id: String,
    pub to_port_id: String,
}

/// Fully adapted pure graph plus bridge-only provenance. Layout and presentation facts never enter the
/// core graph and therefore cannot perturb its compiled hash.
#[derive(Clone, Debug)]
pub struct AdaptedAnimationGraph {
    pub graph: animation::AnimationGraph,
    pub parameter_routes: Vec<AnimationGraphParameterRoute>,
    pub edge_provenance: Vec<AnimationGraphEdgeProvenance>,
    pub diagnostics: Vec<AnimationGraphDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnimationGraphIntentError {
    StaleRevision { expected: String, actual: String },
    Admission(Vec<AnimationGraphDiagnostic>),
    SequenceAlreadyHasGraph { graph_id: String },
    GraphNotFound,
    GraphIdentityMismatch,
    Commit(String),
}

impl std::fmt::Display for AnimationGraphIntentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StaleRevision { expected, actual } => write!(
                formatter,
                "animation graph changed while this edit was open (expected {expected}, now {actual})"
            ),
            Self::Admission(diagnostics) => {
                let summary = diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                write!(formatter, "animation graph storage rejected the draft: {summary}")
            }
            Self::SequenceAlreadyHasGraph { graph_id } => write!(
                formatter,
                "this sequence already owns graph {graph_id}; delete it before creating another"
            ),
            Self::GraphNotFound => formatter.write_str("the animation graph no longer exists"),
            Self::GraphIdentityMismatch => {
                formatter.write_str("the requested graph identity does not match the stored graph")
            }
            Self::Commit(reason) => write!(formatter, "the animation graph edit was rejected: {reason}"),
        }
    }
}

impl std::error::Error for AnimationGraphIntentError {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredNodePayload {
    kind: AnimationGraphNodeKind,
    name: String,
    enabled: bool,
    source_id: Option<String>,
    parameter_ids: Vec<String>,
    #[serde(default)]
    samples: Vec<AnimationGraphBlendSample>,
    #[serde(default)]
    thresholds: Vec<f64>,
    triangles: Vec<[String; 3]>,
    mask_bindings: Vec<String>,
    state_machine_id: Option<String>,
}

impl From<&AnimationGraphNode> for StoredNodePayload {
    fn from(node: &AnimationGraphNode) -> Self {
        Self {
            kind: node.kind,
            name: node.name.clone(),
            enabled: node.enabled,
            source_id: node.source_id.clone(),
            parameter_ids: node.parameter_ids.clone(),
            samples: node.samples.clone(),
            thresholds: node.thresholds.clone(),
            triangles: node.triangles.clone(),
            mask_bindings: node.mask_bindings.clone(),
            state_machine_id: node.state_machine_id.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredEdgePayload {
    from_node_id: String,
    from_port_id: String,
    to_node_id: String,
    to_port_id: String,
    enabled: bool,
    weight: Option<f64>,
    #[serde(default)]
    weight_parameter_id: Option<String>,
}

impl From<&AnimationGraphEdge> for StoredEdgePayload {
    fn from(edge: &AnimationGraphEdge) -> Self {
        Self {
            from_node_id: edge.from_node_id.clone(),
            from_port_id: edge.from_port_id.clone(),
            to_node_id: edge.to_node_id.clone(),
            to_port_id: edge.to_port_id.clone(),
            enabled: edge.enabled,
            weight: edge.weight,
            weight_parameter_id: edge.weight_parameter_id.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredParameterPayload {
    name: String,
    kind: AnimationGraphParameterKind,
    default_value: AnimationGraphValue,
    min: Option<f64>,
    max: Option<f64>,
}

impl From<&AnimationGraphParameter> for StoredParameterPayload {
    fn from(parameter: &AnimationGraphParameter) -> Self {
        Self {
            name: parameter.name.clone(),
            kind: parameter.kind,
            default_value: parameter.default_value.clone(),
            min: parameter.min,
            max: parameter.max,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredStatePayload {
    name: String,
    pose_node_id: String,
    reset_on_entry: bool,
}

impl From<&AnimationGraphState> for StoredStatePayload {
    fn from(state: &AnimationGraphState) -> Self {
        Self {
            name: state.name.clone(),
            pose_node_id: state.pose_node_id.clone(),
            reset_on_entry: state.reset_on_entry,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredTransitionPayload {
    from_state_id: String,
    to_state_id: String,
    priority: i32,
    duration_tick: i64,
    curve: AnimationGraphCurve,
    interruption: AnimationGraphInterruption,
    conditions: Vec<AnimationGraphCondition>,
    exit_time: Option<f64>,
}

impl From<&AnimationGraphTransition> for StoredTransitionPayload {
    fn from(transition: &AnimationGraphTransition) -> Self {
        Self {
            from_state_id: transition.from_state_id.clone(),
            to_state_id: transition.to_state_id.clone(),
            priority: transition.priority,
            duration_tick: transition.duration_tick,
            curve: transition.curve,
            interruption: transition.interruption,
            conditions: transition.conditions.clone(),
            exit_time: transition.exit_time,
        }
    }
}

#[derive(Default)]
struct StoredRecords {
    graph: BTreeMap<String, FieldValue>,
    nodes: BTreeMap<String, BTreeMap<String, FieldValue>>,
    edges: BTreeMap<String, BTreeMap<String, FieldValue>>,
    parameters: BTreeMap<String, BTreeMap<String, FieldValue>>,
    machines: BTreeMap<String, BTreeMap<String, FieldValue>>,
    states: BTreeMap<(String, String), BTreeMap<String, FieldValue>>,
    transitions: BTreeMap<(String, String), BTreeMap<String, FieldValue>>,
}

/// Deterministically upgrades the positional compatibility fields accepted by schema v1. The old format
/// did not retain the association itself, so migration uses lexical edge identity and reports that fact
/// instead of pretending an edge-array order was authoritative.
fn canonicalize_animation_graph_document(
    document: &AnimationGraphDocument,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) -> AnimationGraphDocument {
    let mut canonical = document.clone();
    if document.schema_version != 1 {
        return canonical;
    }
    canonical.schema_version = ANIMATION_GRAPH_SCHEMA_VERSION;
    diagnostics.push(diagnostic(
        AnimationGraphDiagnosticSeverity::Warning,
        "legacy_graph_schema_migrated",
        "Animation graph schema 1 was upgraded to canonical schema 2 with stable sample and edge-weight identities.",
        Some("Review migrated blend associations once, then save to persist schema 2.".into()),
        None,
        None,
        None,
    ));
    for node_index in 0..canonical.nodes.len() {
        let node_id = canonical.nodes[node_index].id.clone();
        let kind = canonical.nodes[node_index].kind;
        let mut incoming: Vec<_> = canonical
            .edges
            .iter()
            .filter(|edge| edge.enabled && edge.to_node_id == node_id && edge.to_port_id == "poses")
            .map(|edge| (edge.id.clone(), edge.from_node_id.clone()))
            .collect();
        incoming.sort_by(|left, right| left.0.cmp(&right.0));

        if matches!(
            kind,
            AnimationGraphNodeKind::BlendNormalized | AnimationGraphNodeKind::BlendDirect
        ) && !incoming.is_empty()
            && canonical.nodes[node_index].parameter_ids.len() == incoming.len()
        {
            let positional = canonical.nodes[node_index].parameter_ids.clone();
            for ((edge_id, _), parameter_id) in incoming.iter().zip(positional) {
                if let Some(edge) = canonical.edges.iter_mut().find(|edge| edge.id == *edge_id) {
                    if edge.weight_parameter_id.is_none() {
                        edge.weight_parameter_id = Some(parameter_id);
                    }
                }
            }
            canonical.nodes[node_index].parameter_ids.clear();
            diagnostics.push(node_migration_diagnostic(
                &node_id,
                "legacy_positional_weight_migrated",
                "Legacy positional blend weights were mapped by lexical edge identity; the exact old association cannot be proven after prior persistence ordering.",
                "Review the per-connection weight parameters once; future edits keep the association by edge ID.",
            ));
        }

        if !matches!(
            kind,
            AnimationGraphNodeKind::Blend1d | AnimationGraphNodeKind::Blend2dCartesian
        ) {
            continue;
        }
        if canonical.nodes[node_index].samples.is_empty()
            && !canonical.nodes[node_index].thresholds.is_empty()
        {
            let thresholds = canonical.nodes[node_index].thresholds.clone();
            let mut samples = Vec::new();
            for (index, (edge_id, _)) in incoming.iter().enumerate() {
                let position = if kind == AnimationGraphNodeKind::Blend1d {
                    thresholds.get(index).copied().map(|x| [x, 0.0])
                } else {
                    thresholds
                        .get(index * 2)
                        .copied()
                        .zip(thresholds.get(index * 2 + 1).copied())
                        .map(|(x, y)| [x, y])
                };
                if let Some(position) = position {
                    samples.push(AnimationGraphBlendSample {
                        id: format!("sample-{edge_id}"),
                        edge_id: edge_id.clone(),
                        position,
                    });
                }
            }
            if kind == AnimationGraphNodeKind::Blend2dCartesian {
                let sample_ids: BTreeSet<_> =
                    samples.iter().map(|sample| sample.id.clone()).collect();
                let by_edge: BTreeMap<_, _> = samples
                    .iter()
                    .map(|sample| (sample.edge_id.clone(), sample.id.clone()))
                    .collect();
                let mut by_source: BTreeMap<String, Vec<String>> = BTreeMap::new();
                for sample in &samples {
                    if let Some((_, source)) =
                        incoming.iter().find(|(edge, _)| edge == &sample.edge_id)
                    {
                        by_source
                            .entry(source.clone())
                            .or_default()
                            .push(sample.id.clone());
                    }
                }
                for triangle in &mut canonical.nodes[node_index].triangles {
                    for point in triangle {
                        if sample_ids.contains(point) {
                            continue;
                        }
                        if let Some(sample_id) = by_edge.get(point) {
                            *point = sample_id.clone();
                        } else if let Some(ids) = by_source.get(point).filter(|ids| ids.len() == 1)
                        {
                            *point = ids[0].clone();
                        }
                    }
                }
            }
            canonical.nodes[node_index].samples = samples;
            diagnostics.push(node_migration_diagnostic(
                &node_id,
                "legacy_positional_sample_migrated",
                "Legacy positional blend coordinates were mapped by lexical edge identity; the exact old association cannot be proven after prior persistence ordering.",
                "Review the keyed samples once; future connection reordering cannot move their coordinates.",
            ));
        }
        if !canonical.nodes[node_index].samples.is_empty() {
            canonical.nodes[node_index].thresholds.clear();
        }
    }
    canonical
}

fn node_migration_diagnostic(
    node_id: &str,
    code: &str,
    message: impl Into<String>,
    fix: impl Into<String>,
) -> AnimationGraphDiagnostic {
    diagnostic(
        AnimationGraphDiagnosticSeverity::Warning,
        code,
        message,
        Some(fix.into()),
        Some(node_id.to_owned()),
        None,
        Some("poses".into()),
    )
}

/// Load the one active graph for a sequence. Incomplete records are surfaced as diagnostics; no corrupt
/// field is silently invented. Multiple active controllers fail closed and use stable entity ordering.
#[must_use]
pub fn load_animation_graph(engine: &Engine<FlecsWorld>, sequence_id: &str) -> AnimationGraphLoad {
    let revision = authored_animation_graph_revision(engine, sequence_id);
    let mut candidates = active_graph_candidates(engine, sequence_id);
    candidates.sort_by(|left, right| (left.1.as_str(), left.0).cmp(&(right.1.as_str(), right.0)));
    let mut diagnostics = Vec::new();
    if candidates.len() > 1 {
        diagnostics.push(diagnostic(
            AnimationGraphDiagnosticSeverity::Error,
            "multiple_active_graphs",
            format!(
                "Sequence {sequence_id} has {} active graph controllers; only the first stable identity can be shown.",
                candidates.len()
            ),
            Some("Delete or merge the duplicate graph controllers before playback.".into()),
            None,
            None,
            None,
        ));
    }
    let Some((owner, graph_id)) = candidates.first().cloned() else {
        return AnimationGraphLoad {
            document: None,
            revision,
            diagnostics,
            controller_id: None,
        };
    };
    let fields = component_fields(engine, owner);
    let records = collect_records(&fields, &graph_id);
    let document = reconstruct_document(&graph_id, records, &mut diagnostics)
        .map(|document| canonicalize_animation_graph_document(&document, &mut diagnostics));
    if let Some(document) = document.as_ref() {
        diagnostics.extend(validate_animation_graph_document(document));
    }
    deduplicate_diagnostics(&mut diagnostics);
    AnimationGraphLoad {
        document,
        revision,
        diagnostics,
        controller_id: Some(owner.to_loro_key()),
    }
}

/// Save a complete editor draft as one atomic, undoable granular intent. Semantic compiler errors are
/// allowed to persist so users do not lose incomplete work; admission errors that would corrupt identity,
/// exceed hard budgets, or serialize non-finite values reject the transaction.
pub fn save_animation_graph(
    engine: &mut Engine<FlecsWorld>,
    expected_revision: &str,
    document: &AnimationGraphDocument,
) -> Result<AnimationGraphLoad, AnimationGraphIntentError> {
    let current_revision = authored_animation_graph_revision(engine, &document.sequence_id);
    if current_revision != expected_revision {
        return Err(AnimationGraphIntentError::StaleRevision {
            expected: expected_revision.to_owned(),
            actual: current_revision,
        });
    }
    let mut migration_diagnostics = Vec::new();
    let canonical = canonicalize_animation_graph_document(document, &mut migration_diagnostics);
    let document = &canonical;
    let validation = validate_animation_graph_document(document);
    let admission: Vec<_> = validation
        .iter()
        .filter(|item| is_admission_code(&item.code))
        .cloned()
        .collect();
    if !admission.is_empty() {
        return Err(AnimationGraphIntentError::Admission(admission));
    }
    let active = active_graph_candidates(engine, &document.sequence_id);
    if active.len() > 1 {
        return Err(AnimationGraphIntentError::SequenceAlreadyHasGraph {
            graph_id: active[0].1.clone(),
        });
    }
    if let Some((_, graph_id)) = active.iter().find(|(_, id)| id != &document.id) {
        return Err(AnimationGraphIntentError::SequenceAlreadyHasGraph {
            graph_id: graph_id.clone(),
        });
    }
    let existing_owner = graph_owner(engine, &document.id);
    let owner = existing_owner.unwrap_or_else(|| engine.alloc_entity_id());
    let existing = component_fields(engine, owner);
    let desired = desired_fields(document)?;
    let mut ops = Vec::new();
    if existing_owner.is_none() {
        ops.push(Op::CreateEntity {
            id: owner,
            parent: None,
        });
        ops.push(Op::SetField {
            entity: owner,
            component: "__meta__".into(),
            field: "name".into(),
            value: FieldValue::Str(format!("{} (animation graph)", document.name)),
        });
        ops.push(Op::SetField {
            entity: owner,
            component: "__meta__".into(),
            field: "kind".into(),
            value: FieldValue::Str("animation_graph_controller".into()),
        });
    }
    for (field, value) in &desired {
        if existing.get(field) != Some(value) {
            ops.push(set_graph_field(owner, field.clone(), value.clone()));
        }
    }
    for field in existing.keys() {
        if !belongs_to_graph_record(field, &document.id) || desired.contains_key(field) {
            continue;
        }
        if field.ends_with("::active") && existing.get(field) != Some(&FieldValue::Bool(false)) {
            ops.push(set_graph_field(
                owner,
                field.clone(),
                FieldValue::Bool(false),
            ));
        }
    }
    if !ops.is_empty() {
        engine
            .commit("animation-graph-save", ops)
            .map_err(|error| AnimationGraphIntentError::Commit(error.to_string()))?;
    }
    let mut load = load_animation_graph(engine, &document.sequence_id);
    load.diagnostics.extend(migration_diagnostics);
    deduplicate_diagnostics(&mut load.diagnostics);
    Ok(load)
}

/// Tombstone a graph and every stable child element in one undoable transaction.
pub fn delete_animation_graph(
    engine: &mut Engine<FlecsWorld>,
    sequence_id: &str,
    graph_id: &str,
    expected_revision: &str,
) -> Result<AnimationGraphLoad, AnimationGraphIntentError> {
    let current_revision = authored_animation_graph_revision(engine, sequence_id);
    if current_revision != expected_revision {
        return Err(AnimationGraphIntentError::StaleRevision {
            expected: expected_revision.to_owned(),
            actual: current_revision,
        });
    }
    let loaded = load_animation_graph(engine, sequence_id);
    let document = loaded
        .document
        .as_ref()
        .ok_or(AnimationGraphIntentError::GraphNotFound)?;
    if document.id != graph_id {
        return Err(AnimationGraphIntentError::GraphIdentityMismatch);
    }
    let owner = graph_owner(engine, graph_id).ok_or(AnimationGraphIntentError::GraphNotFound)?;
    let fields = component_fields(engine, owner);
    let mut ops = Vec::new();
    for (field, value) in fields {
        if belongs_to_graph_record(&field, graph_id)
            && field.ends_with("::active")
            && value != FieldValue::Bool(false)
        {
            ops.push(set_graph_field(owner, field, FieldValue::Bool(false)));
        }
    }
    if ops.is_empty() {
        return Err(AnimationGraphIntentError::GraphNotFound);
    }
    engine
        .commit("animation-graph-delete", ops)
        .map_err(|error| AnimationGraphIntentError::Commit(error.to_string()))?;
    Ok(load_animation_graph(engine, sequence_id))
}

/// Sequence-scoped authored revision. Layout participates because it is collaborative authoring state;
/// the pure graph compiler independently excludes layout from its runtime hash.
#[must_use]
pub fn authored_animation_graph_revision(engine: &Engine<FlecsWorld>, sequence_id: &str) -> String {
    let mut graph_ids = BTreeSet::new();
    for entity in engine.entity_ids() {
        for (field, value) in component_fields(engine, entity) {
            let parts: Vec<_> = field.split("::").collect();
            if parts.len() == 3
                && parts[0] == GRAPH_PREFIX
                && parts[2] == "sequence_id"
                && value == FieldValue::Str(sequence_id.to_owned())
            {
                graph_ids.insert(parts[1].to_owned());
            }
        }
    }
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    revision_hash_frame(&mut hash, b"metrocalk-authored-animation-graph-v1");
    revision_hash_frame(&mut hash, sequence_id.as_bytes());
    let mut entities = engine.entity_ids();
    entities.sort();
    for entity in entities {
        let fields = component_fields(engine, entity);
        let matching: Vec<_> = fields
            .into_iter()
            .filter(|(field, _)| {
                field
                    .split("::")
                    .nth(1)
                    .is_some_and(|graph_id| graph_ids.contains(graph_id))
            })
            .collect();
        if matching.is_empty() {
            continue;
        }
        revision_hash_frame(&mut hash, entity.to_loro_key().as_bytes());
        for (field, value) in matching {
            revision_hash_frame(&mut hash, field.as_bytes());
            hash_field_value(&mut hash, &value);
        }
    }
    format!("graph-{hash:016x}")
}

/// Structural and semantic preflight with stable, navigable diagnostics. Only the subset identified by
/// [`is_admission_code`] blocks persistence; the remaining errors intentionally describe an editable but
/// not currently compilable draft.
#[must_use]
pub fn validate_animation_graph_document(
    document: &AnimationGraphDocument,
) -> Vec<AnimationGraphDiagnostic> {
    let mut diagnostics = Vec::new();
    let canonical = canonicalize_animation_graph_document(document, &mut diagnostics);
    let document = &canonical;
    validate_document_identity(document, &mut diagnostics);
    validate_budgets(document, &mut diagnostics);

    let _node_ids = validate_unique_ids(
        document
            .nodes
            .iter()
            .map(|node| (&node.id, Some(node.id.as_str()))),
        "node",
        &mut diagnostics,
    );
    let _edge_ids = validate_unique_ids(
        document.edges.iter().map(|edge| (&edge.id, None)),
        "edge",
        &mut diagnostics,
    );
    let _sample_ids = validate_unique_ids(
        document.nodes.iter().flat_map(|node| {
            node.samples
                .iter()
                .map(move |sample| (&sample.id, Some(node.id.as_str())))
        }),
        "blend sample",
        &mut diagnostics,
    );
    let parameter_ids = validate_unique_ids(
        document
            .parameters
            .iter()
            .map(|parameter| (&parameter.id, None)),
        "parameter",
        &mut diagnostics,
    );
    let _machine_ids = validate_unique_ids(
        document
            .state_machines
            .iter()
            .map(|machine| (&machine.id, None)),
        "state machine",
        &mut diagnostics,
    );
    for node in &document.nodes {
        validate_name(&node.name, "node", Some(&node.id), &mut diagnostics);
        if !node.thresholds.is_empty()
            || (matches!(
                node.kind,
                AnimationGraphNodeKind::BlendNormalized | AnimationGraphNodeKind::BlendDirect
            ) && !node.parameter_ids.is_empty())
        {
            diagnostics.push(node_diagnostic(
                &node.id,
                "legacy_positional_contract_in_v2",
                "Schema 2 cannot persist positional blend thresholds or positional input-weight bindings.",
                "Mark a legacy source as schema 1 for explicit migration, or author stable sample/edge bindings.",
            ));
        }
        if !node.position.x.is_finite() || !node.position.y.is_finite() {
            diagnostics.push(node_diagnostic(
                &node.id,
                "non_finite_layout",
                "Node position must contain finite numbers.",
                "Reset or move the node to a finite canvas coordinate.",
            ));
        }
        if node.thresholds.iter().any(|value| !value.is_finite()) {
            diagnostics.push(node_diagnostic(
                &node.id,
                "non_finite_threshold",
                "Blend thresholds must contain only finite numbers.",
                "Replace NaN or infinite thresholds with finite authored values.",
            ));
        }
        if node
            .samples
            .iter()
            .any(|sample| sample.position.iter().any(|value| !value.is_finite()))
        {
            diagnostics.push(node_diagnostic(
                &node.id,
                "non_finite_sample",
                "Blend sample positions must contain only finite numbers.",
                "Replace NaN or infinite sample positions with finite authored values.",
            ));
        }
        let mut sampled_edges = BTreeSet::new();
        for sample in &node.samples {
            validate_id(
                &sample.edge_id,
                "blend sample edge",
                Some(&node.id),
                &mut diagnostics,
            );
            if !sampled_edges.insert(sample.edge_id.as_str()) {
                diagnostics.push(node_diagnostic(
                    &node.id,
                    "duplicate_sample_edge",
                    format!(
                        "More than one blend sample references edge {}.",
                        sample.edge_id
                    ),
                    "Keep exactly one durable sample per connected pose edge.",
                ));
            }
        }
        for parameter_id in &node.parameter_ids {
            if !parameter_ids.contains(parameter_id) {
                diagnostics.push(node_diagnostic(
                    &node.id,
                    "missing_parameter",
                    format!("Node references missing parameter {parameter_id}."),
                    "Choose an existing typed parameter or remove the reference.",
                ));
            }
        }
    }

    for parameter in &document.parameters {
        validate_parameter(parameter, &mut diagnostics);
    }

    let nodes: BTreeMap<_, _> = document
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let parameters: BTreeMap<_, _> = document
        .parameters
        .iter()
        .map(|parameter| (parameter.id.as_str(), parameter))
        .collect();
    let enabled_edges: Vec<_> = document.edges.iter().filter(|edge| edge.enabled).collect();
    for edge in &document.edges {
        validate_edge(edge, &nodes, &parameters, &mut diagnostics);
    }
    validate_output(document, &nodes, &enabled_edges, &mut diagnostics);
    validate_node_inputs(document, &nodes, &enabled_edges, &mut diagnostics);
    validate_pose_dag(document, &nodes, &mut diagnostics);
    validate_state_machines(document, &nodes, &parameter_ids, &mut diagnostics);
    deduplicate_diagnostics(&mut diagnostics);
    diagnostics
}

/// Adapt the editor wire document to the renderer/ECS-independent graph kernel. Source sequences remain
/// explicit inputs so imported clips cannot become accidentally playable merely because their metadata
/// exists. The returned pure graph is ready for `CompiledAnimationGraph::compile` on a worker thread.
pub fn adapt_animation_graph_document(
    engine: &Engine<FlecsWorld>,
    document: &AnimationGraphDocument,
    sequences: &BTreeMap<animation::SequenceId, animation::Sequence>,
) -> Result<AdaptedAnimationGraph, Vec<AnimationGraphDiagnostic>> {
    let mut migration_diagnostics = Vec::new();
    let canonical = canonicalize_animation_graph_document(document, &mut migration_diagnostics);
    let document = &canonical;
    let mut diagnostics = validate_animation_graph_document(document);
    diagnostics.extend(migration_diagnostics);
    if diagnostics
        .iter()
        .any(|item| item.severity == AnimationGraphDiagnosticSeverity::Error)
    {
        return Err(diagnostics);
    }

    let nodes_by_id: BTreeMap<_, _> = document
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let parameters_by_id: BTreeMap<_, _> = document
        .parameters
        .iter()
        .map(|parameter| (parameter.id.as_str(), parameter))
        .collect();

    let (parameters, parameter_routes) = adapt_parameters(document, &mut diagnostics);
    let routes_by_id: BTreeMap<_, _> = parameter_routes
        .iter()
        .map(|route| (route.editor_parameter_id.as_str(), route))
        .collect();

    let referenced_sources: BTreeSet<_> = document
        .nodes
        .iter()
        .filter(|node| node.enabled && node.kind == AnimationGraphNodeKind::Sequence)
        .filter_map(|node| node.source_id.as_deref())
        .map(animation::SequenceId::new)
        .collect();
    let reference_pose = adapt_reference_pose(
        engine,
        document,
        sequences,
        &referenced_sources,
        &mut diagnostics,
    );
    let (masks, mask_ids) = adapt_masks(document, &reference_pose, &mut diagnostics);

    let output_edge = enabled_incoming(document, &document.output_node_id, Some("pose"))
        .into_iter()
        .next();
    let output_source =
        output_edge.and_then(|edge| nodes_by_id.get(edge.from_node_id.as_str()).copied());

    let mut state_machine = None;
    let output_root = match output_source {
        Some(source) if source.kind == AnimationGraphNodeKind::StateMachine => {
            let machine = source.state_machine_id.as_deref().and_then(|machine_id| {
                document
                    .state_machines
                    .iter()
                    .find(|machine| machine.id == machine_id)
            });
            match machine {
                Some(machine) => {
                    state_machine = adapt_state_machine(
                        machine,
                        &nodes_by_id,
                        &parameters_by_id,
                        &routes_by_id,
                        &mut diagnostics,
                    );
                    machine
                        .states
                        .iter()
                        .find(|state| state.id == machine.entry_state_id)
                        .map_or_else(
                            || animation::GraphNodeId::new("missing-output-root"),
                            |state| animation::GraphNodeId::new(state.pose_node_id.clone()),
                        )
                }
                None => {
                    diagnostics.push(node_diagnostic(
                        &source.id,
                        "missing_state_machine",
                        "The output state-machine node has no matching machine record.",
                        "Choose an existing state machine or restore its record.",
                    ));
                    animation::GraphNodeId::new("missing-output-root")
                }
            }
        }
        Some(source) => {
            if !document.state_machines.is_empty()
                || document
                    .nodes
                    .iter()
                    .any(|node| node.enabled && node.kind == AnimationGraphNodeKind::StateMachine)
            {
                diagnostics.push(node_diagnostic(
                    &source.id,
                    "state_machine_composition_unsupported",
                    "The first graph tier requires a State Machine node to feed Output directly.",
                    "Connect the State Machine directly to Output, or remove the unused machine.",
                ));
            }
            animation::GraphNodeId::new(source.id.clone())
        }
        None => animation::GraphNodeId::new("missing-output-root"),
    };

    let mut nodes = Vec::new();
    for node in document.nodes.iter().filter(|node| node.enabled) {
        if matches!(
            node.kind,
            AnimationGraphNodeKind::Output | AnimationGraphNodeKind::StateMachine
        ) {
            continue;
        }
        if let Some(kind) = adapt_node_kind(
            document,
            node,
            &parameters_by_id,
            &routes_by_id,
            &mask_ids,
            &mut diagnostics,
        ) {
            nodes.push(animation::GraphNode {
                id: animation::GraphNodeId::new(node.id.clone()),
                name: node.name.clone(),
                kind,
            });
        }
    }

    let mut edge_provenance: Vec<_> = document
        .edges
        .iter()
        .filter(|edge| edge.enabled)
        .map(|edge| AnimationGraphEdgeProvenance {
            edge_id: edge.id.clone(),
            from_node_id: edge.from_node_id.clone(),
            to_node_id: edge.to_node_id.clone(),
            to_port_id: edge.to_port_id.clone(),
        })
        .collect();
    edge_provenance.sort_by(|left, right| left.edge_id.cmp(&right.edge_id));

    deduplicate_diagnostics(&mut diagnostics);
    if diagnostics
        .iter()
        .any(|item| item.severity == AnimationGraphDiagnosticSeverity::Error)
    {
        return Err(diagnostics);
    }
    Ok(AdaptedAnimationGraph {
        graph: animation::AnimationGraph {
            schema_version: animation::ANIMATION_GRAPH_SCHEMA_VERSION,
            id: animation::AnimationGraphId::new(document.id.clone()),
            name: document.name.clone(),
            parameters,
            reference_pose,
            masks,
            nodes,
            output: output_root,
            state_machine,
            event_policy: animation::GraphEventPolicy::Dominant,
            limits: animation::AnimationGraphLimits::default(),
        },
        parameter_routes,
        edge_provenance,
        diagnostics,
    })
}

fn adapt_parameters(
    document: &AnimationGraphDocument,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) -> (
    Vec<animation::GraphParameter>,
    Vec<AnimationGraphParameterRoute>,
) {
    let authored_ids: BTreeSet<_> = document
        .parameters
        .iter()
        .map(|parameter| parameter.id.as_str())
        .collect();
    let mut runtime_ids = BTreeSet::new();
    let mut parameters = Vec::new();
    let mut routes = Vec::new();
    for parameter in &document.parameters {
        match (&parameter.kind, &parameter.default_value) {
            (AnimationGraphParameterKind::Float, AnimationGraphValue::Number(value)) => {
                push_direct_parameter(
                    parameter,
                    animation::GraphParameterKind::Number,
                    animation::GraphParameterValue::Number(*value),
                    &mut runtime_ids,
                    &mut parameters,
                    &mut routes,
                );
            }
            (AnimationGraphParameterKind::Integer, AnimationGraphValue::Number(value))
                if value.fract() == 0.0
                    && *value >= i64::MIN as f64
                    && *value <= i64::MAX as f64 =>
            {
                push_direct_parameter(
                    parameter,
                    animation::GraphParameterKind::Integer,
                    animation::GraphParameterValue::Integer(*value as i64),
                    &mut runtime_ids,
                    &mut parameters,
                    &mut routes,
                );
            }
            (AnimationGraphParameterKind::Boolean, AnimationGraphValue::Boolean(value)) => {
                push_direct_parameter(
                    parameter,
                    animation::GraphParameterKind::Boolean,
                    animation::GraphParameterValue::Boolean(*value),
                    &mut runtime_ids,
                    &mut parameters,
                    &mut routes,
                );
            }
            (AnimationGraphParameterKind::Trigger, AnimationGraphValue::Boolean(value)) => {
                push_direct_parameter(
                    parameter,
                    animation::GraphParameterKind::Trigger,
                    animation::GraphParameterValue::Trigger(*value),
                    &mut runtime_ids,
                    &mut parameters,
                    &mut routes,
                );
            }
            (AnimationGraphParameterKind::Vec2, AnimationGraphValue::Vec2(value)) => {
                let x_id = format!("{}.__x", parameter.id);
                let y_id = format!("{}.__y", parameter.id);
                if authored_ids.contains(x_id.as_str())
                    || authored_ids.contains(y_id.as_str())
                    || !runtime_ids.insert(x_id.clone())
                    || !runtime_ids.insert(y_id.clone())
                {
                    diagnostics.push(diagnostic(
                        AnimationGraphDiagnosticSeverity::Error,
                        "synthetic_parameter_collision",
                        format!(
                            "Vec2 parameter {} cannot reserve its stable X/Y runtime identities.",
                            parameter.name
                        ),
                        Some("Rename the colliding authored parameter.".into()),
                        None,
                        None,
                        None,
                    ));
                    continue;
                }
                parameters.push(animation::GraphParameter {
                    id: animation::GraphParameterId::new(x_id.clone()),
                    name: format!("{} X", parameter.name),
                    kind: animation::GraphParameterKind::Number,
                    default: animation::GraphParameterValue::Number(value[0]),
                });
                parameters.push(animation::GraphParameter {
                    id: animation::GraphParameterId::new(y_id.clone()),
                    name: format!("{} Y", parameter.name),
                    kind: animation::GraphParameterKind::Number,
                    default: animation::GraphParameterValue::Number(value[1]),
                });
                routes.push(AnimationGraphParameterRoute {
                    editor_parameter_id: parameter.id.clone(),
                    runtime_parameter_ids: vec![x_id, y_id],
                    kind: parameter.kind,
                });
            }
            _ => diagnostics.push(diagnostic(
                AnimationGraphDiagnosticSeverity::Error,
                "parameter_type_mismatch",
                format!(
                    "Parameter {} default cannot be adapted to its runtime type.",
                    parameter.name
                ),
                Some("Repair the parameter's declared type and default value.".into()),
                None,
                None,
                None,
            )),
        }
    }
    parameters.sort_by(|left, right| left.id.cmp(&right.id));
    routes.sort_by(|left, right| left.editor_parameter_id.cmp(&right.editor_parameter_id));
    (parameters, routes)
}

fn push_direct_parameter(
    parameter: &AnimationGraphParameter,
    kind: animation::GraphParameterKind,
    default: animation::GraphParameterValue,
    runtime_ids: &mut BTreeSet<String>,
    parameters: &mut Vec<animation::GraphParameter>,
    routes: &mut Vec<AnimationGraphParameterRoute>,
) {
    runtime_ids.insert(parameter.id.clone());
    parameters.push(animation::GraphParameter {
        id: animation::GraphParameterId::new(parameter.id.clone()),
        name: parameter.name.clone(),
        kind,
        default,
    });
    routes.push(AnimationGraphParameterRoute {
        editor_parameter_id: parameter.id.clone(),
        runtime_parameter_ids: vec![parameter.id.clone()],
        kind: parameter.kind,
    });
}

fn adapt_reference_pose(
    engine: &Engine<FlecsWorld>,
    document: &AnimationGraphDocument,
    sequences: &BTreeMap<animation::SequenceId, animation::Sequence>,
    referenced_sources: &BTreeSet<animation::SequenceId>,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) -> Vec<animation::GraphReferenceValue> {
    let source_nodes: BTreeMap<_, _> = document
        .nodes
        .iter()
        .filter(|node| node.kind == AnimationGraphNodeKind::Sequence)
        .filter_map(|node| {
            node.source_id
                .as_deref()
                .map(|source| (source, node.id.as_str()))
        })
        .collect();
    let mut values = BTreeMap::new();
    for source_id in referenced_sources {
        let Some(sequence) = sequences.get(source_id) else {
            diagnostics.push(diagnostic(
                AnimationGraphDiagnosticSeverity::Error,
                "source_not_ready",
                format!("Animation source {source_id} is not available as a runtime sequence."),
                Some("Choose a ready authored sequence; imported decoded-only clips cannot play yet.".into()),
                source_nodes
                    .get(source_id.as_str())
                    .map(|node_id| (*node_id).to_owned()),
                None,
                None,
            ));
            continue;
        };
        for track in sequence.tracks.iter().filter(|track| track.enabled) {
            if values.contains_key(&track.binding) {
                continue;
            }
            match resolved_reference_value(engine, &track.binding) {
                Some(value) => {
                    values.insert(track.binding.clone(), value);
                }
                None => diagnostics.push(diagnostic(
                    AnimationGraphDiagnosticSeverity::Error,
                    "missing_reference_value",
                    format!(
                        "No explicit live reference value exists for {}.",
                        track.binding.path.display_path()
                    ),
                    Some(
                        "Restore the target property or provide a compatible skeleton/property reference pose."
                            .into(),
                    ),
                    source_nodes
                        .get(source_id.as_str())
                        .map(|node_id| (*node_id).to_owned()),
                    None,
                    Some("reference_pose".into()),
                )),
            }
        }
    }
    if values.is_empty() {
        diagnostics.push(diagnostic(
            AnimationGraphDiagnosticSeverity::Error,
            "empty_reference_pose",
            "The graph has no explicit typed property reference values.",
            Some(
                "Add a ready sequence with live target properties before compiling the graph."
                    .into(),
            ),
            None,
            None,
            Some("reference_pose".into()),
        ));
    }
    values
        .into_iter()
        .map(|(binding, value)| animation::GraphReferenceValue { binding, value })
        .collect()
}

fn resolved_reference_value(
    engine: &Engine<FlecsWorld>,
    binding: &animation::Binding,
) -> Option<animation::AnimValue> {
    if !binding.path.subpath.is_empty() {
        return None;
    }
    let entity = EntityId::from_loro_key(&binding.path.target)?;
    // Composite TRS bindings are virtual animation properties backed by the canonical scalar capscene
    // fields. Resolve their reference pose through the same local-transform adapter as playback so graph
    // blending never fails merely because `Transform.translation` is not a literal ECS field.
    if binding.path.component == "Transform" {
        let local = crate::capscene::local_transform(engine, entity);
        let composite = match (binding.path.property.as_str(), binding.value_kind) {
            ("translation", animation::ValueKind::Vec3) => {
                Some(animation::AnimValue::Vec3(local.translation.map(f64::from)))
            }
            ("rotation", animation::ValueKind::Quaternion) => Some(
                animation::AnimValue::Quaternion(local.rotation.map(f64::from)),
            ),
            ("scale3", animation::ValueKind::Vec3) => {
                Some(animation::AnimValue::Vec3(local.scale.map(f64::from)))
            }
            ("scale", animation::ValueKind::Number) => {
                Some(animation::AnimValue::Number(f64::from(local.scale[0])))
            }
            _ => None,
        };
        if composite.is_some() {
            return composite;
        }
    }
    let components = engine.resolved_components(entity);
    let value = components
        .get(&binding.path.component)?
        .get(&binding.path.property)?;
    let adapted = match (binding.value_kind, value) {
        (animation::ValueKind::Number, FieldValue::Number(value)) => {
            animation::AnimValue::Number(*value)
        }
        (animation::ValueKind::Integer, FieldValue::Integer(value)) => {
            animation::AnimValue::Integer(*value)
        }
        (animation::ValueKind::Boolean, FieldValue::Bool(value)) => {
            animation::AnimValue::Boolean(*value)
        }
        (animation::ValueKind::String, FieldValue::Str(value)) => {
            animation::AnimValue::String(value.clone())
        }
        (_, FieldValue::Str(value)) => serde_json::from_str::<animation::AnimValue>(value).ok()?,
        _ => return None,
    };
    (adapted.kind() == binding.value_kind).then_some(adapted)
}

fn adapt_masks(
    document: &AnimationGraphDocument,
    reference_pose: &[animation::GraphReferenceValue],
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) -> (
    Vec<animation::GraphPropertyMask>,
    BTreeMap<String, animation::GraphMaskId>,
) {
    let mut masks = Vec::new();
    let mut ids = BTreeMap::new();
    for node in document.nodes.iter().filter(|node| {
        node.enabled
            && matches!(
                node.kind,
                AnimationGraphNodeKind::LayerOverride | AnimationGraphNodeKind::LayerAdditive
            )
            && !node.mask_bindings.is_empty()
    }) {
        let mask_id = animation::GraphMaskId::new(format!("mask-{}", node.id));
        let mut paths = BTreeSet::new();
        for selector in &node.mask_bindings {
            let matches: Vec<_> = reference_pose
                .iter()
                .filter(|reference| {
                    binding_selector_matches(selector, &reference.binding.path.display_path())
                })
                .map(|reference| reference.binding.path.clone())
                .collect();
            if matches.is_empty() {
                diagnostics.push(diagnostic(
                    AnimationGraphDiagnosticSeverity::Warning,
                    "empty_mask_selector",
                    format!("Mask selector {selector:?} matches no compiled property binding."),
                    Some(
                        "Choose an exact path or '*'/'**' pattern from the binding browser.".into(),
                    ),
                    Some(node.id.clone()),
                    None,
                    Some("mask".into()),
                ));
            }
            paths.extend(matches);
        }
        masks.push(animation::GraphPropertyMask {
            id: mask_id.clone(),
            name: format!("{} mask", node.name),
            default_weight: 0.0,
            entries: paths
                .into_iter()
                .map(|path| animation::GraphMaskEntry { path, weight: 1.0 })
                .collect(),
        });
        ids.insert(node.id.clone(), mask_id);
    }
    masks.sort_by(|left, right| left.id.cmp(&right.id));
    (masks, ids)
}

fn binding_selector_matches(selector: &str, path: &str) -> bool {
    fn matches(pattern: &[&str], value: &[&str]) -> bool {
        match (pattern.split_first(), value.split_first()) {
            (None, None) => true,
            (Some((&"**", rest)), _) => {
                matches(rest, value)
                    || value
                        .split_first()
                        .is_some_and(|(_, tail)| matches(pattern, tail))
            }
            (Some((&"*", rest)), Some((_, tail))) => matches(rest, tail),
            (Some((head, rest)), Some((actual, tail))) if head == actual => matches(rest, tail),
            _ => false,
        }
    }
    matches(
        &selector.split('/').collect::<Vec<_>>(),
        &path.split('/').collect::<Vec<_>>(),
    )
}

fn mask_selector_error(selector: &str) -> Option<&'static str> {
    if selector.is_empty() {
        return Some("selector cannot be empty");
    }
    if selector.trim() != selector || selector.contains(['\\', '\0']) {
        return Some("use a trimmed forward-slash path");
    }
    let segments: Vec<_> = selector.split('/').collect();
    if segments.len() < 3 || segments.iter().any(|segment| segment.is_empty()) {
        return Some("use at least target/component/property segments with no empty segment");
    }
    if segments
        .iter()
        .any(|segment| segment.contains('*') && !matches!(*segment, "*" | "**"))
    {
        return Some("wildcards must occupy a complete segment: '*' or '**'");
    }
    None
}

fn adapt_node_kind(
    document: &AnimationGraphDocument,
    node: &AnimationGraphNode,
    parameters: &BTreeMap<&str, &AnimationGraphParameter>,
    routes: &BTreeMap<&str, &AnimationGraphParameterRoute>,
    mask_ids: &BTreeMap<String, animation::GraphMaskId>,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) -> Option<animation::GraphNodeKind> {
    match node.kind {
        AnimationGraphNodeKind::ReferencePose => Some(animation::GraphNodeKind::ReferencePose),
        AnimationGraphNodeKind::Sequence => Some(animation::GraphNodeKind::Clip {
            sequence: animation::SequenceId::new(node.source_id.clone()?),
            rate: animation::RationalRate::ONE,
            start_tick: animation::Tick(0),
        }),
        AnimationGraphNodeKind::BlendNormalized | AnimationGraphNodeKind::BlendDirect => {
            let edges = enabled_incoming(document, &node.id, Some("poses"));
            if !node.parameter_ids.is_empty() {
                diagnostics.push(node_diagnostic(
                    &node.id,
                    "blend_weight_count",
                    "Canonical blends bind weight parameters on stable input edges.",
                    "Move each positional parameter binding onto its intended connection.",
                ));
            }
            for edge in &edges {
                if let Some(id) = &edge.weight_parameter_id {
                    if runtime_number_parameter(id, parameters, routes).is_none() {
                        diagnostics.push(edge_diagnostic(
                            &edge.id,
                            "blend_parameter_type",
                            format!("Blend weight parameter {id} must be a float."),
                            "Bind a float parameter or use a constant edge weight.",
                        ));
                    }
                }
            }
            let inputs = edges
                .iter()
                .map(|edge| animation::GraphBlendInput {
                    node: animation::GraphNodeId::new(edge.from_node_id.clone()),
                    weight: edge
                        .weight_parameter_id
                        .as_deref()
                        .and_then(|id| runtime_number_parameter(id, parameters, routes))
                        .map_or_else(
                            || animation::GraphWeight::Constant(edge.weight.unwrap_or(1.0)),
                            animation::GraphWeight::Parameter,
                        ),
                })
                .collect();
            Some(animation::GraphNodeKind::Blend {
                mode: if node.kind == AnimationGraphNodeKind::BlendNormalized {
                    animation::GraphBlendMode::Normalized
                } else {
                    animation::GraphBlendMode::Direct
                },
                inputs,
            })
        }
        AnimationGraphNodeKind::Blend1d => {
            let parameter = node
                .parameter_ids
                .first()
                .and_then(|id| runtime_number_parameter(id, parameters, routes));
            if parameter.is_none() {
                diagnostics.push(node_diagnostic(
                    &node.id,
                    "blend_parameter_type",
                    "Blend1D parameter must be a float.",
                    "Bind one float parameter.",
                ));
            }
            let edges: BTreeMap<_, _> = enabled_incoming(document, &node.id, Some("poses"))
                .into_iter()
                .map(|edge| (edge.id.as_str(), edge))
                .collect();
            let points = node
                .samples
                .iter()
                .filter_map(|sample| {
                    let edge = edges.get(sample.edge_id.as_str())?;
                    Some(animation::GraphBlend1dPoint {
                        id: animation::GraphBlendPointId::new(sample.id.clone()),
                        position: sample.position[0],
                        node: animation::GraphNodeId::new(edge.from_node_id.clone()),
                    })
                })
                .collect();
            Some(animation::GraphNodeKind::Blend1d {
                parameter: parameter.unwrap_or_else(|| animation::GraphParameterId::new("missing")),
                points,
            })
        }
        AnimationGraphNodeKind::Blend2dCartesian => {
            let (parameter_x, parameter_y) =
                blend_2d_parameters(node, parameters, routes, diagnostics);
            let edges: BTreeMap<_, _> = enabled_incoming(document, &node.id, Some("poses"))
                .into_iter()
                .map(|edge| (edge.id.as_str(), edge))
                .collect();
            let points: Vec<_> = node
                .samples
                .iter()
                .filter_map(|sample| {
                    let edge = edges.get(sample.edge_id.as_str())?;
                    Some(animation::GraphBlend2dPoint {
                        id: animation::GraphBlendPointId::new(sample.id.clone()),
                        position: sample.position,
                        node: animation::GraphNodeId::new(edge.from_node_id.clone()),
                    })
                })
                .collect();
            let triangles = node
                .triangles
                .iter()
                .map(|triangle| animation::GraphBlend2dTriangle {
                    points: [
                        animation::GraphBlendPointId::new(triangle[0].clone()),
                        animation::GraphBlendPointId::new(triangle[1].clone()),
                        animation::GraphBlendPointId::new(triangle[2].clone()),
                    ],
                })
                .collect();
            Some(animation::GraphNodeKind::Blend2d {
                parameter_x: parameter_x
                    .unwrap_or_else(|| animation::GraphParameterId::new("missing-x")),
                parameter_y: parameter_y
                    .unwrap_or_else(|| animation::GraphParameterId::new("missing-y")),
                points,
                triangles,
                outside_hull: animation::GraphOutsideHullMode::ProjectToHull,
            })
        }
        AnimationGraphNodeKind::LayerOverride | AnimationGraphNodeKind::LayerAdditive => {
            let base = enabled_incoming(document, &node.id, Some("base"))
                .first()
                .map(|edge| animation::GraphNodeId::new(edge.from_node_id.clone()))?;
            let layer_edge = enabled_incoming(document, &node.id, Some("layer"))
                .first()
                .copied()?;
            let layer = animation::GraphNodeId::new(layer_edge.from_node_id.clone());
            let weight = node
                .parameter_ids
                .first()
                .and_then(|id| runtime_number_parameter(id, parameters, routes))
                .map_or_else(
                    || animation::GraphWeight::Constant(layer_edge.weight.unwrap_or(1.0)),
                    animation::GraphWeight::Parameter,
                );
            if node
                .parameter_ids
                .first()
                .is_some_and(|id| runtime_number_parameter(id, parameters, routes).is_none())
            {
                diagnostics.push(node_diagnostic(
                    &node.id,
                    "blend_parameter_type",
                    "Layer weight parameter must be a float.",
                    "Bind a float parameter or use the layer edge's constant weight.",
                ));
            }
            if node.parameter_ids.len() > 1 {
                diagnostics.push(node_diagnostic(
                    &node.id,
                    "layer_weight_count",
                    "A layer accepts at most one float weight parameter.",
                    "Keep one weight parameter or use the layer edge's constant weight.",
                ));
            }
            let mask = mask_ids.get(&node.id).cloned();
            if node.kind == AnimationGraphNodeKind::LayerOverride {
                Some(animation::GraphNodeKind::OverrideLayer {
                    base,
                    layer,
                    weight,
                    mask,
                })
            } else {
                Some(animation::GraphNodeKind::AdditiveLayer {
                    base,
                    layer,
                    weight,
                    mask,
                })
            }
        }
        AnimationGraphNodeKind::StateMachine | AnimationGraphNodeKind::Output => None,
    }
}

fn runtime_number_parameter(
    editor_id: &str,
    parameters: &BTreeMap<&str, &AnimationGraphParameter>,
    routes: &BTreeMap<&str, &AnimationGraphParameterRoute>,
) -> Option<animation::GraphParameterId> {
    let parameter = parameters.get(editor_id)?;
    if parameter.kind != AnimationGraphParameterKind::Float {
        return None;
    }
    routes
        .get(editor_id)?
        .runtime_parameter_ids
        .first()
        .cloned()
        .map(animation::GraphParameterId::new)
}

fn blend_2d_parameters(
    node: &AnimationGraphNode,
    parameters: &BTreeMap<&str, &AnimationGraphParameter>,
    routes: &BTreeMap<&str, &AnimationGraphParameterRoute>,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) -> (
    Option<animation::GraphParameterId>,
    Option<animation::GraphParameterId>,
) {
    let result = match node.parameter_ids.as_slice() {
        [id] if parameters
            .get(id.as_str())
            .is_some_and(|parameter| parameter.kind == AnimationGraphParameterKind::Vec2) =>
        {
            routes.get(id.as_str()).and_then(|route| {
                Some((
                    animation::GraphParameterId::new(route.runtime_parameter_ids.first()?.clone()),
                    animation::GraphParameterId::new(route.runtime_parameter_ids.get(1)?.clone()),
                ))
            })
        }
        [x, y] => runtime_number_parameter(x, parameters, routes)
            .zip(runtime_number_parameter(y, parameters, routes)),
        _ => None,
    };
    if result.is_none() {
        diagnostics.push(node_diagnostic(
            &node.id,
            "blend_parameter_type",
            "Cartesian Blend2D needs one Vec2 or two float parameters.",
            "Bind a Vec2 parameter or an explicit float X/Y pair.",
        ));
    }
    result.map_or((None, None), |(x, y)| (Some(x), Some(y)))
}

fn adapt_state_machine(
    machine: &AnimationGraphStateMachine,
    nodes: &BTreeMap<&str, &AnimationGraphNode>,
    parameters: &BTreeMap<&str, &AnimationGraphParameter>,
    routes: &BTreeMap<&str, &AnimationGraphParameterRoute>,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) -> Option<animation::GraphStateMachine> {
    let states: Vec<_> = machine
        .states
        .iter()
        .filter_map(|state| {
            let node = nodes.get(state.pose_node_id.as_str())?;
            if !node.enabled
                || matches!(
                    node.kind,
                    AnimationGraphNodeKind::Output | AnimationGraphNodeKind::StateMachine
                )
            {
                diagnostics.push(diagnostic(
                    AnimationGraphDiagnosticSeverity::Error,
                    "invalid_state_pose",
                    format!(
                        "State {} does not resolve to an enabled pose node.",
                        state.name
                    ),
                    Some("Choose an enabled pose-producing DAG node.".into()),
                    Some(state.pose_node_id.clone()),
                    None,
                    None,
                ));
                return None;
            }
            Some(animation::GraphState {
                id: animation::GraphStateId::new(state.id.clone()),
                name: state.name.clone(),
                node: animation::GraphNodeId::new(state.pose_node_id.clone()),
                reset_on_entry: state.reset_on_entry,
            })
        })
        .collect();
    let transitions = machine
        .transitions
        .iter()
        .map(|transition| animation::GraphTransition {
            id: animation::GraphTransitionId::new(transition.id.clone()),
            from: animation::GraphStateId::new(transition.from_state_id.clone()),
            to: animation::GraphStateId::new(transition.to_state_id.clone()),
            priority: transition.priority,
            duration: animation::Tick(transition.duration_tick),
            curve: match transition.curve {
                AnimationGraphCurve::Linear => animation::GraphTransitionCurve::Linear,
                AnimationGraphCurve::EaseIn => animation::GraphTransitionCurve::EaseIn,
                AnimationGraphCurve::EaseOut => animation::GraphTransitionCurve::EaseOut,
                AnimationGraphCurve::EaseInOut => animation::GraphTransitionCurve::EaseInOut,
                AnimationGraphCurve::Smoothstep => animation::GraphTransitionCurve::Smoothstep,
            },
            interruption: match transition.interruption {
                AnimationGraphInterruption::None => animation::GraphInterruptionPolicy::None,
                AnimationGraphInterruption::Source => animation::GraphInterruptionPolicy::Source,
                AnimationGraphInterruption::Destination => {
                    animation::GraphInterruptionPolicy::Destination
                }
                AnimationGraphInterruption::Both => animation::GraphInterruptionPolicy::Both,
            },
            mode: animation::GraphTransitionMode::Crossfade,
            conditions: transition
                .conditions
                .iter()
                .filter_map(|condition| adapt_condition(condition, parameters, routes, diagnostics))
                .collect(),
        })
        .collect();
    Some(animation::GraphStateMachine {
        initial_state: animation::GraphStateId::new(machine.entry_state_id.clone()),
        states,
        transitions,
    })
}

fn adapt_condition(
    condition: &AnimationGraphCondition,
    parameters: &BTreeMap<&str, &AnimationGraphParameter>,
    routes: &BTreeMap<&str, &AnimationGraphParameterRoute>,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) -> Option<animation::GraphCondition> {
    let parameter = parameters.get(condition.parameter_id.as_str())?;
    let route = routes.get(condition.parameter_id.as_str())?;
    let runtime_id = route.runtime_parameter_ids.first()?.clone();
    let test = match parameter.kind {
        AnimationGraphParameterKind::Float => {
            let Some(value) = condition_number(condition) else {
                invalid_condition_operator(condition, diagnostics);
                return None;
            };
            match condition.operator {
                AnimationGraphConditionOperator::Equal => {
                    animation::GraphConditionTest::NumberEquals(value)
                }
                AnimationGraphConditionOperator::NotEqual => {
                    animation::GraphConditionTest::NumberNotEquals(value)
                }
                AnimationGraphConditionOperator::Greater => {
                    animation::GraphConditionTest::NumberGreaterThan(value)
                }
                AnimationGraphConditionOperator::GreaterEqual => {
                    animation::GraphConditionTest::NumberGreaterOrEqual(value)
                }
                AnimationGraphConditionOperator::Less => {
                    animation::GraphConditionTest::NumberLessThan(value)
                }
                AnimationGraphConditionOperator::LessEqual => {
                    animation::GraphConditionTest::NumberLessOrEqual(value)
                }
                _ => {
                    invalid_condition_operator(condition, diagnostics);
                    return None;
                }
            }
        }
        AnimationGraphParameterKind::Integer => {
            let Some(number) = condition_number(condition) else {
                invalid_condition_operator(condition, diagnostics);
                return None;
            };
            if number.fract() != 0.0 || number < i64::MIN as f64 || number > i64::MAX as f64 {
                invalid_condition_operator(condition, diagnostics);
                return None;
            }
            let value = number as i64;
            match condition.operator {
                AnimationGraphConditionOperator::Equal => {
                    animation::GraphConditionTest::IntegerEquals(value)
                }
                AnimationGraphConditionOperator::NotEqual => {
                    animation::GraphConditionTest::IntegerNotEquals(value)
                }
                AnimationGraphConditionOperator::Greater => {
                    animation::GraphConditionTest::IntegerGreaterThan(value)
                }
                AnimationGraphConditionOperator::GreaterEqual => {
                    animation::GraphConditionTest::IntegerGreaterOrEqual(value)
                }
                AnimationGraphConditionOperator::Less => {
                    animation::GraphConditionTest::IntegerLessThan(value)
                }
                AnimationGraphConditionOperator::LessEqual => {
                    animation::GraphConditionTest::IntegerLessOrEqual(value)
                }
                _ => {
                    invalid_condition_operator(condition, diagnostics);
                    return None;
                }
            }
        }
        AnimationGraphParameterKind::Boolean => {
            let expected = match condition.operator {
                AnimationGraphConditionOperator::IsTrue => true,
                AnimationGraphConditionOperator::IsFalse => false,
                AnimationGraphConditionOperator::Equal => {
                    let Some(value) = condition_boolean(condition) else {
                        invalid_condition_operator(condition, diagnostics);
                        return None;
                    };
                    value
                }
                AnimationGraphConditionOperator::NotEqual => {
                    let Some(value) = condition_boolean(condition) else {
                        invalid_condition_operator(condition, diagnostics);
                        return None;
                    };
                    !value
                }
                _ => {
                    invalid_condition_operator(condition, diagnostics);
                    return None;
                }
            };
            animation::GraphConditionTest::BooleanEquals(expected)
        }
        AnimationGraphParameterKind::Trigger => {
            if condition.operator != AnimationGraphConditionOperator::Triggered {
                invalid_condition_operator(condition, diagnostics);
                return None;
            }
            animation::GraphConditionTest::Triggered
        }
        AnimationGraphParameterKind::Vec2 => {
            diagnostics.push(diagnostic(
                AnimationGraphDiagnosticSeverity::Error,
                "unsupported_vec2_condition",
                format!("Condition {} compares a Vec2 parameter.", condition.id),
                Some("Compare a scalar/boolean parameter; Vec2 conditions need a named projection node.".into()),
                None,
                None,
                None,
            ));
            return None;
        }
    };
    Some(animation::GraphCondition {
        parameter: animation::GraphParameterId::new(runtime_id),
        test,
    })
}

fn condition_number(condition: &AnimationGraphCondition) -> Option<f64> {
    match condition.value.as_ref()? {
        AnimationGraphValue::Number(value) if value.is_finite() => Some(*value),
        _ => None,
    }
}

fn condition_boolean(condition: &AnimationGraphCondition) -> Option<bool> {
    match condition.value.as_ref()? {
        AnimationGraphValue::Boolean(value) => Some(*value),
        _ => None,
    }
}

fn invalid_condition_operator(
    condition: &AnimationGraphCondition,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) {
    diagnostics.push(diagnostic(
        AnimationGraphDiagnosticSeverity::Error,
        "condition_type_mismatch",
        format!(
            "Condition {} operator/value does not match its parameter type.",
            condition.id
        ),
        Some("Choose a comparison supported by the typed parameter.".into()),
        None,
        None,
        None,
    ));
}

fn enabled_incoming<'a>(
    document: &'a AnimationGraphDocument,
    node_id: &str,
    port: Option<&str>,
) -> Vec<&'a AnimationGraphEdge> {
    let enabled_nodes: BTreeSet<_> = document
        .nodes
        .iter()
        .filter(|node| node.enabled)
        .map(|node| node.id.as_str())
        .collect();
    let mut edges: Vec<_> = document
        .edges
        .iter()
        .filter(|edge| {
            edge.enabled
                && edge.to_node_id == node_id
                && port.is_none_or(|port| edge.to_port_id == port)
                && enabled_nodes.contains(edge.from_node_id.as_str())
                && enabled_nodes.contains(edge.to_node_id.as_str())
        })
        .collect();
    edges.sort_by(|left, right| left.id.cmp(&right.id));
    edges
}

fn validate_document_identity(
    document: &AnimationGraphDocument,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) {
    if document.schema_version != ANIMATION_GRAPH_SCHEMA_VERSION {
        diagnostics.push(diagnostic(
            AnimationGraphDiagnosticSeverity::Error,
            "unsupported_schema",
            format!(
                "Graph schema {} is not supported by this engine (expected {}).",
                document.schema_version, ANIMATION_GRAPH_SCHEMA_VERSION
            ),
            Some("Run the explicit graph migration before saving.".into()),
            None,
            None,
            None,
        ));
    }
    validate_id(&document.id, "graph", None, diagnostics);
    validate_id(&document.sequence_id, "sequence", None, diagnostics);
    validate_id(&document.output_node_id, "output node", None, diagnostics);
    validate_name(&document.name, "graph", None, diagnostics);
}

fn validate_budgets(
    document: &AnimationGraphDocument,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) {
    for (label, actual, limit) in [
        ("nodes", document.nodes.len(), MAX_NODES),
        ("edges", document.edges.len(), MAX_EDGES),
        ("parameters", document.parameters.len(), MAX_PARAMETERS),
        (
            "state machines",
            document.state_machines.len(),
            MAX_STATE_MACHINES,
        ),
    ] {
        if actual > limit {
            diagnostics.push(diagnostic(
                AnimationGraphDiagnosticSeverity::Error,
                "admission_limit",
                format!("Graph has {actual} {label}; the hard admission limit is {limit}."),
                Some(format!("Reduce the graph to at most {limit} {label}.")),
                None,
                None,
                None,
            ));
        }
    }
    let state_count: usize = document.state_machines.iter().map(|m| m.states.len()).sum();
    let transition_count: usize = document
        .state_machines
        .iter()
        .map(|m| m.transitions.len())
        .sum();
    for (label, actual, limit) in [
        ("states", state_count, MAX_STATES),
        ("transitions", transition_count, MAX_TRANSITIONS),
    ] {
        if actual > limit {
            diagnostics.push(diagnostic(
                AnimationGraphDiagnosticSeverity::Error,
                "admission_limit",
                format!("Graph has {actual} {label}; the hard admission limit is {limit}."),
                Some(format!("Reduce the graph to at most {limit} {label}.")),
                None,
                None,
                None,
            ));
        }
    }
}

fn validate_unique_ids<'a>(
    values: impl Iterator<Item = (&'a String, Option<&'a str>)>,
    kind: &str,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for (id, node_id) in values {
        validate_id(id, kind, node_id, diagnostics);
        if !ids.insert(id.clone()) {
            diagnostics.push(diagnostic(
                AnimationGraphDiagnosticSeverity::Error,
                "duplicate_id",
                format!("Duplicate {kind} identity {id} would overwrite collaborative storage."),
                Some(format!("Assign every {kind} a unique stable identity.")),
                node_id.map(ToOwned::to_owned),
                (kind == "edge").then(|| id.clone()),
                None,
            ));
        }
    }
    ids
}

fn validate_id(
    id: &str,
    kind: &str,
    node_id: Option<&str>,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) {
    let valid = !id.is_empty()
        && id.len() <= MAX_ID_BYTES
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if !valid {
        diagnostics.push(diagnostic(
            AnimationGraphDiagnosticSeverity::Error,
            "invalid_id",
            format!(
                "{kind} identity {id:?} must be 1-{MAX_ID_BYTES} ASCII letters, numbers, '.', '_' or '-'."
            ),
            Some("Generate a new stable UUID-style identity for this element.".into()),
            node_id.map(ToOwned::to_owned),
            None,
            None,
        ));
    }
}

fn validate_name(
    name: &str,
    kind: &str,
    node_id: Option<&str>,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) {
    if name.trim().is_empty() || name.len() > MAX_NAME_BYTES {
        diagnostics.push(diagnostic(
            AnimationGraphDiagnosticSeverity::Error,
            "invalid_name",
            format!("{kind} name must be non-empty and at most {MAX_NAME_BYTES} bytes."),
            Some("Choose a short, descriptive name.".into()),
            node_id.map(ToOwned::to_owned),
            None,
            None,
        ));
    }
}

fn validate_parameter(
    parameter: &AnimationGraphParameter,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) {
    validate_name(&parameter.name, "parameter", None, diagnostics);
    if !parameter.default_value.is_finite()
        || parameter.min.is_some_and(|value| !value.is_finite())
        || parameter.max.is_some_and(|value| !value.is_finite())
    {
        diagnostics.push(diagnostic(
            AnimationGraphDiagnosticSeverity::Error,
            "non_finite_parameter",
            format!(
                "Parameter {} contains a non-finite default or range.",
                parameter.name
            ),
            Some("Use finite parameter defaults and bounds.".into()),
            None,
            None,
            None,
        ));
        return;
    }
    let type_matches = match (&parameter.kind, &parameter.default_value) {
        (AnimationGraphParameterKind::Float, AnimationGraphValue::Number(_))
        | (
            AnimationGraphParameterKind::Boolean | AnimationGraphParameterKind::Trigger,
            AnimationGraphValue::Boolean(_),
        )
        | (AnimationGraphParameterKind::Vec2, AnimationGraphValue::Vec2(_)) => true,
        (AnimationGraphParameterKind::Integer, AnimationGraphValue::Number(value)) => {
            value.fract() == 0.0
        }
        _ => false,
    };
    if !type_matches {
        diagnostics.push(diagnostic(
            AnimationGraphDiagnosticSeverity::Error,
            "parameter_type_mismatch",
            format!(
                "Parameter {} default does not match its declared type.",
                parameter.name
            ),
            Some("Choose a default value with the parameter's declared type.".into()),
            None,
            None,
            None,
        ));
    }
    if parameter.kind == AnimationGraphParameterKind::Trigger
        && parameter.default_value == AnimationGraphValue::Boolean(true)
    {
        diagnostics.push(diagnostic(
            AnimationGraphDiagnosticSeverity::Error,
            "trigger_default_true",
            format!(
                "Trigger parameter {} is momentary and cannot have an authored true default.",
                parameter.name
            ),
            Some(
                "Keep trigger defaults false and fire them through transient runtime input.".into(),
            ),
            None,
            None,
            None,
        ));
    }
    let numeric = matches!(
        parameter.kind,
        AnimationGraphParameterKind::Float | AnimationGraphParameterKind::Integer
    );
    if !numeric && (parameter.min.is_some() || parameter.max.is_some()) {
        diagnostics.push(diagnostic(
            AnimationGraphDiagnosticSeverity::Warning,
            "irrelevant_parameter_range",
            format!(
                "Parameter {} is not numeric, so min/max are ignored.",
                parameter.name
            ),
            Some("Clear both range fields for boolean, trigger and Vec2 parameters.".into()),
            None,
            None,
            None,
        ));
    }
    if let (Some(min), Some(max)) = (parameter.min, parameter.max) {
        if min > max {
            diagnostics.push(diagnostic(
                AnimationGraphDiagnosticSeverity::Error,
                "invalid_parameter_range",
                format!(
                    "Parameter {} minimum is greater than its maximum.",
                    parameter.name
                ),
                Some("Swap or correct the numeric bounds.".into()),
                None,
                None,
                None,
            ));
        }
    }
}

fn validate_edge(
    edge: &AnimationGraphEdge,
    nodes: &BTreeMap<&str, &AnimationGraphNode>,
    parameters: &BTreeMap<&str, &AnimationGraphParameter>,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) {
    for (id, label) in [
        (&edge.from_port_id, "edge source port"),
        (&edge.to_port_id, "edge destination port"),
    ] {
        validate_id(id, label, None, diagnostics);
    }
    if edge
        .weight
        .is_some_and(|weight| !weight.is_finite() || weight < 0.0)
    {
        diagnostics.push(edge_diagnostic(
            &edge.id,
            "invalid_weight",
            "An explicit edge weight must be finite and non-negative.",
            "Use a finite weight of zero or greater.",
        ));
    }
    if edge
        .weight_parameter_id
        .as_ref()
        .is_some_and(|id| !parameters.contains_key(id.as_str()))
    {
        diagnostics.push(edge_diagnostic(
            &edge.id,
            "missing_parameter",
            format!(
                "Connection weight references missing parameter {}.",
                edge.weight_parameter_id.as_deref().unwrap_or_default()
            ),
            "Choose an existing float parameter or clear the connection binding.",
        ));
    } else if edge.weight_parameter_id.as_ref().is_some_and(|id| {
        parameters
            .get(id.as_str())
            .is_some_and(|parameter| parameter.kind != AnimationGraphParameterKind::Float)
    }) {
        diagnostics.push(edge_diagnostic(
            &edge.id,
            "edge_weight_parameter_type",
            "A connection weight binding must reference a float parameter.",
            "Choose a float parameter or clear the connection binding.",
        ));
    }
    let Some(source) = nodes.get(edge.from_node_id.as_str()) else {
        diagnostics.push(edge_diagnostic(
            &edge.id,
            "dangling_edge",
            format!(
                "Connection source node {} does not exist.",
                edge.from_node_id
            ),
            "Reconnect the edge to an existing pose node or remove it.",
        ));
        return;
    };
    let Some(destination) = nodes.get(edge.to_node_id.as_str()) else {
        diagnostics.push(edge_diagnostic(
            &edge.id,
            "dangling_edge",
            format!(
                "Connection destination node {} does not exist.",
                edge.to_node_id
            ),
            "Reconnect the edge to an existing input or remove it.",
        ));
        return;
    };
    if source.kind == AnimationGraphNodeKind::Output || edge.from_port_id != "pose" {
        diagnostics.push(edge_port_diagnostic(
            &edge.id,
            &edge.from_node_id,
            &edge.from_port_id,
            "invalid_source_port",
            "Only a pose-producing node's 'pose' port can feed the animation graph.",
            "Start the connection from a pose output.",
        ));
    }
    let valid_destination = match destination.kind {
        AnimationGraphNodeKind::ReferencePose | AnimationGraphNodeKind::Sequence => false,
        AnimationGraphNodeKind::BlendNormalized
        | AnimationGraphNodeKind::BlendDirect
        | AnimationGraphNodeKind::Blend1d
        | AnimationGraphNodeKind::Blend2dCartesian => edge.to_port_id == "poses",
        AnimationGraphNodeKind::LayerOverride | AnimationGraphNodeKind::LayerAdditive => {
            matches!(edge.to_port_id.as_str(), "base" | "layer")
        }
        AnimationGraphNodeKind::StateMachine => edge.to_port_id == "states",
        AnimationGraphNodeKind::Output => edge.to_port_id == "pose",
    };
    if !valid_destination {
        diagnostics.push(edge_port_diagnostic(
            &edge.id,
            &edge.to_node_id,
            &edge.to_port_id,
            "invalid_destination_port",
            "The destination port is not a supported pose input for this node kind.",
            "Reconnect to one of the node's named pose inputs.",
        ));
    }
    let supports_parameter = matches!(
        destination.kind,
        AnimationGraphNodeKind::BlendNormalized | AnimationGraphNodeKind::BlendDirect
    ) && edge.to_port_id == "poses";
    let supports_explicit = supports_parameter
        || (matches!(
            destination.kind,
            AnimationGraphNodeKind::LayerOverride | AnimationGraphNodeKind::LayerAdditive
        ) && edge.to_port_id == "layer"
            && destination.parameter_ids.is_empty());
    if (edge.weight.is_some() && !supports_explicit)
        || (edge.weight_parameter_id.is_some() && !supports_parameter)
        || (edge.weight.is_some() && edge.weight_parameter_id.is_some())
    {
        diagnostics.push(edge_port_diagnostic(
            &edge.id,
            &edge.to_node_id,
            &edge.to_port_id,
            "unsupported_edge_weight_contract",
            "This connection persists weight data that its destination adapter does not consume.",
            if supports_explicit || supports_parameter {
                "Choose one supported weight source and clear the other field."
            } else if matches!(
                destination.kind,
                AnimationGraphNodeKind::LayerOverride | AnimationGraphNodeKind::LayerAdditive
            ) && edge.to_port_id == "layer"
                && !destination.parameter_ids.is_empty()
            {
                "The layer node parameter owns this weight; clear the ignored layer-edge constant and binding."
            } else {
                "Clear both edge weight fields; this input has no authored edge-weight contract."
            },
        ));
    }
}

fn validate_output(
    document: &AnimationGraphDocument,
    nodes: &BTreeMap<&str, &AnimationGraphNode>,
    enabled_edges: &[&AnimationGraphEdge],
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) {
    let outputs: Vec<_> = document
        .nodes
        .iter()
        .filter(|node| node.kind == AnimationGraphNodeKind::Output && node.enabled)
        .collect();
    if outputs.len() != 1 {
        diagnostics.push(diagnostic(
            AnimationGraphDiagnosticSeverity::Error,
            "output_count",
            format!(
                "Graph needs exactly one enabled Output node; found {}.",
                outputs.len()
            ),
            Some("Enable one Output node and disable or remove every other Output.".into()),
            None,
            None,
            None,
        ));
    }
    let Some(output) = nodes.get(document.output_node_id.as_str()) else {
        diagnostics.push(diagnostic(
            AnimationGraphDiagnosticSeverity::Error,
            "missing_output",
            "The selected graph output node no longer exists.",
            Some("Choose an existing Output node.".into()),
            Some(document.output_node_id.clone()),
            None,
            Some("pose".into()),
        ));
        return;
    };
    if output.kind != AnimationGraphNodeKind::Output || !output.enabled {
        diagnostics.push(node_diagnostic(
            &output.id,
            "invalid_output",
            "The selected graph output is not an enabled Output node.",
            "Choose and enable the final Output node.",
        ));
    }
    let incoming: Vec<_> = enabled_edges
        .iter()
        .filter(|edge| edge.to_node_id == output.id && edge.to_port_id == "pose")
        .collect();
    if incoming.len() != 1 {
        diagnostics.push(diagnostic(
            AnimationGraphDiagnosticSeverity::Error,
            "output_input_count",
            format!(
                "Output needs exactly one enabled pose input; found {}.",
                incoming.len()
            ),
            Some("Connect one pose-producing node to Output.".into()),
            Some(output.id.clone()),
            None,
            Some("pose".into()),
        ));
    }
}

fn validate_node_inputs(
    document: &AnimationGraphDocument,
    nodes: &BTreeMap<&str, &AnimationGraphNode>,
    enabled_edges: &[&AnimationGraphEdge],
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) {
    for node in document.nodes.iter().filter(|node| node.enabled) {
        let mut incoming: Vec<_> = enabled_edges
            .iter()
            .copied()
            .filter(|edge| edge.to_node_id == node.id)
            .collect();
        incoming.sort_by(|left, right| left.id.cmp(&right.id));
        match node.kind {
            AnimationGraphNodeKind::ReferencePose => {
                if !incoming.is_empty() {
                    diagnostics.push(node_diagnostic(
                        &node.id,
                        "source_has_input",
                        "Reference-pose nodes cannot consume pose inputs.",
                        "Remove incoming connections from the source node.",
                    ));
                }
            }
            AnimationGraphNodeKind::Sequence => {
                if node.source_id.as_deref().is_none_or(str::is_empty) {
                    diagnostics.push(node_diagnostic(
                        &node.id,
                        "missing_source",
                        "Sequence node has no source identity.",
                        "Choose a ready authored sequence or imported clip source.",
                    ));
                }
                if !incoming.is_empty() {
                    diagnostics.push(node_diagnostic(
                        &node.id,
                        "source_has_input",
                        "Sequence nodes cannot consume pose inputs.",
                        "Remove incoming connections from the source node.",
                    ));
                }
            }
            AnimationGraphNodeKind::BlendNormalized | AnimationGraphNodeKind::BlendDirect => {
                if incoming.is_empty() {
                    diagnostics.push(node_diagnostic(
                        &node.id,
                        "missing_blend_inputs",
                        "Blend node needs at least one enabled pose input.",
                        "Connect one or more pose sources.",
                    ));
                }
            }
            AnimationGraphNodeKind::Blend1d => {
                validate_blend_1d(node, &incoming, nodes, diagnostics);
            }
            AnimationGraphNodeKind::Blend2dCartesian => {
                validate_blend_2d(node, &incoming, nodes, diagnostics);
            }
            AnimationGraphNodeKind::LayerOverride | AnimationGraphNodeKind::LayerAdditive => {
                for port in ["base", "layer"] {
                    let count = incoming
                        .iter()
                        .filter(|edge| edge.to_port_id == port)
                        .count();
                    if count != 1 {
                        diagnostics.push(diagnostic(
                            AnimationGraphDiagnosticSeverity::Error,
                            "layer_input_count",
                            format!(
                                "Layer node input '{port}' needs exactly one pose; found {count}."
                            ),
                            Some(format!("Connect exactly one pose to the {port} input.")),
                            Some(node.id.clone()),
                            None,
                            Some(port.into()),
                        ));
                    }
                }
                if node.mask_bindings.is_empty() {
                    diagnostics.push(diagnostic(
                        AnimationGraphDiagnosticSeverity::Warning,
                        "empty_mask",
                        "An empty layer mask affects the complete property bundle.",
                        Some("Add exact binding selectors when only part of the asset should be layered.".into()),
                        Some(node.id.clone()),
                        None,
                        None,
                    ));
                }
                for selector in &node.mask_bindings {
                    if let Some(reason) = mask_selector_error(selector) {
                        diagnostics.push(diagnostic(
                            AnimationGraphDiagnosticSeverity::Error,
                            "invalid_mask_selector",
                            format!("Mask selector {selector:?} is invalid: {reason}"),
                            Some("Use a slash path such as **/Transform/rotation; '*' matches one segment and '**' any depth.".into()),
                            Some(node.id.clone()),
                            None,
                            Some("mask".into()),
                        ));
                    }
                }
            }
            AnimationGraphNodeKind::StateMachine => {
                if node.state_machine_id.as_deref().is_none_or(str::is_empty) {
                    diagnostics.push(node_diagnostic(
                        &node.id,
                        "missing_state_machine",
                        "State-machine node has no state-machine identity.",
                        "Choose or create one state machine.",
                    ));
                }
            }
            AnimationGraphNodeKind::Output => {}
        }
    }
}

fn validate_blend_1d(
    node: &AnimationGraphNode,
    incoming: &[&AnimationGraphEdge],
    _nodes: &BTreeMap<&str, &AnimationGraphNode>,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) {
    if incoming.len() < 2 {
        diagnostics.push(node_diagnostic(
            &node.id,
            "blend_1d_input_count",
            "Blend1D needs at least two enabled pose samples.",
            "Connect two or more pose sources.",
        ));
    }
    if node.parameter_ids.len() != 1 {
        diagnostics.push(node_diagnostic(
            &node.id,
            "blend_1d_parameter_count",
            "Blend1D needs exactly one scalar parameter.",
            "Bind one float or integer parameter.",
        ));
    }
    let incoming_ids: BTreeSet<_> = incoming.iter().map(|edge| edge.id.as_str()).collect();
    if node.samples.len() != incoming.len()
        || node
            .samples
            .iter()
            .any(|sample| !incoming_ids.contains(sample.edge_id.as_str()))
    {
        diagnostics.push(node_diagnostic(
            &node.id,
            "blend_1d_sample_count",
            format!(
                "Blend1D has {} inputs but {} keyed samples.",
                incoming.len(),
                node.samples.len()
            ),
            "Author exactly one finite sample keyed to each input edge identity.",
        ));
    }
    let mut sorted: Vec<_> = node
        .samples
        .iter()
        .map(|sample| sample.position[0])
        .collect();
    sorted.sort_by(f64::total_cmp);
    if sorted.windows(2).any(|pair| pair[0] == pair[1]) {
        diagnostics.push(node_diagnostic(
            &node.id,
            "duplicate_blend_1d_position",
            "Blend1D sample x positions must be unique.",
            "Move overlapping samples to distinct parameter values.",
        ));
    }
    if node.samples.iter().any(|sample| sample.position[1] != 0.0) {
        diagnostics.push(node_diagnostic(
            &node.id,
            "blend_1d_nonzero_y",
            "Blend1D samples must use zero for the unused y coordinate.",
            "Set every Blend1D sample y coordinate to zero.",
        ));
    }
}

fn validate_blend_2d(
    node: &AnimationGraphNode,
    incoming: &[&AnimationGraphEdge],
    _nodes: &BTreeMap<&str, &AnimationGraphNode>,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) {
    if incoming.len() < 3 {
        diagnostics.push(node_diagnostic(
            &node.id,
            "blend_2d_input_count",
            "Cartesian Blend2D needs at least three enabled pose samples.",
            "Connect at least three pose sources.",
        ));
    }
    if !matches!(node.parameter_ids.len(), 1 | 2) {
        diagnostics.push(node_diagnostic(
            &node.id,
            "blend_2d_parameter_count",
            "Cartesian Blend2D needs one Vec2 parameter or two scalar parameters.",
            "Bind one Vec2 or an explicit X/Y scalar pair.",
        ));
    }
    let incoming_by_id: BTreeMap<_, _> = incoming
        .iter()
        .map(|edge| (edge.id.as_str(), edge.from_node_id.as_str()))
        .collect();
    if node.samples.len() != incoming.len()
        || node
            .samples
            .iter()
            .any(|sample| !incoming_by_id.contains_key(sample.edge_id.as_str()))
    {
        diagnostics.push(node_diagnostic(
            &node.id,
            "blend_2d_point_count",
            format!(
                "Cartesian Blend2D has {} inputs but {} keyed samples.",
                incoming.len(),
                node.samples.len()
            ),
            "Author one finite x/y sample keyed to each input edge identity.",
        ));
    }
    let points: BTreeMap<_, _> = node
        .samples
        .iter()
        .map(|sample| (sample.id.as_str(), sample.position))
        .collect();
    if points.len() != node.samples.len() {
        diagnostics.push(node_diagnostic(
            &node.id,
            "duplicate_blend_2d_sample",
            "Cartesian Blend2D sample identities must be unique.",
            "Give each keyed point a stable unique sample identity.",
        ));
    }
    if node.triangles.is_empty() {
        diagnostics.push(node_diagnostic(
            &node.id,
            "missing_blend_2d_triangles",
            "Cartesian Blend2D requires an authored stable triangulation.",
            "Triangulate the sample points in the editor and save the triangle identities.",
        ));
    }
    for triangle in &node.triangles {
        if triangle.iter().collect::<BTreeSet<_>>().len() != 3
            || triangle.iter().any(|id| !points.contains_key(id.as_str()))
        {
            diagnostics.push(node_diagnostic(
                &node.id,
                "invalid_blend_2d_triangle",
                "A Blend2D triangle must reference three distinct connected sample identities.",
                "Repair or regenerate the authored triangle list.",
            ));
            continue;
        }
        let a = points[triangle[0].as_str()];
        let b = points[triangle[1].as_str()];
        let c = points[triangle[2].as_str()];
        let area2 = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        if area2.abs() <= f64::EPSILON {
            diagnostics.push(node_diagnostic(
                &node.id,
                "degenerate_blend_2d_triangle",
                "A Blend2D triangle is collinear or degenerate.",
                "Move one sample point or regenerate the triangulation.",
            ));
        }
    }
}

fn validate_pose_dag(
    document: &AnimationGraphDocument,
    nodes: &BTreeMap<&str, &AnimationGraphNode>,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) {
    let enabled_nodes: BTreeSet<_> = document
        .nodes
        .iter()
        .filter(|node| node.enabled)
        .map(|node| node.id.as_str())
        .collect();
    let mut indegree: BTreeMap<&str, usize> = enabled_nodes.iter().map(|id| (*id, 0)).collect();
    let mut outgoing: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for edge in document.edges.iter().filter(|edge| edge.enabled) {
        if nodes.contains_key(edge.from_node_id.as_str())
            && nodes.contains_key(edge.to_node_id.as_str())
            && enabled_nodes.contains(edge.from_node_id.as_str())
            && enabled_nodes.contains(edge.to_node_id.as_str())
        {
            *indegree.entry(edge.to_node_id.as_str()).or_default() += 1;
            outgoing
                .entry(edge.from_node_id.as_str())
                .or_default()
                .push(edge.to_node_id.as_str());
        }
    }
    for targets in outgoing.values_mut() {
        targets.sort_unstable();
    }
    let mut queue: VecDeque<_> = indegree
        .iter()
        .filter_map(|(id, degree)| (*degree == 0).then_some(*id))
        .collect();
    let mut visited = 0;
    while let Some(id) = queue.pop_front() {
        visited += 1;
        for target in outgoing.get(id).into_iter().flatten() {
            let degree = indegree.get_mut(target).expect("known graph node");
            *degree -= 1;
            if *degree == 0 {
                queue.push_back(target);
            }
        }
    }
    if visited != enabled_nodes.len() {
        let cycle_nodes: Vec<_> = indegree
            .into_iter()
            .filter_map(|(id, degree)| (degree > 0).then_some(id))
            .collect();
        diagnostics.push(diagnostic(
            AnimationGraphDiagnosticSeverity::Error,
            "pose_cycle",
            format!(
                "Pose graph contains a cycle involving {}.",
                cycle_nodes.join(", ")
            ),
            Some(
                "Remove a pose edge; state-machine transitions are the only cyclic graph layer."
                    .into(),
            ),
            cycle_nodes.first().map(|id| (*id).to_owned()),
            None,
            None,
        ));
    }
}

fn validate_state_machines(
    document: &AnimationGraphDocument,
    nodes: &BTreeMap<&str, &AnimationGraphNode>,
    parameter_ids: &BTreeSet<String>,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) {
    let machine_nodes: BTreeMap<_, _> = document
        .nodes
        .iter()
        .filter_map(|node| {
            (node.kind == AnimationGraphNodeKind::StateMachine)
                .then(|| node.state_machine_id.as_deref().map(|id| (id, node)))
                .flatten()
        })
        .collect();
    for machine in &document.state_machines {
        validate_name(&machine.name, "state machine", None, diagnostics);
        if !machine_nodes.contains_key(machine.id.as_str()) {
            diagnostics.push(diagnostic(
                AnimationGraphDiagnosticSeverity::Error,
                "orphan_state_machine",
                format!("State machine {} has no graph node.", machine.name),
                Some("Add a State Machine node or remove this orphaned machine.".into()),
                None,
                None,
                None,
            ));
        } else if let Some(machine_node) = machine_nodes.get(machine.id.as_str()) {
            // State pose identities are semantic control data; the façade edges must be the exact visual
            // projection of those identities so the canvas never depicts a different program than runtime.
            let expected_sources: BTreeSet<_> = machine
                .states
                .iter()
                .map(|state| state.pose_node_id.as_str())
                .collect();
            let incoming: Vec<_> = document
                .edges
                .iter()
                .filter(|edge| {
                    edge.enabled
                        && edge.to_node_id == machine_node.id
                        && edge.to_port_id == "states"
                })
                .collect();
            let actual_sources: BTreeSet<_> = incoming
                .iter()
                .map(|edge| edge.from_node_id.as_str())
                .collect();
            let mut edges_by_source: BTreeMap<&str, Vec<&AnimationGraphEdge>> = BTreeMap::new();
            for edge in &incoming {
                edges_by_source
                    .entry(edge.from_node_id.as_str())
                    .or_default()
                    .push(edge);
            }
            for (source, edges) in &edges_by_source {
                if expected_sources.contains(source) && edges.len() > 1 {
                    diagnostics.push(edge_port_diagnostic(
                        &edges[1].id,
                        &machine_node.id,
                        "states",
                        "duplicate_state_pose_edge",
                        format!(
                            "State machine {} has {} enabled façade edges for state pose {source}.",
                            machine.name,
                            edges.len()
                        ),
                        "Keep exactly one enabled façade edge for each semantic state pose root.",
                    ));
                }
            }
            for missing in expected_sources.difference(&actual_sources) {
                diagnostics.push(diagnostic(
                    AnimationGraphDiagnosticSeverity::Error,
                    "missing_state_pose_edge",
                    format!(
                        "State machine {} hides state pose {missing} because its façade edge is missing.",
                        machine.name
                    ),
                    Some("Connect the state pose to the machine's 'states' port.".into()),
                    Some(machine_node.id.clone()),
                    None,
                    Some("states".into()),
                ));
            }
            for edge in incoming {
                if !expected_sources.contains(edge.from_node_id.as_str()) {
                    diagnostics.push(edge_port_diagnostic(
                        &edge.id,
                        &machine_node.id,
                        "states",
                        "orphan_state_pose_edge",
                        "This façade edge is not referenced by any state and would misrepresent runtime.",
                        "Remove the edge or assign its source pose to a state.",
                    ));
                }
            }
        }
        let state_ids = validate_unique_ids(
            machine.states.iter().map(|state| (&state.id, None)),
            "state",
            diagnostics,
        );
        let _transition_ids = validate_unique_ids(
            machine
                .transitions
                .iter()
                .map(|transition| (&transition.id, None)),
            "transition",
            diagnostics,
        );
        if !state_ids.contains(&machine.entry_state_id) {
            diagnostics.push(diagnostic(
                AnimationGraphDiagnosticSeverity::Error,
                "missing_entry_state",
                format!("State machine {} entry state does not exist.", machine.name),
                Some("Choose one of the machine's stable state identities as its entry.".into()),
                None,
                None,
                None,
            ));
        }
        for state in &machine.states {
            validate_name(&state.name, "state", None, diagnostics);
            let valid_pose = nodes.get(state.pose_node_id.as_str()).is_some_and(|node| {
                !matches!(
                    node.kind,
                    AnimationGraphNodeKind::Output | AnimationGraphNodeKind::StateMachine
                )
            });
            if !valid_pose {
                diagnostics.push(diagnostic(
                    AnimationGraphDiagnosticSeverity::Error,
                    "invalid_state_pose",
                    format!(
                        "State {} references a missing or non-pose node.",
                        state.name
                    ),
                    Some("Choose a pose-producing DAG node for this state.".into()),
                    Some(state.pose_node_id.clone()),
                    None,
                    None,
                ));
            }
        }
        for transition in &machine.transitions {
            validate_transition(transition, &state_ids, parameter_ids, diagnostics);
        }
        if let Some(cycle) = unconditional_zero_duration_transition_cycle(machine) {
            diagnostics.push(diagnostic(
                AnimationGraphDiagnosticSeverity::Error,
                "unconditional_zero_duration_cycle",
                format!(
                    "Unconditional zero-duration transitions form a cycle: {}.",
                    cycle.join(", ")
                ),
                Some("Add a typed condition, give at least one transition a positive duration, or break the cycle.".into()),
                machine_nodes.get(machine.id.as_str()).map(|node| node.id.clone()),
                None,
                Some("states".into()),
            ));
        }
    }
    for (machine_id, node) in machine_nodes {
        if !document
            .state_machines
            .iter()
            .any(|machine| machine.id == machine_id)
        {
            diagnostics.push(node_diagnostic(
                &node.id,
                "missing_state_machine",
                format!("State-machine record {machine_id} does not exist."),
                "Create the named state machine or choose an existing one.",
            ));
        }
    }
}

fn unconditional_zero_duration_transition_cycle(
    machine: &AnimationGraphStateMachine,
) -> Option<Vec<String>> {
    let mut adjacency: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
    for transition in machine.transitions.iter().filter(|transition| {
        transition.duration_tick == 0
            && transition.conditions.is_empty()
            && transition.exit_time.is_none()
    }) {
        adjacency
            .entry(transition.from_state_id.as_str())
            .or_default()
            .push((transition.to_state_id.as_str(), transition.id.as_str()));
    }
    for edges in adjacency.values_mut() {
        edges.sort_unstable_by(|left, right| left.1.cmp(right.1).then(left.0.cmp(right.0)));
    }
    fn visit<'a>(
        state: &'a str,
        adjacency: &BTreeMap<&'a str, Vec<(&'a str, &'a str)>>,
        colors: &mut BTreeMap<&'a str, u8>,
        state_stack: &mut Vec<&'a str>,
        edge_stack: &mut Vec<&'a str>,
    ) -> Option<Vec<String>> {
        colors.insert(state, 1);
        state_stack.push(state);
        for (target, transition_id) in adjacency.get(state).into_iter().flatten() {
            match colors.get(target).copied().unwrap_or(0) {
                0 => {
                    edge_stack.push(transition_id);
                    if let Some(cycle) = visit(target, adjacency, colors, state_stack, edge_stack) {
                        return Some(cycle);
                    }
                    edge_stack.pop();
                }
                1 => {
                    let start = state_stack
                        .iter()
                        .position(|candidate| candidate == target)?;
                    let mut cycle: Vec<_> = edge_stack[start..]
                        .iter()
                        .map(|id| (*id).to_owned())
                        .collect();
                    cycle.push((*transition_id).to_owned());
                    return Some(cycle);
                }
                _ => {}
            }
        }
        state_stack.pop();
        colors.insert(state, 2);
        None
    }
    let mut colors = BTreeMap::new();
    let mut state_stack = Vec::new();
    let mut edge_stack = Vec::new();
    let mut states: Vec<_> = machine
        .states
        .iter()
        .map(|state| state.id.as_str())
        .collect();
    states.sort_unstable();
    for state in states {
        if colors.get(state).copied().unwrap_or(0) == 0 {
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

fn validate_transition(
    transition: &AnimationGraphTransition,
    state_ids: &BTreeSet<String>,
    parameter_ids: &BTreeSet<String>,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) {
    if !state_ids.contains(&transition.from_state_id)
        || !state_ids.contains(&transition.to_state_id)
    {
        diagnostics.push(diagnostic(
            AnimationGraphDiagnosticSeverity::Error,
            "dangling_transition",
            format!(
                "Transition {} references a missing source or destination state.",
                transition.id
            ),
            Some("Reconnect the transition to states in the same machine.".into()),
            None,
            None,
            None,
        ));
    }
    if transition.duration_tick < 0 {
        diagnostics.push(diagnostic(
            AnimationGraphDiagnosticSeverity::Error,
            "negative_transition_duration",
            format!("Transition {} has a negative duration.", transition.id),
            Some(
                "Use zero ticks for an instant transition or a positive crossfade duration.".into(),
            ),
            None,
            None,
            None,
        ));
    }
    if transition
        .exit_time
        .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
    {
        diagnostics.push(diagnostic(
            AnimationGraphDiagnosticSeverity::Error,
            "invalid_exit_time",
            format!(
                "Transition {} exit time must be normalized to [0, 1].",
                transition.id
            ),
            Some("Use a finite normalized source time or clear exit time.".into()),
            None,
            None,
            None,
        ));
    }
    if transition.exit_time.is_some() {
        diagnostics.push(diagnostic(
            AnimationGraphDiagnosticSeverity::Error,
            "unsupported_exit_time",
            format!("Transition {} uses exit time, but arbitrary graph roots have no canonical duration yet.", transition.id),
            Some("Clear exit time and use typed conditions until normalized-cycle synchronization is implemented.".into()),
            None,
            None,
            None,
        ));
    }
    if transition.conditions.len() > MAX_CONDITIONS_PER_TRANSITION {
        diagnostics.push(diagnostic(
            AnimationGraphDiagnosticSeverity::Error,
            "admission_limit",
            format!(
                "Transition {} has {} conditions; the limit is {}.",
                transition.id,
                transition.conditions.len(),
                MAX_CONDITIONS_PER_TRANSITION
            ),
            Some("Split or simplify the transition condition set.".into()),
            None,
            None,
            None,
        ));
    }
    let mut condition_ids = BTreeSet::new();
    for condition in &transition.conditions {
        validate_id(&condition.id, "condition", None, diagnostics);
        if !condition_ids.insert(condition.id.clone()) {
            diagnostics.push(diagnostic(
                AnimationGraphDiagnosticSeverity::Error,
                "duplicate_id",
                format!(
                    "Transition {} repeats condition identity {}.",
                    transition.id, condition.id
                ),
                Some("Assign every condition a unique stable identity.".into()),
                None,
                None,
                None,
            ));
        }
        if !parameter_ids.contains(&condition.parameter_id) {
            diagnostics.push(diagnostic(
                AnimationGraphDiagnosticSeverity::Error,
                "missing_parameter",
                format!("Condition {} references a missing parameter.", condition.id),
                Some("Choose an existing typed parameter.".into()),
                None,
                None,
                None,
            ));
        }
        if condition
            .value
            .as_ref()
            .is_some_and(|value| !value.is_finite())
        {
            diagnostics.push(diagnostic(
                AnimationGraphDiagnosticSeverity::Error,
                "non_finite_condition",
                format!(
                    "Condition {} contains a non-finite comparison value.",
                    condition.id
                ),
                Some("Use a finite typed comparison value.".into()),
                None,
                None,
                None,
            ));
        }
    }
}

fn desired_fields(
    document: &AnimationGraphDocument,
) -> Result<BTreeMap<String, FieldValue>, AnimationGraphIntentError> {
    let mut fields = BTreeMap::new();
    let graph = &document.id;
    insert_field(
        &mut fields,
        graph_field(graph, "active"),
        FieldValue::Bool(true),
    );
    insert_field(
        &mut fields,
        graph_field(graph, "schema"),
        FieldValue::Integer(i64::from(document.schema_version)),
    );
    insert_field(
        &mut fields,
        graph_field(graph, "sequence_id"),
        FieldValue::Str(document.sequence_id.clone()),
    );
    insert_field(
        &mut fields,
        graph_field(graph, "name"),
        FieldValue::Str(document.name.clone()),
    );
    insert_field(
        &mut fields,
        graph_field(graph, "output_node_id"),
        FieldValue::Str(document.output_node_id.clone()),
    );
    for node in &document.nodes {
        insert_field(
            &mut fields,
            element_field(NODE_PREFIX, graph, &node.id, "active"),
            FieldValue::Bool(true),
        );
        insert_field(
            &mut fields,
            element_field(NODE_PREFIX, graph, &node.id, "payload"),
            FieldValue::Str(encode(&StoredNodePayload::from(node))?),
        );
        insert_field(
            &mut fields,
            element_field(NODE_PREFIX, graph, &node.id, "x"),
            FieldValue::Number(node.position.x),
        );
        insert_field(
            &mut fields,
            element_field(NODE_PREFIX, graph, &node.id, "y"),
            FieldValue::Number(node.position.y),
        );
    }
    for edge in &document.edges {
        insert_field(
            &mut fields,
            element_field(EDGE_PREFIX, graph, &edge.id, "active"),
            FieldValue::Bool(true),
        );
        insert_field(
            &mut fields,
            element_field(EDGE_PREFIX, graph, &edge.id, "payload"),
            FieldValue::Str(encode(&StoredEdgePayload::from(edge))?),
        );
    }
    for parameter in &document.parameters {
        insert_field(
            &mut fields,
            element_field(PARAMETER_PREFIX, graph, &parameter.id, "active"),
            FieldValue::Bool(true),
        );
        insert_field(
            &mut fields,
            element_field(PARAMETER_PREFIX, graph, &parameter.id, "payload"),
            FieldValue::Str(encode(&StoredParameterPayload::from(parameter))?),
        );
    }
    for machine in &document.state_machines {
        insert_field(
            &mut fields,
            element_field(MACHINE_PREFIX, graph, &machine.id, "active"),
            FieldValue::Bool(true),
        );
        insert_field(
            &mut fields,
            element_field(MACHINE_PREFIX, graph, &machine.id, "name"),
            FieldValue::Str(machine.name.clone()),
        );
        insert_field(
            &mut fields,
            element_field(MACHINE_PREFIX, graph, &machine.id, "entry_state_id"),
            FieldValue::Str(machine.entry_state_id.clone()),
        );
        for state in &machine.states {
            insert_field(
                &mut fields,
                nested_element_field(STATE_PREFIX, graph, &machine.id, &state.id, "active"),
                FieldValue::Bool(true),
            );
            insert_field(
                &mut fields,
                nested_element_field(STATE_PREFIX, graph, &machine.id, &state.id, "payload"),
                FieldValue::Str(encode(&StoredStatePayload::from(state))?),
            );
        }
        for transition in &machine.transitions {
            insert_field(
                &mut fields,
                nested_element_field(
                    TRANSITION_PREFIX,
                    graph,
                    &machine.id,
                    &transition.id,
                    "active",
                ),
                FieldValue::Bool(true),
            );
            insert_field(
                &mut fields,
                nested_element_field(
                    TRANSITION_PREFIX,
                    graph,
                    &machine.id,
                    &transition.id,
                    "payload",
                ),
                FieldValue::Str(encode(&StoredTransitionPayload::from(transition))?),
            );
        }
    }
    Ok(fields)
}

fn reconstruct_document(
    graph_id: &str,
    records: StoredRecords,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) -> Option<AnimationGraphDocument> {
    if !is_active(&records.graph) {
        return None;
    }
    let schema_version = required_integer(&records.graph, "schema", "graph", diagnostics)?;
    let schema_version = u32::try_from(schema_version).ok()?;
    let sequence_id = required_string(&records.graph, "sequence_id", "graph", diagnostics)?;
    let name = required_string(&records.graph, "name", "graph", diagnostics)?;
    let output_node_id = required_string(&records.graph, "output_node_id", "graph", diagnostics)?;
    let mut nodes = Vec::new();
    for (id, fields) in records.nodes {
        if !is_active(&fields) {
            continue;
        }
        let Some(payload) =
            required_payload::<StoredNodePayload>(&fields, "payload", "node", &id, diagnostics)
        else {
            continue;
        };
        let Some(x) = required_number(&fields, "x", "node", &id, diagnostics) else {
            continue;
        };
        let Some(y) = required_number(&fields, "y", "node", &id, diagnostics) else {
            continue;
        };
        nodes.push(AnimationGraphNode {
            id,
            kind: payload.kind,
            name: payload.name,
            position: AnimationGraphPosition { x, y },
            enabled: payload.enabled,
            source_id: payload.source_id,
            parameter_ids: payload.parameter_ids,
            samples: payload.samples,
            thresholds: payload.thresholds,
            triangles: payload.triangles,
            mask_bindings: payload.mask_bindings,
            state_machine_id: payload.state_machine_id,
        });
    }
    let mut edges = Vec::new();
    for (id, fields) in records.edges {
        if !is_active(&fields) {
            continue;
        }
        let Some(payload) =
            required_payload::<StoredEdgePayload>(&fields, "payload", "edge", &id, diagnostics)
        else {
            continue;
        };
        edges.push(AnimationGraphEdge {
            id,
            from_node_id: payload.from_node_id,
            from_port_id: payload.from_port_id,
            to_node_id: payload.to_node_id,
            to_port_id: payload.to_port_id,
            enabled: payload.enabled,
            weight: payload.weight,
            weight_parameter_id: payload.weight_parameter_id,
        });
    }
    let mut parameters = Vec::new();
    for (id, fields) in records.parameters {
        if !is_active(&fields) {
            continue;
        }
        let Some(payload) = required_payload::<StoredParameterPayload>(
            &fields,
            "payload",
            "parameter",
            &id,
            diagnostics,
        ) else {
            continue;
        };
        parameters.push(AnimationGraphParameter {
            id,
            name: payload.name,
            kind: payload.kind,
            default_value: payload.default_value,
            min: payload.min,
            max: payload.max,
        });
    }
    let mut state_machines = Vec::new();
    for (id, fields) in records.machines {
        if !is_active(&fields) {
            continue;
        }
        let Some(machine_name) =
            required_string_record(&fields, "name", "state machine", &id, diagnostics)
        else {
            continue;
        };
        let Some(entry_state_id) =
            required_string_record(&fields, "entry_state_id", "state machine", &id, diagnostics)
        else {
            continue;
        };
        let mut states = Vec::new();
        for ((machine_id, state_id), state_fields) in &records.states {
            if machine_id != &id || !is_active(state_fields) {
                continue;
            }
            let Some(payload) = required_payload::<StoredStatePayload>(
                state_fields,
                "payload",
                "state",
                state_id,
                diagnostics,
            ) else {
                continue;
            };
            states.push(AnimationGraphState {
                id: state_id.clone(),
                name: payload.name,
                pose_node_id: payload.pose_node_id,
                reset_on_entry: payload.reset_on_entry,
            });
        }
        let mut transitions = Vec::new();
        for ((machine_id, transition_id), transition_fields) in &records.transitions {
            if machine_id != &id || !is_active(transition_fields) {
                continue;
            }
            let Some(payload) = required_payload::<StoredTransitionPayload>(
                transition_fields,
                "payload",
                "transition",
                transition_id,
                diagnostics,
            ) else {
                continue;
            };
            transitions.push(AnimationGraphTransition {
                id: transition_id.clone(),
                from_state_id: payload.from_state_id,
                to_state_id: payload.to_state_id,
                priority: payload.priority,
                duration_tick: payload.duration_tick,
                curve: payload.curve,
                interruption: payload.interruption,
                conditions: payload.conditions,
                exit_time: payload.exit_time,
            });
        }
        state_machines.push(AnimationGraphStateMachine {
            id,
            name: machine_name,
            entry_state_id,
            states,
            transitions,
        });
    }
    let document = AnimationGraphDocument {
        schema_version,
        id: graph_id.to_owned(),
        sequence_id,
        name,
        output_node_id,
        nodes,
        edges,
        parameters,
        state_machines,
    };
    Some(document)
}

fn active_graph_candidates(
    engine: &Engine<FlecsWorld>,
    sequence_id: &str,
) -> Vec<(EntityId, String)> {
    let mut out = Vec::new();
    for entity in engine.entity_ids() {
        let fields = component_fields(engine, entity);
        let mut graphs: BTreeMap<String, BTreeMap<String, FieldValue>> = BTreeMap::new();
        for (field, value) in fields {
            let parts: Vec<_> = field.split("::").collect();
            if parts.len() == 3 && parts[0] == GRAPH_PREFIX {
                graphs
                    .entry(parts[1].to_owned())
                    .or_default()
                    .insert(parts[2].to_owned(), value);
            }
        }
        for (graph_id, values) in graphs {
            if is_active(&values)
                && values.get("sequence_id") == Some(&FieldValue::Str(sequence_id.to_owned()))
            {
                out.push((entity, graph_id));
            }
        }
    }
    out
}

fn graph_owner(engine: &Engine<FlecsWorld>, graph_id: &str) -> Option<EntityId> {
    let active_field = graph_field(graph_id, "active");
    let mut owners: Vec<_> = engine
        .entity_ids()
        .into_iter()
        .filter_map(|entity| {
            let fields = component_fields(engine, entity);
            fields
                .get(&active_field)
                .map(|active| (active != &FieldValue::Bool(true), entity))
        })
        .collect();
    owners.sort();
    owners.into_iter().next().map(|(_, entity)| entity)
}

fn component_fields(engine: &Engine<FlecsWorld>, entity: EntityId) -> BTreeMap<String, FieldValue> {
    engine
        .components_of(entity)
        .get(ANIMATION_GRAPH)
        .map(|fields| {
            fields
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn collect_records(fields: &BTreeMap<String, FieldValue>, graph_id: &str) -> StoredRecords {
    let mut records = StoredRecords::default();
    for (field, value) in fields {
        let parts: Vec<_> = field.split("::").collect();
        match parts.as_slice() {
            [GRAPH_PREFIX, graph, name] if *graph == graph_id => {
                records.graph.insert((*name).to_owned(), value.clone());
            }
            [NODE_PREFIX, graph, id, name] if *graph == graph_id => {
                records
                    .nodes
                    .entry((*id).to_owned())
                    .or_default()
                    .insert((*name).to_owned(), value.clone());
            }
            [EDGE_PREFIX, graph, id, name] if *graph == graph_id => {
                records
                    .edges
                    .entry((*id).to_owned())
                    .or_default()
                    .insert((*name).to_owned(), value.clone());
            }
            [PARAMETER_PREFIX, graph, id, name] if *graph == graph_id => {
                records
                    .parameters
                    .entry((*id).to_owned())
                    .or_default()
                    .insert((*name).to_owned(), value.clone());
            }
            [MACHINE_PREFIX, graph, id, name] if *graph == graph_id => {
                records
                    .machines
                    .entry((*id).to_owned())
                    .or_default()
                    .insert((*name).to_owned(), value.clone());
            }
            [STATE_PREFIX, graph, machine, id, name] if *graph == graph_id => {
                records
                    .states
                    .entry(((*machine).to_owned(), (*id).to_owned()))
                    .or_default()
                    .insert((*name).to_owned(), value.clone());
            }
            [TRANSITION_PREFIX, graph, machine, id, name] if *graph == graph_id => {
                records
                    .transitions
                    .entry(((*machine).to_owned(), (*id).to_owned()))
                    .or_default()
                    .insert((*name).to_owned(), value.clone());
            }
            _ => {}
        }
    }
    records
}

fn belongs_to_graph_record(field: &str, graph_id: &str) -> bool {
    field.split("::").nth(1) == Some(graph_id)
}

fn graph_field(graph_id: &str, name: &str) -> String {
    format!("{GRAPH_PREFIX}::{graph_id}::{name}")
}

fn element_field(prefix: &str, graph_id: &str, element_id: &str, name: &str) -> String {
    format!("{prefix}::{graph_id}::{element_id}::{name}")
}

fn nested_element_field(
    prefix: &str,
    graph_id: &str,
    parent_id: &str,
    element_id: &str,
    name: &str,
) -> String {
    format!("{prefix}::{graph_id}::{parent_id}::{element_id}::{name}")
}

fn set_graph_field(entity: EntityId, field: String, value: FieldValue) -> Op {
    Op::SetField {
        entity,
        component: ANIMATION_GRAPH.into(),
        field,
        value,
    }
}

fn insert_field(fields: &mut BTreeMap<String, FieldValue>, name: String, value: FieldValue) {
    fields.insert(name, value);
}

fn is_active(fields: &BTreeMap<String, FieldValue>) -> bool {
    fields.get("active") == Some(&FieldValue::Bool(true))
}

fn required_string(
    fields: &BTreeMap<String, FieldValue>,
    name: &str,
    record: &str,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) -> Option<String> {
    required_string_record(fields, name, record, record, diagnostics)
}

fn required_string_record(
    fields: &BTreeMap<String, FieldValue>,
    name: &str,
    record: &str,
    id: &str,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) -> Option<String> {
    match fields.get(name) {
        Some(FieldValue::Str(value)) => Some(value.clone()),
        _ => {
            diagnostics.push(storage_diagnostic(record, id, name));
            None
        }
    }
}

fn required_integer(
    fields: &BTreeMap<String, FieldValue>,
    name: &str,
    record: &str,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) -> Option<i64> {
    match fields.get(name) {
        Some(FieldValue::Integer(value)) => Some(*value),
        _ => {
            diagnostics.push(storage_diagnostic(record, record, name));
            None
        }
    }
}

fn required_number(
    fields: &BTreeMap<String, FieldValue>,
    name: &str,
    record: &str,
    id: &str,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) -> Option<f64> {
    match fields.get(name) {
        Some(FieldValue::Number(value)) => Some(*value),
        _ => {
            diagnostics.push(storage_diagnostic(record, id, name));
            None
        }
    }
}

fn required_payload<T: DeserializeOwned>(
    fields: &BTreeMap<String, FieldValue>,
    name: &str,
    record: &str,
    id: &str,
    diagnostics: &mut Vec<AnimationGraphDiagnostic>,
) -> Option<T> {
    match fields.get(name) {
        Some(FieldValue::Str(value)) => match serde_json::from_str(value) {
            Ok(value) => Some(value),
            Err(error) => {
                diagnostics.push(diagnostic(
                    AnimationGraphDiagnosticSeverity::Error,
                    "corrupt_graph_storage",
                    format!("Stored {record} {id} field {name} is invalid: {error}."),
                    Some("Restore the field from history or remove the damaged record.".into()),
                    (record == "node").then(|| id.to_owned()),
                    (record == "edge").then(|| id.to_owned()),
                    None,
                ));
                None
            }
        },
        _ => {
            diagnostics.push(storage_diagnostic(record, id, name));
            None
        }
    }
}

fn storage_diagnostic(record: &str, id: &str, field: &str) -> AnimationGraphDiagnostic {
    diagnostic(
        AnimationGraphDiagnosticSeverity::Error,
        "corrupt_graph_storage",
        format!("Stored {record} {id} is missing typed field {field}."),
        Some("Restore the field from history or remove the incomplete record.".into()),
        (record == "node").then(|| id.to_owned()),
        (record == "edge").then(|| id.to_owned()),
        None,
    )
}

fn encode<T: Serialize>(value: &T) -> Result<String, AnimationGraphIntentError> {
    serde_json::to_string(value).map_err(|error| {
        AnimationGraphIntentError::Admission(vec![diagnostic(
            AnimationGraphDiagnosticSeverity::Error,
            "serialization_failed",
            format!("Animation graph value could not be serialized: {error}."),
            Some("Repair non-finite or unsupported authored data.".into()),
            None,
            None,
            None,
        )])
    })
}

fn is_admission_code(code: &str) -> bool {
    matches!(
        code,
        "unsupported_schema"
            | "invalid_id"
            | "duplicate_id"
            | "admission_limit"
            | "non_finite_layout"
            | "non_finite_threshold"
            | "non_finite_sample"
            | "non_finite_parameter"
            | "non_finite_condition"
            | "duplicate_sample_edge"
            | "trigger_default_true"
            | "legacy_positional_contract_in_v2"
            | "unsupported_edge_weight_contract"
    )
}

fn node_diagnostic(
    node_id: &str,
    code: &str,
    message: impl Into<String>,
    fix: impl Into<String>,
) -> AnimationGraphDiagnostic {
    diagnostic(
        AnimationGraphDiagnosticSeverity::Error,
        code,
        message,
        Some(fix.into()),
        Some(node_id.to_owned()),
        None,
        None,
    )
}

fn edge_diagnostic(
    edge_id: &str,
    code: &str,
    message: impl Into<String>,
    fix: impl Into<String>,
) -> AnimationGraphDiagnostic {
    diagnostic(
        AnimationGraphDiagnosticSeverity::Error,
        code,
        message,
        Some(fix.into()),
        None,
        Some(edge_id.to_owned()),
        None,
    )
}

fn edge_port_diagnostic(
    edge_id: &str,
    node_id: &str,
    port_id: &str,
    code: &str,
    message: impl Into<String>,
    fix: impl Into<String>,
) -> AnimationGraphDiagnostic {
    diagnostic(
        AnimationGraphDiagnosticSeverity::Error,
        code,
        message,
        Some(fix.into()),
        Some(node_id.to_owned()),
        Some(edge_id.to_owned()),
        Some(port_id.to_owned()),
    )
}

fn diagnostic(
    severity: AnimationGraphDiagnosticSeverity,
    code: impl Into<String>,
    message: impl Into<String>,
    fix: Option<String>,
    node_id: Option<String>,
    edge_id: Option<String>,
    port_id: Option<String>,
) -> AnimationGraphDiagnostic {
    let code = code.into();
    let identity = format!(
        "{}\0{}\0{}\0{}",
        code,
        node_id.as_deref().unwrap_or_default(),
        edge_id.as_deref().unwrap_or_default(),
        port_id.as_deref().unwrap_or_default()
    );
    AnimationGraphDiagnostic {
        id: format!("graph-diagnostic-{:016x}", stable_hash(identity.as_bytes())),
        severity,
        code,
        message: message.into(),
        fix,
        node_id,
        edge_id,
        port_id,
    }
}

fn deduplicate_diagnostics(diagnostics: &mut Vec<AnimationGraphDiagnostic>) {
    diagnostics.sort_by(|left, right| {
        left.id
            .cmp(&right.id)
            .then(left.message.cmp(&right.message))
    });
    diagnostics.dedup_by(|left, right| left.id == right.id && left.message == right.message);
}

fn revision_hash_frame(hash: &mut u64, bytes: &[u8]) {
    for byte in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x100_0000_01b3);
    }
}

fn hash_field_value(hash: &mut u64, value: &FieldValue) {
    match value {
        FieldValue::Integer(value) => {
            revision_hash_frame(hash, b"integer");
            revision_hash_frame(hash, &value.to_le_bytes());
        }
        FieldValue::Number(value) => {
            revision_hash_frame(hash, b"number");
            revision_hash_frame(hash, &value.to_bits().to_le_bytes());
        }
        FieldValue::Bool(value) => {
            revision_hash_frame(hash, b"bool");
            revision_hash_frame(hash, &[u8::from(*value)]);
        }
        FieldValue::Str(value) => {
            revision_hash_frame(hash, b"string");
            revision_hash_frame(hash, value.as_bytes());
        }
    }
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_node(id: &str, x: f64) -> AnimationGraphNode {
        AnimationGraphNode {
            id: id.into(),
            kind: AnimationGraphNodeKind::Sequence,
            name: id.into(),
            position: AnimationGraphPosition { x, y: 10.0 },
            enabled: true,
            source_id: Some("main".into()),
            parameter_ids: Vec::new(),
            samples: Vec::new(),
            thresholds: Vec::new(),
            triangles: Vec::new(),
            mask_bindings: Vec::new(),
            state_machine_id: None,
        }
    }

    fn output_node() -> AnimationGraphNode {
        AnimationGraphNode {
            id: "output".into(),
            kind: AnimationGraphNodeKind::Output,
            name: "Output".into(),
            position: AnimationGraphPosition { x: 500.0, y: 10.0 },
            enabled: true,
            source_id: None,
            parameter_ids: Vec::new(),
            samples: Vec::new(),
            thresholds: Vec::new(),
            triangles: Vec::new(),
            mask_bindings: Vec::new(),
            state_machine_id: None,
        }
    }

    fn graph() -> AnimationGraphDocument {
        AnimationGraphDocument {
            schema_version: ANIMATION_GRAPH_SCHEMA_VERSION,
            id: "graph-main".into(),
            sequence_id: "main".into(),
            name: "Main graph".into(),
            output_node_id: "output".into(),
            // Storage reconstructs stable element arrays in lexical identity order.
            nodes: vec![output_node(), source_node("source", 10.0)],
            edges: vec![AnimationGraphEdge {
                id: "edge-source-output".into(),
                from_node_id: "source".into(),
                from_port_id: "pose".into(),
                to_node_id: "output".into(),
                to_port_id: "pose".into(),
                enabled: true,
                weight: None,
                weight_parameter_id: None,
            }],
            parameters: Vec::new(),
            state_machines: Vec::new(),
        }
    }

    fn locomotion_graph() -> AnimationGraphDocument {
        let mut walk = source_node("walk", 10.0);
        walk.source_id = Some("walk-sequence".into());
        let mut run = source_node("run", 30.0);
        run.source_id = Some("run-sequence".into());
        AnimationGraphDocument {
            schema_version: ANIMATION_GRAPH_SCHEMA_VERSION,
            id: "graph-locomotion".into(),
            sequence_id: "main".into(),
            name: "Locomotion".into(),
            output_node_id: "output".into(),
            nodes: vec![
                output_node(),
                walk,
                run,
                AnimationGraphNode {
                    id: "speed-blend".into(),
                    kind: AnimationGraphNodeKind::Blend1d,
                    name: "Speed blend".into(),
                    position: AnimationGraphPosition { x: 250.0, y: 10.0 },
                    enabled: true,
                    source_id: None,
                    parameter_ids: vec!["speed".into()],
                    samples: vec![
                        AnimationGraphBlendSample {
                            id: "sample-walk".into(),
                            edge_id: "edge-z-walk".into(),
                            position: [0.0, 0.0],
                        },
                        AnimationGraphBlendSample {
                            id: "sample-run".into(),
                            edge_id: "edge-a-run".into(),
                            position: [1.0, 0.0],
                        },
                    ],
                    thresholds: Vec::new(),
                    triangles: Vec::new(),
                    mask_bindings: Vec::new(),
                    state_machine_id: None,
                },
            ],
            edges: vec![
                AnimationGraphEdge {
                    id: "edge-a-run".into(),
                    from_node_id: "run".into(),
                    from_port_id: "pose".into(),
                    to_node_id: "speed-blend".into(),
                    to_port_id: "poses".into(),
                    enabled: true,
                    weight: None,
                    weight_parameter_id: None,
                },
                AnimationGraphEdge {
                    id: "edge-z-walk".into(),
                    from_node_id: "walk".into(),
                    from_port_id: "pose".into(),
                    to_node_id: "speed-blend".into(),
                    to_port_id: "poses".into(),
                    enabled: true,
                    weight: None,
                    weight_parameter_id: None,
                },
                AnimationGraphEdge {
                    id: "edge-output".into(),
                    from_node_id: "speed-blend".into(),
                    from_port_id: "pose".into(),
                    to_node_id: "output".into(),
                    to_port_id: "pose".into(),
                    enabled: true,
                    weight: None,
                    weight_parameter_id: None,
                },
            ],
            parameters: vec![AnimationGraphParameter {
                id: "speed".into(),
                name: "Speed".into(),
                kind: AnimationGraphParameterKind::Float,
                default_value: AnimationGraphValue::Number(0.0),
                min: Some(0.0),
                max: Some(1.0),
            }],
            state_machines: Vec::new(),
        }
    }

    fn authored_sequence(engine: &mut Engine<FlecsWorld>) -> animation::Sequence {
        let target = engine.alloc_entity_id();
        engine
            .commit(
                "animated target",
                vec![
                    Op::CreateEntity {
                        id: target,
                        parent: None,
                    },
                    Op::SetField {
                        entity: target,
                        component: "Transform".into(),
                        field: "x".into(),
                        value: FieldValue::Number(3.0),
                    },
                ],
            )
            .expect("target");
        crate::animation_intent::key_property(
            engine,
            target,
            "Transform",
            "x",
            animation::Tick(0),
            animation::Interpolation::Linear,
        )
        .expect("key property");
        crate::animation_intent::load_animation_document(engine).sequence
    }

    #[test]
    fn graph_round_trips_through_granular_fields_and_one_undo() {
        let mut engine = Engine::new(FlecsWorld::new(), 81);
        let empty_revision = authored_animation_graph_revision(&engine, "main");
        let saved =
            save_animation_graph(&mut engine, &empty_revision, &graph()).expect("save graph");
        assert_eq!(saved.document.as_ref(), Some(&graph()));
        let owner = EntityId::from_loro_key(saved.controller_id.as_deref().expect("controller"))
            .expect("controller identity");
        let fields = component_fields(&engine, owner);
        assert!(fields.contains_key("node::graph-main::source::payload"));
        assert!(fields.contains_key("node::graph-main::source::x"));
        assert!(fields.contains_key("edge::graph-main::edge-source-output::payload"));
        assert!(engine.undo(), "one graph save is one undo step");
        assert!(load_animation_graph(&engine, "main").document.is_none());
    }

    #[test]
    fn layout_has_its_own_field_and_tombstones_removed_elements() {
        let mut engine = Engine::new(FlecsWorld::new(), 82);
        let revision = authored_animation_graph_revision(&engine, "main");
        let first = save_animation_graph(&mut engine, &revision, &graph()).expect("initial save");
        let mut edited = first.document.expect("document");
        edited
            .nodes
            .iter_mut()
            .find(|node| node.id == "source")
            .expect("source node")
            .position
            .x = 42.0;
        edited.edges.clear();
        let saved =
            save_animation_graph(&mut engine, &first.revision, &edited).expect("edit graph");
        let owner = EntityId::from_loro_key(saved.controller_id.as_deref().expect("controller"))
            .expect("controller identity");
        let fields = component_fields(&engine, owner);
        assert_eq!(
            fields.get("node::graph-main::source::x"),
            Some(&FieldValue::Number(42.0))
        );
        assert_eq!(
            fields.get("edge::graph-main::edge-source-output::active"),
            Some(&FieldValue::Bool(false))
        );
    }

    #[test]
    fn stale_save_is_rejected_without_mutation() {
        let mut engine = Engine::new(FlecsWorld::new(), 83);
        let initial = authored_animation_graph_revision(&engine, "main");
        let saved = save_animation_graph(&mut engine, &initial, &graph()).expect("save");
        let before = authored_animation_graph_revision(&engine, "main");
        let error = save_animation_graph(&mut engine, &initial, &graph()).expect_err("stale");
        assert!(matches!(
            error,
            AnimationGraphIntentError::StaleRevision { .. }
        ));
        assert_eq!(authored_animation_graph_revision(&engine, "main"), before);
        assert_eq!(saved.revision, before);
    }

    #[test]
    fn concurrent_graph_fields_merge_without_erasing_sibling_edits() {
        let mut seed = Engine::new(FlecsWorld::new(), 830);
        let initial = authored_animation_graph_revision(&seed, "main");
        save_animation_graph(&mut seed, &initial, &graph()).expect("seed graph");

        let shared = seed.fork_doc();
        let mut layout_peer = Engine::new(FlecsWorld::new(), 831);
        layout_peer.merge(&shared).expect("seed layout peer");
        let mut naming_peer = Engine::new(FlecsWorld::new(), 832);
        naming_peer.merge(&shared).expect("seed naming peer");

        let mut layout_edit = load_animation_graph(&layout_peer, "main")
            .document
            .expect("layout graph");
        layout_edit
            .nodes
            .iter_mut()
            .find(|node| node.id == "source")
            .expect("source node")
            .position
            .x = 246.0;
        let layout_revision = authored_animation_graph_revision(&layout_peer, "main");
        save_animation_graph(&mut layout_peer, &layout_revision, &layout_edit)
            .expect("save layout field");

        let mut naming_edit = load_animation_graph(&naming_peer, "main")
            .document
            .expect("naming graph");
        naming_edit
            .nodes
            .iter_mut()
            .find(|node| node.id == "output")
            .expect("output node")
            .name = "Gameplay output".into();
        let naming_revision = authored_animation_graph_revision(&naming_peer, "main");
        save_animation_graph(&mut naming_peer, &naming_revision, &naming_edit)
            .expect("save node payload");

        let layout_updates = layout_peer.export_updates();
        let naming_updates = naming_peer.export_updates();
        layout_peer
            .merge(&naming_updates)
            .expect("merge naming change");
        naming_peer
            .merge(&layout_updates)
            .expect("merge layout change");

        for peer in [&layout_peer, &naming_peer] {
            let merged = load_animation_graph(peer, "main")
                .document
                .expect("merged graph");
            assert_eq!(
                merged
                    .nodes
                    .iter()
                    .find(|node| node.id == "source")
                    .expect("merged source")
                    .position
                    .x,
                246.0
            );
            assert_eq!(
                merged
                    .nodes
                    .iter()
                    .find(|node| node.id == "output")
                    .expect("merged output")
                    .name,
                "Gameplay output"
            );
        }
        assert_eq!(layout_peer.canonical_state(), naming_peer.canonical_state());
    }

    #[test]
    fn invalid_editable_draft_persists_but_non_finite_storage_does_not() {
        let mut engine = Engine::new(FlecsWorld::new(), 84);
        let mut incomplete = graph();
        incomplete.edges.clear();
        let revision = authored_animation_graph_revision(&engine, "main");
        let saved = save_animation_graph(&mut engine, &revision, &incomplete)
            .expect("editable invalid draft persists");
        assert!(saved
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "output_input_count"));
        let mut corrupt = incomplete;
        corrupt.nodes[0].position.x = f64::NAN;
        let error = save_animation_graph(&mut engine, &saved.revision, &corrupt)
            .expect_err("non-finite layout is an admission failure");
        assert!(matches!(error, AnimationGraphIntentError::Admission(_)));
    }

    #[test]
    fn pose_cycles_and_dangling_edges_are_navigable_diagnostics() {
        let mut draft = graph();
        draft.nodes.insert(
            1,
            AnimationGraphNode {
                id: "blend".into(),
                kind: AnimationGraphNodeKind::BlendNormalized,
                name: "Blend".into(),
                position: AnimationGraphPosition { x: 250.0, y: 10.0 },
                enabled: true,
                source_id: None,
                parameter_ids: Vec::new(),
                samples: Vec::new(),
                thresholds: Vec::new(),
                triangles: Vec::new(),
                mask_bindings: Vec::new(),
                state_machine_id: None,
            },
        );
        draft.edges = vec![
            AnimationGraphEdge {
                id: "cycle-a".into(),
                from_node_id: "source".into(),
                from_port_id: "pose".into(),
                to_node_id: "blend".into(),
                to_port_id: "poses".into(),
                enabled: true,
                weight: None,
                weight_parameter_id: None,
            },
            AnimationGraphEdge {
                id: "cycle-b".into(),
                from_node_id: "blend".into(),
                from_port_id: "pose".into(),
                to_node_id: "source".into(),
                to_port_id: "poses".into(),
                enabled: true,
                weight: None,
                weight_parameter_id: None,
            },
            AnimationGraphEdge {
                id: "dangling".into(),
                from_node_id: "missing".into(),
                from_port_id: "pose".into(),
                to_node_id: "output".into(),
                to_port_id: "pose".into(),
                enabled: true,
                weight: None,
                weight_parameter_id: None,
            },
        ];
        let diagnostics = validate_animation_graph_document(&draft);
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "pose_cycle"));
        let dangling = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "dangling_edge")
            .expect("dangling diagnostic");
        assert_eq!(dangling.edge_id.as_deref(), Some("dangling"));
    }

    #[test]
    fn state_machine_facade_edges_cannot_hide_runtime_state_roots() {
        let mut draft = graph();
        draft.nodes.insert(
            1,
            AnimationGraphNode {
                id: "machine-node".into(),
                kind: AnimationGraphNodeKind::StateMachine,
                name: "Machine".into(),
                position: AnimationGraphPosition { x: 250.0, y: 10.0 },
                enabled: true,
                source_id: None,
                parameter_ids: Vec::new(),
                samples: Vec::new(),
                thresholds: Vec::new(),
                triangles: Vec::new(),
                mask_bindings: Vec::new(),
                state_machine_id: Some("machine".into()),
            },
        );
        draft.edges = vec![AnimationGraphEdge {
            id: "machine-output".into(),
            from_node_id: "machine-node".into(),
            from_port_id: "pose".into(),
            to_node_id: "output".into(),
            to_port_id: "pose".into(),
            enabled: true,
            weight: None,
            weight_parameter_id: None,
        }];
        draft.state_machines.push(AnimationGraphStateMachine {
            id: "machine".into(),
            name: "Machine".into(),
            entry_state_id: "idle".into(),
            states: vec![AnimationGraphState {
                id: "idle".into(),
                name: "Idle".into(),
                pose_node_id: "source".into(),
                reset_on_entry: true,
            }],
            transitions: Vec::new(),
        });
        let diagnostics = validate_animation_graph_document(&draft);
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "missing_state_pose_edge"));
        for id in ["state-source-a", "state-source-b"] {
            draft.edges.push(AnimationGraphEdge {
                id: id.into(),
                from_node_id: "source".into(),
                from_port_id: "pose".into(),
                to_node_id: "machine-node".into(),
                to_port_id: "states".into(),
                enabled: true,
                weight: None,
                weight_parameter_id: None,
            });
        }
        let duplicate = validate_animation_graph_document(&draft);
        assert!(duplicate
            .iter()
            .any(|diagnostic| diagnostic.code == "duplicate_state_pose_edge"));
    }

    #[test]
    fn delete_tombstones_graph_and_undo_restores_it() {
        let mut engine = Engine::new(FlecsWorld::new(), 85);
        let revision = authored_animation_graph_revision(&engine, "main");
        let saved = save_animation_graph(&mut engine, &revision, &graph()).expect("save");
        let deleted = delete_animation_graph(&mut engine, "main", "graph-main", &saved.revision)
            .expect("delete");
        assert!(deleted.document.is_none());
        assert!(engine.undo(), "delete is one undo step");
        assert_eq!(
            load_animation_graph(&engine, "main").document.as_ref(),
            Some(&graph())
        );
    }

    #[test]
    fn adapter_builds_an_explicit_reference_pose_and_core_plan() {
        let mut engine = Engine::new(FlecsWorld::new(), 86);
        let sequence = authored_sequence(&mut engine);
        let mut sequences = BTreeMap::new();
        sequences.insert(sequence.id.clone(), sequence.clone());
        let adapted =
            adapt_animation_graph_document(&engine, &graph(), &sequences).expect("adapt graph");
        assert_eq!(adapted.graph.reference_pose.len(), 1);
        assert!(adapted.graph.reference_pose[0]
            .binding
            .path
            .subpath
            .is_empty());
        let mut compiled_sequences = BTreeMap::new();
        compiled_sequences.insert(
            sequence.id.clone(),
            sequence.compile().expect("compile sequence"),
        );
        let compiled =
            animation::CompiledAnimationGraph::compile(&adapted.graph, &compiled_sequences)
                .expect("compile graph");
        assert!(!compiled.stable_hash.is_empty());
        let instance = animation::AnimationGraphRuntimeInstance::new(compiled);
        assert_eq!(instance.frame().values.len(), 1);
    }

    #[test]
    fn keyed_locomotion_sample_survives_persistence_adapter_and_runtime_edge_sorting() {
        let mut engine = Engine::new(FlecsWorld::new(), 861);
        let template = authored_sequence(&mut engine);
        let mut walk = template.clone();
        walk.id = animation::SequenceId::new("walk-sequence");
        walk.name = "Walk".into();
        for key in walk
            .tracks
            .iter_mut()
            .flat_map(|track| &mut track.keyframes)
        {
            key.value = animation::AnimValue::Number(10.0);
        }
        let mut run = template;
        run.id = animation::SequenceId::new("run-sequence");
        run.name = "Run".into();
        for key in run.tracks.iter_mut().flat_map(|track| &mut track.keyframes) {
            key.value = animation::AnimValue::Number(20.0);
        }
        let revision = authored_animation_graph_revision(&engine, "main");
        let saved = save_animation_graph(&mut engine, &revision, &locomotion_graph())
            .expect("persist keyed locomotion graph");
        let persisted = saved.document.expect("persisted graph");
        let blend = persisted
            .nodes
            .iter()
            .find(|node| node.id == "speed-blend")
            .expect("speed blend");
        assert_eq!(blend.thresholds, Vec::<f64>::new());
        assert!(blend.samples.iter().any(|sample| {
            sample.id == "sample-walk"
                && sample.edge_id == "edge-z-walk"
                && sample.position == [0.0, 0.0]
        }));

        let sequences = BTreeMap::from([
            (walk.id.clone(), walk.clone()),
            (run.id.clone(), run.clone()),
        ]);
        let adapted = adapt_animation_graph_document(&engine, &persisted, &sequences)
            .expect("adapt keyed graph");
        let adapted_blend = adapted
            .graph
            .nodes
            .iter()
            .find(|node| node.id.as_str() == "speed-blend")
            .expect("adapted speed blend");
        match &adapted_blend.kind {
            animation::GraphNodeKind::Blend1d { points, .. } => {
                assert!(points.iter().any(|point| {
                    point.id.as_str() == "sample-walk"
                        && point.node.as_str() == "walk"
                        && point.position == 0.0
                }));
            }
            other => panic!("unexpected adapted node {other:?}"),
        }
        let compiled_sequences = BTreeMap::from([
            (walk.id.clone(), walk.compile().expect("compile walk")),
            (run.id.clone(), run.compile().expect("compile run")),
        ]);
        let compiled =
            animation::CompiledAnimationGraph::compile(&adapted.graph, &compiled_sequences)
                .expect("compile keyed graph");
        let runtime = animation::AnimationGraphRuntimeInstance::new(compiled);
        assert!(matches!(
            runtime.frame().values.first().map(|value| &value.value),
            Some(animation::AnimValue::Number(value)) if *value == 10.0
        ));
    }

    #[test]
    fn legacy_positional_samples_are_canonicalized_with_one_time_review_warning() {
        let mut engine = Engine::new(FlecsWorld::new(), 862);
        let mut legacy = locomotion_graph();
        legacy.schema_version = 1;
        let blend = legacy
            .nodes
            .iter_mut()
            .find(|node| node.id == "speed-blend")
            .expect("blend");
        blend.samples.clear();
        blend.thresholds = vec![0.0, 1.0];
        let revision = authored_animation_graph_revision(&engine, "main");
        let saved = save_animation_graph(&mut engine, &revision, &legacy).expect("migrate save");
        assert_eq!(
            saved
                .document
                .as_ref()
                .map(|document| document.schema_version),
            Some(ANIMATION_GRAPH_SCHEMA_VERSION)
        );
        assert!(saved
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "legacy_graph_schema_migrated"));
        assert!(saved.diagnostics.iter().any(|diagnostic| diagnostic.code
            == "legacy_positional_sample_migrated"
            && diagnostic.message.contains("cannot be proven")));
        let blend = saved
            .document
            .as_ref()
            .and_then(|document| document.nodes.iter().find(|node| node.id == "speed-blend"))
            .expect("canonical blend");
        assert!(blend.thresholds.is_empty());
        assert_eq!(
            blend
                .samples
                .iter()
                .map(|sample| sample.edge_id.as_str())
                .collect::<Vec<_>>(),
            ["edge-a-run", "edge-z-walk"]
        );
        assert!(!load_animation_graph(&engine, "main")
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "legacy_positional_sample_migrated"));
    }

    #[test]
    fn load_upgrades_persisted_v1_and_newer_peers_fail_closed() {
        let mut engine = Engine::new(FlecsWorld::new(), 8621);
        let revision = authored_animation_graph_revision(&engine, "main");
        let saved = save_animation_graph(&mut engine, &revision, &graph()).expect("seed graph");
        let owner = EntityId::from_loro_key(saved.controller_id.as_deref().expect("controller"))
            .expect("controller identity");
        engine
            .commit(
                "simulate-v1-storage",
                vec![Op::SetField {
                    entity: owner,
                    component: ANIMATION_GRAPH.into(),
                    field: graph_field("graph-main", "schema"),
                    value: FieldValue::Integer(1),
                }],
            )
            .expect("write v1 schema");
        let migrated = load_animation_graph(&engine, "main");
        assert_eq!(
            migrated
                .document
                .as_ref()
                .map(|document| document.schema_version),
            Some(ANIMATION_GRAPH_SCHEMA_VERSION)
        );
        assert!(migrated
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "legacy_graph_schema_migrated"));

        engine
            .commit(
                "simulate-newer-storage",
                vec![Op::SetField {
                    entity: owner,
                    component: ANIMATION_GRAPH.into(),
                    field: graph_field("graph-main", "schema"),
                    value: FieldValue::Integer(i64::from(ANIMATION_GRAPH_SCHEMA_VERSION) + 1),
                }],
            )
            .expect("write newer schema");
        let newer = load_animation_graph(&engine, "main");
        assert!(newer
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "unsupported_schema"));
    }

    #[test]
    fn admission_rejects_duplicate_samples_and_true_trigger_defaults() {
        let mut draft = locomotion_graph();
        let blend = draft
            .nodes
            .iter_mut()
            .find(|node| node.id == "speed-blend")
            .expect("blend");
        blend.samples[1].id = blend.samples[0].id.clone();
        blend.samples[1].edge_id = blend.samples[0].edge_id.clone();
        draft.parameters.push(AnimationGraphParameter {
            id: "jump".into(),
            name: "Jump".into(),
            kind: AnimationGraphParameterKind::Trigger,
            default_value: AnimationGraphValue::Boolean(true),
            min: None,
            max: None,
        });
        let diagnostics = validate_animation_graph_document(&draft);
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "duplicate_id"));
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "duplicate_sample_edge"));
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "trigger_default_true"));
        let mut engine = Engine::new(FlecsWorld::new(), 863);
        let revision = authored_animation_graph_revision(&engine, "main");
        assert!(matches!(
            save_animation_graph(&mut engine, &revision, &draft),
            Err(AnimationGraphIntentError::Admission(_))
        ));
    }

    #[test]
    fn edge_weight_contracts_reject_ignored_fields_and_layer_double_authoring() {
        let mut unsupported = graph();
        unsupported.edges[0].weight = Some(0.5);
        let diagnostics = validate_animation_graph_document(&unsupported);
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "unsupported_edge_weight_contract"
                && diagnostic.edge_id.as_deref() == Some("edge-source-output")
        }));
        let mut engine = Engine::new(FlecsWorld::new(), 8631);
        let revision = authored_animation_graph_revision(&engine, "main");
        assert!(matches!(
            save_animation_graph(&mut engine, &revision, &unsupported),
            Err(AnimationGraphIntentError::Admission(_))
        ));

        let mut normalized = graph();
        normalized.nodes.insert(
            1,
            AnimationGraphNode {
                id: "weighted-blend".into(),
                kind: AnimationGraphNodeKind::BlendNormalized,
                name: "Weighted blend".into(),
                position: AnimationGraphPosition { x: 250.0, y: 10.0 },
                enabled: true,
                source_id: None,
                parameter_ids: Vec::new(),
                samples: Vec::new(),
                thresholds: Vec::new(),
                triangles: Vec::new(),
                mask_bindings: Vec::new(),
                state_machine_id: None,
            },
        );
        normalized.edges = vec![
            AnimationGraphEdge {
                id: "weighted-input".into(),
                from_node_id: "source".into(),
                from_port_id: "pose".into(),
                to_node_id: "weighted-blend".into(),
                to_port_id: "poses".into(),
                enabled: true,
                weight: Some(0.5),
                weight_parameter_id: None,
            },
            AnimationGraphEdge {
                id: "weighted-output".into(),
                from_node_id: "weighted-blend".into(),
                from_port_id: "pose".into(),
                to_node_id: "output".into(),
                to_port_id: "pose".into(),
                enabled: true,
                weight: None,
                weight_parameter_id: None,
            },
        ];
        assert!(!validate_animation_graph_document(&normalized)
            .iter()
            .any(|diagnostic| diagnostic.code == "unsupported_edge_weight_contract"));

        let layer = normalized
            .nodes
            .iter_mut()
            .find(|node| node.id == "weighted-blend")
            .expect("layer node");
        layer.kind = AnimationGraphNodeKind::LayerAdditive;
        layer.parameter_ids = vec!["layer-weight".into()];
        layer.mask_bindings = vec!["**/Transform/rotation".into()];
        normalized.parameters.push(AnimationGraphParameter {
            id: "layer-weight".into(),
            name: "Layer weight".into(),
            kind: AnimationGraphParameterKind::Float,
            default_value: AnimationGraphValue::Number(0.5),
            min: Some(0.0),
            max: Some(1.0),
        });
        normalized.edges[0].to_port_id = "layer".into();
        assert!(validate_animation_graph_document(&normalized)
            .iter()
            .any(|diagnostic| diagnostic.code == "unsupported_edge_weight_contract"));
    }

    #[test]
    fn transition_preflight_matches_core_automatic_cycle_policy() {
        let state = |id: &str| AnimationGraphState {
            id: id.into(),
            name: id.into(),
            pose_node_id: "source".into(),
            reset_on_entry: false,
        };
        let transition = |id: &str, from: &str, to: &str, duration_tick| AnimationGraphTransition {
            id: id.into(),
            from_state_id: from.into(),
            to_state_id: to.into(),
            priority: 0,
            duration_tick,
            curve: AnimationGraphCurve::Linear,
            interruption: AnimationGraphInterruption::None,
            conditions: Vec::new(),
            exit_time: None,
        };
        let mut machine = AnimationGraphStateMachine {
            id: "machine".into(),
            name: "Machine".into(),
            entry_state_id: "a".into(),
            states: vec![state("a"), state("b")],
            transitions: vec![transition("a-b", "a", "b", 0)],
        };
        assert!(unconditional_zero_duration_transition_cycle(&machine).is_none());
        machine.transitions.push(transition("b-a", "b", "a", 1));
        assert!(unconditional_zero_duration_transition_cycle(&machine).is_none());
        machine.transitions[1].duration_tick = 0;
        assert_eq!(
            unconditional_zero_duration_transition_cycle(&machine),
            Some(vec!["a-b".into(), "b-a".into()])
        );
    }

    #[test]
    fn vec2_parameter_expands_to_stable_scalar_runtime_routes() {
        let mut engine = Engine::new(FlecsWorld::new(), 87);
        let sequence = authored_sequence(&mut engine);
        let mut sequences = BTreeMap::new();
        sequences.insert(sequence.id.clone(), sequence);
        let mut draft = graph();
        draft.parameters.push(AnimationGraphParameter {
            id: "movement".into(),
            name: "Movement".into(),
            kind: AnimationGraphParameterKind::Vec2,
            default_value: AnimationGraphValue::Vec2([0.25, -0.5]),
            min: None,
            max: None,
        });
        for (index, id) in ["a", "b", "c"].into_iter().enumerate() {
            draft
                .nodes
                .push(source_node(id, 20.0 + index as f64 * 40.0));
        }
        draft.nodes.push(AnimationGraphNode {
            id: "blend2d".into(),
            kind: AnimationGraphNodeKind::Blend2dCartesian,
            name: "Movement blend".into(),
            position: AnimationGraphPosition { x: 300.0, y: 10.0 },
            enabled: true,
            source_id: None,
            parameter_ids: vec!["movement".into()],
            samples: vec![
                AnimationGraphBlendSample {
                    id: "a".into(),
                    edge_id: "edge-a".into(),
                    position: [0.0, 0.0],
                },
                AnimationGraphBlendSample {
                    id: "b".into(),
                    edge_id: "edge-b".into(),
                    position: [1.0, 0.0],
                },
                AnimationGraphBlendSample {
                    id: "c".into(),
                    edge_id: "edge-c".into(),
                    position: [0.0, 1.0],
                },
            ],
            thresholds: Vec::new(),
            triangles: vec![["a".into(), "b".into(), "c".into()]],
            mask_bindings: Vec::new(),
            state_machine_id: None,
        });
        draft.edges = vec![
            AnimationGraphEdge {
                id: "edge-a".into(),
                from_node_id: "a".into(),
                from_port_id: "pose".into(),
                to_node_id: "blend2d".into(),
                to_port_id: "poses".into(),
                enabled: true,
                weight: None,
                weight_parameter_id: None,
            },
            AnimationGraphEdge {
                id: "edge-b".into(),
                from_node_id: "b".into(),
                from_port_id: "pose".into(),
                to_node_id: "blend2d".into(),
                to_port_id: "poses".into(),
                enabled: true,
                weight: None,
                weight_parameter_id: None,
            },
            AnimationGraphEdge {
                id: "edge-c".into(),
                from_node_id: "c".into(),
                from_port_id: "pose".into(),
                to_node_id: "blend2d".into(),
                to_port_id: "poses".into(),
                enabled: true,
                weight: None,
                weight_parameter_id: None,
            },
            AnimationGraphEdge {
                id: "edge-output".into(),
                from_node_id: "blend2d".into(),
                from_port_id: "pose".into(),
                to_node_id: "output".into(),
                to_port_id: "pose".into(),
                enabled: true,
                weight: None,
                weight_parameter_id: None,
            },
        ];
        let adapted =
            adapt_animation_graph_document(&engine, &draft, &sequences).expect("adapt Vec2 graph");
        let route = adapted
            .parameter_routes
            .iter()
            .find(|route| route.editor_parameter_id == "movement")
            .expect("route");
        assert_eq!(
            route.runtime_parameter_ids,
            ["movement.__x", "movement.__y"]
        );
    }
}
