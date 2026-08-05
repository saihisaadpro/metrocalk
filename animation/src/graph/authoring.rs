use serde::{Deserialize, Serialize};

use crate::{AnimValue, Binding, PropertyPath, SequenceId, Tick, ValueKind};

use super::ANIMATION_GRAPH_SCHEMA_VERSION;

/// Keeps every difference, squared distance and 2D determinant used by Cartesian blending inside
/// finite `f64` range while remaining far beyond any practical animation-space coordinate.
pub(crate) const MAX_BLEND_2D_COORDINATE_ABS: f64 = 1.0e150;

macro_rules! graph_id {
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

graph_id!(AnimationGraphId);
graph_id!(GraphNodeId);
graph_id!(GraphParameterId);
graph_id!(GraphMaskId);
graph_id!(GraphBlendPointId);
graph_id!(GraphStateId);
graph_id!(GraphTransitionId);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphParameterKind {
    Number,
    Integer,
    Boolean,
    Trigger,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphParameterValue {
    Number(f64),
    Integer(i64),
    Boolean(bool),
    Trigger(bool),
}

impl GraphParameterValue {
    #[must_use]
    pub const fn kind(&self) -> GraphParameterKind {
        match self {
            Self::Number(_) => GraphParameterKind::Number,
            Self::Integer(_) => GraphParameterKind::Integer,
            Self::Boolean(_) => GraphParameterKind::Boolean,
            Self::Trigger(_) => GraphParameterKind::Trigger,
        }
    }

    #[must_use]
    pub const fn is_finite(&self) -> bool {
        !matches!(self, Self::Number(value) if !value.is_finite())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphParameter {
    pub id: GraphParameterId,
    pub name: String,
    pub kind: GraphParameterKind,
    pub default: GraphParameterValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphReferenceValue {
    pub binding: Binding,
    pub value: AnimValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphMaskEntry {
    pub path: PropertyPath,
    /// Multiplicative influence in `[0, 1]`.
    pub weight: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphPropertyMask {
    pub id: GraphMaskId,
    pub name: String,
    /// Influence for properties without an explicit entry.
    pub default_weight: f64,
    #[serde(default)]
    pub entries: Vec<GraphMaskEntry>,
}

/// Exact clip-rate ratio. No wall-clock conversion occurs in the graph hot path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RationalRate {
    pub numerator: i32,
    pub denominator: u32,
}

impl RationalRate {
    pub const ONE: Self = Self {
        numerator: 1,
        denominator: 1,
    };

    #[must_use]
    pub const fn new(numerator: i32, denominator: u32) -> Self {
        Self {
            numerator,
            denominator,
        }
    }
}

impl Default for RationalRate {
    fn default() -> Self {
        Self::ONE
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphWeight {
    Constant(f64),
    Parameter(GraphParameterId),
}

impl Default for GraphWeight {
    fn default() -> Self {
        Self::Constant(1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphBlendMode {
    /// Positive inputs are divided by their sum. A zero sum resolves to the reference pose.
    Normalized,
    /// Weights are used directly and any residual below one belongs to the reference pose.
    Direct,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphBlendInput {
    pub node: GraphNodeId,
    pub weight: GraphWeight,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphBlend1dPoint {
    pub id: GraphBlendPointId,
    pub position: f64,
    pub node: GraphNodeId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphBlend2dPoint {
    pub id: GraphBlendPointId,
    pub position: [f64; 2],
    pub node: GraphNodeId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphBlend2dTriangle {
    /// Counter-clockwise or clockwise is accepted; winding does not affect evaluation.
    pub points: [GraphBlendPointId; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphOutsideHullMode {
    /// Project to the closest point on the authored triangle mesh.
    ProjectToHull,
    /// Use the closest authored sample, with point ID as the exact-distance tie-break.
    NearestPoint,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GraphNodeKind {
    /// Emits the graph's complete explicit reference pose.
    ReferencePose,
    Clip {
        sequence: SequenceId,
        #[serde(default)]
        rate: RationalRate,
        #[serde(default)]
        start_tick: Tick,
    },
    Blend {
        mode: GraphBlendMode,
        inputs: Vec<GraphBlendInput>,
    },
    Blend1d {
        parameter: GraphParameterId,
        points: Vec<GraphBlend1dPoint>,
    },
    Blend2d {
        parameter_x: GraphParameterId,
        parameter_y: GraphParameterId,
        points: Vec<GraphBlend2dPoint>,
        triangles: Vec<GraphBlend2dTriangle>,
        outside_hull: GraphOutsideHullMode,
    },
    OverrideLayer {
        base: GraphNodeId,
        layer: GraphNodeId,
        #[serde(default)]
        weight: GraphWeight,
        #[serde(default)]
        mask: Option<GraphMaskId>,
    },
    AdditiveLayer {
        base: GraphNodeId,
        layer: GraphNodeId,
        #[serde(default)]
        weight: GraphWeight,
        #[serde(default)]
        mask: Option<GraphMaskId>,
    },
}

impl GraphNodeKind {
    pub(crate) fn dependencies(&self) -> Vec<&GraphNodeId> {
        match self {
            Self::ReferencePose | Self::Clip { .. } => Vec::new(),
            Self::Blend { inputs, .. } => inputs.iter().map(|input| &input.node).collect(),
            Self::Blend1d { points, .. } => points.iter().map(|point| &point.node).collect(),
            Self::Blend2d { points, .. } => points.iter().map(|point| &point.node).collect(),
            Self::OverrideLayer { base, layer, .. } | Self::AdditiveLayer { base, layer, .. } => {
                vec![base, layer]
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: GraphNodeId,
    pub name: String,
    pub kind: GraphNodeKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "test", content = "value", rename_all = "snake_case")]
pub enum GraphConditionTest {
    BooleanEquals(bool),
    NumberEquals(f64),
    NumberNotEquals(f64),
    NumberGreaterThan(f64),
    NumberGreaterOrEqual(f64),
    NumberLessThan(f64),
    NumberLessOrEqual(f64),
    IntegerEquals(i64),
    IntegerNotEquals(i64),
    IntegerGreaterThan(i64),
    IntegerGreaterOrEqual(i64),
    IntegerLessThan(i64),
    IntegerLessOrEqual(i64),
    Triggered,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphCondition {
    pub parameter: GraphParameterId,
    pub test: GraphConditionTest,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphTransitionMode {
    #[default]
    Crossfade,
    /// Reserved and rejected until marker phase matching is implemented.
    MarkerSynchronized,
    /// Reserved and rejected until deterministic inertial decay is implemented.
    Inertialized,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphTransitionCurve {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    Smoothstep,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphInterruptionPolicy {
    #[default]
    None,
    Source,
    Destination,
    Both,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphState {
    pub id: GraphStateId,
    pub name: String,
    pub node: GraphNodeId,
    /// Reset this state's local clip clock whenever a transition enters it.
    #[serde(default = "default_true")]
    pub reset_on_entry: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphTransition {
    pub id: GraphTransitionId,
    pub from: GraphStateId,
    pub to: GraphStateId,
    /// Higher priority wins; transition ID is the deterministic tie-break.
    pub priority: i32,
    pub duration: Tick,
    #[serde(default)]
    pub curve: GraphTransitionCurve,
    #[serde(default)]
    pub interruption: GraphInterruptionPolicy,
    #[serde(default)]
    pub mode: GraphTransitionMode,
    #[serde(default)]
    pub conditions: Vec<GraphCondition>,
}

const fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphStateMachine {
    pub initial_state: GraphStateId,
    pub states: Vec<GraphState>,
    #[serde(default)]
    pub transitions: Vec<GraphTransition>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphEventPolicy {
    All,
    #[default]
    Dominant,
    Threshold {
        minimum_weight: f64,
    },
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimationGraphLimits {
    pub max_nodes: usize,
    #[serde(default = "default_max_pose_edges")]
    pub max_pose_edges: usize,
    pub max_parameters: usize,
    pub max_states: usize,
    pub max_transitions: usize,
    pub max_transitions_per_update: usize,
    pub max_event_candidates_per_update: usize,
}

impl Default for AnimationGraphLimits {
    fn default() -> Self {
        Self {
            max_nodes: 512,
            max_pose_edges: default_max_pose_edges(),
            max_parameters: 128,
            max_states: 128,
            max_transitions: 1_024,
            max_transitions_per_update: 8,
            max_event_candidates_per_update: 4_096,
        }
    }
}

const fn default_max_pose_edges() -> usize {
    2_048
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnimationGraph {
    pub schema_version: u32,
    pub id: AnimationGraphId,
    pub name: String,
    #[serde(default)]
    pub parameters: Vec<GraphParameter>,
    /// Complete baseline for every property the graph can emit.
    #[serde(default)]
    pub reference_pose: Vec<GraphReferenceValue>,
    #[serde(default)]
    pub masks: Vec<GraphPropertyMask>,
    #[serde(default)]
    pub nodes: Vec<GraphNode>,
    pub output: GraphNodeId,
    #[serde(default)]
    pub state_machine: Option<GraphStateMachine>,
    #[serde(default)]
    pub event_policy: GraphEventPolicy,
    #[serde(default)]
    pub limits: AnimationGraphLimits,
}

impl AnimationGraph {
    #[must_use]
    pub fn flat(
        id: impl Into<AnimationGraphId>,
        name: impl Into<String>,
        output: GraphNodeId,
        reference_pose: Vec<GraphReferenceValue>,
        nodes: Vec<GraphNode>,
    ) -> Self {
        Self {
            schema_version: ANIMATION_GRAPH_SCHEMA_VERSION,
            id: id.into(),
            name: name.into(),
            parameters: Vec::new(),
            reference_pose,
            masks: Vec::new(),
            nodes,
            output,
            state_machine: None,
            event_policy: GraphEventPolicy::default(),
            limits: AnimationGraphLimits::default(),
        }
    }
}

/// Public binding signature used by readiness/admission surfaces without sampling an instance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphBindingSignature {
    pub binding: Binding,
    pub reference_kind: ValueKind,
}
