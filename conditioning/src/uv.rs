//! Chart-based UV unwrap contracts and the native xatlas adapter.
//!
//! The public surface is engine-owned and portable. The C++ xatlas implementation is compiled only with
//! the `xatlas` feature; its output is validated, normalized, converted back to [`MeshAsset`], and followed
//! by portable MikkTSpace generation so stale pre-unwrap tangents can never escape.

// Atlas dimensions are validated to 8192 and element streams to MAX_ELEMENTS before numeric narrowing.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::default_trait_access,
    clippy::many_single_char_names,
    clippy::wildcard_imports
)]

use metrocalk_assets::MeshAsset;
use std::fmt;

/// Production charting and packing controls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChartUvConfig {
    /// Requested atlas resolution in pixels. The current model supports exactly one square atlas.
    pub resolution: u32,
    /// Empty pixels reserved between packed charts.
    pub padding: u32,
    /// Exact requested pixels per local-space unit. `None` asks xatlas to fit the requested resolution.
    pub texels_per_unit: Option<f32>,
    /// Chart growth/seeding iterations.
    pub max_iterations: u32,
    /// Align packed charts to compression-friendly 4x4 blocks.
    pub block_align: bool,
    /// Use xatlas's slower exhaustive packer. This is the quality-first default.
    pub brute_force: bool,
    /// Explicit permission to replace UV0 while materials still reference textures. This must only be set
    /// by a pipeline that will reproject/rebake every bound channel before publishing the derivative.
    pub replace_textured_uv0: bool,
}

impl Default for ChartUvConfig {
    fn default() -> Self {
        Self {
            resolution: 2_048,
            padding: 8,
            texels_per_unit: None,
            max_iterations: 4,
            block_align: true,
            brute_force: true,
            replace_textured_uv0: false,
        }
    }
}

/// Measured chart, packing and texel-density evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartUvReport {
    /// Requested fixed atlas width.
    pub atlas_width: u32,
    /// Requested fixed atlas height.
    pub atlas_height: u32,
    /// Number of charts produced across every primitive.
    pub charts: usize,
    /// Fraction of atlas pixels occupied by charts, including xatlas's packed chart masks.
    pub utilization: f32,
    /// Effective xatlas unit-to-texel scale.
    pub texels_per_unit: f32,
    /// Minimum measured per-triangle texel density in pixels per local-space unit.
    pub min_texel_density: f32,
    /// Area-weighted global texel density in pixels per local-space unit.
    pub mean_texel_density: f32,
    /// Maximum measured per-triangle texel density in pixels per local-space unit.
    pub max_texel_density: f32,
    /// Source vertices before chart seams.
    pub source_vertices: usize,
    /// Vertices directly emitted by xatlas.
    pub chart_vertices: usize,
    /// Additional xatlas vertices introduced at chart seams.
    pub chart_split_vertices: usize,
    /// Additional vertices introduced by MikkTSpace orientation discontinuities.
    pub tangent_split_vertices: usize,
    /// Final production vertex count.
    pub output_vertices: usize,
    /// Honest native determinism boundary.
    pub determinism: String,
}

/// A chart-unwrapped, packed, tangent-ready derivative.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartUvResult {
    /// Immutable derived asset; source materials, textures and skeleton are retained.
    pub asset: MeshAsset,
    /// Measured evidence for the generated layout.
    pub report: ChartUvReport,
}

/// A bounded, foreign-type-free unwrap failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChartUvError {
    /// User/configuration input is outside supported production bounds.
    InvalidConfig(String),
    /// Mesh streams are malformed or unsuitable for a production chart unwrap.
    InvalidInput(String),
    /// The native adapter rejected validated input.
    NativeFailure(String),
    /// xatlas produced more than the one atlas page currently representable by `MeshAsset`.
    MultipleAtlases {
        /// Number of output pages.
        pages: u32,
    },
}

impl fmt::Display for ChartUvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(reason) => write!(f, "invalid chart UV configuration: {reason}"),
            Self::InvalidInput(reason) => write!(f, "invalid chart UV input: {reason}"),
            Self::NativeFailure(reason) => write!(f, "xatlas failed: {reason}"),
            Self::MultipleAtlases { pages } => write!(
                f,
                "xatlas produced {pages} atlas pages; the current asset model supports exactly one"
            ),
        }
    }
}

impl std::error::Error for ChartUvError {}

/// Project-owned UV unwrap boundary. Native/server and future implementations share this contract.
pub trait UvUnwrapper {
    /// Generate a non-overlapping chart layout without mutating `asset`.
    fn unwrap(
        &self,
        asset: &MeshAsset,
        config: &ChartUvConfig,
    ) -> Result<ChartUvResult, ChartUvError>;
}

#[cfg(feature = "xatlas")]
mod native {
    use super::*;
    use crate::tangent::generate_mikktspace_tangents;
    use metrocalk_assets::{Primitive, MAX_ELEMENTS};
    use metrocalk_xatlas::{Atlas, GenerateOptions};

    const MAX_ATLAS_RESOLUTION: u32 = 8_192;
    const DETERMINISM_BOUNDARY: &str = "Pinned xatlas is repeatable on the validated supported native build, but C++ floating-point charting/packing is not promised bit-identical across architectures or toolchains; persist the conditioned artifact bytes.";

    /// Native chart unwrap and packing backed by pinned xatlas.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct XatlasUvUnwrapper;

    struct AtlasInput {
        positions: Vec<f32>,
        normals: Vec<f32>,
        indices: Vec<u32>,
    }

    impl UvUnwrapper for XatlasUvUnwrapper {
        #[allow(clippy::too_many_lines)]
        fn unwrap(
            &self,
            asset: &MeshAsset,
            config: &ChartUvConfig,
        ) -> Result<ChartUvResult, ChartUvError> {
            validate_config(config)?;
            validate_asset(asset, config)?;

            let source_vertices = asset.vertex_count();
            let inputs: Vec<AtlasInput> = asset
                .primitives
                .iter()
                .map(|primitive| AtlasInput {
                    positions: primitive.positions.iter().flatten().copied().collect(),
                    normals: primitive.normals.iter().flatten().copied().collect(),
                    indices: primitive.indices.clone(),
                })
                .collect();
            let mut atlas =
                Atlas::new().map_err(|error| ChartUvError::NativeFailure(error.to_string()))?;
            let mesh_count = u32::try_from(inputs.len()).unwrap_or(u32::MAX);
            for input in &inputs {
                atlas
                    .add_mesh(&input.positions, &input.normals, &input.indices, mesh_count)
                    .map_err(|error| ChartUvError::NativeFailure(format!("add mesh: {error}")))?;
            }

            atlas.generate(GenerateOptions {
                // xatlas documents values above 1000 as fully respecting normal seams.
                normal_seam_weight: 1_001.0,
                max_iterations: config.max_iterations,
                fix_winding: true,
                padding: config.padding,
                texels_per_unit: config.texels_per_unit.unwrap_or(0.0),
                resolution: config.resolution,
                block_align: config.block_align,
                brute_force: config.brute_force,
            });

            let pages = atlas.atlas_count();
            if pages != 1 {
                return Err(if pages > 1 {
                    ChartUvError::MultipleAtlases { pages }
                } else {
                    ChartUvError::NativeFailure("no atlas page was generated".into())
                });
            }
            let packed_width = atlas.width();
            let packed_height = atlas.height();
            if packed_width == 0 || packed_height == 0 {
                return Err(ChartUvError::NativeFailure(format!(
                    "invalid output dimensions {packed_width}x{packed_height} for requested {}",
                    config.resolution
                )));
            }
            // xatlas treats `resolution` as a packing target and can return a non-square rectangle whose
            // longer dimension is slightly larger. Uniformly fit that rectangle into the requested square
            // page: chart shapes and padding remain isotropic while the editor gets an exact atlas contract.
            let packed_scale = (config.resolution as f32 / packed_width as f32)
                .min(config.resolution as f32 / packed_height as f32);
            let packed_utilization = atlas
                .utilization(0)
                .is_finite()
                .then(|| atlas.utilization(0))
                .filter(|value| (0.0..=1.0).contains(value))
                .ok_or_else(|| ChartUvError::NativeFailure("invalid utilization report".into()))?;
            let occupied_page_fraction =
                packed_width as f32 * packed_height as f32 * packed_scale * packed_scale
                    / (config.resolution as f32 * config.resolution as f32);
            let utilization = (packed_utilization * occupied_page_fraction).clamp(0.0, 1.0);
            let charts = usize::try_from(atlas.chart_count()).unwrap_or(usize::MAX);
            let effective_texels_per_unit = atlas.texels_per_unit() * packed_scale;
            if charts == 0
                || !effective_texels_per_unit.is_finite()
                || effective_texels_per_unit <= 0.0
            {
                return Err(ChartUvError::NativeFailure(
                    "xatlas returned no charts or an invalid texel scale".into(),
                ));
            }

            let atlas_meshes = atlas
                .meshes()
                .map_err(|error| ChartUvError::NativeFailure(error.to_string()))?;
            if atlas_meshes.len() != asset.primitives.len() {
                return Err(ChartUvError::NativeFailure(format!(
                    "output mesh count {} does not match input {}",
                    atlas_meshes.len(),
                    asset.primitives.len()
                )));
            }

            let mut primitives = Vec::with_capacity(asset.primitives.len());
            let mut chart_vertices = 0usize;
            for (primitive_index, (source, atlas_mesh)) in
                asset.primitives.iter().zip(&atlas_meshes).enumerate()
            {
                let output_vertices = atlas_mesh.vertices.len();
                chart_vertices = chart_vertices.saturating_add(output_vertices);
                if chart_vertices > MAX_ELEMENTS {
                    return Err(ChartUvError::NativeFailure(format!(
                        "chart seams exceed the {MAX_ELEMENTS} vertex limit"
                    )));
                }
                if atlas_mesh.indices.is_empty()
                    || !atlas_mesh.indices.len().is_multiple_of(3)
                    || atlas_mesh.indices.len() > MAX_ELEMENTS
                {
                    return Err(ChartUvError::NativeFailure(format!(
                        "primitive {primitive_index} returned an invalid triangle index stream"
                    )));
                }

                let has_skin = !source.joints.is_empty();
                let mut primitive = Primitive {
                    positions: Vec::with_capacity(output_vertices),
                    normals: Vec::with_capacity(output_vertices),
                    uvs: Vec::with_capacity(output_vertices),
                    // The UV parameterization changed, so source tangents are deliberately replaced below.
                    tangents: Vec::new(),
                    indices: atlas_mesh.indices.clone(),
                    material: source.material,
                    joints: if has_skin {
                        Vec::with_capacity(output_vertices)
                    } else {
                        Vec::new()
                    },
                    weights: if has_skin {
                        Vec::with_capacity(output_vertices)
                    } else {
                        Vec::new()
                    },
                };
                for vertex in &atlas_mesh.vertices {
                    if vertex.atlas_index != 0 || vertex.chart_index < 0 {
                        return Err(ChartUvError::NativeFailure(format!(
                            "primitive {primitive_index} references an invalid atlas/chart"
                        )));
                    }
                    let source_index = usize::try_from(vertex.xref)
                        .ok()
                        .filter(|&index| index < source.positions.len())
                        .ok_or_else(|| {
                            ChartUvError::NativeFailure(format!(
                                "primitive {primitive_index} returned an invalid source vertex reference"
                            ))
                        })?;
                    let mut uv = [
                        vertex.uv[0] * packed_scale / config.resolution as f32,
                        vertex.uv[1] * packed_scale / config.resolution as f32,
                    ];
                    if uv
                        .iter()
                        .any(|value| !value.is_finite() || *value < -1.0e-6 || *value > 1.000_001)
                    {
                        return Err(ChartUvError::NativeFailure(format!(
                            "primitive {primitive_index} returned a UV outside its atlas page"
                        )));
                    }
                    uv[0] = uv[0].clamp(0.0, 1.0);
                    uv[1] = uv[1].clamp(0.0, 1.0);
                    primitive.positions.push(source.positions[source_index]);
                    primitive.normals.push(source.normals[source_index]);
                    primitive.uvs.push(uv);
                    if has_skin {
                        primitive.joints.push(source.joints[source_index]);
                        primitive.weights.push(source.weights[source_index]);
                    }
                }
                if primitive
                    .indices
                    .iter()
                    .any(|&index| index as usize >= primitive.positions.len())
                {
                    return Err(ChartUvError::NativeFailure(format!(
                        "primitive {primitive_index} output index is out of range"
                    )));
                }
                primitives.push(primitive);
            }

            let chart_asset = MeshAsset {
                name: asset.name.clone(),
                primitives,
                materials: asset.materials.clone(),
                textures: asset.textures.clone(),
                skeleton: asset.skeleton.clone(),
            };
            // Mikk must run after the final UV layout. This also performs the exact orientation seam split
            // while duplicating skin streams, so callers cannot accidentally render stale source tangents.
            let (conditioned, tangent_report) = generate_mikktspace_tangents(&chart_asset)
                .map_err(|error| ChartUvError::NativeFailure(error.to_string()))?;
            let density = texel_density(&conditioned, config.resolution, config.resolution)?;
            let output_vertices = conditioned.vertex_count();

            Ok(ChartUvResult {
                asset: conditioned,
                report: ChartUvReport {
                    atlas_width: config.resolution,
                    atlas_height: config.resolution,
                    charts,
                    utilization,
                    texels_per_unit: effective_texels_per_unit,
                    min_texel_density: density.0,
                    mean_texel_density: density.1,
                    max_texel_density: density.2,
                    source_vertices,
                    chart_vertices,
                    chart_split_vertices: chart_vertices.saturating_sub(source_vertices),
                    tangent_split_vertices: tangent_report.split_vertices,
                    output_vertices,
                    determinism: DETERMINISM_BOUNDARY.into(),
                },
            })
        }
    }

    fn validate_config(config: &ChartUvConfig) -> Result<(), ChartUvError> {
        if !(64..=MAX_ATLAS_RESOLUTION).contains(&config.resolution) {
            return Err(ChartUvError::InvalidConfig(format!(
                "resolution must be between 64 and {MAX_ATLAS_RESOLUTION}"
            )));
        }
        if config.padding.saturating_mul(2) >= config.resolution {
            return Err(ChartUvError::InvalidConfig(
                "padding must leave usable atlas area".into(),
            ));
        }
        if config.max_iterations == 0 || config.max_iterations > 64 {
            return Err(ChartUvError::InvalidConfig(
                "max_iterations must be between 1 and 64".into(),
            ));
        }
        if config
            .texels_per_unit
            .is_some_and(|value| !value.is_finite() || value <= 0.0 || value > 1.0e6)
        {
            return Err(ChartUvError::InvalidConfig(
                "texels_per_unit must be finite and between zero and one million".into(),
            ));
        }
        Ok(())
    }

    fn validate_asset(asset: &MeshAsset, config: &ChartUvConfig) -> Result<(), ChartUvError> {
        if asset.primitives.is_empty() {
            return Err(ChartUvError::InvalidInput(
                "the asset has no primitives".into(),
            ));
        }
        let has_bound_textures = asset.materials.iter().any(|material| {
            material.base_color_texture.is_some()
                || material.metallic_roughness_texture.is_some()
                || material.normal_texture.is_some()
                || material.occlusion_texture.is_some()
                || material.curvature_texture.is_some()
        });
        if has_bound_textures && !config.replace_textured_uv0 {
            return Err(ChartUvError::InvalidInput(
                "replacing UV0 would invalidate bound textures; enable replace_textured_uv0 only inside a complete texture-reprojection bake"
                    .into(),
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
                return Err(ChartUvError::InvalidInput(format!(
                    "primitive {primitive_index} is not a non-empty triangle list"
                )));
            }
            if primitive.normals.len() != count {
                return Err(ChartUvError::InvalidInput(format!(
                    "primitive {primitive_index} needs complete normals before charting"
                )));
            }
            if !primitive.tangents.is_empty() && primitive.tangents.len() != count {
                return Err(ChartUvError::InvalidInput(format!(
                    "primitive {primitive_index} has a partial tangent stream"
                )));
            }
            let skin_empty = primitive.joints.is_empty() && primitive.weights.is_empty();
            let skin_complete = primitive.joints.len() == count && primitive.weights.len() == count;
            if !skin_empty && !skin_complete {
                return Err(ChartUvError::InvalidInput(format!(
                    "primitive {primitive_index} has partial skin streams"
                )));
            }
            if primitive.material >= asset.materials.len() {
                return Err(ChartUvError::InvalidInput(format!(
                    "primitive {primitive_index} material index is out of range"
                )));
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
                return Err(ChartUvError::InvalidInput(format!(
                    "primitive {primitive_index} contains non-finite vertex data"
                )));
            }
            for triangle in primitive.indices.chunks_exact(3) {
                let [a, b, c] = [
                    triangle[0] as usize,
                    triangle[1] as usize,
                    triangle[2] as usize,
                ];
                if a >= count || b >= count || c >= count {
                    return Err(ChartUvError::InvalidInput(format!(
                        "primitive {primitive_index} has an out-of-range index"
                    )));
                }
                if a == b
                    || b == c
                    || a == c
                    || triangle_area3(
                        primitive.positions[a],
                        primitive.positions[b],
                        primitive.positions[c],
                    ) <= 1.0e-20
                {
                    return Err(ChartUvError::InvalidInput(format!(
                        "primitive {primitive_index} has a degenerate triangle"
                    )));
                }
            }
            vertices = vertices.saturating_add(count);
            indices = indices.saturating_add(primitive.indices.len());
        }
        if vertices > MAX_ELEMENTS || indices > MAX_ELEMENTS {
            return Err(ChartUvError::InvalidInput(format!(
                "asset exceeds the {MAX_ELEMENTS} element limit"
            )));
        }
        Ok(())
    }

    fn texel_density(
        asset: &MeshAsset,
        width: u32,
        height: u32,
    ) -> Result<(f32, f32, f32), ChartUvError> {
        let mut minimum = f32::INFINITY;
        let mut maximum = 0.0_f32;
        let mut total_world_area = 0.0_f64;
        let mut total_pixel_area = 0.0_f64;
        for primitive in &asset.primitives {
            for triangle in primitive.indices.chunks_exact(3) {
                let [a, b, c] = [
                    triangle[0] as usize,
                    triangle[1] as usize,
                    triangle[2] as usize,
                ];
                let world_area = triangle_area3(
                    primitive.positions[a],
                    primitive.positions[b],
                    primitive.positions[c],
                );
                let uv_area = triangle_area2(primitive.uvs[a], primitive.uvs[b], primitive.uvs[c]);
                let pixel_area = uv_area * width as f32 * height as f32;
                if world_area <= 1.0e-20 || pixel_area <= 1.0e-20 {
                    return Err(ChartUvError::NativeFailure(
                        "chart output contains a degenerate geometry or UV triangle".into(),
                    ));
                }
                let density = (pixel_area / world_area).sqrt();
                minimum = minimum.min(density);
                maximum = maximum.max(density);
                total_world_area += f64::from(world_area);
                total_pixel_area += f64::from(pixel_area);
            }
        }
        if !minimum.is_finite() || total_world_area <= 0.0 || total_pixel_area <= 0.0 {
            return Err(ChartUvError::NativeFailure(
                "could not measure output texel density".into(),
            ));
        }
        let mean = (total_pixel_area / total_world_area).sqrt() as f32;
        Ok((minimum, mean, maximum))
    }

    fn triangle_area3(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f32 {
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let cross = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        0.5 * (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt()
    }

    fn triangle_area2(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
        0.5 * ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs()
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use metrocalk_assets::{Material, Texture};

        fn cube() -> MeshAsset {
            let faces = [
                (
                    [1.0, 0.0, 0.0],
                    [
                        [1.0, -1.0, -1.0],
                        [1.0, 1.0, -1.0],
                        [1.0, 1.0, 1.0],
                        [1.0, -1.0, 1.0],
                    ],
                ),
                (
                    [-1.0, 0.0, 0.0],
                    [
                        [-1.0, -1.0, 1.0],
                        [-1.0, 1.0, 1.0],
                        [-1.0, 1.0, -1.0],
                        [-1.0, -1.0, -1.0],
                    ],
                ),
                (
                    [0.0, 1.0, 0.0],
                    [
                        [-1.0, 1.0, -1.0],
                        [-1.0, 1.0, 1.0],
                        [1.0, 1.0, 1.0],
                        [1.0, 1.0, -1.0],
                    ],
                ),
                (
                    [0.0, -1.0, 0.0],
                    [
                        [-1.0, -1.0, 1.0],
                        [-1.0, -1.0, -1.0],
                        [1.0, -1.0, -1.0],
                        [1.0, -1.0, 1.0],
                    ],
                ),
                (
                    [0.0, 0.0, 1.0],
                    [
                        [-1.0, -1.0, 1.0],
                        [1.0, -1.0, 1.0],
                        [1.0, 1.0, 1.0],
                        [-1.0, 1.0, 1.0],
                    ],
                ),
                (
                    [0.0, 0.0, -1.0],
                    [
                        [1.0, -1.0, -1.0],
                        [-1.0, -1.0, -1.0],
                        [-1.0, 1.0, -1.0],
                        [1.0, 1.0, -1.0],
                    ],
                ),
            ];
            let mut positions = Vec::new();
            let mut normals = Vec::new();
            let mut uvs = Vec::new();
            let mut indices = Vec::new();
            for (normal, corners) in faces {
                let base = positions.len() as u32;
                positions.extend(corners);
                normals.extend([normal; 4]);
                uvs.extend([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
                indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            }
            let vertex_count = positions.len();
            MeshAsset {
                name: "cube".into(),
                primitives: vec![Primitive {
                    positions,
                    normals,
                    uvs,
                    tangents: vec![[1.0, 0.0, 0.0, 1.0]; vertex_count],
                    indices,
                    material: 0,
                    joints: vec![[0, 1, 0, 0]; vertex_count],
                    weights: vec![[0.8, 0.2, 0.0, 0.0]; vertex_count],
                }],
                materials: vec![Material::default()],
                textures: vec![Texture {
                    width: 1,
                    height: 1,
                    rgba8: vec![255; 4],
                }],
                skeleton: Some(Default::default()),
            }
        }

        #[test]
        fn unwrap_is_single_page_measured_and_preserves_parallel_streams() {
            let source = cube();
            let result = XatlasUvUnwrapper
                .unwrap(
                    &source,
                    &ChartUvConfig {
                        resolution: 256,
                        padding: 4,
                        ..ChartUvConfig::default()
                    },
                )
                .expect("unwrap");
            let primitive = &result.asset.primitives[0];
            assert_eq!(result.report.atlas_width, 256);
            assert_eq!(result.report.atlas_height, 256);
            assert!(result.report.charts >= 6);
            assert!((0.0..=1.0).contains(&result.report.utilization));
            assert!(result.report.min_texel_density > 0.0);
            assert!(result.report.mean_texel_density >= result.report.min_texel_density);
            assert!(result.report.max_texel_density >= result.report.mean_texel_density);
            assert_eq!(primitive.positions.len(), primitive.normals.len());
            assert_eq!(primitive.positions.len(), primitive.uvs.len());
            assert_eq!(primitive.positions.len(), primitive.tangents.len());
            assert_eq!(primitive.positions.len(), primitive.joints.len());
            assert_eq!(primitive.positions.len(), primitive.weights.len());
            assert!(primitive
                .uvs
                .iter()
                .flatten()
                .all(|value| (0.0..=1.0).contains(value)));
            assert_eq!(result.asset.materials, source.materials);
            assert_eq!(result.asset.textures, source.textures);
            assert_eq!(result.asset.skeleton, source.skeleton);
        }

        #[test]
        fn pinned_native_build_is_repeatable_without_claiming_cross_platform_bits() {
            let source = cube();
            let config = ChartUvConfig {
                resolution: 128,
                padding: 2,
                ..ChartUvConfig::default()
            };
            let first = XatlasUvUnwrapper.unwrap(&source, &config).expect("first");
            let second = XatlasUvUnwrapper.unwrap(&source, &config).expect("second");
            assert_eq!(first, second);
            assert!(first
                .report
                .determinism
                .contains("not promised bit-identical"));
        }

        #[test]
        fn unsafe_configuration_and_partial_skin_are_rejected_before_ffi() {
            let source = cube();
            let error = XatlasUvUnwrapper
                .unwrap(
                    &source,
                    &ChartUvConfig {
                        resolution: 32,
                        ..ChartUvConfig::default()
                    },
                )
                .expect_err("small atlas");
            assert!(matches!(error, ChartUvError::InvalidConfig(_)));

            let mut source = cube();
            source.primitives[0].weights.pop();
            let error = XatlasUvUnwrapper
                .unwrap(&source, &ChartUvConfig::default())
                .expect_err("partial skin");
            assert!(matches!(error, ChartUvError::InvalidInput(_)));

            let mut source = cube();
            source.materials[0].normal_texture = Some(0);
            let error = XatlasUvUnwrapper
                .unwrap(&source, &ChartUvConfig::default())
                .expect_err("textured UV0 needs an explicit full-reprojection contract");
            assert!(error.to_string().contains("replace_textured_uv0"));
        }
    }
}

#[cfg(feature = "xatlas")]
pub use native::XatlasUvUnwrapper;
