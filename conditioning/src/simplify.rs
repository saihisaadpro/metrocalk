//! Attribute-, seam- and rig-aware QEM simplification contracts.
//!
//! The public API is project-owned. The native implementation uses pinned meshoptimizer behind the `qem`
//! feature, keeps original vertex payloads (index-only simplification), locks semantic discontinuities, and
//! compacts every parallel stream deterministically. Tangents are regenerated after topology changes.

// Counts are validated against MAX_ELEMENTS before narrowing; ratios are evidence-only floating values.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::default_trait_access,
    clippy::if_not_else,
    clippy::many_single_char_names,
    clippy::too_many_lines,
    clippy::wildcard_imports
)]

use metrocalk_assets::MeshAsset;
use std::fmt;

/// Quality and preservation controls for QEM simplification.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SimplifyConfig {
    /// Requested output/source triangle ratio in `(0, 1)`.
    pub target_ratio: f32,
    /// Maximum meshoptimizer relative geometric error.
    pub target_error: f32,
    /// QEM weight assigned to each normal component.
    pub normal_weight: f32,
    /// QEM weight assigned to each UV component.
    pub uv_weight: f32,
    /// QEM weight assigned to each canonicalized skin-weight component.
    pub skin_weight: f32,
    /// Preserve open, primitive and material boundaries exactly.
    pub lock_borders: bool,
    /// Lock coincident UV seams, hard-normal seams and geometric creases.
    pub preserve_attribute_seams: bool,
    /// Conservatively lock edges where the set of influencing joints changes.
    pub preserve_skinning: bool,
    /// Geometric edge angle at or above which both endpoints are locked.
    pub hard_edge_angle_degrees: f32,
    /// Prefer regular triangles for rigged/deforming output.
    pub regularize_rigged: bool,
}

impl Default for SimplifyConfig {
    fn default() -> Self {
        Self {
            target_ratio: 0.5,
            target_error: 0.01,
            normal_weight: 1.0,
            uv_weight: 1.0,
            skin_weight: 2.0,
            lock_borders: true,
            preserve_attribute_seams: true,
            preserve_skinning: true,
            hard_edge_angle_degrees: 60.0,
            regularize_rigged: true,
        }
    }
}

/// Per-primitive measured QEM evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct PrimitiveSimplifyReport {
    /// Primitive array index.
    pub primitive: usize,
    /// Source triangle count.
    pub source_triangles: usize,
    /// Output triangle count.
    pub output_triangles: usize,
    /// Source vertex count.
    pub source_vertices: usize,
    /// Compacted vertex count before any final tangent splits.
    pub output_vertices: usize,
    /// Explicitly locked semantic/boundary vertices.
    pub locked_vertices: usize,
    /// Relative error reported by meshoptimizer.
    pub result_error: f32,
}

/// Aggregate measured simplification evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct SimplifyReport {
    /// Requested triangle ratio.
    pub requested_ratio: f32,
    /// Measured output/source triangle ratio.
    pub achieved_ratio: f32,
    /// Source triangle count.
    pub source_triangles: usize,
    /// Output triangle count.
    pub output_triangles: usize,
    /// Source vertex count.
    pub source_vertices: usize,
    /// Final compacted and tangent-split vertex count.
    pub output_vertices: usize,
    /// Number of explicitly locked vertices across primitives.
    pub locked_vertices: usize,
    /// Maximum relative error reported across primitives.
    pub max_result_error: f32,
    /// Number of primitives that achieved a triangle reduction.
    pub reduced_primitives: usize,
    /// Per-primitive evidence.
    pub primitives: Vec<PrimitiveSimplifyReport>,
    /// Honest native determinism boundary.
    pub determinism: String,
}

/// A source-preserving QEM derivative and its evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct SimplifyResult {
    /// Optimized asset with materials, textures and skeleton retained.
    pub asset: MeshAsset,
    /// Measured simplification evidence.
    pub report: SimplifyReport,
}

/// A bounded, dependency-type-free simplification failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SimplifyError {
    /// Invalid user/configuration input.
    InvalidConfig(String),
    /// Malformed or unsupported source mesh data.
    InvalidInput(String),
    /// The native QEM adapter rejected validated input.
    NativeFailure(String),
    /// Tangent regeneration or output validation failed.
    PostProcess(String),
}

impl fmt::Display for SimplifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(reason) => write!(f, "invalid QEM configuration: {reason}"),
            Self::InvalidInput(reason) => write!(f, "invalid QEM input: {reason}"),
            Self::NativeFailure(reason) => write!(f, "meshoptimizer failed: {reason}"),
            Self::PostProcess(reason) => write!(f, "QEM post-processing failed: {reason}"),
        }
    }
}

impl std::error::Error for SimplifyError {}

/// Project-owned mesh simplification boundary.
pub trait MeshSimplifier {
    /// Produce an immutable optimized derivative.
    fn simplify(
        &self,
        asset: &MeshAsset,
        config: &SimplifyConfig,
    ) -> Result<SimplifyResult, SimplifyError>;
}

#[cfg(feature = "qem")]
mod native {
    use super::*;
    use crate::tangent::generate_mikktspace_tangents;
    use meshopt::{SimplifyOptions, VertexDataAdapter};
    use metrocalk_assets::{Primitive, MAX_ELEMENTS};
    use std::collections::BTreeMap;
    use std::mem;

    const DETERMINISM_BOUNDARY: &str = "Pinned meshoptimizer is repeatable on the validated supported native build, but its C++ floating-point QEM result is not promised bit-identical across architectures or toolchains; persist the conditioned artifact bytes.";

    /// Native attribute- and lock-aware QEM backed by pinned meshoptimizer.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct MeshoptQemSimplifier;

    impl MeshSimplifier for MeshoptQemSimplifier {
        #[allow(clippy::too_many_lines)]
        fn simplify(
            &self,
            asset: &MeshAsset,
            config: &SimplifyConfig,
        ) -> Result<SimplifyResult, SimplifyError> {
            validate_config(config)?;
            validate_asset(asset)?;
            let locks = build_locks(asset, config)?;

            let source_triangles = asset.triangle_count();
            let source_vertices = asset.vertex_count();
            let mut primitives = Vec::with_capacity(asset.primitives.len());
            let mut primitive_reports = Vec::with_capacity(asset.primitives.len());
            let mut reduced_primitives = 0usize;
            let mut locked_vertices = 0usize;
            let mut max_result_error = 0.0_f32;

            for (primitive_index, primitive) in asset.primitives.iter().enumerate() {
                let source_triangle_count = primitive.indices.len() / 3;
                let desired_triangles = ((source_triangle_count as f64
                    * f64::from(config.target_ratio))
                .round() as usize)
                    .clamp(1, source_triangle_count);
                let lock_slice: Box<[bool]> = locks[primitive_index]
                    .iter()
                    .map(|&locked| locked != 0)
                    .collect();
                let primitive_locked = lock_slice.iter().filter(|&&locked| locked).count();
                locked_vertices = locked_vertices.saturating_add(primitive_locked);

                let adapter = VertexDataAdapter::new(
                    meshopt::typed_to_bytes(&primitive.positions),
                    mem::size_of::<[f32; 3]>(),
                    0,
                )
                .map_err(|error| SimplifyError::NativeFailure(error.to_string()))?;
                let (attributes, attribute_weights) = vertex_attributes(primitive, config);
                let mut options = SimplifyOptions::None;
                if config.lock_borders {
                    options |= SimplifyOptions::LockBorder;
                }
                if config.regularize_rigged && !primitive.joints.is_empty() {
                    options |= SimplifyOptions::Regularize;
                }
                // Attribute-aware QEM can occasionally choose a geometrically legal collapse that makes
                // the UV derivative singular. Such a mesh cannot carry a valid MikkTSpace frame. Search a
                // small, deterministic ladder towards the source topology and publish the most aggressive
                // candidate that survives the actual tangent generator. This makes target_ratio best-effort
                // while keeping renderability a hard invariant.
                let mut accepted = None;
                let mut last_tangent_error = None;
                for candidate_triangles in
                    candidate_triangle_targets(desired_triangles, source_triangle_count)
                {
                    let target_indices = candidate_triangles.saturating_mul(3);
                    let mut candidate_error = 0.0_f32;
                    let simplified = if attributes.is_empty() {
                        meshopt::simplify_with_locks(
                            &primitive.indices,
                            &adapter,
                            &lock_slice,
                            target_indices,
                            config.target_error,
                            options,
                            Some(&mut candidate_error),
                        )
                    } else {
                        let stride = attribute_weights.len() * mem::size_of::<f32>();
                        meshopt::simplify_with_attributes_and_locks(
                            &primitive.indices,
                            &adapter,
                            &attributes,
                            &attribute_weights,
                            stride,
                            &lock_slice,
                            target_indices,
                            config.target_error,
                            options,
                            Some(&mut candidate_error),
                        )
                    };
                    validate_native_output(
                        primitive,
                        primitive_index,
                        &simplified,
                        candidate_error,
                    )?;
                    let compacted = compact_primitive(primitive, &simplified)?;
                    let compacted_vertices = compacted.positions.len();
                    match regenerate_candidate_tangents(compacted) {
                        Ok(conditioned) => {
                            accepted = Some((conditioned, compacted_vertices, candidate_error));
                            break;
                        }
                        Err(error) => last_tangent_error = Some(error),
                    }
                }
                let (conditioned, compacted_vertices, result_error) =
                    accepted.ok_or_else(|| {
                        SimplifyError::PostProcess(format!(
                            "no invariant-safe QEM candidate could regenerate tangents: {}",
                            last_tangent_error
                                .as_deref()
                                .unwrap_or("unknown tangent failure")
                        ))
                    })?;
                let output_triangle_count = conditioned.indices.len() / 3;
                if output_triangle_count < source_triangle_count {
                    reduced_primitives += 1;
                }
                max_result_error = max_result_error.max(result_error);
                primitive_reports.push(PrimitiveSimplifyReport {
                    primitive: primitive_index,
                    source_triangles: source_triangle_count,
                    output_triangles: output_triangle_count,
                    source_vertices: primitive.positions.len(),
                    output_vertices: compacted_vertices,
                    locked_vertices: primitive_locked,
                    result_error,
                });
                primitives.push(conditioned);
            }

            let output = MeshAsset {
                name: asset.name.clone(),
                primitives,
                materials: asset.materials.clone(),
                textures: asset.textures.clone(),
                skeleton: asset.skeleton.clone(),
            };
            let output_triangles = output.triangle_count();
            if output_triangles == 0 || output_triangles > source_triangles {
                return Err(SimplifyError::PostProcess(
                    "the optimized asset has an invalid triangle count".into(),
                ));
            }
            let achieved_ratio = output_triangles as f32 / source_triangles as f32;

            Ok(SimplifyResult {
                report: SimplifyReport {
                    requested_ratio: config.target_ratio,
                    achieved_ratio,
                    source_triangles,
                    output_triangles,
                    source_vertices,
                    output_vertices: output.vertex_count(),
                    locked_vertices,
                    max_result_error,
                    reduced_primitives,
                    primitives: primitive_reports,
                    determinism: DETERMINISM_BOUNDARY.into(),
                },
                asset: output,
            })
        }
    }

    fn candidate_triangle_targets(desired: usize, source: usize) -> Vec<usize> {
        const REPAIR_STEPS: usize = 12;

        let mut targets = vec![desired];
        let span = source.saturating_sub(desired);
        for step in 1..=REPAIR_STEPS {
            let target = desired.saturating_add(span.saturating_mul(step).div_ceil(REPAIR_STEPS));
            if targets.last().copied() != Some(target) {
                targets.push(target);
            }
        }
        targets
    }

    fn regenerate_candidate_tangents(primitive: Primitive) -> Result<Primitive, String> {
        let tangent_ready = primitive.normals.len() == primitive.positions.len()
            && primitive.uvs.len() == primitive.positions.len();
        if !tangent_ready {
            return Ok(primitive);
        }
        let probe = MeshAsset {
            name: String::new(),
            primitives: vec![primitive],
            materials: Vec::new(),
            textures: Vec::new(),
            skeleton: None,
        };
        let (mut conditioned, _) =
            generate_mikktspace_tangents(&probe).map_err(|error| error.to_string())?;
        conditioned
            .primitives
            .pop()
            .ok_or_else(|| "MikkTSpace returned no conditioned primitive".into())
    }

    fn validate_config(config: &SimplifyConfig) -> Result<(), SimplifyError> {
        if !config.target_ratio.is_finite() || !(0.0..1.0).contains(&config.target_ratio) {
            return Err(SimplifyError::InvalidConfig(
                "target_ratio must be finite and strictly between zero and one".into(),
            ));
        }
        if !config.target_error.is_finite() || !(0.0..=1.0).contains(&config.target_error) {
            return Err(SimplifyError::InvalidConfig(
                "target_error must be finite and between zero and one".into(),
            ));
        }
        for (name, weight) in [
            ("normal_weight", config.normal_weight),
            ("uv_weight", config.uv_weight),
            ("skin_weight", config.skin_weight),
        ] {
            if !weight.is_finite() || !(0.0..=1.0e6).contains(&weight) {
                return Err(SimplifyError::InvalidConfig(format!(
                    "{name} must be finite and between zero and one million"
                )));
            }
        }
        if !config.hard_edge_angle_degrees.is_finite()
            || !(0.0..=180.0).contains(&config.hard_edge_angle_degrees)
        {
            return Err(SimplifyError::InvalidConfig(
                "hard_edge_angle_degrees must be between zero and 180".into(),
            ));
        }
        Ok(())
    }

    fn validate_asset(asset: &MeshAsset) -> Result<(), SimplifyError> {
        if asset.primitives.is_empty() {
            return Err(SimplifyError::InvalidInput(
                "the asset has no primitives".into(),
            ));
        }
        let mut vertices = 0usize;
        let mut indices = 0usize;
        for (primitive_index, primitive) in asset.primitives.iter().enumerate() {
            let count = primitive.positions.len();
            if count == 0
                || primitive.indices.is_empty()
                || !primitive.indices.len().is_multiple_of(3)
            {
                return Err(SimplifyError::InvalidInput(format!(
                    "primitive {primitive_index} is not a non-empty triangle list"
                )));
            }
            for (name, length) in [
                ("normal", primitive.normals.len()),
                ("UV0", primitive.uvs.len()),
                ("tangent", primitive.tangents.len()),
            ] {
                if length != 0 && length != count {
                    return Err(SimplifyError::InvalidInput(format!(
                        "primitive {primitive_index} has a partial {name} stream"
                    )));
                }
            }
            let skin_empty = primitive.joints.is_empty() && primitive.weights.is_empty();
            let skin_complete = primitive.joints.len() == count && primitive.weights.len() == count;
            if !skin_empty && !skin_complete {
                return Err(SimplifyError::InvalidInput(format!(
                    "primitive {primitive_index} has partial skin streams"
                )));
            }
            if !skin_empty && asset.skeleton.is_none() {
                return Err(SimplifyError::InvalidInput(format!(
                    "primitive {primitive_index} has skin streams without a skeleton"
                )));
            }
            if primitive.material >= asset.materials.len() {
                return Err(SimplifyError::InvalidInput(format!(
                    "primitive {primitive_index} material index is out of range"
                )));
            }
            if primitive
                .positions
                .iter()
                .flatten()
                .chain(primitive.normals.iter().flatten())
                .chain(primitive.uvs.iter().flatten())
                .chain(primitive.tangents.iter().flatten())
                .chain(primitive.weights.iter().flatten())
                .any(|value| !value.is_finite())
            {
                return Err(SimplifyError::InvalidInput(format!(
                    "primitive {primitive_index} contains non-finite vertex data"
                )));
            }
            for (vertex, weights) in primitive.weights.iter().enumerate() {
                if weights.iter().any(|weight| *weight < 0.0) {
                    return Err(SimplifyError::InvalidInput(format!(
                        "primitive {primitive_index} vertex {vertex} has a negative skin weight"
                    )));
                }
                let sum = weights.iter().sum::<f32>();
                if (sum - 1.0).abs() > 1.0e-3 {
                    return Err(SimplifyError::InvalidInput(format!(
                        "primitive {primitive_index} vertex {vertex} skin weights do not sum to one"
                    )));
                }
            }
            for triangle in primitive.indices.as_chunks::<3>().0 {
                let [a, b, c] = [
                    triangle[0] as usize,
                    triangle[1] as usize,
                    triangle[2] as usize,
                ];
                if a >= count || b >= count || c >= count {
                    return Err(SimplifyError::InvalidInput(format!(
                        "primitive {primitive_index} contains an out-of-range index"
                    )));
                }
                if a == b
                    || b == c
                    || a == c
                    || face_normal(
                        primitive.positions[a],
                        primitive.positions[b],
                        primitive.positions[c],
                    )
                    .is_none()
                {
                    return Err(SimplifyError::InvalidInput(format!(
                        "primitive {primitive_index} contains a degenerate triangle"
                    )));
                }
            }
            vertices = vertices.saturating_add(count);
            indices = indices.saturating_add(primitive.indices.len());
        }
        if vertices > MAX_ELEMENTS || indices > MAX_ELEMENTS {
            return Err(SimplifyError::InvalidInput(format!(
                "asset exceeds the {MAX_ELEMENTS} element limit"
            )));
        }
        for (material_index, material) in asset.materials.iter().enumerate() {
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
                if texture >= asset.textures.len() {
                    return Err(SimplifyError::InvalidInput(format!(
                        "material {material_index} has an out-of-range texture reference"
                    )));
                }
            }
        }
        Ok(())
    }

    #[derive(Clone, Copy)]
    struct VertexRef {
        primitive: usize,
        vertex: usize,
    }

    fn build_locks(
        asset: &MeshAsset,
        config: &SimplifyConfig,
    ) -> Result<Vec<Vec<u8>>, SimplifyError> {
        let mut locks: Vec<Vec<u8>> = asset
            .primitives
            .iter()
            .map(|primitive| vec![0; primitive.positions.len()])
            .collect();
        let hard_cosine = config.hard_edge_angle_degrees.to_radians().cos();

        for (primitive_index, primitive) in asset.primitives.iter().enumerate() {
            let normals: Vec<[f32; 3]> = primitive
                .indices
                .as_chunks::<3>()
                .0
                .iter()
                .map(|triangle| {
                    face_normal(
                        primitive.positions[triangle[0] as usize],
                        primitive.positions[triangle[1] as usize],
                        primitive.positions[triangle[2] as usize],
                    )
                    .expect("validated non-degenerate triangle")
                })
                .collect();
            let mut edges: BTreeMap<(u32, u32), Vec<usize>> = BTreeMap::new();
            for (face, triangle) in primitive.indices.as_chunks::<3>().0.iter().enumerate() {
                for (a, b) in [
                    (triangle[0], triangle[1]),
                    (triangle[1], triangle[2]),
                    (triangle[2], triangle[0]),
                ] {
                    edges.entry(edge_key(a, b)).or_default().push(face);
                }
                if config.preserve_skinning && !primitive.joints.is_empty() {
                    for (a, b) in [
                        (triangle[0] as usize, triangle[1] as usize),
                        (triangle[1] as usize, triangle[2] as usize),
                        (triangle[2] as usize, triangle[0] as usize),
                    ] {
                        if joint_support(primitive, a) != joint_support(primitive, b) {
                            locks[primitive_index][a] = 1;
                            locks[primitive_index][b] = 1;
                        }
                    }
                }
            }
            for (&(a, b), uses) in &edges {
                if uses.len() > 2 {
                    return Err(SimplifyError::InvalidInput(format!(
                        "primitive {primitive_index} has a non-manifold edge"
                    )));
                }
                let boundary = uses.len() == 1 && config.lock_borders;
                let crease = uses.len() == 2
                    && config.preserve_attribute_seams
                    && dot(normals[uses[0]], normals[uses[1]]) <= hard_cosine;
                if boundary || crease {
                    locks[primitive_index][a as usize] = 1;
                    locks[primitive_index][b as usize] = 1;
                }
            }
        }

        if config.preserve_attribute_seams || config.preserve_skinning {
            let mut coincident: BTreeMap<[u32; 3], Vec<VertexRef>> = BTreeMap::new();
            for (primitive_index, primitive) in asset.primitives.iter().enumerate() {
                for (vertex, &position) in primitive.positions.iter().enumerate() {
                    coincident
                        .entry(position.map(canonical_bits))
                        .or_default()
                        .push(VertexRef {
                            primitive: primitive_index,
                            vertex,
                        });
                }
            }
            for group in coincident.values().filter(|group| group.len() > 1) {
                let first = group[0];
                let differs = group.iter().skip(1).any(|&other| {
                    seam_signature(asset, first, config) != seam_signature(asset, other, config)
                });
                if differs {
                    for reference in group {
                        locks[reference.primitive][reference.vertex] = 1;
                    }
                }
            }
        }
        Ok(locks)
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct SeamSignature {
        material: usize,
        uv: Option<[u32; 2]>,
        normal: Option<[u32; 3]>,
        joints: Option<[u16; 4]>,
    }

    fn seam_signature(
        asset: &MeshAsset,
        reference: VertexRef,
        config: &SimplifyConfig,
    ) -> SeamSignature {
        let primitive = &asset.primitives[reference.primitive];
        SeamSignature {
            material: primitive.material,
            uv: config
                .preserve_attribute_seams
                .then(|| {
                    primitive
                        .uvs
                        .get(reference.vertex)
                        .copied()
                        .map(|uv| uv.map(canonical_bits))
                })
                .flatten(),
            normal: config
                .preserve_attribute_seams
                .then(|| {
                    primitive
                        .normals
                        .get(reference.vertex)
                        .copied()
                        .map(|normal| normal.map(canonical_bits))
                })
                .flatten(),
            joints: (config.preserve_skinning && !primitive.joints.is_empty())
                .then(|| joint_support(primitive, reference.vertex)),
        }
    }

    fn canonical_bits(value: f32) -> u32 {
        if value == 0.0 {
            0
        } else {
            value.to_bits()
        }
    }

    fn joint_support(primitive: &Primitive, vertex: usize) -> [u16; 4] {
        let mut support = [u16::MAX; 4];
        if primitive.joints.is_empty() {
            return support;
        }
        let mut count = 0usize;
        for slot in 0..4 {
            if primitive.weights[vertex][slot] > 1.0e-6 {
                support[count] = primitive.joints[vertex][slot];
                count += 1;
            }
        }
        support[..count].sort_unstable();
        support
    }

    fn canonical_skin_weights(primitive: &Primitive, vertex: usize) -> [f32; 4] {
        let mut influences = [(u16::MAX, 0.0_f32); 4];
        for (slot, influence) in influences.iter_mut().enumerate() {
            if primitive.weights[vertex][slot] > 1.0e-6 {
                *influence = (
                    primitive.joints[vertex][slot],
                    primitive.weights[vertex][slot],
                );
            }
        }
        influences.sort_by_key(|influence| influence.0);
        influences.map(|influence| influence.1)
    }

    fn vertex_attributes(primitive: &Primitive, config: &SimplifyConfig) -> (Vec<f32>, Vec<f32>) {
        let normals = !primitive.normals.is_empty() && config.normal_weight > 0.0;
        let uvs = !primitive.uvs.is_empty() && config.uv_weight > 0.0;
        let skin = !primitive.weights.is_empty() && config.skin_weight > 0.0;
        let mut weights = Vec::new();
        if normals {
            weights.extend([config.normal_weight; 3]);
        }
        if uvs {
            weights.extend([config.uv_weight; 2]);
        }
        if skin {
            weights.extend([config.skin_weight; 4]);
        }
        let mut attributes = Vec::with_capacity(primitive.positions.len() * weights.len());
        for vertex in 0..primitive.positions.len() {
            if normals {
                attributes.extend(primitive.normals[vertex]);
            }
            if uvs {
                attributes.extend(primitive.uvs[vertex]);
            }
            if skin {
                attributes.extend(canonical_skin_weights(primitive, vertex));
            }
        }
        (attributes, weights)
    }

    fn validate_native_output(
        primitive: &Primitive,
        primitive_index: usize,
        indices: &[u32],
        result_error: f32,
    ) -> Result<(), SimplifyError> {
        if indices.is_empty() || !indices.len().is_multiple_of(3) {
            return Err(SimplifyError::NativeFailure(format!(
                "primitive {primitive_index} returned no complete triangles"
            )));
        }
        if indices.len() > primitive.indices.len()
            || indices
                .iter()
                .any(|&index| index as usize >= primitive.positions.len())
        {
            return Err(SimplifyError::NativeFailure(format!(
                "primitive {primitive_index} returned invalid indices"
            )));
        }
        if !result_error.is_finite() || result_error < 0.0 {
            return Err(SimplifyError::NativeFailure(format!(
                "primitive {primitive_index} returned an invalid error metric"
            )));
        }
        Ok(())
    }

    fn compact_primitive(
        primitive: &Primitive,
        indices: &[u32],
    ) -> Result<Primitive, SimplifyError> {
        let mut old_to_new = vec![u32::MAX; primitive.positions.len()];
        let mut output = Primitive {
            positions: Vec::new(),
            normals: Vec::new(),
            uvs: Vec::new(),
            // Invalid after topology changes; regenerated over the complete asset below.
            tangents: Vec::new(),
            indices: Vec::with_capacity(indices.len()),
            material: primitive.material,
            joints: Vec::new(),
            weights: Vec::new(),
        };
        let has_normals = !primitive.normals.is_empty();
        let has_uvs = !primitive.uvs.is_empty();
        let has_skin = !primitive.joints.is_empty();

        for &old in indices {
            let old_index = old as usize;
            let mapped = if old_to_new[old_index] != u32::MAX {
                old_to_new[old_index]
            } else {
                if output.positions.len() >= MAX_ELEMENTS {
                    return Err(SimplifyError::PostProcess(format!(
                        "compacted output exceeds the {MAX_ELEMENTS} vertex limit"
                    )));
                }
                let new = u32::try_from(output.positions.len()).map_err(|_| {
                    SimplifyError::PostProcess("compacted vertex index overflow".into())
                })?;
                old_to_new[old_index] = new;
                output.positions.push(primitive.positions[old_index]);
                if has_normals {
                    output.normals.push(primitive.normals[old_index]);
                }
                if has_uvs {
                    output.uvs.push(primitive.uvs[old_index]);
                }
                if has_skin {
                    output.joints.push(primitive.joints[old_index]);
                    output.weights.push(primitive.weights[old_index]);
                }
                new
            };
            output.indices.push(mapped);
        }
        Ok(output)
    }

    fn edge_key(a: u32, b: u32) -> (u32, u32) {
        if a < b {
            (a, b)
        } else {
            (b, a)
        }
    }

    fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> Option<[f32; 3]> {
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let cross = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let length2 = dot(cross, cross);
        (length2 > 1.0e-20).then(|| {
            let inverse = length2.sqrt().recip();
            [cross[0] * inverse, cross[1] * inverse, cross[2] * inverse]
        })
    }

    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use metrocalk_assets::{Material, Texture};

        fn grid(side: usize) -> MeshAsset {
            let mut positions = Vec::new();
            let mut normals = Vec::new();
            let mut uvs = Vec::new();
            for z in 0..side {
                for x in 0..side {
                    let xf = x as f32 / (side - 1) as f32;
                    let zf = z as f32 / (side - 1) as f32;
                    positions.push([xf, 0.05 * (xf * 9.0).sin() * (zf * 7.0).sin(), zf]);
                    normals.push([0.0, 1.0, 0.0]);
                    uvs.push([xf, zf]);
                }
            }
            let mut indices = Vec::new();
            for z in 0..side - 1 {
                for x in 0..side - 1 {
                    let a = (z * side + x) as u32;
                    let b = a + 1;
                    let c = a + side as u32;
                    let d = c + 1;
                    indices.extend([a, c, b, b, c, d]);
                }
            }
            let count = positions.len();
            MeshAsset {
                name: "rigged textured grid".into(),
                primitives: vec![Primitive {
                    positions,
                    normals,
                    uvs,
                    tangents: vec![[1.0, 0.0, 0.0, 1.0]; count],
                    indices,
                    material: 0,
                    joints: vec![[0, 0, 0, 0]; count],
                    weights: vec![[1.0, 0.0, 0.0, 0.0]; count],
                }],
                materials: vec![Material {
                    base_color_texture: Some(0),
                    ..Material::default()
                }],
                textures: vec![Texture {
                    width: 1,
                    height: 1,
                    rgba8: vec![255; 4],
                }],
                skeleton: Some(Default::default()),
            }
        }

        #[test]
        fn qem_reduces_textured_rigged_mesh_and_preserves_every_payload() {
            let source = grid(14);
            let result = MeshoptQemSimplifier
                .simplify(
                    &source,
                    &SimplifyConfig {
                        target_ratio: 0.55,
                        target_error: 0.1,
                        ..SimplifyConfig::default()
                    },
                )
                .expect("qem");
            let primitive = &result.asset.primitives[0];
            assert!(result.report.output_triangles < result.report.source_triangles);
            assert!(result.report.achieved_ratio < 1.0);
            assert!(result.report.locked_vertices > 0);
            assert_eq!(primitive.positions.len(), primitive.normals.len());
            assert_eq!(primitive.positions.len(), primitive.uvs.len());
            assert_eq!(primitive.positions.len(), primitive.tangents.len());
            assert_eq!(primitive.positions.len(), primitive.joints.len());
            assert_eq!(primitive.positions.len(), primitive.weights.len());
            assert!(primitive
                .weights
                .iter()
                .all(|weights| { (weights.iter().sum::<f32>() - 1.0).abs() <= 1.0e-6 }));
            assert_eq!(result.asset.materials, source.materials);
            assert_eq!(result.asset.textures, source.textures);
            assert_eq!(result.asset.skeleton, source.skeleton);
        }

        #[test]
        fn exact_uv_and_normal_discontinuities_are_explicitly_locked() {
            let mut source = grid(4);
            let primitive = &mut source.primitives[0];
            let duplicate = primitive.positions.len();
            primitive.positions.push(primitive.positions[0]);
            primitive.normals.push([0.0, -1.0, 0.0]);
            primitive.uvs.push([0.75, 0.75]);
            primitive.tangents.push([1.0, 0.0, 0.0, -1.0]);
            primitive.joints.push([0; 4]);
            primitive.weights.push([1.0, 0.0, 0.0, 0.0]);
            // Reference the duplicate in one corner so validation sees no isolated data.
            primitive.indices[0] = duplicate as u32;
            let locks = build_locks(
                &source,
                &SimplifyConfig {
                    lock_borders: false,
                    hard_edge_angle_degrees: 180.0,
                    ..SimplifyConfig::default()
                },
            )
            .expect("locks");
            assert_eq!(locks[0][0], 1);
            assert_eq!(locks[0][duplicate], 1);
        }

        #[test]
        fn pinned_native_build_is_repeatable_and_malformed_input_is_rejected() {
            let source = grid(10);
            let config = SimplifyConfig {
                target_ratio: 0.6,
                target_error: 0.1,
                ..SimplifyConfig::default()
            };
            let first = MeshoptQemSimplifier
                .simplify(&source, &config)
                .expect("first");
            let second = MeshoptQemSimplifier
                .simplify(&source, &config)
                .expect("second");
            assert_eq!(first, second);
            assert!(first
                .report
                .determinism
                .contains("not promised bit-identical"));

            let mut malformed = source;
            malformed.primitives[0].weights.pop();
            let error = MeshoptQemSimplifier
                .simplify(&malformed, &config)
                .expect_err("partial skin");
            assert!(matches!(error, SimplifyError::InvalidInput(_)));
        }
    }
}

#[cfg(feature = "qem")]
pub use native::MeshoptQemSimplifier;
