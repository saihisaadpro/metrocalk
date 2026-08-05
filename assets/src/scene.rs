//! Versioned, project-owned neutral scene representation.
//!
//! [`MeshAsset`](crate::mesh::MeshAsset) remains reusable geometry. This module adds the information a
//! complete scene needs around those meshes: stable nodes, an exact local matrix, shared mesh references,
//! skin metadata, and typed time-sampled animation channels. It deliberately contains no USD, glTF, FBX,
//! ECS, or renderer types, so importers/exporters can meet at this boundary without becoming the scene
//! authority.

use crate::mesh::MeshAsset;

/// Current neutral-scene schema version.
pub const SCENE_IR_VERSION: u32 = 1;

/// Stable node identity within one [`SceneAsset`]. It is not a vector index and survives reordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SceneNodeId(pub u64);

/// Reference to one reusable entry in [`SceneAsset::meshes`]. Multiple nodes may share it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SceneMeshRef(pub usize);

/// Reference to one skin in [`SceneAsset::skins`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SceneSkinRef(pub usize);

/// An exact row-major 4x4 matrix. No TRS decomposition is required, so non-uniform scale and shear are
/// preserved by the neutral scene and by exporters that support a matrix transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix4d {
    /// Matrix rows, indexed as `[row][column]`.
    pub rows: [[f64; 4]; 4],
}

impl Matrix4d {
    /// Identity transform.
    pub const IDENTITY: Self = Self {
        rows: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
    };

    /// True only when every matrix element is finite.
    #[must_use]
    pub fn is_finite(self) -> bool {
        self.rows.iter().flatten().all(|value| value.is_finite())
    }
}

impl Default for Matrix4d {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// USD-compatible scene up axis. OpenUSD stages use Y or Z as their declared up axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SceneUpAxis {
    /// Y-up (the neutral-scene default).
    #[default]
    Y,
    /// Z-up.
    Z,
}

/// One node in the scene hierarchy.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneNode {
    /// Stable identity within the scene.
    pub id: SceneNodeId,
    /// Safe path-segment name. Exporters validate it before using it as a foreign-format identifier.
    pub name: String,
    /// Parent node, or `None` for a scene root.
    pub parent: Option<SceneNodeId>,
    /// Exact local transform relative to [`Self::parent`].
    pub local_transform: Matrix4d,
    /// Optional reusable mesh payload.
    pub mesh: Option<SceneMeshRef>,
    /// Optional skin binding for the mesh payload.
    pub skin: Option<SceneSkinRef>,
    /// Authored visibility. Invisible nodes remain in the hierarchy and in interchange output.
    pub visible: bool,
}

/// Metadata for one skeleton joint.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneJoint {
    /// Scene node driven by this joint.
    pub node: SceneNodeId,
    /// Joint path segment, independent from the scene-node display/path name.
    pub name: String,
    /// Parent joint index, or `None` for a skeleton root. Parents must precede children.
    pub parent: Option<usize>,
    /// Local rest transform relative to the parent joint.
    pub rest_local_transform: Matrix4d,
    /// Global bind transform in skeleton space.
    pub bind_transform: Matrix4d,
    /// Inverse bind matrix retained explicitly for lossless round-trip metadata.
    pub inverse_bind_matrix: Matrix4d,
}

/// One named skin/skeleton that can be bound by one or more mesh nodes.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneSkin {
    /// Safe name used by interchange formats.
    pub name: String,
    /// Optional scene node that acts as the skeleton root.
    pub skeleton_root: Option<SceneNodeId>,
    /// Topologically ordered joint metadata.
    pub joints: Vec<SceneJoint>,
}

/// The declared payload type of an animation channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationValueType {
    /// Exact 4x4 node-local transform.
    Matrix4d,
    /// Three-component floating-point vector.
    Vector3d,
    /// Quaternion stored as `[x, y, z, w]`.
    Quaterniond,
    /// Floating-point scalar.
    Number,
    /// Signed integer scalar.
    Integer,
    /// Boolean scalar.
    Boolean,
}

/// A typed animation sample value.
#[derive(Clone, Debug, PartialEq)]
pub enum AnimationValue {
    /// Exact 4x4 matrix.
    Matrix4d(Matrix4d),
    /// Three-component vector.
    Vector3d([f64; 3]),
    /// Quaternion stored as `[x, y, z, w]`.
    Quaterniond([f64; 4]),
    /// Floating-point scalar.
    Number(f64),
    /// Signed integer scalar.
    Integer(i64),
    /// Boolean scalar.
    Boolean(bool),
}

impl AnimationValue {
    /// Runtime tag used to verify that every sample matches its channel declaration.
    #[must_use]
    pub const fn value_type(&self) -> AnimationValueType {
        match self {
            Self::Matrix4d(_) => AnimationValueType::Matrix4d,
            Self::Vector3d(_) => AnimationValueType::Vector3d,
            Self::Quaterniond(_) => AnimationValueType::Quaterniond,
            Self::Number(_) => AnimationValueType::Number,
            Self::Integer(_) => AnimationValueType::Integer,
            Self::Boolean(_) => AnimationValueType::Boolean,
        }
    }

    /// True only when every floating-point element is finite.
    #[must_use]
    pub fn is_finite(&self) -> bool {
        match self {
            Self::Matrix4d(value) => value.is_finite(),
            Self::Vector3d(value) => value.iter().all(|part| part.is_finite()),
            Self::Quaterniond(value) => value.iter().all(|part| part.is_finite()),
            Self::Number(value) => value.is_finite(),
            Self::Integer(_) | Self::Boolean(_) => true,
        }
    }
}

/// Interpolation requested between adjacent samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationInterpolation {
    /// Hold the previous value.
    Step,
    /// Linear interpolation where the payload type supports it.
    Linear,
    /// Cubic interpolation. Exporters may preserve it as metadata when the destination has no equivalent.
    Cubic,
}

/// Target address for a typed animation channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnimationTarget {
    /// Replace a node's exact local 4x4 transform. This target requires [`AnimationValueType::Matrix4d`].
    NodeLocalTransform(SceneNodeId),
    /// A typed project property. This remains a project-owned address rather than a USD-specific path.
    NodeProperty {
        /// Target node.
        node: SceneNodeId,
        /// Component/type segment.
        component: String,
        /// Field segment.
        field: String,
    },
}

/// One value at one stage time code.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationSample {
    /// Time in stage time codes.
    pub time_code: f64,
    /// Typed payload.
    pub value: AnimationValue,
}

/// One typed animation channel.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationChannel {
    /// Project-owned target address.
    pub target: AnimationTarget,
    /// Declared payload type; every sample must match it.
    pub value_type: AnimationValueType,
    /// Requested interpolation.
    pub interpolation: AnimationInterpolation,
    /// Strictly increasing samples.
    pub samples: Vec<AnimationSample>,
}

/// A named group of typed animation channels sharing the stage time domain.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneAnimation {
    /// Safe interchange name.
    pub name: String,
    /// Typed channels in authored order.
    pub channels: Vec<AnimationChannel>,
}

/// A complete neutral scene around reusable [`MeshAsset`] values.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneAsset {
    /// Neutral-scene schema version. Exporters reject versions they do not understand.
    pub version: u32,
    /// Human-readable scene name. It is metadata, not a foreign-format path.
    pub name: String,
    /// Physical metres represented by one authored distance unit.
    pub meters_per_unit: f64,
    /// Declared up axis.
    pub up_axis: SceneUpAxis,
    /// Stage clock rate.
    pub time_codes_per_second: f64,
    /// Stable hierarchy nodes.
    pub nodes: Vec<SceneNode>,
    /// Reusable meshes. Multiple nodes can reference the same entry without duplicating geometry.
    pub meshes: Vec<MeshAsset>,
    /// Skin/skeleton metadata.
    pub skins: Vec<SceneSkin>,
    /// Typed animation groups.
    pub animations: Vec<SceneAnimation>,
}

impl SceneAsset {
    /// Construct an empty scene using the current schema version and conservative interchange defaults.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            version: SCENE_IR_VERSION,
            name: name.into(),
            meters_per_unit: 1.0,
            up_axis: SceneUpAxis::Y,
            time_codes_per_second: 24.0,
            nodes: Vec::new(),
            meshes: Vec::new(),
            skins: Vec::new(),
            animations: Vec::new(),
        }
    }
}

impl Default for SceneAsset {
    fn default() -> Self {
        Self::new("Scene")
    }
}
