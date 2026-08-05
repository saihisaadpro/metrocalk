//! Deterministic complete-scene glTF 2.0 binary export.
//!
//! [`crate::scene::SceneAsset`] is the authority for hierarchy, mesh instances, skin bindings and
//! typed animation. This writer maps the subset glTF can represent to standard glTF objects and returns
//! a structured fidelity ledger for conversions or omissions. In particular, static 4x4 matrices retain
//! shear, while animated matrices are decomposed to standard translation/rotation/scale channels only
//! when every authored sample is safely representable.

#![allow(clippy::cast_possible_truncation)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};

use crate::mesh::{Material, MeshAsset, Primitive, Texture};
use crate::scene::{
    AnimationInterpolation, AnimationTarget, AnimationValue, AnimationValueType, Matrix4d,
    SceneAsset, SceneNodeId, SceneUpAxis, SCENE_IR_VERSION,
};
use crate::source::MAX_ELEMENTS;
use crate::usd_export::{FidelityEntry, FidelityReport, FidelityStatus};

const FLOAT: u32 = 5_126;
const UNSIGNED_SHORT: u32 = 5_123;
const UNSIGNED_INT: u32 = 5_125;
const ARRAY_BUFFER: u32 = 34_962;
const ELEMENT_ARRAY_BUFFER: u32 = 34_963;

/// Maximum trusted, internally generated complete-scene GLB output (256 MiB).
///
/// This is intentionally separate from the 64 MiB untrusted external-import cap: baked 4K texture
/// artifacts can legitimately exceed that ingress budget, while still requiring a hard persistence bound.
pub const MAX_SCENE_GLB_EXPORT_BYTES: usize = 256 * 1_024 * 1_024;

/// Production safety budgets for complete-scene GLB export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SceneGlbExportLimits {
    /// Maximum scene nodes, excluding the optional generated unit/up-axis conversion root.
    pub max_nodes: usize,
    /// Maximum reusable mesh payloads.
    pub max_meshes: usize,
    /// Maximum primitives across all mesh payloads.
    pub max_primitives: usize,
    /// Maximum materials across all mesh payloads.
    pub max_materials: usize,
    /// Maximum embedded decoded textures across all mesh payloads.
    pub max_textures: usize,
    /// Maximum skins.
    pub max_skins: usize,
    /// Maximum joints across all skins.
    pub max_joints: usize,
    /// Maximum animation clips.
    pub max_animations: usize,
    /// Maximum authored animation channels.
    pub max_channels: usize,
    /// Maximum authored animation samples.
    pub max_samples: usize,
    /// Maximum UTF-8 bytes in one display name or property address segment.
    pub max_name_bytes: usize,
    /// Maximum completed GLB size.
    pub max_output_bytes: usize,
}

impl Default for SceneGlbExportLimits {
    fn default() -> Self {
        Self {
            max_nodes: 65_535,
            max_meshes: 65_536,
            max_primitives: 65_536,
            max_materials: 65_536,
            max_textures: 65_536,
            max_skins: 4_096,
            max_joints: 262_144,
            max_animations: 4_096,
            max_channels: 100_000,
            max_samples: 1_000_000,
            max_name_bytes: 16 * 1_024,
            max_output_bytes: MAX_SCENE_GLB_EXPORT_BYTES,
        }
    }
}

/// A complete-scene GLB plus a machine-readable account of format conversions and omissions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneGlbExport {
    /// Self-contained glTF 2.0 binary bytes.
    pub bytes: Vec<u8>,
    /// Feature-level fidelity ledger suitable for export preflight/result UI.
    pub report: FidelityReport,
}

/// A rejected complete-scene GLB export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SceneGlbExportError {
    /// The scene schema is newer or older than this writer understands.
    UnsupportedSceneVersion {
        /// Supplied schema version.
        found: u32,
        /// Supported schema version.
        supported: u32,
    },
    /// A deterministic count or byte budget was exceeded.
    BudgetExceeded {
        /// Collection or byte stream.
        kind: &'static str,
        /// Observed count.
        count: usize,
        /// Configured limit.
        limit: usize,
    },
    /// Node identities, parentage, or graph shape are invalid.
    InvalidHierarchy(String),
    /// A node, mesh, skin, material, or texture reference does not resolve.
    InvalidReference(String),
    /// A reusable mesh payload is malformed.
    InvalidMesh {
        /// Scene mesh index.
        mesh: usize,
        /// Stable explanation.
        detail: String,
    },
    /// A skin or joint hierarchy is malformed or contradictory.
    InvalidSkin {
        /// Scene skin index.
        skin: usize,
        /// Stable explanation.
        detail: String,
    },
    /// A typed animation channel is malformed. Representability limitations are reported, not errors.
    InvalidAnimation {
        /// Animation index.
        animation: usize,
        /// Channel index within the animation.
        channel: usize,
        /// Stable explanation.
        detail: String,
    },
    /// An embedded RGBA texture could not be encoded as PNG.
    TextureEncode {
        /// Scene mesh index.
        mesh: usize,
        /// Mesh-local texture index.
        texture: usize,
        /// Flattened codec explanation.
        reason: String,
    },
    /// Internal size arithmetic overflowed.
    SizeOverflow,
}

impl std::fmt::Display for SceneGlbExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedSceneVersion { found, supported } => write!(
                f,
                "unsupported neutral scene version {found}; this GLB writer supports {supported}"
            ),
            Self::BudgetExceeded { kind, count, limit } => {
                write!(
                    f,
                    "scene GLB {kind} budget exceeded: {count} (limit {limit})"
                )
            }
            Self::InvalidHierarchy(detail) => write!(f, "invalid scene hierarchy: {detail}"),
            Self::InvalidReference(detail) => write!(f, "invalid scene reference: {detail}"),
            Self::InvalidMesh { mesh, detail } => {
                write!(f, "invalid scene mesh {mesh}: {detail}")
            }
            Self::InvalidSkin { skin, detail } => {
                write!(f, "invalid scene skin {skin}: {detail}")
            }
            Self::InvalidAnimation {
                animation,
                channel,
                detail,
            } => write!(
                f,
                "invalid animation {animation} channel {channel}: {detail}"
            ),
            Self::TextureEncode {
                mesh,
                texture,
                reason,
            } => write!(
                f,
                "scene mesh {mesh} texture {texture} could not be encoded as PNG: {reason}"
            ),
            Self::SizeOverflow => f.write_str("scene GLB size arithmetic overflowed"),
        }
    }
}

impl std::error::Error for SceneGlbExportError {}

#[derive(Clone, Copy, Debug)]
struct Trs {
    translation: [f32; 3],
    rotation: [f32; 4],
    scale: [f32; 3],
}

#[derive(Debug)]
struct PlannedTransformChannel {
    node: usize,
    interpolation: &'static str,
    times: Vec<f32>,
    translations: Vec<[f32; 3]>,
    rotations: Vec<[f32; 4]>,
    scales: Vec<[f32; 3]>,
}

#[derive(Debug)]
struct PlannedAnimation {
    source_index: usize,
    name: String,
    channels: Vec<PlannedTransformChannel>,
}

#[derive(Debug)]
struct ValidatedScene {
    node_indices: BTreeMap<SceneNodeId, usize>,
    children: Vec<Vec<usize>>,
    roots: Vec<usize>,
    material_offsets: Vec<usize>,
    texture_offsets: Vec<usize>,
    animations: Vec<PlannedAnimation>,
    animated_nodes: BTreeSet<usize>,
    coordinate_conversion: bool,
    mesh_instance_count: usize,
    primitive_count: usize,
    material_count: usize,
    texture_count: usize,
    joint_count: usize,
    skinned_primitive_count: usize,
    matrix_animation_count: usize,
    omitted_property_channels: usize,
    omitted_matrix_channels: usize,
    cubic_channels: usize,
    shifted_animation_count: usize,
    invisible_node_count: usize,
    curvature_slot_count: usize,
    legacy_skeleton_count: usize,
}

/// Export a complete neutral scene using production safety budgets.
pub fn export_scene_glb(scene: &SceneAsset) -> Result<SceneGlbExport, SceneGlbExportError> {
    export_scene_glb_with_limits(scene, SceneGlbExportLimits::default())
}

/// Export a complete neutral scene with explicit safety budgets.
pub fn export_scene_glb_with_limits(
    scene: &SceneAsset,
    limits: SceneGlbExportLimits,
) -> Result<SceneGlbExport, SceneGlbExportError> {
    let validated = validate_scene(scene, limits)?;
    let bytes = encode_scene(scene, &validated, limits.max_output_bytes)?;
    let report = fidelity_report(scene, &validated);
    Ok(SceneGlbExport { bytes, report })
}

// One top-to-bottom preflight keeps cross-references auditable: no output allocation starts until the
// hierarchy, mesh streams, skin bindings and every typed animation sample have been checked.
#[allow(clippy::too_many_lines)]
fn validate_scene(
    scene: &SceneAsset,
    limits: SceneGlbExportLimits,
) -> Result<ValidatedScene, SceneGlbExportError> {
    if scene.version != SCENE_IR_VERSION {
        return Err(SceneGlbExportError::UnsupportedSceneVersion {
            found: scene.version,
            supported: SCENE_IR_VERSION,
        });
    }
    validate_name("scene name", &scene.name, limits.max_name_bytes)?;
    if !scene.meters_per_unit.is_finite() || scene.meters_per_unit <= 0.0 {
        return Err(SceneGlbExportError::InvalidReference(
            "meters_per_unit must be finite and greater than zero".into(),
        ));
    }
    if !scene.time_codes_per_second.is_finite() || scene.time_codes_per_second <= 0.0 {
        return Err(SceneGlbExportError::InvalidReference(
            "time_codes_per_second must be finite and greater than zero".into(),
        ));
    }
    guard("nodes", scene.nodes.len(), limits.max_nodes)?;
    guard("meshes", scene.meshes.len(), limits.max_meshes)?;
    guard("skins", scene.skins.len(), limits.max_skins)?;
    guard("animations", scene.animations.len(), limits.max_animations)?;

    let mut node_indices = BTreeMap::new();
    for (index, node) in scene.nodes.iter().enumerate() {
        validate_name("node name", &node.name, limits.max_name_bytes)?;
        if !node.local_transform.is_finite() {
            return Err(SceneGlbExportError::InvalidHierarchy(format!(
                "node {} has a non-finite local matrix",
                node.id.0
            )));
        }
        if node_indices.insert(node.id, index).is_some() {
            return Err(SceneGlbExportError::InvalidHierarchy(format!(
                "duplicate node id {}",
                node.id.0
            )));
        }
    }

    let mut children = vec![Vec::new(); scene.nodes.len()];
    let mut roots = Vec::new();
    for (index, node) in scene.nodes.iter().enumerate() {
        if let Some(parent) = node.parent {
            if parent == node.id {
                return Err(SceneGlbExportError::InvalidHierarchy(format!(
                    "node {} is its own parent",
                    node.id.0
                )));
            }
            let parent_index = *node_indices.get(&parent).ok_or_else(|| {
                SceneGlbExportError::InvalidHierarchy(format!(
                    "node {} references missing parent {}",
                    node.id.0, parent.0
                ))
            })?;
            children[parent_index].push(index);
        } else {
            roots.push(index);
        }
    }
    validate_acyclic(scene, &node_indices)?;
    for child_list in &mut children {
        child_list.sort_unstable_by_key(|&index| scene.nodes[index].id);
    }
    roots.sort_unstable_by_key(|&index| scene.nodes[index].id);

    let mut material_offsets = Vec::with_capacity(scene.meshes.len());
    let mut texture_offsets = Vec::with_capacity(scene.meshes.len());
    let mut primitive_count = 0usize;
    let mut material_count = 0usize;
    let mut texture_count = 0usize;
    let mut vertex_count = 0usize;
    let mut index_count = 0usize;
    let mut skinned_primitive_count = 0usize;
    let mut curvature_slot_count = 0usize;
    let mut legacy_skeleton_count = 0usize;
    for (mesh_index, mesh) in scene.meshes.iter().enumerate() {
        validate_name("mesh name", &mesh.name, limits.max_name_bytes)?;
        validate_mesh(mesh_index, mesh)?;
        material_offsets.push(material_count);
        texture_offsets.push(texture_count);
        primitive_count = checked_add(primitive_count, mesh.primitives.len())?;
        material_count = checked_add(material_count, mesh.materials.len())?;
        texture_count = checked_add(texture_count, mesh.textures.len())?;
        vertex_count = checked_add(vertex_count, mesh.vertex_count())?;
        index_count = checked_add(index_count, mesh.index_count())?;
        skinned_primitive_count += mesh
            .primitives
            .iter()
            .filter(|primitive| !primitive.joints.is_empty())
            .count();
        curvature_slot_count += mesh
            .materials
            .iter()
            .filter(|material| material.curvature_texture.is_some())
            .count();
        legacy_skeleton_count += usize::from(mesh.skeleton.is_some());
    }
    guard("primitives", primitive_count, limits.max_primitives)?;
    guard("materials", material_count, limits.max_materials)?;
    guard("textures", texture_count, limits.max_textures)?;
    guard("vertices", vertex_count, MAX_ELEMENTS)?;
    guard("indices", index_count, MAX_ELEMENTS)?;

    let mut joint_count = 0usize;
    for (skin_index, skin) in scene.skins.iter().enumerate() {
        validate_name("skin name", &skin.name, limits.max_name_bytes)?;
        validate_skin(skin_index, scene, &node_indices, limits.max_name_bytes)?;
        joint_count = checked_add(joint_count, skin.joints.len())?;
    }
    guard("joints", joint_count, limits.max_joints)?;

    let mut mesh_instance_count = 0usize;
    let mut mesh_has_skin_binding = vec![false; scene.meshes.len()];
    for node in &scene.nodes {
        let Some(mesh_reference) = node.mesh else {
            if node.skin.is_some() {
                return Err(SceneGlbExportError::InvalidReference(format!(
                    "node {} binds a skin without a mesh",
                    node.id.0
                )));
            }
            continue;
        };
        mesh_instance_count += 1;
        let mesh = scene.meshes.get(mesh_reference.0).ok_or_else(|| {
            SceneGlbExportError::InvalidReference(format!(
                "node {} references missing mesh {}",
                node.id.0, mesh_reference.0
            ))
        })?;
        let carries_skin = mesh.skeleton.is_some()
            || mesh
                .primitives
                .iter()
                .any(|primitive| !primitive.joints.is_empty());
        match node.skin {
            Some(skin_reference) => {
                let skin = scene.skins.get(skin_reference.0).ok_or_else(|| {
                    SceneGlbExportError::InvalidReference(format!(
                        "node {} references missing skin {}",
                        node.id.0, skin_reference.0
                    ))
                })?;
                if !mesh
                    .primitives
                    .iter()
                    .any(|primitive| !primitive.joints.is_empty())
                {
                    return Err(SceneGlbExportError::InvalidReference(format!(
                        "node {} binds skin {} to a mesh without joint attributes",
                        node.id.0, skin_reference.0
                    )));
                }
                validate_mesh_skin_binding(node.id, mesh, skin.joints.len())?;
                mesh_has_skin_binding[mesh_reference.0] = true;
            }
            None if carries_skin => {
                return Err(SceneGlbExportError::InvalidReference(format!(
                    "node {} instances skinned mesh {} without a SceneSkin binding",
                    node.id.0, mesh_reference.0
                )));
            }
            None => {}
        }
    }
    for (mesh_index, mesh) in scene.meshes.iter().enumerate() {
        let carries_skin = mesh.skeleton.is_some()
            || mesh
                .primitives
                .iter()
                .any(|primitive| !primitive.joints.is_empty());
        if carries_skin && !mesh_has_skin_binding[mesh_index] {
            return Err(SceneGlbExportError::InvalidReference(format!(
                "skinned mesh {mesh_index} has no node-to-SceneSkin binding"
            )));
        }
    }

    let mut animations = Vec::new();
    let mut animated_nodes = BTreeSet::new();
    let mut channel_count = 0usize;
    let mut sample_count = 0usize;
    let mut matrix_animation_count = 0usize;
    let mut omitted_property_channels = 0usize;
    let mut omitted_matrix_channels = 0usize;
    let mut cubic_channels = 0usize;
    let mut shifted_animation_count = 0usize;
    for (animation_index, animation) in scene.animations.iter().enumerate() {
        validate_name("animation name", &animation.name, limits.max_name_bytes)?;
        channel_count = checked_add(channel_count, animation.channels.len())?;
        let mut minimum_time = 0.0f64;
        let mut first_time = true;
        for (channel_index, channel) in animation.channels.iter().enumerate() {
            validate_animation_channel(
                animation_index,
                channel_index,
                channel,
                &node_indices,
                limits.max_name_bytes,
            )?;
            sample_count = checked_add(sample_count, channel.samples.len())?;
            for sample in &channel.samples {
                if first_time || sample.time_code < minimum_time {
                    minimum_time = sample.time_code;
                    first_time = false;
                }
            }
        }
        let shift = if minimum_time < 0.0 {
            shifted_animation_count += 1;
            -minimum_time
        } else {
            0.0
        };
        let mut duplicate_targets = BTreeSet::new();
        let mut planned_channels = Vec::new();
        for (channel_index, channel) in animation.channels.iter().enumerate() {
            match &channel.target {
                AnimationTarget::NodeProperty { .. } => {
                    omitted_property_channels += 1;
                }
                AnimationTarget::NodeLocalTransform(node_id) => {
                    if !duplicate_targets.insert(*node_id) {
                        return Err(SceneGlbExportError::InvalidAnimation {
                            animation: animation_index,
                            channel: channel_index,
                            detail: format!(
                                "more than one local-transform channel targets node {}",
                                node_id.0
                            ),
                        });
                    }
                    let node_index = node_indices[node_id];
                    let Some(base_trs) = decompose_trs(scene.nodes[node_index].local_transform)
                    else {
                        omitted_matrix_channels += 1;
                        continue;
                    };
                    let mut sample_trs = Vec::with_capacity(channel.samples.len());
                    let mut representable = true;
                    for sample in &channel.samples {
                        let AnimationValue::Matrix4d(matrix) = sample.value else {
                            unreachable!("typed samples were validated above");
                        };
                        if let Some(trs) = decompose_trs(matrix) {
                            sample_trs.push(trs);
                        } else {
                            representable = false;
                            break;
                        }
                    }
                    if !representable {
                        omitted_matrix_channels += 1;
                        continue;
                    }
                    canonicalize_quaternions(base_trs.rotation, &mut sample_trs);
                    let mut times = Vec::with_capacity(channel.samples.len());
                    for sample in &channel.samples {
                        let seconds = (sample.time_code + shift) / scene.time_codes_per_second;
                        let encoded = seconds as f32;
                        if !encoded.is_finite() || encoded < 0.0 {
                            return Err(SceneGlbExportError::InvalidAnimation {
                                animation: animation_index,
                                channel: channel_index,
                                detail:
                                    "sample time cannot be represented as non-negative f32 seconds"
                                        .into(),
                            });
                        }
                        times.push(encoded);
                    }
                    if channel.interpolation == AnimationInterpolation::Cubic {
                        cubic_channels += 1;
                    }
                    matrix_animation_count += 1;
                    animated_nodes.insert(node_index);
                    planned_channels.push(PlannedTransformChannel {
                        node: node_index,
                        interpolation: match channel.interpolation {
                            AnimationInterpolation::Step => "STEP",
                            AnimationInterpolation::Linear | AnimationInterpolation::Cubic => {
                                "LINEAR"
                            }
                        },
                        times,
                        translations: sample_trs.iter().map(|trs| trs.translation).collect(),
                        rotations: sample_trs.iter().map(|trs| trs.rotation).collect(),
                        scales: sample_trs.iter().map(|trs| trs.scale).collect(),
                    });
                }
            }
        }
        if !planned_channels.is_empty() {
            animations.push(PlannedAnimation {
                source_index: animation_index,
                name: animation.name.clone(),
                channels: planned_channels,
            });
        }
    }
    guard("animation channels", channel_count, limits.max_channels)?;
    guard("animation samples", sample_count, limits.max_samples)?;

    Ok(ValidatedScene {
        node_indices,
        children,
        roots,
        material_offsets,
        texture_offsets,
        animations,
        animated_nodes,
        coordinate_conversion: scene.up_axis != SceneUpAxis::Y
            || (scene.meters_per_unit - 1.0).abs() > f64::EPSILON,
        mesh_instance_count,
        primitive_count,
        material_count,
        texture_count,
        joint_count,
        skinned_primitive_count,
        matrix_animation_count,
        omitted_property_channels,
        omitted_matrix_channels,
        cubic_channels,
        shifted_animation_count,
        invisible_node_count: scene.nodes.iter().filter(|node| !node.visible).count(),
        curvature_slot_count,
        legacy_skeleton_count,
    })
}

fn guard(kind: &'static str, count: usize, limit: usize) -> Result<(), SceneGlbExportError> {
    if count > limit {
        Err(SceneGlbExportError::BudgetExceeded { kind, count, limit })
    } else {
        Ok(())
    }
}

fn checked_add(left: usize, right: usize) -> Result<usize, SceneGlbExportError> {
    left.checked_add(right)
        .ok_or(SceneGlbExportError::SizeOverflow)
}

fn validate_name(kind: &'static str, name: &str, limit: usize) -> Result<(), SceneGlbExportError> {
    if name.len() <= limit {
        Ok(())
    } else {
        Err(SceneGlbExportError::BudgetExceeded {
            kind,
            count: name.len(),
            limit,
        })
    }
}

fn validate_acyclic(
    scene: &SceneAsset,
    indices: &BTreeMap<SceneNodeId, usize>,
) -> Result<(), SceneGlbExportError> {
    let mut complete = BTreeSet::new();
    for node in &scene.nodes {
        if complete.contains(&node.id) {
            continue;
        }
        let mut active = BTreeSet::new();
        let mut chain = Vec::new();
        let mut current = Some(node.id);
        while let Some(id) = current {
            if complete.contains(&id) {
                break;
            }
            if !active.insert(id) {
                return Err(SceneGlbExportError::InvalidHierarchy(format!(
                    "cycle reaches node {}",
                    id.0
                )));
            }
            chain.push(id);
            current = scene.nodes[indices[&id]].parent;
        }
        complete.extend(chain);
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn validate_mesh(mesh_index: usize, mesh: &MeshAsset) -> Result<(), SceneGlbExportError> {
    if mesh.primitives.is_empty() {
        return Err(invalid_mesh(mesh_index, "mesh has no drawable primitive"));
    }
    if mesh.materials.is_empty() {
        return Err(invalid_mesh(mesh_index, "mesh has no material"));
    }
    for (primitive_index, primitive) in mesh.primitives.iter().enumerate() {
        let prefix = || format!("primitive {primitive_index}");
        if primitive.positions.is_empty() {
            return Err(invalid_mesh(
                mesh_index,
                format!("{} has no positions", prefix()),
            ));
        }
        if primitive.indices.is_empty() || !primitive.indices.len().is_multiple_of(3) {
            return Err(invalid_mesh(
                mesh_index,
                format!("{} indices must be a non-empty triangle list", prefix()),
            ));
        }
        if primitive
            .indices
            .iter()
            .any(|&index| index as usize >= primitive.positions.len())
        {
            return Err(invalid_mesh(
                mesh_index,
                format!("{} has an out-of-range index", prefix()),
            ));
        }
        for (label, count) in [
            ("normals", primitive.normals.len()),
            ("UVs", primitive.uvs.len()),
            ("tangents", primitive.tangents.len()),
        ] {
            if count != 0 && count != primitive.positions.len() {
                return Err(invalid_mesh(
                    mesh_index,
                    format!(
                        "{} {label} count does not match its position count",
                        prefix()
                    ),
                ));
            }
        }
        if primitive
            .positions
            .iter()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err(invalid_mesh(
                mesh_index,
                format!("{} positions contain a non-finite value", prefix()),
            ));
        }
        if primitive.normals.iter().any(|normal| {
            let length_sq = normal.iter().map(|part| part * part).sum::<f32>();
            normal.iter().any(|part| !part.is_finite())
                || !(0.98 * 0.98..=1.02 * 1.02).contains(&length_sq)
        }) {
            return Err(invalid_mesh(
                mesh_index,
                format!("{} normals must be finite and unit length", prefix()),
            ));
        }
        if primitive
            .uvs
            .iter()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err(invalid_mesh(
                mesh_index,
                format!("{} UVs contain a non-finite value", prefix()),
            ));
        }
        if primitive.tangents.iter().any(|tangent| {
            let length_sq = tangent[..3].iter().map(|part| part * part).sum::<f32>();
            tangent.iter().any(|part| !part.is_finite())
                || !(0.98 * 0.98..=1.02 * 1.02).contains(&length_sq)
                || (tangent[3].abs() - 1.0).abs() > 1.0e-4
        }) {
            return Err(invalid_mesh(
                mesh_index,
                format!(
                    "{} tangents must have unit xyz and handedness -1 or 1",
                    prefix()
                ),
            ));
        }
        let joints_empty = primitive.joints.is_empty();
        let weights_empty = primitive.weights.is_empty();
        if joints_empty != weights_empty {
            return Err(invalid_mesh(
                mesh_index,
                format!("{} JOINTS_0 and WEIGHTS_0 must appear together", prefix()),
            ));
        }
        if !joints_empty
            && (primitive.joints.len() != primitive.positions.len()
                || primitive.weights.len() != primitive.positions.len())
        {
            return Err(invalid_mesh(
                mesh_index,
                format!("{} skin attribute counts do not match positions", prefix()),
            ));
        }
        for weights in &primitive.weights {
            if weights
                .iter()
                .any(|weight| !weight.is_finite() || *weight < 0.0)
            {
                return Err(invalid_mesh(
                    mesh_index,
                    format!("{} weights must be finite and non-negative", prefix()),
                ));
            }
            let sum: f32 = weights.iter().sum();
            if (sum - 1.0).abs() > 1.0e-3 {
                return Err(invalid_mesh(
                    mesh_index,
                    format!("{} weights must sum to one per vertex", prefix()),
                ));
            }
        }
        if primitive.material >= mesh.materials.len() {
            return Err(invalid_mesh(
                mesh_index,
                format!("{} material index is out of range", prefix()),
            ));
        }
    }

    for (material_index, material) in mesh.materials.iter().enumerate() {
        if material
            .base_color
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
            || !material.metallic.is_finite()
            || !(0.0..=1.0).contains(&material.metallic)
            || !material.roughness.is_finite()
            || !(0.0..=1.0).contains(&material.roughness)
        {
            return Err(invalid_mesh(
                mesh_index,
                format!(
                    "material {material_index} PBR factors must be finite and within zero to one"
                ),
            ));
        }
        for slot in material_texture_slots(material).into_iter().flatten() {
            if slot >= mesh.textures.len() {
                return Err(invalid_mesh(
                    mesh_index,
                    format!("material {material_index} texture index is out of range"),
                ));
            }
        }
    }
    for (texture_index, texture) in mesh.textures.iter().enumerate() {
        if texture.width == 0 || texture.height == 0 {
            return Err(invalid_mesh(
                mesh_index,
                format!("texture {texture_index} dimensions must be non-zero"),
            ));
        }
        let expected = u64::from(texture.width)
            .checked_mul(u64::from(texture.height))
            .and_then(|value| value.checked_mul(4))
            .and_then(|value| usize::try_from(value).ok())
            .ok_or(SceneGlbExportError::SizeOverflow)?;
        if texture.rgba8.len() != expected {
            return Err(invalid_mesh(
                mesh_index,
                format!("texture {texture_index} RGBA byte length is inconsistent"),
            ));
        }
    }
    Ok(())
}

fn material_texture_slots(material: &Material) -> [Option<usize>; 5] {
    [
        material.base_color_texture,
        material.metallic_roughness_texture,
        material.normal_texture,
        material.occlusion_texture,
        material.curvature_texture,
    ]
}

fn invalid_mesh(mesh: usize, detail: impl Into<String>) -> SceneGlbExportError {
    SceneGlbExportError::InvalidMesh {
        mesh,
        detail: detail.into(),
    }
}

fn validate_skin(
    skin_index: usize,
    scene: &SceneAsset,
    node_indices: &BTreeMap<SceneNodeId, usize>,
    max_name_bytes: usize,
) -> Result<(), SceneGlbExportError> {
    let skin = &scene.skins[skin_index];
    if skin.joints.is_empty() {
        return Err(invalid_skin(skin_index, "skin has no joints"));
    }
    let mut joint_by_node = BTreeMap::new();
    for (joint_index, joint) in skin.joints.iter().enumerate() {
        validate_name("joint name", &joint.name, max_name_bytes)?;
        if !node_indices.contains_key(&joint.node) {
            return Err(invalid_skin(
                skin_index,
                format!(
                    "joint {joint_index} references missing node {}",
                    joint.node.0
                ),
            ));
        }
        if joint_by_node.insert(joint.node, joint_index).is_some() {
            return Err(invalid_skin(
                skin_index,
                format!("more than one joint references node {}", joint.node.0),
            ));
        }
        if joint.parent.is_some_and(|parent| parent >= joint_index) {
            return Err(invalid_skin(
                skin_index,
                format!("joint {joint_index} parent must precede the child in topological order"),
            ));
        }
        if !joint.rest_local_transform.is_finite()
            || !joint.bind_transform.is_finite()
            || !joint.inverse_bind_matrix.is_finite()
        {
            return Err(invalid_skin(
                skin_index,
                format!("joint {joint_index} contains a non-finite matrix"),
            ));
        }
    }

    for (joint_index, joint) in skin.joints.iter().enumerate() {
        let mut ancestor = scene.nodes[node_indices[&joint.node]].parent;
        let nearest_joint_parent = loop {
            let Some(node_id) = ancestor else { break None };
            if let Some(parent_joint) = joint_by_node.get(&node_id) {
                break Some(*parent_joint);
            }
            ancestor = scene.nodes[node_indices[&node_id]].parent;
        };
        if nearest_joint_parent != joint.parent {
            return Err(invalid_skin(
                skin_index,
                format!(
                    "joint {joint_index} parent metadata {:?} contradicts the scene-node hierarchy {:?}",
                    joint.parent, nearest_joint_parent
                ),
            ));
        }
    }

    if let Some(root) = skin.skeleton_root {
        if !node_indices.contains_key(&root) {
            return Err(invalid_skin(
                skin_index,
                format!("skeleton root {} does not exist", root.0),
            ));
        }
        for (joint_index, joint) in skin.joints.iter().enumerate() {
            if !is_ancestor_or_self(scene, node_indices, root, joint.node) {
                return Err(invalid_skin(
                    skin_index,
                    format!(
                        "skeleton root {} is not an ancestor of joint {joint_index}",
                        root.0
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn is_ancestor_or_self(
    scene: &SceneAsset,
    indices: &BTreeMap<SceneNodeId, usize>,
    ancestor: SceneNodeId,
    node: SceneNodeId,
) -> bool {
    let mut current = Some(node);
    while let Some(id) = current {
        if id == ancestor {
            return true;
        }
        current = scene.nodes[indices[&id]].parent;
    }
    false
}

fn invalid_skin(skin: usize, detail: impl Into<String>) -> SceneGlbExportError {
    SceneGlbExportError::InvalidSkin {
        skin,
        detail: detail.into(),
    }
}

fn validate_mesh_skin_binding(
    node: SceneNodeId,
    mesh: &MeshAsset,
    joint_count: usize,
) -> Result<(), SceneGlbExportError> {
    if let Some(skeleton) = &mesh.skeleton {
        if skeleton.joints.len() != joint_count {
            return Err(SceneGlbExportError::InvalidReference(format!(
                "node {} binds a {}-joint SceneSkin to a {}-joint mesh skeleton",
                node.0,
                joint_count,
                skeleton.joints.len()
            )));
        }
    }
    for (primitive_index, primitive) in mesh.primitives.iter().enumerate() {
        for (joints, weights) in primitive.joints.iter().zip(&primitive.weights) {
            for (&joint, &weight) in joints.iter().zip(weights) {
                if weight > 0.0 && usize::from(joint) >= joint_count {
                    return Err(SceneGlbExportError::InvalidReference(format!(
                        "node {} primitive {primitive_index} has weighted joint {} outside skin size {joint_count}",
                        node.0, joint
                    )));
                }
            }
        }
    }
    Ok(())
}

fn validate_animation_channel(
    animation: usize,
    channel_index: usize,
    channel: &crate::scene::AnimationChannel,
    node_indices: &BTreeMap<SceneNodeId, usize>,
    max_name_bytes: usize,
) -> Result<(), SceneGlbExportError> {
    let invalid = |detail: String| SceneGlbExportError::InvalidAnimation {
        animation,
        channel: channel_index,
        detail,
    };
    match &channel.target {
        AnimationTarget::NodeLocalTransform(node) => {
            if !node_indices.contains_key(node) {
                return Err(invalid(format!("target node {} does not exist", node.0)));
            }
            if channel.value_type != AnimationValueType::Matrix4d {
                return Err(invalid(
                    "node-local transform channels must declare Matrix4d".into(),
                ));
            }
        }
        AnimationTarget::NodeProperty {
            node,
            component,
            field,
        } => {
            if !node_indices.contains_key(node) {
                return Err(invalid(format!("target node {} does not exist", node.0)));
            }
            validate_name("property component", component, max_name_bytes)?;
            validate_name("property field", field, max_name_bytes)?;
        }
    }
    if channel.samples.is_empty() {
        return Err(invalid("channel has no samples".into()));
    }
    let mut previous = f64::NEG_INFINITY;
    for (sample_index, sample) in channel.samples.iter().enumerate() {
        if !sample.time_code.is_finite() || sample.time_code <= previous {
            return Err(invalid(format!(
                "sample {sample_index} time must be finite and strictly increasing"
            )));
        }
        if sample.value.value_type() != channel.value_type {
            return Err(invalid(format!(
                "sample {sample_index} payload does not match the declared type"
            )));
        }
        if !sample.value.is_finite() {
            return Err(invalid(format!(
                "sample {sample_index} contains a non-finite payload"
            )));
        }
        previous = sample.time_code;
    }
    Ok(())
}

fn decompose_trs(matrix: Matrix4d) -> Option<Trs> {
    let rows = matrix.rows;
    let tolerance = 1.0e-8;
    if rows[3][0].abs() > tolerance
        || rows[3][1].abs() > tolerance
        || rows[3][2].abs() > tolerance
        || (rows[3][3] - 1.0).abs() > tolerance
    {
        return None;
    }
    let mut columns = [
        [rows[0][0], rows[1][0], rows[2][0]],
        [rows[0][1], rows[1][1], rows[2][1]],
        [rows[0][2], rows[1][2], rows[2][2]],
    ];
    let mut scale = [
        length3(columns[0]),
        length3(columns[1]),
        length3(columns[2]),
    ];
    if scale
        .iter()
        .any(|value| !value.is_finite() || *value < 1.0e-12)
    {
        return None;
    }
    for axis in 0..3 {
        for part in &mut columns[axis] {
            *part /= scale[axis];
        }
    }
    let orthogonality = dot3(columns[0], columns[1])
        .abs()
        .max(dot3(columns[0], columns[2]).abs())
        .max(dot3(columns[1], columns[2]).abs());
    if orthogonality > 1.0e-5 {
        return None;
    }
    let determinant = dot3(columns[0], cross3(columns[1], columns[2]));
    if (determinant.abs() - 1.0).abs() > 1.0e-5 {
        return None;
    }
    if determinant < 0.0 {
        scale[0] = -scale[0];
        for part in &mut columns[0] {
            *part = -*part;
        }
    }
    let rotation = quaternion_from_columns(columns)?;
    let trs = Trs {
        translation: f32_vec3([rows[0][3], rows[1][3], rows[2][3]])?,
        rotation: f32_vec4(rotation)?,
        scale: f32_vec3(scale)?,
    };
    Some(trs)
}

fn length3(value: [f64; 3]) -> f64 {
    dot3(value, value).sqrt()
}

fn dot3(left: [f64; 3], right: [f64; 3]) -> f64 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

fn cross3(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

fn quaternion_from_columns(columns: [[f64; 3]; 3]) -> Option<[f64; 4]> {
    let m00 = columns[0][0];
    let m01 = columns[1][0];
    let m02 = columns[2][0];
    let m10 = columns[0][1];
    let m11 = columns[1][1];
    let m12 = columns[2][1];
    let m20 = columns[0][2];
    let m21 = columns[1][2];
    let m22 = columns[2][2];
    let trace = m00 + m11 + m22;
    let (x, y, z, w) = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        ((m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s)
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        (0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s)
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        ((m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s)
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        ((m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s)
    };
    let length = (x * x + y * y + z * z + w * w).sqrt();
    if !length.is_finite() || length < 1.0e-12 {
        return None;
    }
    let mut quaternion = [x / length, y / length, z / length, w / length];
    if quaternion[3] < 0.0 {
        for part in &mut quaternion {
            *part = -*part;
        }
    }
    Some(quaternion)
}

fn f32_vec3(value: [f64; 3]) -> Option<[f32; 3]> {
    let converted = [value[0] as f32, value[1] as f32, value[2] as f32];
    converted
        .iter()
        .all(|part| part.is_finite())
        .then_some(converted)
}

fn f32_vec4(value: [f64; 4]) -> Option<[f32; 4]> {
    let converted = [
        value[0] as f32,
        value[1] as f32,
        value[2] as f32,
        value[3] as f32,
    ];
    converted
        .iter()
        .all(|part| part.is_finite())
        .then_some(converted)
}

fn canonicalize_quaternions(base: [f32; 4], samples: &mut [Trs]) {
    let mut previous = base;
    for sample in samples {
        let dot = previous
            .iter()
            .zip(sample.rotation)
            .map(|(left, right)| *left * right)
            .sum::<f32>();
        if dot < 0.0 {
            for part in &mut sample.rotation {
                *part = -*part;
            }
        }
        previous = sample.rotation;
    }
}

#[derive(Default)]
struct GlbBuilder {
    bin: Vec<u8>,
    views: Vec<String>,
    accessors: Vec<String>,
}

impl GlbBuilder {
    fn align4(&mut self) {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
    }

    fn add_view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        self.align4();
        let offset = self.bin.len();
        self.bin.extend_from_slice(bytes);
        let index = self.views.len();
        let target = target.map_or(String::new(), |value| format!(",\"target\":{value}"));
        self.views.push(format!(
            "{{\"buffer\":0,\"byteOffset\":{offset},\"byteLength\":{}{target}}}",
            bytes.len()
        ));
        index
    }

    fn add_accessor(
        &mut self,
        view: usize,
        component_type: u32,
        count: usize,
        accessor_type: &'static str,
        bounds: Option<String>,
    ) -> usize {
        let index = self.accessors.len();
        let bounds = bounds.unwrap_or_default();
        self.accessors.push(format!(
            "{{\"bufferView\":{view},\"componentType\":{component_type},\"count\":{count},\"type\":\"{accessor_type}\"{bounds}}}"
        ));
        index
    }
}

#[allow(clippy::too_many_lines)]
fn encode_scene(
    scene: &SceneAsset,
    validated: &ValidatedScene,
    max_output_bytes: usize,
) -> Result<Vec<u8>, SceneGlbExportError> {
    let mut builder = GlbBuilder::default();
    let mut mesh_json = Vec::with_capacity(scene.meshes.len());
    for (mesh_index, mesh) in scene.meshes.iter().enumerate() {
        let primitives = mesh
            .primitives
            .iter()
            .map(|primitive| {
                encode_primitive(
                    &mut builder,
                    primitive,
                    validated.material_offsets[mesh_index],
                )
            })
            .collect::<Vec<_>>();
        mesh_json.push(format!(
            "{{\"name\":{},\"primitives\":[{}],\"extras\":{{\"metrocalkSceneMeshIndex\":{mesh_index}}}}}",
            json_string(&mesh.name),
            primitives.join(",")
        ));
    }

    let mut material_json = Vec::with_capacity(validated.material_count);
    for (mesh_index, mesh) in scene.meshes.iter().enumerate() {
        material_json.extend(
            mesh.materials
                .iter()
                .map(|material| encode_material(material, validated.texture_offsets[mesh_index])),
        );
    }

    let mut image_json = Vec::with_capacity(validated.texture_count);
    let mut texture_json = Vec::with_capacity(validated.texture_count);
    for (mesh_index, mesh) in scene.meshes.iter().enumerate() {
        for (texture_index, texture) in mesh.textures.iter().enumerate() {
            let png = encode_png(texture).map_err(|reason| SceneGlbExportError::TextureEncode {
                mesh: mesh_index,
                texture: texture_index,
                reason,
            })?;
            let view = builder.add_view(&png, None);
            image_json.push(format!(
                "{{\"bufferView\":{view},\"mimeType\":\"image/png\"}}"
            ));
            texture_json.push(format!("{{\"source\":{}}}", texture_json.len()));
        }
    }

    let skin_json = scene
        .skins
        .iter()
        .map(|skin| encode_skin(&mut builder, skin, &validated.node_indices))
        .collect::<Vec<_>>();
    let animation_json = validated
        .animations
        .iter()
        .map(|animation| encode_animation(&mut builder, animation))
        .collect::<Vec<_>>();
    let mut node_json =
        Vec::with_capacity(scene.nodes.len() + usize::from(validated.coordinate_conversion));
    for (node_index, node) in scene.nodes.iter().enumerate() {
        let mut fields = format!("\"name\":{}", json_string(&node.name));
        if let Some(mesh) = node.mesh {
            let _ = write!(fields, ",\"mesh\":{}", mesh.0);
        }
        if let Some(skin) = node.skin {
            let _ = write!(fields, ",\"skin\":{}", skin.0);
        }
        if !validated.children[node_index].is_empty() {
            let _ = write!(
                fields,
                ",\"children\":[{}]",
                usize_list_json(&validated.children[node_index])
            );
        }
        if validated.animated_nodes.contains(&node_index) {
            let trs = decompose_trs(node.local_transform)
                .expect("animated-node base matrices were validated as decomposable");
            let _ = write!(
                fields,
                ",\"translation\":{},\"rotation\":{},\"scale\":{}",
                f32_array_json(&trs.translation),
                f32_array_json(&trs.rotation),
                f32_array_json(&trs.scale)
            );
        } else {
            let _ = write!(
                fields,
                ",\"matrix\":{}",
                matrix_json_f32(node.local_transform)
            );
        }
        let _ = write!(
            fields,
            ",\"extras\":{{\"metrocalkNodeId\":{},\"metrocalkVisible\":{}}}",
            json_string(&node.id.0.to_string()),
            node.visible
        );
        node_json.push(format!("{{{fields}}}"));
    }
    if validated.coordinate_conversion {
        let root_index = scene.nodes.len();
        let conversion = coordinate_conversion_matrix(scene);
        node_json.push(format!(
            "{{\"name\":\"__MetrocalkAxisAndUnitConversion\",\"children\":[{}],\"matrix\":{},\"extras\":{{\"metrocalkGeneratedConversionRoot\":true,\"metrocalkSourceUpAxis\":{},\"metrocalkMetersPerUnit\":{}}}}}",
            usize_list_json(&validated.roots),
            matrix_json_f32(conversion),
            json_string(match scene.up_axis {
                SceneUpAxis::Y => "Y",
                SceneUpAxis::Z => "Z",
            }),
            scene.meters_per_unit
        ));
        debug_assert_eq!(root_index, node_json.len() - 1);
    }

    builder.align4();
    let mut json = format!(
        "{{\"asset\":{{\"version\":\"2.0\",\"generator\":\"Metrocalk complete-scene GLB exporter\",\"extras\":{{\"metrocalkSceneVersion\":{},\"metrocalkMetersPerUnit\":{},\"metrocalkUpAxis\":{},\"metrocalkTimeCodesPerSecond\":{}}}}}",
        scene.version,
        scene.meters_per_unit,
        json_string(match scene.up_axis {
            SceneUpAxis::Y => "Y",
            SceneUpAxis::Z => "Z",
        }),
        scene.time_codes_per_second
    );
    let _ = write!(
        json,
        ",\"buffers\":[{{\"byteLength\":{}}}]",
        builder.bin.len()
    );
    if !builder.views.is_empty() {
        let _ = write!(json, ",\"bufferViews\":[{}]", builder.views.join(","));
    }
    if !builder.accessors.is_empty() {
        let _ = write!(json, ",\"accessors\":[{}]", builder.accessors.join(","));
    }
    if !material_json.is_empty() {
        let _ = write!(json, ",\"materials\":[{}]", material_json.join(","));
    }
    if !image_json.is_empty() {
        let _ = write!(json, ",\"images\":[{}]", image_json.join(","));
        let _ = write!(json, ",\"textures\":[{}]", texture_json.join(","));
    }
    if !mesh_json.is_empty() {
        let _ = write!(json, ",\"meshes\":[{}]", mesh_json.join(","));
    }
    if !skin_json.is_empty() {
        let _ = write!(json, ",\"skins\":[{}]", skin_json.join(","));
    }
    if !animation_json.is_empty() {
        let _ = write!(json, ",\"animations\":[{}]", animation_json.join(","));
    }
    let _ = write!(json, ",\"nodes\":[{}]", node_json.join(","));
    let scene_roots = if validated.coordinate_conversion {
        scene.nodes.len().to_string()
    } else {
        usize_list_json(&validated.roots)
    };
    let _ = write!(
        json,
        ",\"scenes\":[{{\"name\":{},\"nodes\":[{scene_roots}]}}],\"scene\":0}}",
        json_string(&scene.name)
    );
    assemble_glb(&json, &builder.bin, max_output_bytes)
}

fn encode_primitive(
    builder: &mut GlbBuilder,
    primitive: &Primitive,
    material_offset: usize,
) -> String {
    let position_bytes = f32_bytes(primitive.positions.iter().flatten().copied());
    let position_view = builder.add_view(&position_bytes, Some(ARRAY_BUFFER));
    let (minimum, maximum) = position_bounds(&primitive.positions);
    let position_accessor = builder.add_accessor(
        position_view,
        FLOAT,
        primitive.positions.len(),
        "VEC3",
        Some(format!(
            ",\"min\":{},\"max\":{}",
            f32_array_json(&minimum),
            f32_array_json(&maximum)
        )),
    );
    let mut attributes = format!("\"POSITION\":{position_accessor}");
    if !primitive.normals.is_empty() {
        append_f32_attribute(
            builder,
            &mut attributes,
            "NORMAL",
            "VEC3",
            &primitive.normals,
        );
    }
    if !primitive.uvs.is_empty() {
        append_f32_attribute(
            builder,
            &mut attributes,
            "TEXCOORD_0",
            "VEC2",
            &primitive.uvs,
        );
    }
    if !primitive.tangents.is_empty() {
        append_f32_attribute(
            builder,
            &mut attributes,
            "TANGENT",
            "VEC4",
            &primitive.tangents,
        );
    }
    if !primitive.joints.is_empty() {
        let mut bytes = Vec::with_capacity(primitive.joints.len() * 8);
        for joints in &primitive.joints {
            for joint in joints {
                bytes.extend_from_slice(&joint.to_le_bytes());
            }
        }
        let view = builder.add_view(&bytes, Some(ARRAY_BUFFER));
        let accessor =
            builder.add_accessor(view, UNSIGNED_SHORT, primitive.joints.len(), "VEC4", None);
        let _ = write!(attributes, ",\"JOINTS_0\":{accessor}");
        append_f32_attribute(
            builder,
            &mut attributes,
            "WEIGHTS_0",
            "VEC4",
            &primitive.weights,
        );
    }
    let mut index_bytes = Vec::with_capacity(primitive.indices.len() * 4);
    for index in &primitive.indices {
        index_bytes.extend_from_slice(&index.to_le_bytes());
    }
    let index_view = builder.add_view(&index_bytes, Some(ELEMENT_ARRAY_BUFFER));
    let index_accessor = builder.add_accessor(
        index_view,
        UNSIGNED_INT,
        primitive.indices.len(),
        "SCALAR",
        None,
    );
    format!(
        "{{\"attributes\":{{{attributes}}},\"indices\":{index_accessor},\"material\":{},\"mode\":4}}",
        material_offset + primitive.material
    )
}

fn append_f32_attribute<const N: usize>(
    builder: &mut GlbBuilder,
    attributes: &mut String,
    semantic: &'static str,
    accessor_type: &'static str,
    values: &[[f32; N]],
) {
    let bytes = f32_bytes(values.iter().flatten().copied());
    let view = builder.add_view(&bytes, Some(ARRAY_BUFFER));
    let accessor = builder.add_accessor(view, FLOAT, values.len(), accessor_type, None);
    let _ = write!(attributes, ",\"{semantic}\":{accessor}");
}

fn encode_material(material: &Material, texture_offset: usize) -> String {
    let [red, green, blue, alpha] = material.base_color;
    let mut pbr = format!(
        "\"baseColorFactor\":[{red},{green},{blue},{alpha}],\"metallicFactor\":{},\"roughnessFactor\":{}",
        material.metallic, material.roughness
    );
    if let Some(index) = material.base_color_texture {
        let _ = write!(
            pbr,
            ",\"baseColorTexture\":{{\"index\":{}}}",
            texture_offset + index
        );
    }
    if let Some(index) = material.metallic_roughness_texture {
        let _ = write!(
            pbr,
            ",\"metallicRoughnessTexture\":{{\"index\":{}}}",
            texture_offset + index
        );
    }
    let normal = material.normal_texture.map_or(String::new(), |index| {
        format!(
            ",\"normalTexture\":{{\"index\":{}}}",
            texture_offset + index
        )
    });
    let occlusion = material.occlusion_texture.map_or(String::new(), |index| {
        format!(
            ",\"occlusionTexture\":{{\"index\":{}}}",
            texture_offset + index
        )
    });
    let curvature = material.curvature_texture.map_or(String::new(), |index| {
        format!(
            ",\"extras\":{{\"metrocalkCurvatureTexture\":{}}}",
            texture_offset + index
        )
    });
    format!("{{\"pbrMetallicRoughness\":{{{pbr}}}{normal}{occlusion}{curvature}}}")
}

fn encode_skin(
    builder: &mut GlbBuilder,
    skin: &crate::scene::SceneSkin,
    node_indices: &BTreeMap<SceneNodeId, usize>,
) -> String {
    let matrix_bytes = f32_bytes(
        skin.joints
            .iter()
            .flat_map(|joint| matrix_column_major_f32(joint.inverse_bind_matrix)),
    );
    let view = builder.add_view(&matrix_bytes, None);
    let accessor = builder.add_accessor(view, FLOAT, skin.joints.len(), "MAT4", None);
    let joints = skin
        .joints
        .iter()
        .map(|joint| node_indices[&joint.node].to_string())
        .collect::<Vec<_>>()
        .join(",");
    let mut fields = format!(
        "\"name\":{},\"inverseBindMatrices\":{accessor},\"joints\":[{joints}]",
        json_string(&skin.name)
    );
    if let Some(root) = skin.skeleton_root {
        let _ = write!(fields, ",\"skeleton\":{}", node_indices[&root]);
    }
    let metadata = skin
        .joints
        .iter()
        .map(|joint| {
            format!(
                "{{\"nodeId\":{},\"name\":{},\"parent\":{},\"restLocalMatrix\":{},\"bindMatrix\":{}}}",
                json_string(&joint.node.0.to_string()),
                json_string(&joint.name),
                joint.parent.map_or_else(|| "null".into(), |parent| parent.to_string()),
                matrix_json_f64(joint.rest_local_transform),
                matrix_json_f64(joint.bind_transform)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let _ = write!(
        fields,
        ",\"extras\":{{\"metrocalkJointMetadata\":[{metadata}]}}"
    );
    format!("{{{fields}}}")
}

fn encode_animation(builder: &mut GlbBuilder, animation: &PlannedAnimation) -> String {
    let mut samplers = Vec::with_capacity(animation.channels.len() * 3);
    let mut channels = Vec::with_capacity(animation.channels.len() * 3);
    for source in &animation.channels {
        let time_bytes = f32_bytes(source.times.iter().copied());
        let time_view = builder.add_view(&time_bytes, None);
        let time_accessor = builder.add_accessor(
            time_view,
            FLOAT,
            source.times.len(),
            "SCALAR",
            Some(format!(
                ",\"min\":[{}],\"max\":[{}]",
                source.times.first().expect("validated animation sample"),
                source.times.last().expect("validated animation sample")
            )),
        );
        for (path, accessor_type, bytes, count) in [
            (
                "translation",
                "VEC3",
                f32_bytes(source.translations.iter().flatten().copied()),
                source.translations.len(),
            ),
            (
                "rotation",
                "VEC4",
                f32_bytes(source.rotations.iter().flatten().copied()),
                source.rotations.len(),
            ),
            (
                "scale",
                "VEC3",
                f32_bytes(source.scales.iter().flatten().copied()),
                source.scales.len(),
            ),
        ] {
            let output_view = builder.add_view(&bytes, None);
            let output_accessor =
                builder.add_accessor(output_view, FLOAT, count, accessor_type, None);
            let sampler_index = samplers.len();
            samplers.push(format!(
                "{{\"input\":{time_accessor},\"interpolation\":\"{}\",\"output\":{output_accessor}}}",
                source.interpolation
            ));
            channels.push(format!(
                "{{\"sampler\":{sampler_index},\"target\":{{\"node\":{},\"path\":\"{path}\"}}}}",
                source.node
            ));
        }
    }
    format!(
        "{{\"name\":{},\"samplers\":[{}],\"channels\":[{}],\"extras\":{{\"metrocalkSourceAnimationIndex\":{}}}}}",
        json_string(&animation.name),
        samplers.join(","),
        channels.join(","),
        animation.source_index
    )
}

fn coordinate_conversion_matrix(scene: &SceneAsset) -> Matrix4d {
    let scale = scene.meters_per_unit;
    match scene.up_axis {
        SceneUpAxis::Y => Matrix4d {
            rows: [
                [scale, 0.0, 0.0, 0.0],
                [0.0, scale, 0.0, 0.0],
                [0.0, 0.0, scale, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        },
        SceneUpAxis::Z => Matrix4d {
            rows: [
                [scale, 0.0, 0.0, 0.0],
                [0.0, 0.0, scale, 0.0],
                [0.0, -scale, 0.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        },
    }
}

fn encode_png(texture: &Texture) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(
            &texture.rgba8,
            texture.width,
            texture.height,
            ExtendedColorType::Rgba8,
        )
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

fn f32_bytes(values: impl IntoIterator<Item = f32>) -> Vec<u8> {
    let values = values.into_iter();
    let mut bytes = Vec::with_capacity(values.size_hint().0.saturating_mul(4));
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn position_bounds(positions: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    for position in positions {
        for axis in 0..3 {
            minimum[axis] = minimum[axis].min(position[axis]);
            maximum[axis] = maximum[axis].max(position[axis]);
        }
    }
    (minimum, maximum)
}

fn matrix_column_major_f32(matrix: Matrix4d) -> impl Iterator<Item = f32> {
    (0..4).flat_map(move |column| (0..4).map(move |row| matrix.rows[row][column] as f32))
}

fn matrix_json_f32(matrix: Matrix4d) -> String {
    let values = matrix_column_major_f32(matrix).collect::<Vec<_>>();
    f32_array_json(&values)
}

fn matrix_json_f64(matrix: Matrix4d) -> String {
    let values = (0..4)
        .flat_map(|column| (0..4).map(move |row| matrix.rows[row][column]))
        .collect::<Vec<_>>();
    f64_array_json(&values)
}

fn f32_array_json(values: &[f32]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn f64_array_json(values: &[f64]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn usize_list_json(values: &[usize]) -> String {
    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn json_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0c}' => escaped.push_str("\\f"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            control if control <= '\u{1f}' => {
                let _ = write!(escaped, "\\u{:04x}", control as u32);
            }
            other => escaped.push(other),
        }
    }
    escaped.push('"');
    escaped
}

fn assemble_glb(json: &str, bin: &[u8], limit: usize) -> Result<Vec<u8>, SceneGlbExportError> {
    let mut json_chunk = json.as_bytes().to_vec();
    while !json_chunk.len().is_multiple_of(4) {
        json_chunk.push(b' ');
    }
    let mut bin_chunk = bin.to_vec();
    while !bin_chunk.len().is_multiple_of(4) {
        bin_chunk.push(0);
    }
    let total = 12usize
        .checked_add(8)
        .and_then(|value| value.checked_add(json_chunk.len()))
        .and_then(|value| value.checked_add(8))
        .and_then(|value| value.checked_add(bin_chunk.len()))
        .ok_or(SceneGlbExportError::SizeOverflow)?;
    let effective_limit = limit.min(u32::MAX as usize);
    if total > effective_limit {
        return Err(SceneGlbExportError::BudgetExceeded {
            kind: "output bytes",
            count: total,
            limit: effective_limit,
        });
    }
    let mut bytes = Vec::with_capacity(total);
    bytes.extend_from_slice(&0x4654_6c67_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u32.to_le_bytes());
    bytes.extend_from_slice(&(total as u32).to_le_bytes());
    bytes.extend_from_slice(&(json_chunk.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0x4e4f_534a_u32.to_le_bytes());
    bytes.extend_from_slice(&json_chunk);
    bytes.extend_from_slice(&(bin_chunk.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0x004e_4942_u32.to_le_bytes());
    bytes.extend_from_slice(&bin_chunk);
    Ok(bytes)
}

#[allow(clippy::too_many_lines)]
fn fidelity_report(scene: &SceneAsset, validated: &ValidatedScene) -> FidelityReport {
    let mut entries = vec![
        FidelityEntry {
            status: FidelityStatus::Converted,
            feature: "hierarchy_and_local_transforms".into(),
            count: scene.nodes.len(),
            detail: "Parentage and stable node IDs are retained. glTF transform values use f32 precision; static matrices retain affine shear, while animated nodes use decomposed TRS.".into(),
        },
        FidelityEntry {
            status: FidelityStatus::Preserved,
            feature: "shared_mesh_instances".into(),
            count: validated.mesh_instance_count,
            detail: format!(
                "{} reusable mesh payload(s) are referenced by scene nodes without per-instance geometry duplication ({} primitive(s)).",
                scene.meshes.len(), validated.primitive_count
            ),
        },
        FidelityEntry {
            status: FidelityStatus::Preserved,
            feature: "pbr_materials_and_embedded_textures".into(),
            count: validated.material_count,
            detail: format!(
                "Metallic-roughness factors and {} decoded RGBA texture(s) are written to standard glTF material slots and embedded PNG images.",
                validated.texture_count
            ),
        },
    ];
    if validated.coordinate_conversion {
        entries.push(FidelityEntry {
            status: FidelityStatus::Converted,
            feature: "scene_units_and_up_axis".into(),
            count: 1,
            detail: "A generated root converts the authored units/up-axis to glTF's metre, Y-up convention without baking or duplicating mesh data.".into(),
        });
    }
    if validated.joint_count > 0 {
        entries.push(FidelityEntry {
            status: FidelityStatus::Preserved,
            feature: "skins_weights_and_inverse_binds".into(),
            count: validated.skinned_primitive_count,
            detail: format!(
                "{} joint(s), four-wide JOINTS_0/WEIGHTS_0 streams, skeleton roots and inverse-bind matrices use standard glTF skin objects.",
                validated.joint_count
            ),
        });
        entries.push(FidelityEntry {
            status: FidelityStatus::Converted,
            feature: "joint_authoring_metadata".into(),
            count: validated.joint_count,
            detail: "Scene-node rest transforms and inverse binds are standard glTF; independent joint names, parent indices, exact f64 rest matrices and bind matrices are retained in namespaced skin extras.".into(),
        });
    }
    if validated.matrix_animation_count > 0 {
        entries.push(FidelityEntry {
            status: FidelityStatus::Converted,
            feature: "matrix_animation_to_trs".into(),
            count: validated.matrix_animation_count,
            detail: "Representable matrix-channel sample endpoints are retained as synchronized glTF translation, rotation and scale channels in seconds. Inter-sample evaluation follows glTF TRS interpolation.".into(),
        });
    }
    if validated.cubic_channels > 0 {
        entries.push(FidelityEntry {
            status: FidelityStatus::Converted,
            feature: "cubic_animation_without_tangents".into(),
            count: validated.cubic_channels,
            detail: "The neutral channel declares cubic intent but has no in/out tangents required by glTF CUBICSPLINE, so sample endpoints are emitted with LINEAR interpolation.".into(),
        });
    }
    if validated.shifted_animation_count > 0 {
        entries.push(FidelityEntry {
            status: FidelityStatus::Converted,
            feature: "negative_animation_time_origin".into(),
            count: validated.shifted_animation_count,
            detail: "Clips beginning before time code zero are shifted as a whole to non-negative glTF seconds; relative timing is unchanged.".into(),
        });
    }
    if validated.omitted_property_channels > 0 {
        entries.push(FidelityEntry {
            status: FidelityStatus::Omitted,
            feature: "project_property_animation".into(),
            count: validated.omitted_property_channels,
            detail: "Core glTF animation targets only node translation, rotation, scale or morph weights. Typed engine property channels require the USDA or project-scene artifact.".into(),
        });
    }
    if validated.omitted_matrix_channels > 0 {
        entries.push(FidelityEntry {
            status: FidelityStatus::Omitted,
            feature: "non_trs_matrix_animation".into(),
            count: validated.omitted_matrix_channels,
            detail: "At least one base/sample matrix contains shear, perspective, a degenerate axis or an f32 overflow. Static node matrices remain, but glTF cannot animate those matrices without destructive approximation.".into(),
        });
    }
    if validated.invisible_node_count > 0 {
        entries.push(FidelityEntry {
            status: FidelityStatus::Converted,
            feature: "authored_visibility".into(),
            count: validated.invisible_node_count,
            detail: "Visibility is retained in metrocalkVisible node extras because core glTF has no node visibility property; generic viewers may still draw these nodes.".into(),
        });
    }
    if validated.curvature_slot_count > 0 {
        entries.push(FidelityEntry {
            status: FidelityStatus::Converted,
            feature: "curvature_texture_slot".into(),
            count: validated.curvature_slot_count,
            detail: "Embedded curvature images are retained and referenced by metrocalkCurvatureTexture material extras because core glTF has no curvature PBR slot.".into(),
        });
    }
    if validated.legacy_skeleton_count > 0 {
        entries.push(FidelityEntry {
            status: FidelityStatus::Converted,
            feature: "mesh_local_skeleton_metadata".into(),
            count: validated.legacy_skeleton_count,
            detail: "SceneSkin is the complete-scene skeleton authority. Compatible legacy MeshAsset skeleton data is represented through the bound SceneSkin rather than duplicated.".into(),
        });
    }
    FidelityReport {
        format: "glTF 2.0 binary (.glb), complete scene subset".into(),
        scene_version: scene.version,
        entries,
    }
}
