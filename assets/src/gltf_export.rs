//! Deterministic, self-contained GLB export for the engine-owned [`MeshAsset`] representation.
//!
//! The public boundary only accepts our plain asset type and returns bytes or [`GlbExportError`].
//! The PNG encoder is an implementation detail of this wrapper; no foreign codec/exporter type
//! crosses into the editor or asset model. Every buffer and GLB chunk is four-byte aligned, and the
//! completed file is capped to the trusted internal-artifact budget accepted by
//! [`crate::GltfImporter::import_internal`].

#![allow(clippy::cast_possible_truncation)]

use std::fmt::Write as _;

use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};

use crate::mesh::{MeshAsset, Primitive, Texture};
use crate::source::{MAX_ELEMENTS, MAX_INTERNAL_ASSET_BYTES};

const FLOAT: u32 = 5126;
const UNSIGNED_INT: u32 = 5125;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
const MAX_GLTF_OBJECTS: usize = 65_536;
const MAX_NAME_BYTES: usize = 16 * 1024;

/// Maximum size of an exported GLB. Matching the trusted internal-artifact cap guarantees the produced
/// bytes can be handed directly back to [`crate::GltfImporter::import_internal`] without weakening the
/// smaller hostile-file limit used by ordinary imports.
pub const MAX_GLB_EXPORT_BYTES: usize = MAX_INTERNAL_ASSET_BYTES;

/// A production-safe GLB export failure. Codec-specific error types are flattened to strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlbExportError {
    /// The asset contains no drawable primitive.
    NoGeometry,
    /// A primitive has inconsistent, non-finite, out-of-range, or non-triangular data.
    InvalidGeometry {
        /// Primitive array index.
        primitive: usize,
        /// Stable, user-readable reason.
        reason: &'static str,
    },
    /// A material factor or reference is invalid.
    InvalidMaterial {
        /// Material array index.
        material: usize,
        /// Stable, user-readable reason.
        reason: &'static str,
    },
    /// A decoded RGBA texture has invalid dimensions or storage.
    InvalidTexture {
        /// Texture array index.
        texture: usize,
        /// Stable, user-readable reason.
        reason: &'static str,
    },
    /// An asset collection or element stream exceeds a deterministic work budget.
    TooManyElements {
        /// The collection that exceeded its limit.
        kind: &'static str,
        /// Observed count.
        count: usize,
        /// Configured limit.
        limit: usize,
    },
    /// The internal model contains a feature this static GLB tier cannot preserve.
    Unsupported {
        /// Unsupported feature description.
        feature: &'static str,
    },
    /// A validated RGBA texture could not be encoded as PNG.
    TextureEncode {
        /// Texture array index.
        texture: usize,
        /// Flattened codec explanation.
        reason: String,
    },
    /// The raw or encoded asset exceeds the self-contained file budget.
    TooLarge {
        /// Observed or conservatively estimated byte count.
        bytes: usize,
        /// Configured limit.
        limit: usize,
    },
    /// Asset-level metadata is invalid.
    InvalidAsset(&'static str),
}

impl std::fmt::Display for GlbExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoGeometry => f.write_str("asset has no drawable geometry"),
            Self::InvalidGeometry { primitive, reason } => {
                write!(f, "primitive {primitive} is invalid: {reason}")
            }
            Self::InvalidMaterial { material, reason } => {
                write!(f, "material {material} is invalid: {reason}")
            }
            Self::InvalidTexture { texture, reason } => {
                write!(f, "texture {texture} is invalid: {reason}")
            }
            Self::TooManyElements { kind, count, limit } => {
                write!(f, "asset has too many {kind}: {count} (limit {limit})")
            }
            Self::Unsupported { feature } => write!(f, "GLB export does not support {feature}"),
            Self::TextureEncode { texture, reason } => {
                write!(f, "texture {texture} could not be encoded as PNG: {reason}")
            }
            Self::TooLarge { bytes, limit } => {
                write!(f, "export is too large: {bytes} bytes (limit {limit})")
            }
            Self::InvalidAsset(reason) => write!(f, "asset is invalid: {reason}"),
        }
    }
}

impl std::error::Error for GlbExportError {}

/// Export a static [`MeshAsset`] as a deterministic, self-contained glTF 2.0 binary (`.glb`).
///
/// Geometry streams remain separate per primitive, material indices are preserved, and every RGBA8
/// texture is encoded as an embedded PNG bufferView. Skins are rejected because the internal asset
/// model does not retain enough node/animation structure to reproduce a complete rigged glTF without
/// inventing data.
pub fn export_glb(asset: &MeshAsset) -> Result<Vec<u8>, GlbExportError> {
    validate_asset(asset)?;

    let mut builder = GlbBuilder::default();
    let mut primitive_json = Vec::with_capacity(asset.primitives.len());
    for primitive in &asset.primitives {
        primitive_json.push(encode_primitive(&mut builder, primitive));
    }

    let material_json: Vec<String> = asset.materials.iter().map(encode_material).collect();
    let mut image_json = Vec::with_capacity(asset.textures.len());
    let mut texture_json = Vec::with_capacity(asset.textures.len());
    for (index, texture) in asset.textures.iter().enumerate() {
        let png = encode_png(texture).map_err(|reason| GlbExportError::TextureEncode {
            texture: index,
            reason,
        })?;
        let view = builder.add_view(&png, None);
        image_json.push(format!(
            "{{\"bufferView\":{view},\"mimeType\":\"image/png\"}}"
        ));
        texture_json.push(format!("{{\"source\":{index}}}"));
    }

    builder.align4();
    let buffer_len = builder.bin.len();
    let mut json = String::from(
        "{\"asset\":{\"version\":\"2.0\",\"generator\":\"Metrocalk deterministic GLB exporter\"}",
    );
    let _ = write!(json, ",\"buffers\":[{{\"byteLength\":{buffer_len}}}]");
    let _ = write!(json, ",\"bufferViews\":[{}]", builder.views.join(","));
    let _ = write!(json, ",\"accessors\":[{}]", builder.accessors.join(","));
    let _ = write!(json, ",\"materials\":[{}]", material_json.join(","));
    if !asset.textures.is_empty() {
        let _ = write!(json, ",\"images\":[{}]", image_json.join(","));
        let _ = write!(json, ",\"textures\":[{}]", texture_json.join(","));
    }
    let _ = write!(
        json,
        ",\"meshes\":[{{\"name\":{},\"primitives\":[{}]}}]",
        json_string(&asset.name),
        primitive_json.join(",")
    );
    json.push_str(",\"nodes\":[{\"mesh\":0}],\"scenes\":[{\"nodes\":[0]}],\"scene\":0}");

    assemble_glb(&json, &builder.bin)
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
        let target = target.map_or(String::new(), |target| format!(",\"target\":{target}"));
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
        min_max: Option<([f32; 3], [f32; 3])>,
    ) -> usize {
        let index = self.accessors.len();
        let min_max = min_max.map_or(String::new(), |(min, max)| {
            format!(",\"min\":{},\"max\":{}", vec3_json(min), vec3_json(max))
        });
        self.accessors.push(format!(
            "{{\"bufferView\":{view},\"componentType\":{component_type},\"count\":{count},\"type\":\"{accessor_type}\"{min_max}}}"
        ));
        index
    }
}

fn encode_primitive(builder: &mut GlbBuilder, primitive: &Primitive) -> String {
    let position_bytes = f32_bytes(primitive.positions.iter().flatten().copied());
    let position_view = builder.add_view(&position_bytes, Some(ARRAY_BUFFER));
    let position_accessor = builder.add_accessor(
        position_view,
        FLOAT,
        primitive.positions.len(),
        "VEC3",
        Some(position_bounds(&primitive.positions)),
    );
    let mut attributes = format!("\"POSITION\":{position_accessor}");

    if !primitive.normals.is_empty() {
        let bytes = f32_bytes(primitive.normals.iter().flatten().copied());
        let view = builder.add_view(&bytes, Some(ARRAY_BUFFER));
        let accessor = builder.add_accessor(view, FLOAT, primitive.normals.len(), "VEC3", None);
        let _ = write!(attributes, ",\"NORMAL\":{accessor}");
    }
    if !primitive.uvs.is_empty() {
        let bytes = f32_bytes(primitive.uvs.iter().flatten().copied());
        let view = builder.add_view(&bytes, Some(ARRAY_BUFFER));
        let accessor = builder.add_accessor(view, FLOAT, primitive.uvs.len(), "VEC2", None);
        let _ = write!(attributes, ",\"TEXCOORD_0\":{accessor}");
    }
    if !primitive.tangents.is_empty() {
        let bytes = f32_bytes(primitive.tangents.iter().flatten().copied());
        let view = builder.add_view(&bytes, Some(ARRAY_BUFFER));
        let accessor = builder.add_accessor(view, FLOAT, primitive.tangents.len(), "VEC4", None);
        let _ = write!(attributes, ",\"TANGENT\":{accessor}");
    }

    let mut index_bytes = Vec::with_capacity(primitive.indices.len() * 4);
    for &index in &primitive.indices {
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
        primitive.material
    )
}

fn encode_material(material: &crate::mesh::Material) -> String {
    let [r, g, b, a] = material.base_color;
    let mut pbr = format!(
        "\"baseColorFactor\":[{r},{g},{b},{a}],\"metallicFactor\":{},\"roughnessFactor\":{}",
        material.metallic, material.roughness
    );
    if let Some(index) = material.base_color_texture {
        let _ = write!(pbr, ",\"baseColorTexture\":{{\"index\":{index}}}");
    }
    if let Some(index) = material.metallic_roughness_texture {
        let _ = write!(pbr, ",\"metallicRoughnessTexture\":{{\"index\":{index}}}");
    }
    let normal = material.normal_texture.map_or(String::new(), |index| {
        format!(",\"normalTexture\":{{\"index\":{index}}}")
    });
    let occlusion = material.occlusion_texture.map_or(String::new(), |index| {
        format!(",\"occlusionTexture\":{{\"index\":{index}}}")
    });
    let curvature = material.curvature_texture.map_or(String::new(), |index| {
        format!(",\"extras\":{{\"metrocalkCurvatureTexture\":{index}}}")
    });
    format!("{{\"pbrMetallicRoughness\":{{{pbr}}}{normal}{occlusion}{curvature}}}")
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
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for position in positions {
        for axis in 0..3 {
            min[axis] = min[axis].min(position[axis]);
            max[axis] = max[axis].max(position[axis]);
        }
    }
    (min, max)
}

fn vec3_json(value: [f32; 3]) -> String {
    format!("[{},{},{}]", value[0], value[1], value[2])
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

fn assemble_glb(json: &str, bin: &[u8]) -> Result<Vec<u8>, GlbExportError> {
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
        .ok_or(GlbExportError::TooLarge {
            bytes: usize::MAX,
            limit: MAX_GLB_EXPORT_BYTES,
        })?;
    if total > MAX_GLB_EXPORT_BYTES || total > u32::MAX as usize {
        return Err(GlbExportError::TooLarge {
            bytes: total,
            limit: MAX_GLB_EXPORT_BYTES,
        });
    }

    let mut bytes = Vec::with_capacity(total);
    bytes.extend_from_slice(&0x4654_6c67_u32.to_le_bytes()); // glTF
    bytes.extend_from_slice(&2_u32.to_le_bytes());
    bytes.extend_from_slice(&(total as u32).to_le_bytes());
    bytes.extend_from_slice(&(json_chunk.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0x4e4f_534a_u32.to_le_bytes()); // JSON
    bytes.extend_from_slice(&json_chunk);
    bytes.extend_from_slice(&(bin_chunk.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0x004e_4942_u32.to_le_bytes()); // BIN\0
    bytes.extend_from_slice(&bin_chunk);
    Ok(bytes)
}

#[allow(clippy::too_many_lines)]
fn validate_asset(asset: &MeshAsset) -> Result<(), GlbExportError> {
    if asset.name.len() > MAX_NAME_BYTES {
        return Err(GlbExportError::InvalidAsset("name exceeds 16 KiB"));
    }
    if asset.primitives.is_empty() {
        return Err(GlbExportError::NoGeometry);
    }
    guard_objects("primitives", asset.primitives.len())?;
    guard_objects("materials", asset.materials.len())?;
    guard_objects("textures", asset.textures.len())?;
    if asset.materials.is_empty() {
        return Err(GlbExportError::InvalidMaterial {
            material: 0,
            reason: "at least one material is required",
        });
    }
    if asset.skeleton.is_some()
        || asset
            .primitives
            .iter()
            .any(|primitive| !primitive.joints.is_empty() || !primitive.weights.is_empty())
    {
        return Err(GlbExportError::Unsupported {
            feature: "skins, joint weights, or animation",
        });
    }

    let mut vertices = 0usize;
    let mut indices = 0usize;
    let mut raw_bytes = 0usize;
    for (index, primitive) in asset.primitives.iter().enumerate() {
        if primitive.positions.is_empty() {
            return Err(invalid_geometry(index, "positions are empty"));
        }
        if primitive.indices.is_empty() || !primitive.indices.len().is_multiple_of(3) {
            return Err(invalid_geometry(
                index,
                "indices must be a non-empty triangle list",
            ));
        }
        if primitive
            .indices
            .iter()
            .any(|&element| element as usize >= primitive.positions.len())
        {
            return Err(invalid_geometry(index, "an index is out of range"));
        }
        if !primitive.normals.is_empty() && primitive.normals.len() != primitive.positions.len() {
            return Err(invalid_geometry(
                index,
                "normal count does not match positions",
            ));
        }
        if !primitive.uvs.is_empty() && primitive.uvs.len() != primitive.positions.len() {
            return Err(invalid_geometry(index, "UV count does not match positions"));
        }
        if !primitive.tangents.is_empty() && primitive.tangents.len() != primitive.positions.len() {
            return Err(invalid_geometry(
                index,
                "tangent count does not match positions",
            ));
        }
        if primitive
            .positions
            .iter()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err(invalid_geometry(
                index,
                "positions contain a non-finite value",
            ));
        }
        if primitive.normals.iter().any(|normal| {
            let length_sq = normal.iter().map(|value| value * value).sum::<f32>();
            normal.iter().any(|value| !value.is_finite())
                || !(0.98 * 0.98..=1.02 * 1.02).contains(&length_sq)
        }) {
            return Err(invalid_geometry(
                index,
                "normals must be finite and unit length",
            ));
        }
        if primitive
            .uvs
            .iter()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err(invalid_geometry(index, "UVs contain a non-finite value"));
        }
        if primitive.tangents.iter().any(|tangent| {
            let length_sq = tangent[..3].iter().map(|value| value * value).sum::<f32>();
            tangent.iter().any(|value| !value.is_finite())
                || !(0.98 * 0.98..=1.02 * 1.02).contains(&length_sq)
                || (tangent[3].abs() - 1.0).abs() > 1.0e-4
        }) {
            return Err(invalid_geometry(
                index,
                "tangents must have finite unit xyz and a handedness of -1 or 1",
            ));
        }
        if primitive.material >= asset.materials.len() {
            return Err(invalid_geometry(index, "material index is out of range"));
        }
        vertices = checked_sum(vertices, primitive.positions.len())?;
        indices = checked_sum(indices, primitive.indices.len())?;
        raw_bytes = checked_sum(raw_bytes, primitive.positions.len().saturating_mul(12))?;
        raw_bytes = checked_sum(raw_bytes, primitive.normals.len().saturating_mul(12))?;
        raw_bytes = checked_sum(raw_bytes, primitive.uvs.len().saturating_mul(8))?;
        raw_bytes = checked_sum(raw_bytes, primitive.tangents.len().saturating_mul(16))?;
        raw_bytes = checked_sum(raw_bytes, primitive.indices.len().saturating_mul(4))?;
    }
    guard_elements("vertices", vertices)?;
    guard_elements("indices", indices)?;

    for (index, material) in asset.materials.iter().enumerate() {
        if material
            .base_color
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
            || !material.metallic.is_finite()
            || !(0.0..=1.0).contains(&material.metallic)
            || !material.roughness.is_finite()
            || !(0.0..=1.0).contains(&material.roughness)
        {
            return Err(GlbExportError::InvalidMaterial {
                material: index,
                reason: "PBR factors must be finite values between zero and one",
            });
        }
        for slot in [
            material.base_color_texture,
            material.metallic_roughness_texture,
            material.normal_texture,
            material.occlusion_texture,
            material.curvature_texture,
        ]
        .into_iter()
        .flatten()
        {
            if slot >= asset.textures.len() {
                return Err(GlbExportError::InvalidMaterial {
                    material: index,
                    reason: "texture index is out of range",
                });
            }
        }
    }

    for (index, texture) in asset.textures.iter().enumerate() {
        if texture.width == 0 || texture.height == 0 {
            return Err(invalid_texture(index, "dimensions must be non-zero"));
        }
        let expected = u64::from(texture.width)
            .checked_mul(u64::from(texture.height))
            .and_then(|value| value.checked_mul(4))
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| invalid_texture(index, "dimensions overflow the byte count"))?;
        if texture.rgba8.len() != expected {
            return Err(invalid_texture(
                index,
                "RGBA byte length does not match dimensions",
            ));
        }
        raw_bytes = checked_sum(raw_bytes, expected)?;
    }
    if raw_bytes > MAX_GLB_EXPORT_BYTES {
        return Err(GlbExportError::TooLarge {
            bytes: raw_bytes,
            limit: MAX_GLB_EXPORT_BYTES,
        });
    }
    Ok(())
}

fn guard_objects(kind: &'static str, count: usize) -> Result<(), GlbExportError> {
    if count > MAX_GLTF_OBJECTS {
        Err(GlbExportError::TooManyElements {
            kind,
            count,
            limit: MAX_GLTF_OBJECTS,
        })
    } else {
        Ok(())
    }
}

fn guard_elements(kind: &'static str, count: usize) -> Result<(), GlbExportError> {
    if count > MAX_ELEMENTS {
        Err(GlbExportError::TooManyElements {
            kind,
            count,
            limit: MAX_ELEMENTS,
        })
    } else {
        Ok(())
    }
}

fn checked_sum(left: usize, right: usize) -> Result<usize, GlbExportError> {
    left.checked_add(right).ok_or(GlbExportError::TooLarge {
        bytes: usize::MAX,
        limit: MAX_GLB_EXPORT_BYTES,
    })
}

const fn invalid_geometry(primitive: usize, reason: &'static str) -> GlbExportError {
    GlbExportError::InvalidGeometry { primitive, reason }
}

const fn invalid_texture(texture: usize, reason: &'static str) -> GlbExportError {
    GlbExportError::InvalidTexture { texture, reason }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{demo, GltfImporter, Material, MeshSource};

    fn round_trip_fixture() -> MeshAsset {
        let mut asset = GltfImporter::new()
            .import(&demo::normal_mapped_quad_glb())
            .expect("fixture imports");
        asset.name = "PBR \"showcase\"\nmesh".into();
        for primitive in &mut asset.primitives {
            primitive.tangents = vec![[1.0, 0.0, 0.0, 1.0]; primitive.positions.len()];
        }
        let mut second_primitive = asset.primitives[0].clone();
        for position in &mut second_primitive.positions {
            position[0] += 2.0;
        }
        second_primitive.material = 1;
        asset.primitives.push(second_primitive);
        asset.materials.push(Material {
            base_color: [0.25, 0.5, 0.75, 0.8],
            metallic: 0.35,
            roughness: 0.65,
            base_color_texture: Some(0),
            metallic_roughness_texture: Some(1),
            normal_texture: Some(2),
            occlusion_texture: None,
            curvature_texture: None,
        });
        asset
    }

    #[test]
    fn deterministic_multi_material_pbr_asset_round_trips_exactly() {
        let asset = round_trip_fixture();
        let first = export_glb(&asset).expect("export");
        let second = export_glb(&asset).expect("repeat export");
        assert_eq!(first, second, "identical asset produces identical bytes");
        assert_eq!(&first[..4], b"glTF");
        assert_eq!(first.len() % 4, 0);
        assert_eq!(
            u32::from_le_bytes(first[8..12].try_into().unwrap()) as usize,
            first.len()
        );

        let restored = GltfImporter::new()
            .import(&first)
            .expect("re-import export");
        assert_eq!(restored, asset);
        assert_eq!(restored.primitives.len(), 2);
        assert_eq!(restored.materials.len(), 2);
        assert_eq!(restored.textures.len(), 3);
        assert_eq!(restored.primitives[1].material, 1);
    }

    #[test]
    fn buffer_views_and_glb_chunks_are_four_byte_aligned() {
        let bytes = export_glb(&round_trip_fixture()).expect("export");
        let json_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        assert_eq!(json_len % 4, 0);
        let bin_header = 20 + json_len;
        assert_eq!(bin_header % 4, 0);
        let bin_len =
            u32::from_le_bytes(bytes[bin_header..bin_header + 4].try_into().unwrap()) as usize;
        assert_eq!(bin_len % 4, 0);
        assert_eq!(bin_header + 8 + bin_len, bytes.len());

        let document = gltf::Gltf::from_slice(&bytes).expect("valid glTF container");
        for view in document.views() {
            assert_eq!(
                view.offset() % 4,
                0,
                "bufferView {} starts on a four-byte boundary",
                view.index()
            );
        }
    }

    #[test]
    fn malformed_internal_assets_are_rejected_before_encoding() {
        let mut asset = round_trip_fixture();
        asset.primitives[0].positions[0][0] = f32::NAN;
        assert!(matches!(
            export_glb(&asset),
            Err(GlbExportError::InvalidGeometry { primitive: 0, .. })
        ));

        let mut asset = round_trip_fixture();
        asset.primitives[0].normals.pop();
        assert!(matches!(
            export_glb(&asset),
            Err(GlbExportError::InvalidGeometry { primitive: 0, .. })
        ));

        let mut asset = round_trip_fixture();
        asset.materials[0].normal_texture = Some(asset.textures.len());
        assert!(matches!(
            export_glb(&asset),
            Err(GlbExportError::InvalidMaterial { material: 0, .. })
        ));

        let mut asset = round_trip_fixture();
        asset.textures[0].rgba8.pop();
        assert!(matches!(
            export_glb(&asset),
            Err(GlbExportError::InvalidTexture { texture: 0, .. })
        ));
    }

    #[test]
    fn skinned_assets_are_explicitly_unsupported_not_silently_flattened() {
        let asset = GltfImporter::new()
            .import(&demo::skinned_quad_glb())
            .expect("skinned fixture");
        assert!(matches!(
            export_glb(&asset),
            Err(GlbExportError::Unsupported { .. })
        ));
    }
}
