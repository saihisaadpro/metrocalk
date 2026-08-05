//! Portable MikkTSpace tangent generation over the engine-owned mesh representation.
//!
//! MikkTSpace produces a tangent for every triangle corner. An indexed source vertex can legitimately
//! receive more than one result at mirrored UV islands or orientation discontinuities, so this adapter
//! splits that vertex by the exact encoded tangent and copies every parallel vertex stream. The input
//! asset remains immutable.

#![allow(clippy::default_trait_access)]

use metrocalk_assets::{MeshAsset, Primitive, MAX_ELEMENTS};
use std::collections::BTreeMap;
use std::fmt;

/// Measured work performed by [`generate_mikktspace_tangents`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TangentReport {
    /// Number of drawable primitives processed.
    pub primitives: usize,
    /// Number of source triangles processed.
    pub triangles: usize,
    /// Source vertex count before tangent discontinuities were split.
    pub source_vertices: usize,
    /// Vertex count after exact per-corner tangent splits.
    pub output_vertices: usize,
    /// Additional vertices introduced at tangent/orientation discontinuities.
    pub split_vertices: usize,
}

/// A fail-fast, foreign-type-free tangent generation error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TangentError {
    /// The asset contains no drawable triangle geometry.
    NoGeometry,
    /// One primitive violates the complete, finite triangle-stream contract.
    InvalidPrimitive {
        /// Primitive array index.
        primitive: usize,
        /// Actionable validation failure.
        reason: String,
    },
    /// The reference MikkTSpace implementation rejected otherwise validated geometry.
    GenerationFailed {
        /// Primitive array index.
        primitive: usize,
    },
    /// The seam split would exceed the same element budget as imported assets.
    TooManyVertices {
        /// Requested output vertex count.
        vertices: usize,
        /// Configured hard limit.
        limit: usize,
    },
}

impl fmt::Display for TangentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoGeometry => f.write_str("the asset contains no drawable triangles"),
            Self::InvalidPrimitive { primitive, reason } => {
                write!(f, "primitive {primitive} is invalid for tangent generation: {reason}")
            }
            Self::GenerationFailed { primitive } => {
                write!(f, "MikkTSpace could not generate tangents for primitive {primitive}")
            }
            Self::TooManyVertices { vertices, limit } => write!(
                f,
                "tangent seam splitting would create {vertices} vertices, above the {limit} element limit"
            ),
        }
    }
}

impl std::error::Error for TangentError {}

/// Generate reference-compatible MikkTSpace tangents and return a source-preserving derivative.
///
/// Every primitive must be an indexed triangle list with complete positions, normals and UV0. Skin streams
/// may be absent or complete; when present they are duplicated exactly at tangent seams. Mirrored islands
/// are represented by separate vertices carrying the correct `w` handedness rather than averaging
/// incompatible tangent frames.
pub fn generate_mikktspace_tangents(
    asset: &MeshAsset,
) -> Result<(MeshAsset, TangentReport), TangentError> {
    let mut output = MeshAsset {
        name: asset.name.clone(),
        primitives: Vec::with_capacity(asset.primitives.len()),
        materials: asset.materials.clone(),
        textures: asset.textures.clone(),
        skeleton: asset.skeleton.clone(),
    };
    let mut report = TangentReport::default();

    for (primitive_index, primitive) in asset.primitives.iter().enumerate() {
        validate_primitive(primitive, primitive_index)?;
        let mut geometry = CornerGeometry::new(primitive);
        if !mikktspace::generate_tangents(&mut geometry) {
            return Err(TangentError::GenerationFailed {
                primitive: primitive_index,
            });
        }
        if geometry.tangents.iter().any(Option::is_none) {
            return Err(TangentError::GenerationFailed {
                primitive: primitive_index,
            });
        }

        let conditioned = split_corner_tangents(primitive, &geometry.tangents)?;
        report.primitives += 1;
        report.triangles += primitive.indices.len() / 3;
        report.source_vertices += primitive.positions.len();
        report.output_vertices += conditioned.positions.len();
        report.split_vertices += conditioned
            .positions
            .len()
            .saturating_sub(primitive.positions.len());
        output.primitives.push(conditioned);
    }

    if report.triangles == 0 {
        return Err(TangentError::NoGeometry);
    }
    Ok((output, report))
}

fn invalid(primitive: usize, reason: impl Into<String>) -> TangentError {
    TangentError::InvalidPrimitive {
        primitive,
        reason: reason.into(),
    }
}

fn validate_primitive(primitive: &Primitive, primitive_index: usize) -> Result<(), TangentError> {
    let vertices = primitive.positions.len();
    if vertices == 0 || primitive.indices.is_empty() {
        return Err(invalid(
            primitive_index,
            "positions and indices must be non-empty",
        ));
    }
    if vertices > MAX_ELEMENTS || primitive.indices.len() > MAX_ELEMENTS {
        return Err(invalid(
            primitive_index,
            format!("vertex/index count exceeds the {MAX_ELEMENTS} element limit"),
        ));
    }
    if !primitive.indices.len().is_multiple_of(3) {
        return Err(invalid(
            primitive_index,
            "the index count is not a multiple of three",
        ));
    }
    if primitive.normals.len() != vertices {
        return Err(invalid(
            primitive_index,
            "a complete per-vertex normal stream is required",
        ));
    }
    if primitive.uvs.len() != vertices {
        return Err(invalid(
            primitive_index,
            "a complete per-vertex UV0 stream is required",
        ));
    }
    let skin_empty = primitive.joints.is_empty() && primitive.weights.is_empty();
    let skin_complete = primitive.joints.len() == vertices && primitive.weights.len() == vertices;
    if !skin_empty && !skin_complete {
        return Err(invalid(
            primitive_index,
            "joint and weight streams must both be empty or complete",
        ));
    }
    if primitive
        .positions
        .iter()
        .flatten()
        .chain(primitive.normals.iter().flatten())
        .chain(primitive.uvs.iter().flatten())
        .chain(primitive.weights.iter().flatten())
        .any(|value| !value.is_finite())
    {
        return Err(invalid(
            primitive_index,
            "positions, normals, UVs and weights must be finite",
        ));
    }
    if primitive
        .normals
        .iter()
        .any(|normal| length2(*normal) <= 1.0e-20)
    {
        return Err(invalid(primitive_index, "normals must be non-zero"));
    }

    let mut referenced = vec![false; vertices];
    for &index in &primitive.indices {
        let index = usize::try_from(index)
            .ok()
            .filter(|&index| index < vertices)
            .ok_or_else(|| invalid(primitive_index, "an index is out of range"))?;
        referenced[index] = true;
    }
    if referenced.iter().any(|value| !value) {
        return Err(invalid(
            primitive_index,
            "isolated vertices must be removed before tangent generation",
        ));
    }
    Ok(())
}

struct CornerGeometry<'a> {
    primitive: &'a Primitive,
    tangents: Vec<Option<[f32; 4]>>,
}

impl<'a> CornerGeometry<'a> {
    fn new(primitive: &'a Primitive) -> Self {
        Self {
            primitive,
            tangents: vec![None; primitive.indices.len()],
        }
    }

    fn source_index(&self, face: usize, corner: usize) -> usize {
        self.primitive.indices[face * 3 + corner] as usize
    }
}

impl mikktspace::Geometry for CornerGeometry<'_> {
    fn num_faces(&self) -> usize {
        self.primitive.indices.len() / 3
    }

    fn num_vertices_of_face(&self, _face: usize) -> usize {
        3
    }

    fn position(&self, face: usize, vert: usize) -> [f32; 3] {
        self.primitive.positions[self.source_index(face, vert)]
    }

    fn normal(&self, face: usize, vert: usize) -> [f32; 3] {
        self.primitive.normals[self.source_index(face, vert)]
    }

    fn tex_coord(&self, face: usize, vert: usize) -> [f32; 2] {
        self.primitive.uvs[self.source_index(face, vert)]
    }

    fn set_tangent_encoded(&mut self, tangent: [f32; 4], face: usize, vert: usize) {
        self.tangents[face * 3 + vert] = canonical_tangent(tangent);
    }
}

fn canonical_tangent(tangent: [f32; 4]) -> Option<[f32; 4]> {
    if tangent.iter().any(|value| !value.is_finite()) {
        return None;
    }
    let magnitude2 = tangent[0] * tangent[0] + tangent[1] * tangent[1] + tangent[2] * tangent[2];
    if magnitude2 <= 1.0e-20 {
        return None;
    }
    let inverse = magnitude2.sqrt().recip();
    let clean = |value: f32| if value == 0.0 { 0.0 } else { value };
    Some([
        clean(tangent[0] * inverse),
        clean(tangent[1] * inverse),
        clean(tangent[2] * inverse),
        if tangent[3] < 0.0 { -1.0 } else { 1.0 },
    ])
}

fn split_corner_tangents(
    primitive: &Primitive,
    corner_tangents: &[Option<[f32; 4]>],
) -> Result<Primitive, TangentError> {
    let has_skin = !primitive.joints.is_empty();
    let mut output = Primitive {
        positions: Vec::new(),
        normals: Vec::new(),
        uvs: Vec::new(),
        tangents: Vec::new(),
        indices: Vec::with_capacity(primitive.indices.len()),
        material: primitive.material,
        joints: Vec::new(),
        weights: Vec::new(),
    };
    let mut remap: BTreeMap<(u32, [u32; 4]), u32> = BTreeMap::new();

    for (&source, tangent) in primitive.indices.iter().zip(corner_tangents) {
        let tangent = tangent.expect("validated complete corner tangent output");
        let key = (source, tangent.map(f32::to_bits));
        let target = if let Some(&target) = remap.get(&key) {
            target
        } else {
            if output.positions.len() >= MAX_ELEMENTS {
                return Err(TangentError::TooManyVertices {
                    vertices: output.positions.len().saturating_add(1),
                    limit: MAX_ELEMENTS,
                });
            }
            let source_index = source as usize;
            let target = u32::try_from(output.positions.len()).map_err(|_| {
                TangentError::TooManyVertices {
                    vertices: output.positions.len(),
                    limit: MAX_ELEMENTS,
                }
            })?;
            output.positions.push(primitive.positions[source_index]);
            output.normals.push(primitive.normals[source_index]);
            output.uvs.push(primitive.uvs[source_index]);
            output.tangents.push(tangent);
            if has_skin {
                output.joints.push(primitive.joints[source_index]);
                output.weights.push(primitive.weights[source_index]);
            }
            remap.insert(key, target);
            target
        };
        output.indices.push(target);
    }
    Ok(output)
}

fn length2(value: [f32; 3]) -> f32 {
    value[0] * value[0] + value[1] * value[1] + value[2] * value[2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrocalk_assets::{Material, Texture};

    fn mirrored_asset() -> MeshAsset {
        MeshAsset {
            name: "mirrored".into(),
            primitives: vec![Primitive {
                positions: vec![
                    [0.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    [1.0, -1.0, 0.0],
                ],
                normals: vec![[0.0, 0.0, 1.0]; 4],
                uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [0.0, 1.0]],
                tangents: Vec::new(),
                indices: vec![0, 1, 2, 0, 3, 1],
                material: 0,
                joints: vec![[0, 1, 0, 0]; 4],
                weights: vec![[0.75, 0.25, 0.0, 0.0]; 4],
            }],
            materials: vec![Material::default()],
            textures: vec![Texture {
                width: 1,
                height: 1,
                rgba8: vec![255, 255, 255, 255],
            }],
            skeleton: Some(Default::default()),
        }
    }

    #[test]
    fn mirrored_uv_handedness_splits_vertices_and_copies_every_stream() {
        let source = mirrored_asset();
        let (output, report) = generate_mikktspace_tangents(&source).expect("tangents");
        let primitive = &output.primitives[0];

        assert_eq!(
            source.primitives[0].positions.len(),
            4,
            "source is immutable"
        );
        assert!(
            report.split_vertices >= 2,
            "shared mirrored edge must split"
        );
        assert_eq!(primitive.positions.len(), primitive.normals.len());
        assert_eq!(primitive.positions.len(), primitive.uvs.len());
        assert_eq!(primitive.positions.len(), primitive.tangents.len());
        assert_eq!(primitive.positions.len(), primitive.joints.len());
        assert_eq!(primitive.positions.len(), primitive.weights.len());
        assert!(primitive.tangents.iter().any(|tangent| tangent[3] < 0.0));
        assert!(primitive.tangents.iter().any(|tangent| tangent[3] > 0.0));
        assert_eq!(output.materials, source.materials);
        assert_eq!(output.textures, source.textures);
        assert_eq!(output.skeleton, source.skeleton);
    }

    #[test]
    fn generation_is_repeatable_and_tangents_are_orthonormal() {
        let source = mirrored_asset();
        let first = generate_mikktspace_tangents(&source).expect("first");
        let second = generate_mikktspace_tangents(&source).expect("second");
        assert_eq!(first, second);
        for primitive in &first.0.primitives {
            for (normal, tangent) in primitive.normals.iter().zip(&primitive.tangents) {
                let dot = normal[0] * tangent[0] + normal[1] * tangent[1] + normal[2] * tangent[2];
                assert!(dot.abs() < 1.0e-5);
                let magnitude =
                    tangent[0] * tangent[0] + tangent[1] * tangent[1] + tangent[2] * tangent[2];
                assert!((magnitude - 1.0).abs() < 1.0e-5);
                assert!(matches!(tangent[3], -1.0 | 1.0));
            }
        }
    }

    #[test]
    fn partial_skin_and_isolated_vertices_are_explained() {
        let mut source = mirrored_asset();
        source.primitives[0].weights.pop();
        let error = generate_mikktspace_tangents(&source).expect_err("partial skin");
        assert!(error.to_string().contains("joint and weight"));

        let mut source = mirrored_asset();
        source.primitives[0].positions.push([4.0, 4.0, 4.0]);
        source.primitives[0].normals.push([0.0, 0.0, 1.0]);
        source.primitives[0].uvs.push([0.5, 0.5]);
        source.primitives[0].joints.push([0; 4]);
        source.primitives[0].weights.push([1.0, 0.0, 0.0, 0.0]);
        let error = generate_mikktspace_tangents(&source).expect_err("isolated vertex");
        assert!(error.to_string().contains("isolated"));
    }
}
