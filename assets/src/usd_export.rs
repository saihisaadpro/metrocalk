//! Deterministic ASCII USDA scene export.
//!
//! This is deliberately a bounded **USDA 1.0** writer, not a claim of general OpenUSD composition or
//! binary USD support. It writes the complete neutral hierarchy, exact local matrices, reusable mesh
//! prototypes referenced by instance prims, metallic/roughness material factors, skeleton metadata and skin
//! primvars, and typed animation time samples. Anything represented only through project namespaced metadata
//! or not packaged by this ASCII writer is recorded in [`FidelityReport`].

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};

use crate::mesh::{MeshAsset, Primitive};
use crate::scene::{
    AnimationChannel, AnimationInterpolation, AnimationTarget, AnimationValue, AnimationValueType,
    Matrix4d, SceneAsset, SceneNodeId, SceneSkin, SceneUpAxis, SCENE_IR_VERSION,
};

/// Default maximum generated ASCII stage size (256 MiB).
pub const MAX_USDA_EXPORT_BYTES: usize = 256 * 1024 * 1024;

/// Production bounds for a single USDA export. Tests and embedding applications can lower them through
/// [`export_usda_with_limits`] without constructing hostile-size fixtures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsdaExportLimits {
    /// Maximum scene nodes.
    pub max_nodes: usize,
    /// Maximum reusable mesh assets.
    pub max_meshes: usize,
    /// Maximum primitives across all mesh assets.
    pub max_primitives: usize,
    /// Maximum materials across all mesh assets.
    pub max_materials: usize,
    /// Maximum decoded textures across all mesh assets (textures are reported, not packaged, in this tier).
    pub max_textures: usize,
    /// Maximum vertices across all primitives.
    pub max_vertices: usize,
    /// Maximum triangle-list indices across all primitives.
    pub max_indices: usize,
    /// Maximum skins.
    pub max_skins: usize,
    /// Maximum joints across all skins.
    pub max_joints: usize,
    /// Maximum named animation groups.
    pub max_animations: usize,
    /// Maximum channels across all animation groups.
    pub max_channels: usize,
    /// Maximum samples across all channels.
    pub max_samples: usize,
    /// Maximum UTF-8 bytes in any identifier segment.
    pub max_identifier_bytes: usize,
    /// Maximum UTF-8 bytes in escaped display/source metadata strings.
    pub max_metadata_bytes: usize,
    /// Maximum generated ASCII bytes.
    pub max_output_bytes: usize,
}

impl Default for UsdaExportLimits {
    fn default() -> Self {
        Self {
            max_nodes: 100_000,
            max_meshes: 16_384,
            max_primitives: 65_536,
            max_materials: 65_536,
            max_textures: 65_536,
            max_vertices: 8_000_000,
            max_indices: 8_000_000,
            max_skins: 4_096,
            max_joints: 262_144,
            max_animations: 4_096,
            max_channels: 100_000,
            max_samples: 1_000_000,
            max_identifier_bytes: 255,
            max_metadata_bytes: 16 * 1024,
            max_output_bytes: MAX_USDA_EXPORT_BYTES,
        }
    }
}

/// How one feature survived export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FidelityStatus {
    /// Authored through a standard USDA representation without loss.
    Preserved,
    /// Values are retained, but through a project-owned metadata representation or a declared conversion.
    Converted,
    /// Source data is not present in the ASCII artifact. The entry explains the recovery path.
    Omitted,
}

/// One structured fidelity fact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FidelityEntry {
    /// Outcome classification.
    pub status: FidelityStatus,
    /// Stable feature key for UI/tests.
    pub feature: String,
    /// Number of affected source objects.
    pub count: usize,
    /// Human-readable scope and recovery information.
    pub detail: String,
}

/// Explicit export ledger. Consumers should surface `Converted` and `Omitted` entries in preflight/result UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FidelityReport {
    /// Exact writer scope, intentionally excluding USDC/USDZ and full composition claims.
    pub format: String,
    /// Neutral scene version consumed by the writer.
    pub scene_version: u32,
    /// Deterministically ordered fidelity facts.
    pub entries: Vec<FidelityEntry>,
}

impl FidelityReport {
    /// True when no source feature was omitted.
    #[must_use]
    pub fn is_lossless(&self) -> bool {
        self.entries
            .iter()
            .all(|entry| entry.status != FidelityStatus::Omitted)
    }
}

/// Generated ASCII stage plus its fidelity ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsdaExport {
    /// UTF-8 USDA 1.0 document.
    pub text: String,
    /// Structured account of preserved, converted, and omitted source data.
    pub report: FidelityReport,
}

/// A rejected scene/export. Errors are project-owned and safe to surface directly in the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UsdaExportError {
    /// The caller supplied a neutral-scene version this writer cannot interpret.
    UnsupportedSceneVersion { found: u32, supported: u32 },
    /// A name that would become a path/property segment is not a safe USD identifier.
    UnsafeName { kind: &'static str, name: String },
    /// A configured count/output budget was exceeded.
    BudgetExceeded {
        kind: &'static str,
        count: usize,
        limit: usize,
    },
    /// Node identity, parentage, or sibling naming is invalid.
    InvalidHierarchy(String),
    /// A mesh/skin/node reference does not resolve.
    InvalidReference(String),
    /// Geometry or material data is internally inconsistent.
    InvalidMesh { mesh: usize, detail: String },
    /// A matrix, numeric factor, time, or animation payload is not finite.
    NonFinite(String),
    /// A typed channel is malformed or ambiguous.
    InvalidAnimation(String),
}

impl std::fmt::Display for UsdaExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedSceneVersion { found, supported } => write!(
                f,
                "unsupported neutral scene version {found}; this writer supports version {supported}"
            ),
            Self::UnsafeName { kind, name } => {
                write!(f, "unsafe USD {kind} identifier {name:?}")
            }
            Self::BudgetExceeded { kind, count, limit } => {
                write!(f, "USDA {kind} budget exceeded: {count} (limit {limit})")
            }
            Self::InvalidHierarchy(detail) => write!(f, "invalid scene hierarchy: {detail}"),
            Self::InvalidReference(detail) => write!(f, "invalid scene reference: {detail}"),
            Self::InvalidMesh { mesh, detail } => {
                write!(f, "invalid scene mesh {mesh}: {detail}")
            }
            Self::NonFinite(detail) => write!(f, "non-finite scene value: {detail}"),
            Self::InvalidAnimation(detail) => write!(f, "invalid animation: {detail}"),
        }
    }
}

impl std::error::Error for UsdaExportError {}

struct ValidatedScene {
    node_indices: BTreeMap<SceneNodeId, usize>,
    children: BTreeMap<Option<SceneNodeId>, Vec<SceneNodeId>>,
    node_paths: BTreeMap<SceneNodeId, String>,
    joint_paths: Vec<Vec<String>>,
    matrix_channels: BTreeMap<SceneNodeId, (usize, usize)>,
    time_range: Option<(f64, f64)>,
    primitive_count: usize,
    material_count: usize,
    texture_count: usize,
    joint_count: usize,
    channel_count: usize,
    sample_count: usize,
    property_channel_count: usize,
    converted_matrix_interpolation_count: usize,
    skinned_primitive_count: usize,
    mesh_instance_count: usize,
}

/// Export using production limits.
pub fn export_usda(scene: &SceneAsset) -> Result<UsdaExport, UsdaExportError> {
    export_usda_with_limits(scene, UsdaExportLimits::default())
}

/// Export using explicit limits. This is the same production writer; only its safety budgets differ.
pub fn export_usda_with_limits(
    scene: &SceneAsset,
    limits: UsdaExportLimits,
) -> Result<UsdaExport, UsdaExportError> {
    let validated = validate_scene(scene, limits)?;
    preflight_output_estimate(scene, &validated, limits)?;
    let mut text = String::new();
    write_stage(&mut text, scene, &validated);
    if text.len() > limits.max_output_bytes {
        return Err(UsdaExportError::BudgetExceeded {
            kind: "output bytes",
            count: text.len(),
            limit: limits.max_output_bytes,
        });
    }
    Ok(UsdaExport {
        report: fidelity_report(scene, &validated),
        text,
    })
}

// The preflight is intentionally one readable top-to-bottom gate over hierarchy, payloads, bindings, and
// animation. Splitting it into stateful partial validators would make the cross-reference invariants harder
// to audit (and the function does no encoding work).
#[allow(clippy::too_many_lines)]
fn validate_scene(
    scene: &SceneAsset,
    limits: UsdaExportLimits,
) -> Result<ValidatedScene, UsdaExportError> {
    if scene.version != SCENE_IR_VERSION {
        return Err(UsdaExportError::UnsupportedSceneVersion {
            found: scene.version,
            supported: SCENE_IR_VERSION,
        });
    }
    validate_metadata("scene display name", &scene.name, limits.max_metadata_bytes)?;
    if !scene.meters_per_unit.is_finite() || scene.meters_per_unit <= 0.0 {
        return Err(UsdaExportError::NonFinite(
            "meters_per_unit must be finite and greater than zero".into(),
        ));
    }
    if !scene.time_codes_per_second.is_finite() || scene.time_codes_per_second <= 0.0 {
        return Err(UsdaExportError::NonFinite(
            "time_codes_per_second must be finite and greater than zero".into(),
        ));
    }
    guard("nodes", scene.nodes.len(), limits.max_nodes)?;
    guard("meshes", scene.meshes.len(), limits.max_meshes)?;
    guard("skins", scene.skins.len(), limits.max_skins)?;
    guard(
        "animation groups",
        scene.animations.len(),
        limits.max_animations,
    )?;

    let mut node_indices = BTreeMap::new();
    for (index, node) in scene.nodes.iter().enumerate() {
        validate_identifier("node", &node.name, limits.max_identifier_bytes)?;
        if !node.local_transform.is_finite() {
            return Err(UsdaExportError::NonFinite(format!(
                "node {} local transform",
                node.id.0
            )));
        }
        if node_indices.insert(node.id, index).is_some() {
            return Err(UsdaExportError::InvalidHierarchy(format!(
                "duplicate node id {}",
                node.id.0
            )));
        }
    }

    let mut children: BTreeMap<Option<SceneNodeId>, Vec<SceneNodeId>> = BTreeMap::new();
    let mut sibling_names = BTreeSet::new();
    for node in &scene.nodes {
        if let Some(parent) = node.parent {
            if parent == node.id {
                return Err(UsdaExportError::InvalidHierarchy(format!(
                    "node {} is its own parent",
                    node.id.0
                )));
            }
            if !node_indices.contains_key(&parent) {
                return Err(UsdaExportError::InvalidHierarchy(format!(
                    "node {} references missing parent {}",
                    node.id.0, parent.0
                )));
            }
        }
        if !sibling_names.insert((node.parent, node.name.clone())) {
            return Err(UsdaExportError::InvalidHierarchy(format!(
                "duplicate sibling name {:?}",
                node.name
            )));
        }
        children.entry(node.parent).or_default().push(node.id);
    }
    for child_list in children.values_mut() {
        child_list.sort_unstable();
    }
    validate_acyclic(scene, &node_indices)?;

    let node_paths = build_node_paths(scene, &node_indices)?;

    let mut primitive_count = 0usize;
    let mut material_count = 0usize;
    let mut texture_count = 0usize;
    let mut vertex_count = 0usize;
    let mut index_count = 0usize;
    let mut skinned_primitive_count = 0usize;
    for (mesh_index, mesh) in scene.meshes.iter().enumerate() {
        validate_metadata("mesh source name", &mesh.name, limits.max_metadata_bytes)?;
        validate_mesh(mesh_index, mesh)?;
        primitive_count = checked_add("primitives", primitive_count, mesh.primitives.len())?;
        material_count = checked_add("materials", material_count, mesh.materials.len())?;
        texture_count = checked_add("textures", texture_count, mesh.textures.len())?;
        for primitive in &mesh.primitives {
            vertex_count = checked_add("vertices", vertex_count, primitive.positions.len())?;
            index_count = checked_add("indices", index_count, primitive.indices.len())?;
            if !primitive.joints.is_empty() {
                skinned_primitive_count += 1;
            }
        }
    }
    guard("primitives", primitive_count, limits.max_primitives)?;
    guard("materials", material_count, limits.max_materials)?;
    guard("textures", texture_count, limits.max_textures)?;
    guard("vertices", vertex_count, limits.max_vertices)?;
    guard("indices", index_count, limits.max_indices)?;

    let mut joint_count = 0usize;
    let mut joint_paths = Vec::with_capacity(scene.skins.len());
    for (skin_index, skin) in scene.skins.iter().enumerate() {
        let paths = validate_skin(
            skin_index,
            skin,
            &node_indices,
            limits.max_identifier_bytes,
            limits.max_metadata_bytes,
        )?;
        joint_count = checked_add("joints", joint_count, skin.joints.len())?;
        joint_paths.push(paths);
    }
    guard("joints", joint_count, limits.max_joints)?;

    let mut mesh_instance_count = 0usize;
    let mut mesh_has_skin_binding = vec![false; scene.meshes.len()];
    for node in &scene.nodes {
        let mesh = if let Some(reference) = node.mesh {
            mesh_instance_count += 1;
            scene.meshes.get(reference.0).ok_or_else(|| {
                UsdaExportError::InvalidReference(format!(
                    "node {} references missing mesh {}",
                    node.id.0, reference.0
                ))
            })?
        } else {
            if node.skin.is_some() {
                return Err(UsdaExportError::InvalidReference(format!(
                    "node {} binds a skin without a mesh",
                    node.id.0
                )));
            }
            continue;
        };
        let carries_skin = mesh.skeleton.is_some()
            || mesh
                .primitives
                .iter()
                .any(|primitive| !primitive.joints.is_empty() || !primitive.weights.is_empty());
        if carries_skin && node.skin.is_none() {
            return Err(UsdaExportError::InvalidReference(format!(
                "node {} instances skinned mesh {} without a SceneSkin binding",
                node.id.0,
                node.mesh.expect("mesh matched above").0
            )));
        }
        if let Some(skin_reference) = node.skin {
            let skin = scene.skins.get(skin_reference.0).ok_or_else(|| {
                UsdaExportError::InvalidReference(format!(
                    "node {} references missing skin {}",
                    node.id.0, skin_reference.0
                ))
            })?;
            mesh_has_skin_binding[node.mesh.expect("mesh matched above").0] = true;
            validate_mesh_skin_binding(node.id, mesh, skin)?;
        }
    }
    for (mesh_index, mesh) in scene.meshes.iter().enumerate() {
        let carries_skin = mesh.skeleton.is_some()
            || mesh
                .primitives
                .iter()
                .any(|primitive| !primitive.joints.is_empty() || !primitive.weights.is_empty());
        if carries_skin && !mesh_has_skin_binding[mesh_index] {
            return Err(UsdaExportError::InvalidReference(format!(
                "skinned mesh {mesh_index} has no node-to-SceneSkin binding"
            )));
        }
    }

    let mut matrix_channels = BTreeMap::new();
    let mut channel_count = 0usize;
    let mut sample_count = 0usize;
    let mut property_channel_count = 0usize;
    let mut converted_matrix_interpolation_count = 0usize;
    let mut time_range: Option<(f64, f64)> = None;
    for (animation_index, animation) in scene.animations.iter().enumerate() {
        validate_metadata(
            "animation display name",
            &animation.name,
            limits.max_metadata_bytes,
        )?;
        channel_count = checked_add("channels", channel_count, animation.channels.len())?;
        for (channel_index, channel) in animation.channels.iter().enumerate() {
            validate_channel(
                animation_index,
                channel_index,
                channel,
                &node_indices,
                limits.max_identifier_bytes,
            )?;
            sample_count = checked_add("samples", sample_count, channel.samples.len())?;
            if let (Some(first), Some(last)) = (channel.samples.first(), channel.samples.last()) {
                time_range = Some(match time_range {
                    Some((start, end)) => (start.min(first.time_code), end.max(last.time_code)),
                    None => (first.time_code, last.time_code),
                });
            }
            match channel.target {
                AnimationTarget::NodeLocalTransform(node) => {
                    if matrix_channels
                        .insert(node, (animation_index, channel_index))
                        .is_some()
                    {
                        return Err(UsdaExportError::InvalidAnimation(format!(
                            "more than one local-transform channel targets node {}",
                            node.0
                        )));
                    }
                    if channel.interpolation != AnimationInterpolation::Linear {
                        converted_matrix_interpolation_count += 1;
                    }
                }
                AnimationTarget::NodeProperty { .. } => property_channel_count += 1,
            }
        }
    }
    guard("channels", channel_count, limits.max_channels)?;
    guard("samples", sample_count, limits.max_samples)?;

    Ok(ValidatedScene {
        node_indices,
        children,
        node_paths,
        joint_paths,
        matrix_channels,
        time_range,
        primitive_count,
        material_count,
        texture_count,
        joint_count,
        channel_count,
        sample_count,
        property_channel_count,
        converted_matrix_interpolation_count,
        skinned_primitive_count,
        mesh_instance_count,
    })
}

fn validate_identifier(
    kind: &'static str,
    name: &str,
    max_bytes: usize,
) -> Result<(), UsdaExportError> {
    let mut chars = name.chars();
    let first = chars.next();
    let safe = !name.is_empty()
        && name.len() <= max_bytes
        && first.is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric());
    if safe {
        Ok(())
    } else {
        Err(UsdaExportError::UnsafeName {
            kind,
            name: name.to_string(),
        })
    }
}

fn validate_metadata(
    kind: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), UsdaExportError> {
    if value.len() <= max_bytes {
        Ok(())
    } else {
        Err(UsdaExportError::BudgetExceeded {
            kind,
            count: value.len(),
            limit: max_bytes,
        })
    }
}

fn validate_acyclic(
    scene: &SceneAsset,
    indices: &BTreeMap<SceneNodeId, usize>,
) -> Result<(), UsdaExportError> {
    let mut marks: BTreeMap<SceneNodeId, u8> = BTreeMap::new();
    for node in &scene.nodes {
        if marks.get(&node.id) == Some(&2) {
            continue;
        }
        let mut chain = Vec::new();
        let mut current = Some(node.id);
        while let Some(id) = current {
            match marks.get(&id).copied() {
                Some(1) => {
                    return Err(UsdaExportError::InvalidHierarchy(format!(
                        "cycle reaches node {}",
                        id.0
                    )));
                }
                Some(2) => break,
                _ => {
                    marks.insert(id, 1);
                    chain.push(id);
                    current = scene.nodes[*indices.get(&id).expect("validated node id")].parent;
                }
            }
        }
        for id in chain {
            marks.insert(id, 2);
        }
    }
    Ok(())
}

fn build_node_paths(
    scene: &SceneAsset,
    indices: &BTreeMap<SceneNodeId, usize>,
) -> Result<BTreeMap<SceneNodeId, String>, UsdaExportError> {
    let mut paths = BTreeMap::new();
    for node in &scene.nodes {
        let mut segments = Vec::new();
        let mut current = Some(node.id);
        while let Some(id) = current {
            let entry = &scene.nodes[*indices.get(&id).ok_or_else(|| {
                UsdaExportError::InvalidHierarchy(format!("missing node {}", id.0))
            })?];
            segments.push(entry.name.as_str());
            current = entry.parent;
        }
        segments.reverse();
        paths.insert(node.id, format!("/World/{}", segments.join("/")));
    }
    Ok(paths)
}

fn validate_mesh(mesh_index: usize, mesh: &MeshAsset) -> Result<(), UsdaExportError> {
    if mesh.primitives.is_empty() {
        return Err(invalid_mesh(mesh_index, "mesh has no primitives"));
    }
    if mesh.materials.is_empty() {
        return Err(invalid_mesh(mesh_index, "mesh has no materials"));
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
                    "material {material_index} factors must be finite and between zero and one"
                ),
            ));
        }
        for texture in [
            material.base_color_texture,
            material.metallic_roughness_texture,
            material.normal_texture,
            material.occlusion_texture,
            material.curvature_texture,
        ]
        .into_iter()
        .flatten()
        {
            if texture >= mesh.textures.len() {
                return Err(invalid_mesh(
                    mesh_index,
                    format!("material {material_index} references missing texture {texture}"),
                ));
            }
        }
    }
    for (texture_index, texture) in mesh.textures.iter().enumerate() {
        let expected = usize::try_from(texture.width)
            .ok()
            .and_then(|width| {
                usize::try_from(texture.height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|texels| texels.checked_mul(4))
            .ok_or_else(|| {
                invalid_mesh(
                    mesh_index,
                    format!("texture {texture_index} dimensions overflow"),
                )
            })?;
        if texture.width == 0 || texture.height == 0 || texture.rgba8.len() != expected {
            return Err(invalid_mesh(
                mesh_index,
                format!("texture {texture_index} has invalid dimensions or RGBA byte length"),
            ));
        }
    }
    for (primitive_index, primitive) in mesh.primitives.iter().enumerate() {
        validate_primitive(mesh_index, primitive_index, primitive, mesh.materials.len())?;
    }
    Ok(())
}

fn validate_primitive(
    mesh_index: usize,
    primitive_index: usize,
    primitive: &Primitive,
    materials: usize,
) -> Result<(), UsdaExportError> {
    let fail =
        |detail: &str| invalid_mesh(mesh_index, format!("primitive {primitive_index}: {detail}"));
    if primitive.positions.is_empty() {
        return Err(fail("positions are empty"));
    }
    if primitive.indices.is_empty() || !primitive.indices.len().is_multiple_of(3) {
        return Err(fail("indices must be a non-empty triangle list"));
    }
    if primitive
        .indices
        .iter()
        .any(|&index| index as usize >= primitive.positions.len())
    {
        return Err(fail("an index is out of range"));
    }
    if primitive.material >= materials {
        return Err(fail("material index is out of range"));
    }
    if !primitive.normals.is_empty() && primitive.normals.len() != primitive.positions.len() {
        return Err(fail("normal count does not match positions"));
    }
    if !primitive.uvs.is_empty() && primitive.uvs.len() != primitive.positions.len() {
        return Err(fail("UV count does not match positions"));
    }
    let has_joints = !primitive.joints.is_empty();
    let has_weights = !primitive.weights.is_empty();
    if has_joints != has_weights
        || (has_joints
            && (primitive.joints.len() != primitive.positions.len()
                || primitive.weights.len() != primitive.positions.len()))
    {
        return Err(fail(
            "joint and weight streams must both match the position count",
        ));
    }
    if primitive
        .positions
        .iter()
        .flatten()
        .chain(primitive.normals.iter().flatten())
        .chain(primitive.uvs.iter().flatten())
        .any(|value| !value.is_finite())
    {
        return Err(UsdaExportError::NonFinite(format!(
            "mesh {mesh_index} primitive {primitive_index} vertex data"
        )));
    }
    if primitive
        .weights
        .iter()
        .flatten()
        .any(|weight| !weight.is_finite() || *weight < 0.0)
    {
        return Err(UsdaExportError::NonFinite(format!(
            "mesh {mesh_index} primitive {primitive_index} skin weights"
        )));
    }
    if primitive.weights.iter().any(|weights| {
        let sum = weights.iter().sum::<f32>();
        (sum - 1.0).abs() > 1.0e-4
    }) {
        return Err(fail("skin weights must sum to one per vertex"));
    }
    Ok(())
}

fn validate_skin(
    skin_index: usize,
    skin: &SceneSkin,
    nodes: &BTreeMap<SceneNodeId, usize>,
    max_identifier_bytes: usize,
    max_metadata_bytes: usize,
) -> Result<Vec<String>, UsdaExportError> {
    validate_metadata("skin display name", &skin.name, max_metadata_bytes)?;
    if skin.joints.is_empty() {
        return Err(UsdaExportError::InvalidReference(format!(
            "skin {skin_index} has no joints"
        )));
    }
    if let Some(root) = skin.skeleton_root {
        if !nodes.contains_key(&root) {
            return Err(UsdaExportError::InvalidReference(format!(
                "skin {skin_index} references missing skeleton root {}",
                root.0
            )));
        }
    }
    let mut joint_nodes = BTreeSet::new();
    let mut sibling_names = BTreeSet::new();
    let mut paths: Vec<String> = Vec::with_capacity(skin.joints.len());
    for (joint_index, joint) in skin.joints.iter().enumerate() {
        validate_identifier("joint", &joint.name, max_identifier_bytes)?;
        if !nodes.contains_key(&joint.node) {
            return Err(UsdaExportError::InvalidReference(format!(
                "skin {skin_index} joint {joint_index} references missing node {}",
                joint.node.0
            )));
        }
        if !joint_nodes.insert(joint.node) {
            return Err(UsdaExportError::InvalidReference(format!(
                "skin {skin_index} repeats joint node {}",
                joint.node.0
            )));
        }
        if let Some(parent) = joint.parent {
            if parent >= joint_index {
                return Err(UsdaExportError::InvalidReference(format!(
                    "skin {skin_index} joint {joint_index} parent {parent} must precede it"
                )));
            }
        }
        if !sibling_names.insert((joint.parent, joint.name.clone())) {
            return Err(UsdaExportError::InvalidReference(format!(
                "skin {skin_index} has duplicate sibling joint {:?}",
                joint.name
            )));
        }
        for (label, matrix) in [
            ("rest", joint.rest_local_transform),
            ("bind", joint.bind_transform),
            ("inverse bind", joint.inverse_bind_matrix),
        ] {
            if !matrix.is_finite() {
                return Err(UsdaExportError::NonFinite(format!(
                    "skin {skin_index} joint {joint_index} {label} matrix"
                )));
            }
        }
        let path = joint.parent.map_or_else(
            || joint.name.clone(),
            |parent| format!("{}/{}", paths[parent], joint.name),
        );
        paths.push(path);
    }
    Ok(paths)
}

fn validate_mesh_skin_binding(
    node: SceneNodeId,
    mesh: &MeshAsset,
    skin: &SceneSkin,
) -> Result<(), UsdaExportError> {
    if let Some(skeleton) = &mesh.skeleton {
        if skeleton.joints.len() != skin.joints.len() {
            return Err(UsdaExportError::InvalidReference(format!(
                "node {} binds a {}-joint SceneSkin to a {}-joint MeshAsset skeleton",
                node.0,
                skin.joints.len(),
                skeleton.joints.len()
            )));
        }
    }
    for primitive in &mesh.primitives {
        if let Some(max_joint) = primitive.joints.iter().flatten().max() {
            if usize::from(*max_joint) >= skin.joints.len() {
                return Err(UsdaExportError::InvalidReference(format!(
                    "node {} has skin index {} but its SceneSkin has {} joints",
                    node.0,
                    max_joint,
                    skin.joints.len()
                )));
            }
        }
    }
    Ok(())
}

fn validate_channel(
    animation_index: usize,
    channel_index: usize,
    channel: &AnimationChannel,
    nodes: &BTreeMap<SceneNodeId, usize>,
    max_identifier_bytes: usize,
) -> Result<(), UsdaExportError> {
    let target_node = match &channel.target {
        AnimationTarget::NodeLocalTransform(node) => {
            if channel.value_type != AnimationValueType::Matrix4d {
                return Err(UsdaExportError::InvalidAnimation(format!(
                    "animation {animation_index} channel {channel_index} local transform is not matrix4d"
                )));
            }
            *node
        }
        AnimationTarget::NodeProperty {
            node,
            component,
            field,
        } => {
            validate_identifier("animation component", component, max_identifier_bytes)?;
            validate_identifier("animation field", field, max_identifier_bytes)?;
            *node
        }
    };
    if !nodes.contains_key(&target_node) {
        return Err(UsdaExportError::InvalidAnimation(format!(
            "animation {animation_index} channel {channel_index} targets missing node {}",
            target_node.0
        )));
    }
    if channel.samples.is_empty() {
        return Err(UsdaExportError::InvalidAnimation(format!(
            "animation {animation_index} channel {channel_index} has no samples"
        )));
    }
    if matches!(
        channel.value_type,
        AnimationValueType::Integer | AnimationValueType::Boolean
    ) && channel.interpolation != AnimationInterpolation::Step
    {
        return Err(UsdaExportError::InvalidAnimation(format!(
            "animation {animation_index} channel {channel_index} discrete values require step interpolation"
        )));
    }
    let mut previous = None;
    for (sample_index, sample) in channel.samples.iter().enumerate() {
        if !sample.time_code.is_finite() {
            return Err(UsdaExportError::NonFinite(format!(
                "animation {animation_index} channel {channel_index} sample {sample_index} time"
            )));
        }
        if previous.is_some_and(|time| sample.time_code <= time) {
            return Err(UsdaExportError::InvalidAnimation(format!(
                "animation {animation_index} channel {channel_index} sample times are not strictly increasing"
            )));
        }
        if sample.value.value_type() != channel.value_type {
            return Err(UsdaExportError::InvalidAnimation(format!(
                "animation {animation_index} channel {channel_index} sample {sample_index} type does not match its declaration"
            )));
        }
        if !sample.value.is_finite() {
            return Err(UsdaExportError::NonFinite(format!(
                "animation {animation_index} channel {channel_index} sample {sample_index} value"
            )));
        }
        previous = Some(sample.time_code);
    }
    Ok(())
}

fn guard(kind: &'static str, count: usize, limit: usize) -> Result<(), UsdaExportError> {
    if count > limit {
        Err(UsdaExportError::BudgetExceeded { kind, count, limit })
    } else {
        Ok(())
    }
}

fn checked_add(kind: &'static str, left: usize, right: usize) -> Result<usize, UsdaExportError> {
    left.checked_add(right)
        .ok_or(UsdaExportError::BudgetExceeded {
            kind,
            count: usize::MAX,
            limit: usize::MAX - 1,
        })
}

fn invalid_mesh(mesh: usize, detail: impl Into<String>) -> UsdaExportError {
    UsdaExportError::InvalidMesh {
        mesh,
        detail: detail.into(),
    }
}

fn preflight_output_estimate(
    scene: &SceneAsset,
    validated: &ValidatedScene,
    limits: UsdaExportLimits,
) -> Result<(), UsdaExportError> {
    let vertices = scene
        .meshes
        .iter()
        .flat_map(|mesh| &mesh.primitives)
        .map(|primitive| primitive.positions.len())
        .sum::<usize>();
    let indices = scene
        .meshes
        .iter()
        .flat_map(|mesh| &mesh.primitives)
        .map(|primitive| primitive.indices.len())
        .sum::<usize>();
    // Conservative ASCII bound: enough for full-precision tuples, structural declarations, and metadata.
    let estimate = vertices
        .checked_mul(384)
        .and_then(|value| value.checked_add(indices.saturating_mul(24)))
        .and_then(|value| value.checked_add(validated.sample_count.saturating_mul(640)))
        .and_then(|value| value.checked_add(validated.joint_count.saturating_mul(1_024)))
        .and_then(|value| value.checked_add(scene.nodes.len().saturating_mul(2_048)))
        .and_then(|value| value.checked_add(validated.primitive_count.saturating_mul(4_096)))
        .and_then(|value| value.checked_add(validated.material_count.saturating_mul(2_048)))
        .and_then(|value| value.checked_add(16_384))
        .ok_or(UsdaExportError::BudgetExceeded {
            kind: "estimated output bytes",
            count: usize::MAX,
            limit: limits.max_output_bytes,
        })?;
    if estimate > limits.max_output_bytes {
        return Err(UsdaExportError::BudgetExceeded {
            kind: "estimated output bytes",
            count: estimate,
            limit: limits.max_output_bytes,
        });
    }
    Ok(())
}

fn fidelity_report(scene: &SceneAsset, validated: &ValidatedScene) -> FidelityReport {
    let mut entries = vec![
        FidelityEntry {
            status: FidelityStatus::Preserved,
            feature: "hierarchy_and_exact_local_matrices".into(),
            count: scene.nodes.len(),
            detail: "Node parentage and row-major 4x4 local transforms are authored as nested UsdGeom Xforms."
                .into(),
        },
        FidelityEntry {
            status: FidelityStatus::Preserved,
            feature: "mesh_prototypes_and_instances".into(),
            count: validated.mesh_instance_count,
            detail: format!(
                "{} reusable mesh prototype(s) are shared through internal USDA references; geometry is not duplicated per node.",
                scene.meshes.len()
            ),
        },
        FidelityEntry {
            status: FidelityStatus::Preserved,
            feature: "pbr_material_factors".into(),
            count: validated.material_count,
            detail: "Base color, opacity, metallic, and roughness are authored with UsdPreviewSurface."
                .into(),
        },
        FidelityEntry {
            status: FidelityStatus::Preserved,
            feature: "skeleton_joint_metadata".into(),
            count: validated.joint_count,
            detail: "Joint paths, rest/bind transforms, inverse-bind metadata, and node identities are authored in SkelSkeleton prims."
                .into(),
        },
        FidelityEntry {
            status: FidelityStatus::Preserved,
            feature: "typed_animation_samples".into(),
            count: validated.sample_count,
            detail: format!(
                "{} typed channel(s) retain deterministic stage time samples; exact node-matrix channels also drive standard Xform time samples.",
                validated.channel_count
            ),
        },
    ];
    if validated.property_channel_count > 0 {
        entries.push(FidelityEntry {
            status: FidelityStatus::Converted,
            feature: "project_property_animation".into(),
            count: validated.property_channel_count,
            detail: "Non-Xform property channels are retained as typed metrocalk:value time samples because USDA has no engine-registry PropertyPath schema."
                .into(),
        });
    }
    if validated.converted_matrix_interpolation_count > 0 {
        entries.push(FidelityEntry {
            status: FidelityStatus::Converted,
            feature: "matrix_channel_interpolation".into(),
            count: validated.converted_matrix_interpolation_count,
            detail: "Step/cubic intent is retained as channel metadata; standard USD matrix time samples use the consuming stage's interpolation policy."
                .into(),
        });
    }
    if validated.skinned_primitive_count > 0 {
        entries.push(FidelityEntry {
            status: FidelityStatus::Preserved,
            feature: "linear_skin_primvars".into(),
            count: validated.skinned_primitive_count,
            detail: "Four-wide joint indices and weights are authored as SkelBinding primvars on mesh prototypes."
                .into(),
        });
    }
    if validated.texture_count > 0 {
        entries.push(FidelityEntry {
            status: FidelityStatus::Omitted,
            feature: "embedded_rgba_textures".into(),
            count: validated.texture_count,
            detail: "This single-file ASCII writer does not package decoded RGBA textures. Material factors remain; use a future asset-resolver/package tier for texture files."
                .into(),
        });
    }
    FidelityReport {
        format: "USDA 1.0 ASCII (bounded scene subset; not USDC/USDZ or full OpenUSD composition)"
            .into(),
        scene_version: scene.version,
        entries,
    }
}

fn write_stage(out: &mut String, scene: &SceneAsset, validated: &ValidatedScene) {
    line(out, 0, format_args!("#usda 1.0"));
    line(out, 0, format_args!("("));
    line(out, 4, format_args!("defaultPrim = \"World\""));
    line(
        out,
        4,
        format_args!("metersPerUnit = {}", number64(scene.meters_per_unit)),
    );
    line(
        out,
        4,
        format_args!(
            "upAxis = \"{}\"",
            match scene.up_axis {
                SceneUpAxis::Y => "Y",
                SceneUpAxis::Z => "Z",
            }
        ),
    );
    line(
        out,
        4,
        format_args!(
            "framesPerSecond = {}",
            number64(scene.time_codes_per_second)
        ),
    );
    line(
        out,
        4,
        format_args!(
            "timeCodesPerSecond = {}",
            number64(scene.time_codes_per_second)
        ),
    );
    if let Some((start, end)) = validated.time_range {
        line(out, 4, format_args!("startTimeCode = {}", number64(start)));
        line(out, 4, format_args!("endTimeCode = {}", number64(end)));
    }
    line(out, 0, format_args!(")"));
    line(out, 0, format_args!(""));
    line(out, 0, format_args!("def Xform \"World\""));
    line(out, 0, format_args!("{{"));
    line(
        out,
        4,
        format_args!(
            "custom string metrocalk:sceneName = {}",
            usd_string(&scene.name)
        ),
    );
    line(
        out,
        4,
        format_args!("custom int metrocalk:sceneIrVersion = {}", scene.version),
    );

    write_materials(out, scene);
    write_prototypes(out, scene);
    write_skeletons(out, scene, validated);
    write_animation_metadata(out, scene, validated);
    write_nodes(out, scene, validated);
    line(out, 0, format_args!("}}"));
}

fn write_materials(out: &mut String, scene: &SceneAsset) {
    line(out, 4, format_args!("def Scope \"_Materials\""));
    line(out, 4, format_args!("{{"));
    for (mesh_index, mesh) in scene.meshes.iter().enumerate() {
        for (material_index, material) in mesh.materials.iter().enumerate() {
            let name = format!("Mesh_{mesh_index}_Material_{material_index}");
            line(out, 8, format_args!("def Material \"{name}\""));
            line(out, 8, format_args!("{{"));
            line(
                out,
                12,
                format_args!(
                    "token outputs:surface.connect = </World/_Materials/{name}/PreviewSurface.outputs:surface>"
                ),
            );
            line(out, 12, format_args!("def Shader \"PreviewSurface\""));
            line(out, 12, format_args!("{{"));
            line(
                out,
                16,
                format_args!("uniform token info:id = \"UsdPreviewSurface\""),
            );
            line(
                out,
                16,
                format_args!(
                    "color3f inputs:diffuseColor = {}",
                    vec3f([
                        material.base_color[0],
                        material.base_color[1],
                        material.base_color[2]
                    ])
                ),
            );
            line(
                out,
                16,
                format_args!(
                    "float inputs:opacity = {}",
                    number32(material.base_color[3])
                ),
            );
            line(
                out,
                16,
                format_args!("float inputs:metallic = {}", number32(material.metallic)),
            );
            line(
                out,
                16,
                format_args!("float inputs:roughness = {}", number32(material.roughness)),
            );
            line(out, 16, format_args!("token outputs:surface"));
            line(out, 12, format_args!("}}"));
            line(out, 8, format_args!("}}"));
        }
    }
    line(out, 4, format_args!("}}"));
}

fn write_prototypes(out: &mut String, scene: &SceneAsset) {
    line(out, 4, format_args!("def Scope \"_Prototypes\""));
    line(out, 4, format_args!("{{"));
    for (mesh_index, mesh) in scene.meshes.iter().enumerate() {
        line(out, 8, format_args!("def Xform \"Mesh_{mesh_index}\""));
        line(out, 8, format_args!("{{"));
        line(
            out,
            12,
            format_args!(
                "custom string metrocalk:sourceName = {}",
                usd_string(&mesh.name)
            ),
        );
        for (primitive_index, primitive) in mesh.primitives.iter().enumerate() {
            write_primitive(out, mesh_index, primitive_index, primitive);
        }
        line(out, 8, format_args!("}}"));
    }
    line(out, 4, format_args!("}}"));
}

fn write_primitive(out: &mut String, mesh: usize, index: usize, primitive: &Primitive) {
    line(out, 12, format_args!("def Mesh \"Primitive_{index}\""));
    line(out, 12, format_args!("{{"));
    line(
        out,
        16,
        format_args!("uniform token subdivisionScheme = \"none\""),
    );
    line(out, 16, format_args!("point3f[] points = ["));
    for point in &primitive.positions {
        line(out, 20, format_args!("{},", vec3f(*point)));
    }
    line(out, 16, format_args!("]"));
    line(out, 16, format_args!("int[] faceVertexCounts = ["));
    for _ in primitive.indices.as_chunks::<3>().0 {
        line(out, 20, format_args!("3,"));
    }
    line(out, 16, format_args!("]"));
    line(out, 16, format_args!("int[] faceVertexIndices = ["));
    for index in &primitive.indices {
        line(out, 20, format_args!("{index},"));
    }
    line(out, 16, format_args!("]"));
    if !primitive.normals.is_empty() {
        line(out, 16, format_args!("normal3f[] normals = ["));
        for normal in &primitive.normals {
            line(out, 20, format_args!("{},", vec3f(*normal)));
        }
        line(out, 16, format_args!("]"));
        line(
            out,
            16,
            format_args!("uniform token normalsInterpolation = \"vertex\""),
        );
    }
    if !primitive.uvs.is_empty() {
        line(out, 16, format_args!("texCoord2f[] primvars:st = ["));
        for uv in &primitive.uvs {
            line(out, 20, format_args!("{},", vec2f(*uv)));
        }
        line(out, 16, format_args!("]"));
        line(
            out,
            16,
            format_args!("uniform token primvars:st:interpolation = \"vertex\""),
        );
    }
    if !primitive.joints.is_empty() {
        line(
            out,
            16,
            format_args!("int[] primvars:skel:jointIndices = ["),
        );
        for joints in &primitive.joints {
            for joint in joints {
                line(out, 20, format_args!("{joint},"));
            }
        }
        line(out, 16, format_args!("]"));
        line(
            out,
            16,
            format_args!("int primvars:skel:jointIndices:elementSize = 4"),
        );
        line(
            out,
            16,
            format_args!("uniform token primvars:skel:jointIndices:interpolation = \"vertex\""),
        );
        line(
            out,
            16,
            format_args!("float[] primvars:skel:jointWeights = ["),
        );
        for weights in &primitive.weights {
            for weight in weights {
                line(out, 20, format_args!("{},", number32(*weight)));
            }
        }
        line(out, 16, format_args!("]"));
        line(
            out,
            16,
            format_args!("int primvars:skel:jointWeights:elementSize = 4"),
        );
        line(
            out,
            16,
            format_args!("uniform token primvars:skel:jointWeights:interpolation = \"vertex\""),
        );
    }
    line(
        out,
        16,
        format_args!(
            "rel material:binding = </World/_Materials/Mesh_{mesh}_Material_{}>",
            primitive.material
        ),
    );
    line(out, 12, format_args!("}}"));
}

fn write_skeletons(out: &mut String, scene: &SceneAsset, validated: &ValidatedScene) {
    line(out, 4, format_args!("def Scope \"_Skeletons\""));
    line(out, 4, format_args!("{{"));
    for (skin_index, skin) in scene.skins.iter().enumerate() {
        line(
            out,
            8,
            format_args!("def SkelSkeleton \"Skin_{skin_index}\""),
        );
        line(out, 8, format_args!("{{"));
        line(
            out,
            12,
            format_args!("custom string metrocalk:name = {}", usd_string(&skin.name)),
        );
        if let Some(root) = skin.skeleton_root {
            line(
                out,
                12,
                format_args!(
                    "custom string metrocalk:skeletonRoot = {}",
                    usd_string(&validated.node_paths[&root])
                ),
            );
        }
        line(out, 12, format_args!("uniform token[] joints = ["));
        for path in &validated.joint_paths[skin_index] {
            line(out, 16, format_args!("{},", usd_string(path)));
        }
        line(out, 12, format_args!("]"));
        line(
            out,
            12,
            format_args!("uniform matrix4d[] restTransforms = ["),
        );
        for joint in &skin.joints {
            line(
                out,
                16,
                format_args!("{},", matrix(joint.rest_local_transform)),
            );
        }
        line(out, 12, format_args!("]"));
        line(
            out,
            12,
            format_args!("uniform matrix4d[] bindTransforms = ["),
        );
        for joint in &skin.joints {
            line(out, 16, format_args!("{},", matrix(joint.bind_transform)));
        }
        line(out, 12, format_args!("]"));
        line(
            out,
            12,
            format_args!("custom matrix4d[] metrocalk:inverseBindMatrices = ["),
        );
        for joint in &skin.joints {
            line(
                out,
                16,
                format_args!("{},", matrix(joint.inverse_bind_matrix)),
            );
        }
        line(out, 12, format_args!("]"));
        line(
            out,
            12,
            format_args!("custom string[] metrocalk:jointNodeIds = ["),
        );
        for joint in &skin.joints {
            line(out, 16, format_args!("\"{}\",", joint.node.0));
        }
        line(out, 12, format_args!("]"));
        line(
            out,
            12,
            format_args!("custom int[] metrocalk:jointParents = ["),
        );
        for joint in &skin.joints {
            let parent = joint
                .parent
                .map_or_else(|| "-1".to_string(), |value| value.to_string());
            line(out, 16, format_args!("{parent},"));
        }
        line(out, 12, format_args!("]"));
        line(out, 8, format_args!("}}"));
    }
    line(out, 4, format_args!("}}"));
}

fn write_animation_metadata(out: &mut String, scene: &SceneAsset, validated: &ValidatedScene) {
    line(out, 4, format_args!("def Scope \"_Animations\""));
    line(out, 4, format_args!("{{"));
    for (animation_index, animation) in scene.animations.iter().enumerate() {
        line(
            out,
            8,
            format_args!("def Scope \"Animation_{animation_index}\""),
        );
        line(out, 8, format_args!("{{"));
        line(
            out,
            12,
            format_args!(
                "custom string metrocalk:name = {}",
                usd_string(&animation.name)
            ),
        );
        for (channel_index, channel) in animation.channels.iter().enumerate() {
            line(
                out,
                12,
                format_args!("def Scope \"Channel_{channel_index}\""),
            );
            line(out, 12, format_args!("{{"));
            let target = animation_target(channel, &validated.node_paths);
            line(
                out,
                16,
                format_args!("custom string metrocalk:target = {}", usd_string(&target)),
            );
            line(
                out,
                16,
                format_args!(
                    "custom token metrocalk:valueType = \"{}\"",
                    value_type_token(channel.value_type)
                ),
            );
            line(
                out,
                16,
                format_args!(
                    "custom token metrocalk:interpolation = \"{}\"",
                    interpolation_token(channel.interpolation)
                ),
            );
            line(
                out,
                16,
                format_args!(
                    "custom {} metrocalk:value.timeSamples = {{",
                    usd_value_type(channel.value_type)
                ),
            );
            for sample in &channel.samples {
                line(
                    out,
                    20,
                    format_args!(
                        "{}: {},",
                        number64(sample.time_code),
                        animation_value(&sample.value)
                    ),
                );
            }
            line(out, 16, format_args!("}}"));
            line(out, 12, format_args!("}}"));
        }
        line(out, 8, format_args!("}}"));
    }
    line(out, 4, format_args!("}}"));
}

// Iterative open/close events avoid recursion and stack overflow for a deeply nested, but valid, scene.
// Keeping the paired event handling together makes the emitted brace/hierarchy order directly auditable.
#[allow(clippy::too_many_lines)]
fn write_nodes(out: &mut String, scene: &SceneAsset, validated: &ValidatedScene) {
    enum Event {
        Open(SceneNodeId, usize),
        Close(usize),
    }
    let mut stack = Vec::new();
    if let Some(roots) = validated.children.get(&None) {
        for root in roots.iter().rev() {
            stack.push(Event::Open(*root, 4));
        }
    }
    while let Some(event) = stack.pop() {
        match event {
            Event::Close(indent) => line(out, indent, format_args!("}}")),
            Event::Open(id, indent) => {
                let node = &scene.nodes[validated.node_indices[&id]];
                line(out, indent, format_args!("def Xform \"{}\"", node.name));
                line(out, indent, format_args!("{{"));
                line(
                    out,
                    indent + 4,
                    format_args!("custom string metrocalk:nodeId = \"{}\"", node.id.0),
                );
                line(
                    out,
                    indent + 4,
                    format_args!(
                        "matrix4d xformOp:transform = {}",
                        matrix(node.local_transform)
                    ),
                );
                if let Some(&(animation, channel)) = validated.matrix_channels.get(&id) {
                    let channel = &scene.animations[animation].channels[channel];
                    line(
                        out,
                        indent + 4,
                        format_args!("matrix4d xformOp:transform.timeSamples = {{"),
                    );
                    for sample in &channel.samples {
                        if let AnimationValue::Matrix4d(value) = sample.value {
                            line(
                                out,
                                indent + 8,
                                format_args!("{}: {},", number64(sample.time_code), matrix(value)),
                            );
                        }
                    }
                    line(out, indent + 4, format_args!("}}"));
                    line(
                        out,
                        indent + 4,
                        format_args!(
                            "custom token metrocalk:xformInterpolation = \"{}\"",
                            interpolation_token(channel.interpolation)
                        ),
                    );
                }
                line(
                    out,
                    indent + 4,
                    format_args!("uniform token[] xformOpOrder = [\"xformOp:transform\"]"),
                );
                if !node.visible {
                    line(
                        out,
                        indent + 4,
                        format_args!("token visibility = \"invisible\""),
                    );
                }
                if let Some(mesh) = node.mesh {
                    line(out, indent + 4, format_args!("def Xform \"_Mesh\" ("));
                    if node.skin.is_some() {
                        line(
                            out,
                            indent + 8,
                            format_args!("prepend apiSchemas = [\"SkelBindingAPI\"]"),
                        );
                    }
                    line(out, indent + 8, format_args!("instanceable = true"));
                    line(
                        out,
                        indent + 8,
                        format_args!("prepend references = </World/_Prototypes/Mesh_{}>", mesh.0),
                    );
                    line(out, indent + 4, format_args!(")"));
                    line(out, indent + 4, format_args!("{{"));
                    if let Some(skin) = node.skin {
                        line(
                            out,
                            indent + 8,
                            format_args!("rel skel:skeleton = </World/_Skeletons/Skin_{}>", skin.0),
                        );
                    }
                    line(out, indent + 4, format_args!("}}"));
                }
                stack.push(Event::Close(indent));
                if let Some(children) = validated.children.get(&Some(id)) {
                    for child in children.iter().rev() {
                        stack.push(Event::Open(*child, indent + 4));
                    }
                }
            }
        }
    }
}

fn animation_target(channel: &AnimationChannel, paths: &BTreeMap<SceneNodeId, String>) -> String {
    match &channel.target {
        AnimationTarget::NodeLocalTransform(node) => {
            format!("{}#localTransform", paths[node])
        }
        AnimationTarget::NodeProperty {
            node,
            component,
            field,
        } => format!("{}#{}.{}", paths[node], component, field),
    }
}

fn line(out: &mut String, indent: usize, args: fmt::Arguments<'_>) {
    for _ in 0..indent {
        out.push(' ');
    }
    out.write_fmt(args)
        .expect("formatting into a String cannot fail");
    out.push('\n');
}

fn number32(value: f32) -> String {
    if value == 0.0 {
        "0".into()
    } else {
        value.to_string()
    }
}

fn number64(value: f64) -> String {
    if value == 0.0 {
        "0".into()
    } else {
        value.to_string()
    }
}

fn vec2f(value: [f32; 2]) -> String {
    format!("({}, {})", number32(value[0]), number32(value[1]))
}

fn vec3f(value: [f32; 3]) -> String {
    format!(
        "({}, {}, {})",
        number32(value[0]),
        number32(value[1]),
        number32(value[2])
    )
}

fn vec3d(value: [f64; 3]) -> String {
    format!(
        "({}, {}, {})",
        number64(value[0]),
        number64(value[1]),
        number64(value[2])
    )
}

fn vec4d(value: [f64; 4]) -> String {
    format!(
        "({}, {}, {}, {})",
        number64(value[0]),
        number64(value[1]),
        number64(value[2]),
        number64(value[3])
    )
}

fn matrix(value: Matrix4d) -> String {
    let rows = value
        .rows
        .map(|row| {
            format!(
                "({}, {}, {}, {})",
                number64(row[0]),
                number64(row[1]),
                number64(row[2]),
                number64(row[3])
            )
        })
        .join(", ");
    format!("({rows})")
}

fn animation_value(value: &AnimationValue) -> String {
    match value {
        AnimationValue::Matrix4d(value) => matrix(*value),
        AnimationValue::Vector3d(value) => vec3d(*value),
        AnimationValue::Quaterniond(value) => vec4d(*value),
        AnimationValue::Number(value) => number64(*value),
        AnimationValue::Integer(value) => value.to_string(),
        AnimationValue::Boolean(value) => value.to_string(),
    }
}

const fn usd_value_type(value_type: AnimationValueType) -> &'static str {
    match value_type {
        AnimationValueType::Matrix4d => "matrix4d",
        AnimationValueType::Vector3d => "double3",
        // Project convention is explicit xyzw, so use a four-vector rather than USD's wxyz quaternion text.
        AnimationValueType::Quaterniond => "double4",
        AnimationValueType::Number => "double",
        AnimationValueType::Integer => "int64",
        AnimationValueType::Boolean => "bool",
    }
}

const fn value_type_token(value_type: AnimationValueType) -> &'static str {
    match value_type {
        AnimationValueType::Matrix4d => "matrix4d",
        AnimationValueType::Vector3d => "vector3d",
        AnimationValueType::Quaterniond => "quaterniond_xyzw",
        AnimationValueType::Number => "number",
        AnimationValueType::Integer => "integer",
        AnimationValueType::Boolean => "boolean",
    }
}

const fn interpolation_token(interpolation: AnimationInterpolation) -> &'static str {
    match interpolation {
        AnimationInterpolation::Step => "step",
        AnimationInterpolation::Linear => "linear",
        AnimationInterpolation::Cubic => "cubic",
    }
}

fn usd_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            control if control.is_control() => {
                let _ = write!(escaped, "\\u{:04x}", control as u32);
            }
            other => escaped.push(other),
        }
    }
    escaped.push('"');
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{Material, MeshAsset, Primitive, Texture};
    use crate::scene::{
        AnimationSample, SceneAnimation, SceneJoint, SceneMeshRef, SceneNode, SceneSkinRef,
    };

    fn translation(x: f64, y: f64, z: f64) -> Matrix4d {
        Matrix4d {
            rows: [
                [1.0, 0.0, 0.0, x],
                [0.0, 1.0, 0.0, y],
                [0.0, 0.0, 1.0, z],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }

    fn triangle(skinned: bool) -> MeshAsset {
        let mut primitive = Primitive {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            indices: vec![0, 1, 2],
            material: 0,
            ..Primitive::default()
        };
        if skinned {
            primitive.joints = vec![[0, 0, 0, 0], [0, 0, 0, 0], [1, 0, 0, 0]];
            primitive.weights = vec![[1.0, 0.0, 0.0, 0.0]; 3];
        }
        MeshAsset {
            name: "Triangle".into(),
            primitives: vec![primitive],
            materials: vec![Material {
                base_color: [0.2, 0.4, 0.8, 0.75],
                metallic: 0.3,
                roughness: 0.6,
                ..Material::default()
            }],
            ..MeshAsset::default()
        }
    }

    fn node(
        id: u64,
        name: &str,
        parent: Option<u64>,
        transform: Matrix4d,
        mesh: Option<usize>,
    ) -> SceneNode {
        SceneNode {
            id: SceneNodeId(id),
            name: name.into(),
            parent: parent.map(SceneNodeId),
            local_transform: transform,
            mesh: mesh.map(SceneMeshRef),
            skin: None,
            visible: true,
        }
    }

    #[test]
    fn hierarchy_exact_matrices_materials_and_shared_mesh_instances_are_deterministic() {
        let mut scene = SceneAsset::new("Assembly");
        scene.meshes.push(triangle(false));
        // Deliberately unordered input: output is stable by node id and exact parent relation.
        scene.nodes.push(node(
            30,
            "Child",
            Some(10),
            translation(0.0, 2.5, 0.0),
            Some(0),
        ));
        scene.nodes.push(node(
            20,
            "Second",
            None,
            translation(-4.0, 0.0, 0.0),
            Some(0),
        ));
        scene
            .nodes
            .push(node(10, "Root", None, translation(1.25, 0.0, 0.0), None));

        let first = export_usda(&scene).expect("valid scene exports");
        let second = export_usda(&scene).expect("repeat export");
        assert_eq!(first, second, "same scene produces byte-identical USDA");
        assert_eq!(first.text.matches("def Xform \"Mesh_0\"").count(), 1);
        assert_eq!(
            first
                .text
                .matches("prepend references = </World/_Prototypes/Mesh_0>")
                .count(),
            2,
            "both nodes instance one shared prototype"
        );
        assert!(first.text.contains("def Xform \"Root\"\n    {"));
        assert!(first.text.contains("def Xform \"Child\""));
        assert!(first.text.contains(
            "matrix4d xformOp:transform = ((1, 0, 0, 1.25), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1))"
        ));
        assert!(first
            .text
            .contains("uniform token info:id = \"UsdPreviewSurface\""));
        assert!(first.text.contains("float inputs:metallic = 0.3"));
        assert!(first.report.entries.iter().any(|entry| {
            entry.feature == "mesh_prototypes_and_instances"
                && entry.status == FidelityStatus::Preserved
                && entry.count == 2
        }));
    }

    #[test]
    fn rig_joint_metadata_and_linear_skin_streams_are_authored() {
        let mut scene = SceneAsset::new("Rigged");
        scene.meshes.push(triangle(true));
        scene
            .nodes
            .push(node(1, "HipNode", None, Matrix4d::IDENTITY, None));
        scene.nodes.push(node(
            2,
            "KneeNode",
            Some(1),
            translation(0.0, 1.0, 0.0),
            None,
        ));
        let mut mesh_node = node(3, "Body", None, Matrix4d::IDENTITY, Some(0));
        mesh_node.skin = Some(SceneSkinRef(0));
        scene.nodes.push(mesh_node);
        scene.skins.push(SceneSkin {
            name: "BodyRig".into(),
            skeleton_root: Some(SceneNodeId(1)),
            joints: vec![
                SceneJoint {
                    node: SceneNodeId(1),
                    name: "Hip".into(),
                    parent: None,
                    rest_local_transform: Matrix4d::IDENTITY,
                    bind_transform: Matrix4d::IDENTITY,
                    inverse_bind_matrix: Matrix4d::IDENTITY,
                },
                SceneJoint {
                    node: SceneNodeId(2),
                    name: "Knee".into(),
                    parent: Some(0),
                    rest_local_transform: translation(0.0, 1.0, 0.0),
                    bind_transform: translation(0.0, 1.0, 0.0),
                    inverse_bind_matrix: translation(0.0, -1.0, 0.0),
                },
            ],
        });

        let export = export_usda(&scene).expect("rig exports");
        assert!(export.text.contains("def SkelSkeleton \"Skin_0\""));
        assert!(export.text.contains("\"Hip/Knee\""));
        assert!(export.text.contains("metrocalk:inverseBindMatrices"));
        assert!(export.text.contains("primvars:skel:jointIndices"));
        assert!(export.text.contains("primvars:skel:jointWeights"));
        assert!(export
            .text
            .contains("rel skel:skeleton = </World/_Skeletons/Skin_0>"));
        assert!(export.report.entries.iter().any(|entry| {
            entry.feature == "skeleton_joint_metadata"
                && entry.status == FidelityStatus::Preserved
                && entry.count == 2
        }));
    }

    #[test]
    fn typed_animation_channels_emit_standard_and_project_time_samples() {
        let mut scene = SceneAsset::new("Animated");
        scene
            .nodes
            .push(node(7, "Mover", None, Matrix4d::IDENTITY, None));
        scene.animations.push(SceneAnimation {
            name: "MoveAndFade".into(),
            channels: vec![
                AnimationChannel {
                    target: AnimationTarget::NodeLocalTransform(SceneNodeId(7)),
                    value_type: AnimationValueType::Matrix4d,
                    interpolation: AnimationInterpolation::Linear,
                    samples: vec![
                        AnimationSample {
                            time_code: 0.0,
                            value: AnimationValue::Matrix4d(Matrix4d::IDENTITY),
                        },
                        AnimationSample {
                            time_code: 24.0,
                            value: AnimationValue::Matrix4d(translation(3.0, 0.0, 0.0)),
                        },
                    ],
                },
                AnimationChannel {
                    target: AnimationTarget::NodeProperty {
                        node: SceneNodeId(7),
                        component: "Material".into(),
                        field: "Opacity".into(),
                    },
                    value_type: AnimationValueType::Number,
                    interpolation: AnimationInterpolation::Linear,
                    samples: vec![
                        AnimationSample {
                            time_code: 0.0,
                            value: AnimationValue::Number(1.0),
                        },
                        AnimationSample {
                            time_code: 24.0,
                            value: AnimationValue::Number(0.0),
                        },
                    ],
                },
            ],
        });

        let export = export_usda(&scene).expect("animation exports");
        assert!(export.text.contains("startTimeCode = 0"));
        assert!(export.text.contains("endTimeCode = 24"));
        assert!(export
            .text
            .contains("matrix4d xformOp:transform.timeSamples = {"));
        assert!(export.text.contains("24: ((1, 0, 0, 3)"));
        assert!(export
            .text
            .contains("custom double metrocalk:value.timeSamples = {"));
        assert!(export.text.contains("/World/Mover#Material.Opacity"));
        assert!(export.report.entries.iter().any(|entry| {
            entry.feature == "project_property_animation"
                && entry.status == FidelityStatus::Converted
                && entry.count == 1
        }));
    }

    #[test]
    fn texture_non_packaging_is_an_explicit_loss_entry() {
        let mut scene = SceneAsset::new("Textured");
        let mut mesh = triangle(false);
        mesh.textures.push(Texture {
            width: 1,
            height: 1,
            rgba8: vec![255, 255, 255, 255],
        });
        mesh.materials[0].base_color_texture = Some(0);
        scene.meshes.push(mesh);
        scene
            .nodes
            .push(node(1, "Surface", None, Matrix4d::IDENTITY, Some(0)));

        let export = export_usda(&scene).expect("material factors still export");
        assert!(!export.report.is_lossless());
        assert!(export.report.entries.iter().any(|entry| {
            entry.feature == "embedded_rgba_textures"
                && entry.status == FidelityStatus::Omitted
                && entry.count == 1
        }));
    }

    #[test]
    fn rejects_unsafe_prim_names_before_writing() {
        let mut scene = SceneAsset::new("Unsafe");
        scene.nodes.push(node(
            1,
            "bad\" } def Scope \"Injected",
            None,
            Matrix4d::IDENTITY,
            None,
        ));
        assert!(matches!(
            export_usda(&scene),
            Err(UsdaExportError::UnsafeName { kind: "node", .. })
        ));
    }

    #[test]
    fn rejects_non_finite_matrices_and_animation_values() {
        let mut scene = SceneAsset::new("Finite");
        let mut transform = Matrix4d::IDENTITY;
        transform.rows[2][1] = f64::NAN;
        scene.nodes.push(node(1, "Broken", None, transform, None));
        assert!(matches!(
            export_usda(&scene),
            Err(UsdaExportError::NonFinite(_))
        ));

        let mut scene = SceneAsset::new("FiniteAnimation");
        scene
            .nodes
            .push(node(1, "Target", None, Matrix4d::IDENTITY, None));
        scene.animations.push(SceneAnimation {
            name: "Bad".into(),
            channels: vec![AnimationChannel {
                target: AnimationTarget::NodeProperty {
                    node: SceneNodeId(1),
                    component: "Light".into(),
                    field: "Intensity".into(),
                },
                value_type: AnimationValueType::Number,
                interpolation: AnimationInterpolation::Linear,
                samples: vec![AnimationSample {
                    time_code: 0.0,
                    value: AnimationValue::Number(f64::INFINITY),
                }],
            }],
        });
        assert!(matches!(
            export_usda(&scene),
            Err(UsdaExportError::NonFinite(_))
        ));
    }

    #[test]
    fn rejects_configured_count_and_output_budgets() {
        let mut scene = SceneAsset::new("Budget");
        scene
            .nodes
            .push(node(1, "One", None, Matrix4d::IDENTITY, None));
        scene
            .nodes
            .push(node(2, "Two", None, Matrix4d::IDENTITY, None));
        let limits = UsdaExportLimits {
            max_nodes: 1,
            ..UsdaExportLimits::default()
        };
        assert!(matches!(
            export_usda_with_limits(&scene, limits),
            Err(UsdaExportError::BudgetExceeded { kind: "nodes", .. })
        ));

        let limits = UsdaExportLimits {
            max_output_bytes: 32,
            ..UsdaExportLimits::default()
        };
        assert!(matches!(
            export_usda_with_limits(&SceneAsset::new("Tiny"), limits),
            Err(UsdaExportError::BudgetExceeded {
                kind: "estimated output bytes",
                ..
            })
        ));
    }
}
