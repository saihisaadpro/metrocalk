//! Deterministic CPU high-to-low texture baking.
//!
//! The baker deliberately owns no renderer or decoder types. It accepts the engine mesh model plus full
//! affine transforms, rasterizes the final low-poly UV charts, projects against a stable BVH containing
//! any number of high-poly sources, and emits tangent-space normal, ambient-occlusion and signed-curvature
//! textures. The implementation is the portable reference path; a future GPU backend can implement the
//! same [`TextureBaker`] contract without changing Asset Lab or export code.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::float_cmp,
    clippy::struct_excessive_bools
)]

use crate::tangent::generate_mikktspace_tangents;
use metrocalk_assets::{AssetAffine, Material, MeshAsset, Primitive, Texture, MAX_ELEMENTS};
use std::collections::VecDeque;
use std::fmt;

const PI: f32 = std::f32::consts::PI;
const LEAF_TRIANGLES: usize = 8;
const DETERMINISM: &str = "The portable CPU rasterizer, BVH ordering, Hammersley AO sequence and dilation queue are deterministic for identical IEEE-754 inputs on the supported build; persist published artifact bytes when cross-toolchain bit identity is required.";

/// A row-major affine matrix. Points are multiplied as `matrix * [x,y,z,1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BakeTransform {
    /// Row-major 4x4 affine matrix.
    pub matrix: [[f32; 4]; 4],
}

impl BakeTransform {
    /// Identity transform.
    pub const IDENTITY: Self = Self {
        matrix: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
    };

    /// Convert the editor's canonical uniform asset-display affine into the full bake contract.
    #[must_use]
    pub fn from_asset_affine(affine: AssetAffine) -> Self {
        Self {
            matrix: [
                [affine.scale, 0.0, 0.0, affine.translation[0]],
                [0.0, affine.scale, 0.0, affine.translation[1]],
                [0.0, 0.0, affine.scale, affine.translation[2]],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }
}

impl Default for BakeTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// One high-resolution source and its asset-local to common bake-space transform.
#[derive(Clone, Copy, Debug)]
pub struct BakeSource<'a> {
    /// Source mesh. UVs and materials are not required.
    pub asset: &'a MeshAsset,
    /// Source-local to common bake-space transform.
    pub transform: BakeTransform,
}

/// Bounded quality controls for a texture bake.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BakeConfig {
    /// Width and height of the single square output atlas.
    pub resolution: u32,
    /// Dilation distance around chart coverage, in pixels.
    pub padding: u32,
    /// Maximum distance on either side of the low surface searched for a high-poly hit.
    pub cage_distance: f32,
    /// Small world-space origin offset used to avoid AO self-intersections.
    pub ray_bias: f32,
    /// Maximum AO ray length in common bake space.
    pub ao_distance: f32,
    /// Deterministic cosine-hemisphere samples per covered texel.
    pub ao_samples: u32,
    /// Multiplier mapping signed mean curvature to the `[0,1]` output range.
    pub curvature_scale: f32,
    /// Emit a tangent-space OpenGL-style normal map (`+Y`, glTF convention).
    pub bake_normal: bool,
    /// Emit ambient occlusion in the red channel.
    pub bake_ao: bool,
    /// Emit signed mean curvature (concave dark, convex bright).
    pub bake_curvature: bool,
    /// Flip a sampled high normal to the low surface hemisphere when winding disagrees.
    pub match_normal_orientation: bool,
    /// Minimum fraction of covered low-poly texels that must hit a high-detail source. A bake below
    /// this quality floor is rejected before padding or asset publication so correctly-sized blank or
    /// mostly blank textures can never masquerade as a successful result.
    pub min_projection_hit_ratio: f32,
    /// Hard high-poly triangle budget before BVH allocation.
    pub max_source_triangles: usize,
    /// Hard total output texture byte budget.
    pub max_output_bytes: usize,
    /// Hard conservative projection + AO ray budget.
    pub max_rays: u64,
}

impl Default for BakeConfig {
    fn default() -> Self {
        Self {
            resolution: 1_024,
            padding: 16,
            cage_distance: 0.1,
            ray_bias: 1.0e-4,
            ao_distance: 0.25,
            ao_samples: 32,
            curvature_scale: 0.05,
            bake_normal: true,
            bake_ao: true,
            bake_curvature: true,
            match_normal_orientation: true,
            min_projection_hit_ratio: 0.9,
            max_source_triangles: 5_000_000,
            max_output_bytes: 512 * 1_024 * 1_024,
            max_rays: 100_000_000,
        }
    }
}

/// Stage supplied to an asynchronous Asset Lab progress surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BakeStage {
    /// Validate streams, derive normals/tangents and build the high-poly BVH.
    Preparing,
    /// Rasterize UV charts and project covered texels.
    Rasterizing,
    /// Grow chart colors into the configured padding region.
    Dilating,
    /// All output maps and evidence are complete.
    Complete,
}

/// Bounded progress snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BakeProgress {
    /// Current stage.
    pub stage: BakeStage,
    /// Completed deterministic work units.
    pub completed: u64,
    /// Total work units for this stage.
    pub total: u64,
}

/// Cancellation/progress seam used by the native job system.
pub trait BakeObserver: Send + Sync {
    /// Return true to stop at the next bounded checkpoint.
    fn is_cancelled(&self) -> bool {
        false
    }

    /// Receive a monotonic stage-local progress snapshot.
    fn on_progress(&self, _progress: BakeProgress) {}
}

#[derive(Clone, Copy, Debug, Default)]
struct NoopObserver;

impl BakeObserver for NoopObserver {}

/// Generated map payloads. Disabled channels remain `None`; coverage is always emitted as engineering
/// evidence (`R=low coverage`, `G=successful high projection`, `B=dilated padding`).
#[derive(Clone, Debug, PartialEq)]
pub struct BakeMaps {
    /// Tangent-space normal map.
    pub normal: Option<Texture>,
    /// Ambient-occlusion map.
    pub ambient_occlusion: Option<Texture>,
    /// Signed-curvature map.
    pub curvature: Option<Texture>,
    /// Coverage/projection/dilation diagnostic mask.
    pub coverage: Texture,
}

impl BakeMaps {
    /// Attach generated material channels to an immutable derivative of `asset`.
    ///
    /// Every primitive shares the one generated atlas. Assets without a material receive a production
    /// default so the maps are immediately visible in the viewport and exportable.
    pub fn attach_to(&self, asset: &MeshAsset) -> Result<MeshAsset, BakeError> {
        let mut output = asset.clone();
        if output.materials.is_empty() {
            output.materials.push(Material::default());
            for primitive in &mut output.primitives {
                primitive.material = 0;
            }
        }
        if output
            .primitives
            .iter()
            .any(|primitive| primitive.material >= output.materials.len())
        {
            return Err(BakeError::InvalidTarget(
                "a primitive material index is out of range".into(),
            ));
        }

        let normal = append_texture(&mut output, self.normal.as_ref());
        let ao = append_texture(&mut output, self.ambient_occlusion.as_ref());
        let curvature = append_texture(&mut output, self.curvature.as_ref());
        for material in &mut output.materials {
            if normal.is_some() {
                material.normal_texture = normal;
            }
            if ao.is_some() {
                material.occlusion_texture = ao;
            }
            if curvature.is_some() {
                material.curvature_texture = curvature;
            }
        }
        Ok(output)
    }
}

fn append_texture(asset: &mut MeshAsset, texture: Option<&Texture>) -> Option<usize> {
    texture.map(|texture| {
        let index = asset.textures.len();
        asset.textures.push(texture.clone());
        index
    })
}

/// Measured bake evidence suitable for Asset Lab and packaged QA artifacts.
#[derive(Clone, Debug, PartialEq)]
pub struct BakeReport {
    /// Final target triangle count after any tangent seam splitting.
    pub target_triangles: usize,
    /// Final target vertex count after MikkTSpace tangent seam splitting.
    pub target_vertices: usize,
    /// Source triangles in the shared projection BVH.
    pub source_triangles: usize,
    /// Connected low-poly UV chart count.
    pub charts: usize,
    /// Texels covered by low-poly triangles before dilation.
    pub covered_texels: usize,
    /// Covered texels that projected onto a high source.
    pub projected_texels: usize,
    /// Covered texels with no high-poly hit inside the cage.
    pub projection_misses: usize,
    /// Pixels where overlapping UV triangles competed; the stable first triangle won.
    pub overlap_texels: usize,
    /// Previously empty texels populated by chart padding.
    pub dilated_texels: usize,
    /// Actual projection and AO rays submitted to the BVH.
    pub rays_cast: u64,
    /// AO rays that hit geometry.
    pub occluded_rays: u64,
    /// Fraction of atlas pixels covered before padding.
    pub coverage_ratio: f32,
    /// Fraction of covered pixels with a successful high projection.
    pub projection_hit_ratio: f32,
    /// Minimum sampled signed curvature before encoding.
    pub min_curvature: f32,
    /// Maximum sampled signed curvature before encoding.
    pub max_curvature: f32,
    /// Honest reproducibility boundary.
    pub determinism: String,
}

/// Completed output and evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct BakeResult {
    /// Generated maps.
    pub maps: BakeMaps,
    /// Measured evidence.
    pub report: BakeReport,
    /// Low-poly derivative whose normals and MikkTSpace tangents exactly match the bake basis.
    pub conditioned_target: MeshAsset,
}

/// A bounded, dependency-type-free bake failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BakeError {
    /// Invalid configuration or exceeded work budget.
    InvalidConfig(String),
    /// Malformed or unsuitable low-poly target.
    InvalidTarget(String),
    /// Malformed or unsuitable high-poly source.
    InvalidSource { source: usize, reason: String },
    /// MikkTSpace preparation failed.
    TangentGeneration(String),
    /// The target rasterized, but too few covered texels found a high-detail source inside the cage.
    InsufficientProjection {
        /// Low-poly texels covered before padding.
        covered_texels: usize,
        /// Covered texels with a high-detail hit.
        projected_texels: usize,
        /// Required ratio, quantized to millionths for stable, equality-testable evidence.
        minimum_hit_ratio_millionths: u32,
    },
    /// Observer requested cancellation.
    Cancelled,
}

impl fmt::Display for BakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(reason) => write!(f, "invalid bake configuration: {reason}"),
            Self::InvalidTarget(reason) => write!(f, "invalid low-poly bake target: {reason}"),
            Self::InvalidSource { source, reason } => {
                write!(f, "invalid high-poly source {source}: {reason}")
            }
            Self::TangentGeneration(reason) => {
                write!(f, "MikkTSpace target preparation failed: {reason}")
            }
            Self::InsufficientProjection {
                covered_texels,
                projected_texels,
                minimum_hit_ratio_millionths,
            } => {
                let actual = if *covered_texels == 0 {
                    0.0
                } else {
                    *projected_texels as f64 / *covered_texels as f64
                };
                let required = f64::from(*minimum_hit_ratio_millionths) / 1_000_000.0;
                write!(
                    f,
                    "high-to-low projection reached {:.1}% ({projected_texels}/{covered_texels} covered texels), below the required {:.1}%; use Auto-align related assets, increase the cage only when geometry genuinely differs, or choose Preserve scene positions for pre-registered world-space sources",
                    actual * 100.0,
                    required * 100.0,
                )
            }
            Self::Cancelled => f.write_str("texture bake was cancelled"),
        }
    }
}

impl std::error::Error for BakeError {}

/// Project-owned high-to-low baker boundary.
pub trait TextureBaker {
    /// Bake `target` against every `source` in a shared coordinate space.
    fn bake(
        &self,
        target: &MeshAsset,
        target_transform: BakeTransform,
        sources: &[BakeSource<'_>],
        config: &BakeConfig,
        observer: &dyn BakeObserver,
    ) -> Result<BakeResult, BakeError>;
}

/// Portable deterministic CPU reference baker.
#[derive(Clone, Copy, Debug, Default)]
pub struct CpuTextureBaker;

impl CpuTextureBaker {
    /// Convenience path for synchronous callers that do not need progress or cancellation.
    pub fn bake_unobserved(
        self,
        target: &MeshAsset,
        target_transform: BakeTransform,
        sources: &[BakeSource<'_>],
        config: &BakeConfig,
    ) -> Result<BakeResult, BakeError> {
        TextureBaker::bake(
            &self,
            target,
            target_transform,
            sources,
            config,
            &NoopObserver,
        )
    }
}

impl TextureBaker for CpuTextureBaker {
    #[allow(clippy::too_many_lines)]
    fn bake(
        &self,
        target: &MeshAsset,
        target_transform: BakeTransform,
        sources: &[BakeSource<'_>],
        config: &BakeConfig,
        observer: &dyn BakeObserver,
    ) -> Result<BakeResult, BakeError> {
        validate_config(config)?;
        checkpoint(observer, BakeStage::Preparing, 0, 1)?;
        if sources.is_empty() {
            return Err(BakeError::InvalidConfig(
                "at least one high-poly source is required".into(),
            ));
        }
        let target_transform = PreparedTransform::new(target_transform).ok_or_else(|| {
            BakeError::InvalidTarget("target transform is not invertible affine".into())
        })?;
        let conditioned_target = prepare_target(target)?;
        let target_triangles = conditioned_target.triangle_count();
        if target_triangles == 0 {
            return Err(BakeError::InvalidTarget(
                "the target contains no drawable triangles".into(),
            ));
        }

        let mut source_triangles = Vec::new();
        for (source_index, source) in sources.iter().enumerate() {
            let transform = PreparedTransform::new(source.transform).ok_or_else(|| {
                BakeError::InvalidSource {
                    source: source_index,
                    reason: "transform is not invertible affine".into(),
                }
            })?;
            append_source_triangles(
                &mut source_triangles,
                source.asset,
                transform,
                source_index,
                config.max_source_triangles,
            )?;
        }
        if source_triangles.is_empty() {
            return Err(BakeError::InvalidConfig(
                "the high-poly sources contain no drawable triangles".into(),
            ));
        }
        let bvh = TriangleBvh::new(source_triangles);
        checkpoint(observer, BakeStage::Preparing, 1, 1)?;

        let resolution = config.resolution as usize;
        let pixel_count = resolution * resolution;
        let mut normal_pixels = config
            .bake_normal
            .then(|| filled_rgba(pixel_count, [128, 128, 255, 255]));
        let mut ao_pixels = config
            .bake_ao
            .then(|| filled_rgba(pixel_count, [255, 255, 255, 255]));
        let mut curvature_pixels = config
            .bake_curvature
            .then(|| filled_rgba(pixel_count, [128, 128, 128, 255]));
        let mut coverage_pixels = filled_rgba(pixel_count, [0, 0, 0, 255]);
        let mut covered = vec![false; pixel_count];
        let mut chart_owner = vec![u32::MAX; pixel_count];

        let total_triangles = conditioned_target.triangle_count() as u64;
        let mut triangle_base = 0u64;
        let mut chart_base = 0u32;
        let mut chart_count = 0usize;
        let mut projected_texels = 0usize;
        let mut projection_misses = 0usize;
        let mut overlap_texels = 0usize;
        let mut rays_cast = 0u64;
        let mut occluded_rays = 0u64;
        let mut min_curvature = f32::INFINITY;
        let mut max_curvature = f32::NEG_INFINITY;

        for primitive in &conditioned_target.primitives {
            let charts = triangle_charts(primitive);
            let primitive_charts = charts.iter().copied().max().map_or(0, |value| value + 1);
            chart_count += primitive_charts as usize;
            for (triangle, indices) in primitive.indices.chunks_exact(3).enumerate() {
                if triangle % 32 == 0 {
                    checkpoint(
                        observer,
                        BakeStage::Rasterizing,
                        triangle_base + triangle as u64,
                        total_triangles,
                    )?;
                }
                let index = [
                    indices[0] as usize,
                    indices[1] as usize,
                    indices[2] as usize,
                ];
                let uv = [
                    primitive.uvs[index[0]],
                    primitive.uvs[index[1]],
                    primitive.uvs[index[2]],
                ];
                let bounds = uv_pixel_bounds(uv, config.resolution);
                let chart = chart_base + charts[triangle];
                for y in bounds[1]..=bounds[3] {
                    for x in bounds[0]..=bounds[2] {
                        let sample_uv = [
                            (x as f32 + 0.5) / config.resolution as f32,
                            (y as f32 + 0.5) / config.resolution as f32,
                        ];
                        let Some(barycentric) = barycentric_2d(sample_uv, uv) else {
                            continue;
                        };
                        let pixel = y as usize * resolution + x as usize;
                        if covered[pixel] {
                            overlap_texels += 1;
                            continue;
                        }
                        covered[pixel] = true;
                        chart_owner[pixel] = chart;
                        coverage_pixels[pixel * 4] = 255;

                        let low_point = target_transform.point(interpolate3(
                            primitive.positions[index[0]],
                            primitive.positions[index[1]],
                            primitive.positions[index[2]],
                            barycentric,
                        ));
                        let low_normal = target_transform.normal(interpolate3(
                            primitive.normals[index[0]],
                            primitive.normals[index[1]],
                            primitive.normals[index[2]],
                            barycentric,
                        ));
                        let tangent_local = interpolate4(
                            primitive.tangents[index[0]],
                            primitive.tangents[index[1]],
                            primitive.tangents[index[2]],
                            barycentric,
                        );
                        let mut low_tangent = target_transform.vector([
                            tangent_local[0],
                            tangent_local[1],
                            tangent_local[2],
                        ]);
                        low_tangent = normalize(sub(
                            low_tangent,
                            mul(low_normal, dot(low_normal, low_tangent)),
                        ));
                        let handedness = if tangent_local[3] < 0.0 { -1.0 } else { 1.0 };
                        let low_bitangent = mul(cross(low_normal, low_tangent), handedness);

                        let cage = config.cage_distance;
                        let positive = bvh.nearest(
                            add(low_point, mul(low_normal, cage)),
                            mul(low_normal, -1.0),
                            cage * 2.0,
                        );
                        let negative = bvh.nearest(
                            sub(low_point, mul(low_normal, cage)),
                            low_normal,
                            cage * 2.0,
                        );
                        rays_cast += 2;
                        let selected =
                            closest_to_low(positive, negative, low_point, &bvh.triangles);
                        let Some(hit) = selected else {
                            projection_misses += 1;
                            continue;
                        };
                        projected_texels += 1;
                        coverage_pixels[pixel * 4 + 1] = 255;
                        let triangle = &bvh.triangles[hit.triangle];
                        let high_point = interpolate3(
                            triangle.positions[0],
                            triangle.positions[1],
                            triangle.positions[2],
                            hit.barycentric,
                        );
                        let mut high_normal = normalize(interpolate3(
                            triangle.normals[0],
                            triangle.normals[1],
                            triangle.normals[2],
                            hit.barycentric,
                        ));
                        if config.match_normal_orientation && dot(high_normal, low_normal) < 0.0 {
                            high_normal = mul(high_normal, -1.0);
                        }

                        if let Some(pixels) = &mut normal_pixels {
                            let tangent_normal = normalize([
                                dot(high_normal, low_tangent),
                                dot(high_normal, low_bitangent),
                                dot(high_normal, low_normal),
                            ]);
                            write_gray_or_vector(
                                pixels,
                                pixel,
                                [
                                    encode_signed(tangent_normal[0]),
                                    encode_signed(tangent_normal[1]),
                                    encode_signed(tangent_normal[2]),
                                ],
                            );
                        }

                        if let Some(pixels) = &mut curvature_pixels {
                            let curvature = interpolate1(triangle.curvature, hit.barycentric);
                            min_curvature = min_curvature.min(curvature);
                            max_curvature = max_curvature.max(curvature);
                            let encoded = encode_signed(curvature * config.curvature_scale);
                            write_gray_or_vector(pixels, pixel, [encoded; 3]);
                        }

                        if let Some(pixels) = &mut ao_pixels {
                            let mut blocked = 0u32;
                            for sample in 0..config.ao_samples {
                                let direction =
                                    hemisphere_direction(sample, config.ao_samples, high_normal);
                                let origin = add(high_point, mul(high_normal, config.ray_bias));
                                if bvh.any_hit(origin, direction, config.ao_distance) {
                                    blocked += 1;
                                }
                            }
                            rays_cast += u64::from(config.ao_samples);
                            occluded_rays += u64::from(blocked);
                            let ao = 1.0 - blocked as f32 / config.ao_samples as f32;
                            let encoded = encode_unit(ao);
                            write_gray_or_vector(pixels, pixel, [encoded; 3]);
                        }
                    }
                }
            }
            triangle_base += (primitive.indices.len() / 3) as u64;
            chart_base += primitive_charts;
        }
        checkpoint(
            observer,
            BakeStage::Rasterizing,
            total_triangles,
            total_triangles,
        )?;

        let covered_texels = covered.iter().filter(|value| **value).count();
        if covered_texels == 0 {
            return Err(BakeError::InvalidTarget(
                "UV triangles cover no pixel centers at the requested resolution".into(),
            ));
        }
        let projection_hit_ratio = projected_texels as f64 / covered_texels as f64;
        if projected_texels == 0
            || projection_hit_ratio + f64::EPSILON < f64::from(config.min_projection_hit_ratio)
        {
            return Err(BakeError::InsufficientProjection {
                covered_texels,
                projected_texels,
                minimum_hit_ratio_millionths: (config.min_projection_hit_ratio * 1_000_000.0)
                    .round() as u32,
            });
        }
        checkpoint(observer, BakeStage::Dilating, 0, u64::from(config.padding))?;
        let dilated_texels = dilate_maps(
            resolution,
            config.padding,
            &covered,
            &mut chart_owner,
            normal_pixels.as_mut(),
            ao_pixels.as_mut(),
            curvature_pixels.as_mut(),
            &mut coverage_pixels,
            observer,
        )?;
        checkpoint(observer, BakeStage::Complete, 1, 1)?;

        if !min_curvature.is_finite() {
            min_curvature = 0.0;
            max_curvature = 0.0;
        }
        let texture = |rgba8| Texture {
            width: config.resolution,
            height: config.resolution,
            rgba8,
        };
        Ok(BakeResult {
            maps: BakeMaps {
                normal: normal_pixels.map(texture),
                ambient_occlusion: ao_pixels.map(texture),
                curvature: curvature_pixels.map(texture),
                coverage: texture(coverage_pixels),
            },
            report: BakeReport {
                target_triangles,
                target_vertices: conditioned_target.vertex_count(),
                source_triangles: bvh.triangles.len(),
                charts: chart_count,
                covered_texels,
                projected_texels,
                projection_misses,
                overlap_texels,
                dilated_texels,
                rays_cast,
                occluded_rays,
                coverage_ratio: covered_texels as f32 / pixel_count as f32,
                projection_hit_ratio: projection_hit_ratio as f32,
                min_curvature,
                max_curvature,
                determinism: DETERMINISM.into(),
            },
            conditioned_target,
        })
    }
}

fn validate_config(config: &BakeConfig) -> Result<(), BakeError> {
    if !(16..=8_192).contains(&config.resolution) {
        return Err(BakeError::InvalidConfig(
            "resolution must be between 16 and 8192".into(),
        ));
    }
    if config.padding > 128 || config.padding * 2 >= config.resolution {
        return Err(BakeError::InvalidConfig(
            "padding must be at most 128 pixels and smaller than half the atlas".into(),
        ));
    }
    if !config.cage_distance.is_finite() || config.cage_distance <= 0.0 {
        return Err(BakeError::InvalidConfig(
            "cage distance must be finite and positive".into(),
        ));
    }
    if !config.ray_bias.is_finite()
        || config.ray_bias <= 0.0
        || config.ray_bias >= config.cage_distance
    {
        return Err(BakeError::InvalidConfig(
            "ray bias must be finite, positive, and below cage distance".into(),
        ));
    }
    if config.bake_ao
        && (!config.ao_distance.is_finite()
            || config.ao_distance <= config.ray_bias
            || !(1..=256).contains(&config.ao_samples))
    {
        return Err(BakeError::InvalidConfig(
            "AO requires a positive distance and 1 to 256 samples".into(),
        ));
    }
    if !config.curvature_scale.is_finite() || config.curvature_scale <= 0.0 {
        return Err(BakeError::InvalidConfig(
            "curvature scale must be finite and positive".into(),
        ));
    }
    if !config.min_projection_hit_ratio.is_finite()
        || !(0.01..=1.0).contains(&config.min_projection_hit_ratio)
    {
        return Err(BakeError::InvalidConfig(
            "minimum projection hit ratio must be between 0.01 and 1.0".into(),
        ));
    }
    let map_count = usize::from(config.bake_normal)
        + usize::from(config.bake_ao)
        + usize::from(config.bake_curvature)
        + 1;
    if map_count == 1 {
        return Err(BakeError::InvalidConfig(
            "at least one material map must be enabled".into(),
        ));
    }
    let pixels = u64::from(config.resolution) * u64::from(config.resolution);
    let output_bytes = pixels
        .checked_mul(4)
        .and_then(|value| value.checked_mul(map_count as u64))
        .ok_or_else(|| BakeError::InvalidConfig("output byte count overflowed".into()))?;
    if output_bytes > config.max_output_bytes as u64 {
        return Err(BakeError::InvalidConfig(format!(
            "requested maps need {output_bytes} bytes, above the {} byte output budget",
            config.max_output_bytes
        )));
    }
    let rays_per_pixel = 2 + if config.bake_ao {
        u64::from(config.ao_samples)
    } else {
        0
    };
    let conservative_rays = pixels.saturating_mul(rays_per_pixel);
    if conservative_rays > config.max_rays {
        return Err(BakeError::InvalidConfig(format!(
            "conservative ray count {conservative_rays} exceeds the {} ray budget",
            config.max_rays
        )));
    }
    if config.max_source_triangles == 0 || config.max_source_triangles > MAX_ELEMENTS {
        return Err(BakeError::InvalidConfig(format!(
            "source triangle budget must be between 1 and {MAX_ELEMENTS}"
        )));
    }
    Ok(())
}

fn prepare_target(asset: &MeshAsset) -> Result<MeshAsset, BakeError> {
    if asset.primitives.is_empty() {
        return Err(BakeError::InvalidTarget("no primitives".into()));
    }
    let mut prepared = asset.clone();
    for (primitive_index, primitive) in prepared.primitives.iter_mut().enumerate() {
        validate_target_primitive(primitive, primitive_index)?;
        if primitive.normals.len() != primitive.positions.len() {
            primitive.normals = derive_vertex_normals(primitive).ok_or_else(|| {
                BakeError::InvalidTarget(format!(
                    "primitive {primitive_index} has no non-degenerate triangles for normal generation"
                ))
            })?;
        }
    }
    generate_mikktspace_tangents(&prepared)
        .map(|(asset, _report)| asset)
        .map_err(|error| BakeError::TangentGeneration(error.to_string()))
}

fn validate_target_primitive(
    primitive: &Primitive,
    primitive_index: usize,
) -> Result<(), BakeError> {
    let vertices = primitive.positions.len();
    if vertices == 0 || primitive.indices.is_empty() || !primitive.indices.len().is_multiple_of(3) {
        return Err(BakeError::InvalidTarget(format!(
            "primitive {primitive_index} must be a non-empty indexed triangle list"
        )));
    }
    if primitive.uvs.len() != vertices {
        return Err(BakeError::InvalidTarget(format!(
            "primitive {primitive_index} requires complete UV0"
        )));
    }
    if !primitive.normals.is_empty() && primitive.normals.len() != vertices {
        return Err(BakeError::InvalidTarget(format!(
            "primitive {primitive_index} has a partial normal stream"
        )));
    }
    if primitive
        .indices
        .iter()
        .any(|&index| index as usize >= vertices)
    {
        return Err(BakeError::InvalidTarget(format!(
            "primitive {primitive_index} contains an out-of-range index"
        )));
    }
    if primitive
        .positions
        .iter()
        .flatten()
        .chain(primitive.uvs.iter().flatten())
        .chain(primitive.normals.iter().flatten())
        .any(|value| !value.is_finite())
    {
        return Err(BakeError::InvalidTarget(format!(
            "primitive {primitive_index} contains non-finite geometry"
        )));
    }
    if primitive
        .uvs
        .iter()
        .flatten()
        .any(|value| !(0.0..=1.0).contains(value))
    {
        return Err(BakeError::InvalidTarget(format!(
            "primitive {primitive_index} UV0 must fit the single [0,1] atlas"
        )));
    }
    Ok(())
}

fn derive_vertex_normals(primitive: &Primitive) -> Option<Vec<[f32; 3]>> {
    let mut normals = vec![[0.0; 3]; primitive.positions.len()];
    let mut valid = 0usize;
    for triangle in primitive.indices.chunks_exact(3) {
        let index = [
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        ];
        let face = cross(
            sub(primitive.positions[index[1]], primitive.positions[index[0]]),
            sub(primitive.positions[index[2]], primitive.positions[index[0]]),
        );
        if length2(face) <= 1.0e-20 {
            continue;
        }
        valid += 1;
        for &vertex in &index {
            normals[vertex] = add(normals[vertex], face);
        }
    }
    (valid > 0).then(|| normals.into_iter().map(normalize).collect())
}

#[allow(clippy::too_many_lines)]
fn append_source_triangles(
    output: &mut Vec<SourceTriangle>,
    asset: &MeshAsset,
    transform: PreparedTransform,
    source_index: usize,
    limit: usize,
) -> Result<(), BakeError> {
    for (primitive_index, primitive) in asset.primitives.iter().enumerate() {
        let vertices = primitive.positions.len();
        if vertices == 0
            || primitive.indices.is_empty()
            || !primitive.indices.len().is_multiple_of(3)
        {
            return Err(source_error(
                source_index,
                primitive_index,
                "must be a non-empty indexed triangle list",
            ));
        }
        if primitive
            .indices
            .iter()
            .any(|&index| index as usize >= vertices)
        {
            return Err(source_error(
                source_index,
                primitive_index,
                "contains an out-of-range index",
            ));
        }
        if !primitive.normals.is_empty() && primitive.normals.len() != vertices {
            return Err(source_error(
                source_index,
                primitive_index,
                "has a partial normal stream",
            ));
        }
        if primitive
            .positions
            .iter()
            .flatten()
            .chain(primitive.normals.iter().flatten())
            .any(|value| !value.is_finite())
        {
            return Err(source_error(
                source_index,
                primitive_index,
                "contains non-finite geometry",
            ));
        }

        let positions: Vec<[f32; 3]> = primitive
            .positions
            .iter()
            .copied()
            .map(|point| transform.point(point))
            .collect();
        let mut normals: Vec<[f32; 3]> = if primitive.normals.len() == vertices {
            primitive
                .normals
                .iter()
                .copied()
                .map(|normal| transform.normal(normal))
                .collect()
        } else {
            let local = derive_vertex_normals(primitive).ok_or_else(|| {
                source_error(
                    source_index,
                    primitive_index,
                    "has no non-degenerate triangles for normal generation",
                )
            })?;
            local
                .into_iter()
                .map(|normal| transform.normal(normal))
                .collect()
        };
        for normal in &mut normals {
            *normal = normalize(*normal);
        }
        let curvature = vertex_curvature(&positions, &normals, &primitive.indices);

        for (triangle_index, triangle) in primitive.indices.chunks_exact(3).enumerate() {
            if output.len() >= limit {
                return Err(BakeError::InvalidConfig(format!(
                    "high-poly sources exceed the {limit} triangle budget"
                )));
            }
            let index = [
                triangle[0] as usize,
                triangle[1] as usize,
                triangle[2] as usize,
            ];
            let triangle_positions = [
                positions[index[0]],
                positions[index[1]],
                positions[index[2]],
            ];
            if length2(cross(
                sub(triangle_positions[1], triangle_positions[0]),
                sub(triangle_positions[2], triangle_positions[0]),
            )) <= 1.0e-20
            {
                continue;
            }
            output.push(SourceTriangle {
                positions: triangle_positions,
                normals: [normals[index[0]], normals[index[1]], normals[index[2]]],
                curvature: [
                    curvature[index[0]],
                    curvature[index[1]],
                    curvature[index[2]],
                ],
                stable_id: [source_index, primitive_index, triangle_index],
            });
        }
    }
    Ok(())
}

fn source_error(source: usize, primitive: usize, reason: &str) -> BakeError {
    BakeError::InvalidSource {
        source,
        reason: format!("primitive {primitive} {reason}"),
    }
}

/// Cotangent Laplace-Beltrami mean-curvature estimate in common bake space.
fn vertex_curvature(positions: &[[f32; 3]], normals: &[[f32; 3]], indices: &[u32]) -> Vec<f32> {
    let mut laplacian = vec![[0.0; 3]; positions.len()];
    let mut area = vec![0.0f32; positions.len()];
    for triangle in indices.chunks_exact(3) {
        let i = [
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        ];
        let p = [positions[i[0]], positions[i[1]], positions[i[2]]];
        let cross_value = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        let double_area = length(cross_value);
        if double_area <= 1.0e-12 {
            continue;
        }
        let cot = [
            dot(sub(p[1], p[0]), sub(p[2], p[0])) / double_area,
            dot(sub(p[2], p[1]), sub(p[0], p[1])) / double_area,
            dot(sub(p[0], p[2]), sub(p[1], p[2])) / double_area,
        ];
        add_edge_laplacian(&mut laplacian, i[1], i[2], cot[0], positions);
        add_edge_laplacian(&mut laplacian, i[2], i[0], cot[1], positions);
        add_edge_laplacian(&mut laplacian, i[0], i[1], cot[2], positions);
        for &vertex in &i {
            area[vertex] += double_area / 6.0;
        }
    }
    laplacian
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            if area[index] <= 1.0e-12 {
                0.0
            } else {
                dot(value, normals[index]) / (4.0 * area[index])
            }
        })
        .collect()
}

fn add_edge_laplacian(
    laplacian: &mut [[f32; 3]],
    a: usize,
    b: usize,
    weight: f32,
    positions: &[[f32; 3]],
) {
    let delta = mul(sub(positions[b], positions[a]), weight);
    laplacian[a] = add(laplacian[a], delta);
    laplacian[b] = sub(laplacian[b], delta);
}

#[derive(Clone, Copy, Debug)]
struct PreparedTransform {
    linear: [[f32; 3]; 3],
    normal_matrix: [[f32; 3]; 3],
    translation: [f32; 3],
}

impl PreparedTransform {
    fn new(transform: BakeTransform) -> Option<Self> {
        let m = transform.matrix;
        if m.iter().flatten().any(|value| !value.is_finite())
            || m[3][0].abs() > 1.0e-6
            || m[3][1].abs() > 1.0e-6
            || m[3][2].abs() > 1.0e-6
            || (m[3][3] - 1.0).abs() > 1.0e-6
        {
            return None;
        }
        let linear = [
            [m[0][0], m[0][1], m[0][2]],
            [m[1][0], m[1][1], m[1][2]],
            [m[2][0], m[2][1], m[2][2]],
        ];
        let inverse = inverse3(linear)?;
        let normal_matrix = transpose3(inverse);
        Some(Self {
            linear,
            normal_matrix,
            translation: [m[0][3], m[1][3], m[2][3]],
        })
    }

    fn point(self, value: [f32; 3]) -> [f32; 3] {
        add(mul3(self.linear, value), self.translation)
    }

    fn vector(self, value: [f32; 3]) -> [f32; 3] {
        normalize(mul3(self.linear, value))
    }

    fn normal(self, value: [f32; 3]) -> [f32; 3] {
        normalize(mul3(self.normal_matrix, value))
    }
}

fn inverse3(m: [[f32; 3]; 3]) -> Option<[[f32; 3]; 3]> {
    let determinant = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if !determinant.is_finite() || determinant.abs() <= 1.0e-12 {
        return None;
    }
    let inv = 1.0 / determinant;
    Some([
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv,
        ],
    ])
}

fn transpose3(m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    [
        [m[0][0], m[1][0], m[2][0]],
        [m[0][1], m[1][1], m[2][1]],
        [m[0][2], m[1][2], m[2][2]],
    ]
}

fn mul3(matrix: [[f32; 3]; 3], value: [f32; 3]) -> [f32; 3] {
    [
        dot(matrix[0], value),
        dot(matrix[1], value),
        dot(matrix[2], value),
    ]
}

#[derive(Clone, Debug)]
struct SourceTriangle {
    positions: [[f32; 3]; 3],
    normals: [[f32; 3]; 3],
    curvature: [f32; 3],
    stable_id: [usize; 3],
}

impl SourceTriangle {
    fn bounds(&self) -> Aabb {
        let mut bounds = Aabb::empty();
        for point in self.positions {
            bounds.expand(point);
        }
        bounds
    }

    fn centroid(&self) -> [f32; 3] {
        mul(
            add(add(self.positions[0], self.positions[1]), self.positions[2]),
            1.0 / 3.0,
        )
    }
}

#[derive(Clone, Copy, Debug)]
struct Aabb {
    min: [f32; 3],
    max: [f32; 3],
}

impl Aabb {
    fn empty() -> Self {
        Self {
            min: [f32::INFINITY; 3],
            max: [f32::NEG_INFINITY; 3],
        }
    }

    fn expand(&mut self, point: [f32; 3]) {
        for (axis, value) in point.into_iter().enumerate() {
            self.min[axis] = self.min[axis].min(value);
            self.max[axis] = self.max[axis].max(value);
        }
    }

    fn include(&mut self, other: Self) {
        self.expand(other.min);
        self.expand(other.max);
    }

    fn ray_hit(self, origin: [f32; 3], direction: [f32; 3], max_t: f32) -> bool {
        let mut near = 0.0f32;
        let mut far = max_t;
        for axis in 0..3 {
            if direction[axis].abs() <= 1.0e-12 {
                if origin[axis] < self.min[axis] || origin[axis] > self.max[axis] {
                    return false;
                }
                continue;
            }
            let inv = 1.0 / direction[axis];
            let mut a = (self.min[axis] - origin[axis]) * inv;
            let mut b = (self.max[axis] - origin[axis]) * inv;
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            near = near.max(a);
            far = far.min(b);
            if near > far {
                return false;
            }
        }
        far >= 0.0
    }
}

#[derive(Clone, Debug)]
enum BvhKind {
    Leaf { start: usize, count: usize },
    Branch { left: usize, right: usize },
}

#[derive(Clone, Debug)]
struct BvhNode {
    bounds: Aabb,
    kind: BvhKind,
}

#[derive(Clone, Copy, Debug)]
struct RayHit {
    t: f32,
    triangle: usize,
    barycentric: [f32; 3],
}

struct TriangleBvh {
    triangles: Vec<SourceTriangle>,
    order: Vec<usize>,
    nodes: Vec<BvhNode>,
}

impl TriangleBvh {
    fn new(triangles: Vec<SourceTriangle>) -> Self {
        let mut result = Self {
            order: (0..triangles.len()).collect(),
            triangles,
            nodes: Vec::new(),
        };
        result.build(0, result.order.len());
        result
    }

    fn build(&mut self, start: usize, end: usize) -> usize {
        let node_index = self.nodes.len();
        self.nodes.push(BvhNode {
            bounds: Aabb::empty(),
            kind: BvhKind::Leaf { start, count: 0 },
        });
        let mut bounds = Aabb::empty();
        let mut centroid_bounds = Aabb::empty();
        for &triangle in &self.order[start..end] {
            bounds.include(self.triangles[triangle].bounds());
            centroid_bounds.expand(self.triangles[triangle].centroid());
        }
        let count = end - start;
        let kind = if count <= LEAF_TRIANGLES {
            BvhKind::Leaf { start, count }
        } else {
            let extents = sub(centroid_bounds.max, centroid_bounds.min);
            let axis = if extents[1] > extents[0] && extents[1] >= extents[2] {
                1
            } else if extents[2] > extents[0] {
                2
            } else {
                0
            };
            self.order[start..end].sort_by(|a, b| {
                self.triangles[*a].centroid()[axis]
                    .total_cmp(&self.triangles[*b].centroid()[axis])
                    .then_with(|| {
                        self.triangles[*a]
                            .stable_id
                            .cmp(&self.triangles[*b].stable_id)
                    })
            });
            let middle = start + count / 2;
            let left = self.build(start, middle);
            let right = self.build(middle, end);
            BvhKind::Branch { left, right }
        };
        self.nodes[node_index] = BvhNode { bounds, kind };
        node_index
    }

    fn nearest(&self, origin: [f32; 3], direction: [f32; 3], max_t: f32) -> Option<RayHit> {
        let mut closest: Option<RayHit> = None;
        let mut stack = vec![0usize];
        while let Some(node_index) = stack.pop() {
            let limit = closest.map_or(max_t, |hit| hit.t.min(max_t));
            let node = &self.nodes[node_index];
            if !node.bounds.ray_hit(origin, direction, limit) {
                continue;
            }
            match node.kind {
                BvhKind::Leaf { start, count } => {
                    for &triangle in &self.order[start..start + count] {
                        if let Some(hit) = ray_triangle(
                            origin,
                            direction,
                            &self.triangles[triangle],
                            limit,
                            triangle,
                        ) {
                            let replace = closest.is_none_or(|current| {
                                hit.t < current.t
                                    || (hit.t == current.t
                                        && self.triangles[hit.triangle].stable_id
                                            < self.triangles[current.triangle].stable_id)
                            });
                            if replace {
                                closest = Some(hit);
                            }
                        }
                    }
                }
                BvhKind::Branch { left, right } => {
                    stack.push(right);
                    stack.push(left);
                }
            }
        }
        closest
    }

    fn any_hit(&self, origin: [f32; 3], direction: [f32; 3], max_t: f32) -> bool {
        let mut stack = vec![0usize];
        while let Some(node_index) = stack.pop() {
            let node = &self.nodes[node_index];
            if !node.bounds.ray_hit(origin, direction, max_t) {
                continue;
            }
            match node.kind {
                BvhKind::Leaf { start, count } => {
                    if self.order[start..start + count].iter().any(|&triangle| {
                        ray_triangle(
                            origin,
                            direction,
                            &self.triangles[triangle],
                            max_t,
                            triangle,
                        )
                        .is_some()
                    }) {
                        return true;
                    }
                }
                BvhKind::Branch { left, right } => {
                    stack.push(right);
                    stack.push(left);
                }
            }
        }
        false
    }
}

#[allow(clippy::many_single_char_names)]
fn ray_triangle(
    origin: [f32; 3],
    direction: [f32; 3],
    triangle: &SourceTriangle,
    max_t: f32,
    triangle_index: usize,
) -> Option<RayHit> {
    let edge1 = sub(triangle.positions[1], triangle.positions[0]);
    let edge2 = sub(triangle.positions[2], triangle.positions[0]);
    let p = cross(direction, edge2);
    let determinant = dot(edge1, p);
    if determinant.abs() <= 1.0e-9 {
        return None;
    }
    let inv = 1.0 / determinant;
    let tvec = sub(origin, triangle.positions[0]);
    let u = dot(tvec, p) * inv;
    if !(-1.0e-6..=1.0 + 1.0e-6).contains(&u) {
        return None;
    }
    let q = cross(tvec, edge1);
    let v = dot(direction, q) * inv;
    if v < -1.0e-6 || u + v > 1.0 + 1.0e-6 {
        return None;
    }
    let t = dot(edge2, q) * inv;
    if t <= 1.0e-7 || t > max_t {
        return None;
    }
    Some(RayHit {
        t,
        triangle: triangle_index,
        barycentric: [1.0 - u - v, u, v],
    })
}

fn closest_to_low(
    a: Option<RayHit>,
    b: Option<RayHit>,
    low: [f32; 3],
    triangles: &[SourceTriangle],
) -> Option<RayHit> {
    match (a, b) {
        (Some(a), Some(b)) => {
            let point_a = interpolate3(
                triangles[a.triangle].positions[0],
                triangles[a.triangle].positions[1],
                triangles[a.triangle].positions[2],
                a.barycentric,
            );
            let point_b = interpolate3(
                triangles[b.triangle].positions[0],
                triangles[b.triangle].positions[1],
                triangles[b.triangle].positions[2],
                b.barycentric,
            );
            let da = length2(sub(point_a, low));
            let db = length2(sub(point_b, low));
            if da < db
                || (da == db && triangles[a.triangle].stable_id < triangles[b.triangle].stable_id)
            {
                Some(a)
            } else {
                Some(b)
            }
        }
        (Some(hit), None) | (None, Some(hit)) => Some(hit),
        (None, None) => None,
    }
}

fn triangle_charts(primitive: &Primitive) -> Vec<u32> {
    let triangle_count = primitive.indices.len() / 3;
    let mut vertex_triangles = vec![Vec::new(); primitive.positions.len()];
    for (triangle, indices) in primitive.indices.chunks_exact(3).enumerate() {
        for &index in indices {
            vertex_triangles[index as usize].push(triangle);
        }
    }
    let mut chart = vec![u32::MAX; triangle_count];
    let mut next_chart = 0u32;
    let mut queue = VecDeque::new();
    for seed in 0..triangle_count {
        if chart[seed] != u32::MAX {
            continue;
        }
        chart[seed] = next_chart;
        queue.push_back(seed);
        while let Some(triangle) = queue.pop_front() {
            for &vertex in &primitive.indices[triangle * 3..triangle * 3 + 3] {
                for &neighbor in &vertex_triangles[vertex as usize] {
                    if chart[neighbor] == u32::MAX {
                        chart[neighbor] = next_chart;
                        queue.push_back(neighbor);
                    }
                }
            }
        }
        next_chart += 1;
    }
    chart
}

fn uv_pixel_bounds(uv: [[f32; 2]; 3], resolution: u32) -> [u32; 4] {
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for point in uv {
        min[0] = min[0].min(point[0]);
        min[1] = min[1].min(point[1]);
        max[0] = max[0].max(point[0]);
        max[1] = max[1].max(point[1]);
    }
    let last = resolution.saturating_sub(1) as f32;
    [
        (min[0] * resolution as f32 - 0.5).floor().clamp(0.0, last) as u32,
        (min[1] * resolution as f32 - 0.5).floor().clamp(0.0, last) as u32,
        (max[0] * resolution as f32 - 0.5).ceil().clamp(0.0, last) as u32,
        (max[1] * resolution as f32 - 0.5).ceil().clamp(0.0, last) as u32,
    ]
}

fn barycentric_2d(point: [f32; 2], triangle: [[f32; 2]; 3]) -> Option<[f32; 3]> {
    let edge0 = [
        triangle[1][0] - triangle[0][0],
        triangle[1][1] - triangle[0][1],
    ];
    let edge1 = [
        triangle[2][0] - triangle[0][0],
        triangle[2][1] - triangle[0][1],
    ];
    let relative = [point[0] - triangle[0][0], point[1] - triangle[0][1]];
    let determinant = edge0[0] * edge1[1] - edge0[1] * edge1[0];
    if determinant.abs() <= 1.0e-12 {
        return None;
    }
    let inv = 1.0 / determinant;
    let b = (relative[0] * edge1[1] - relative[1] * edge1[0]) * inv;
    let c = (edge0[0] * relative[1] - edge0[1] * relative[0]) * inv;
    let a = 1.0 - b - c;
    (a >= -1.0e-6 && b >= -1.0e-6 && c >= -1.0e-6).then_some([a, b, c])
}

#[allow(clippy::too_many_arguments)]
fn dilate_maps(
    resolution: usize,
    padding: u32,
    covered: &[bool],
    chart_owner: &mut [u32],
    mut normal: Option<&mut Vec<u8>>,
    mut ao: Option<&mut Vec<u8>>,
    mut curvature: Option<&mut Vec<u8>>,
    coverage: &mut [u8],
    observer: &dyn BakeObserver,
) -> Result<usize, BakeError> {
    if padding == 0 {
        return Ok(0);
    }
    let mut queue = VecDeque::new();
    let mut distance = vec![u16::MAX; covered.len()];
    for (index, value) in covered.iter().enumerate() {
        if *value {
            distance[index] = 0;
            queue.push_back(index);
        }
    }
    let mut dilated = 0usize;
    let mut reported = 0u16;
    while let Some(index) = queue.pop_front() {
        let current = distance[index];
        if current >= padding as u16 {
            continue;
        }
        if current > reported {
            reported = current;
            checkpoint(
                observer,
                BakeStage::Dilating,
                u64::from(reported),
                u64::from(padding),
            )?;
        }
        let x = index % resolution;
        let y = index / resolution;
        let neighbors = [
            if x > 0 { Some(index - 1) } else { None },
            if x + 1 < resolution {
                Some(index + 1)
            } else {
                None
            },
            if y > 0 {
                Some(index - resolution)
            } else {
                None
            },
            if y + 1 < resolution {
                Some(index + resolution)
            } else {
                None
            },
        ];
        for neighbor in neighbors.into_iter().flatten() {
            if distance[neighbor] != u16::MAX || covered[neighbor] {
                continue;
            }
            distance[neighbor] = current + 1;
            chart_owner[neighbor] = chart_owner[index];
            copy_pixel(normal.as_deref_mut(), index, neighbor);
            copy_pixel(ao.as_deref_mut(), index, neighbor);
            copy_pixel(curvature.as_deref_mut(), index, neighbor);
            coverage[neighbor * 4 + 2] = 255;
            dilated += 1;
            queue.push_back(neighbor);
        }
    }
    Ok(dilated)
}

fn copy_pixel(pixels: Option<&mut Vec<u8>>, source: usize, target: usize) {
    if let Some(pixels) = pixels {
        let value = [
            pixels[source * 4],
            pixels[source * 4 + 1],
            pixels[source * 4 + 2],
            pixels[source * 4 + 3],
        ];
        pixels[target * 4..target * 4 + 4].copy_from_slice(&value);
    }
}

fn hemisphere_direction(index: u32, count: u32, normal: [f32; 3]) -> [f32; 3] {
    let u = (index as f32 + 0.5) / count as f32;
    let v = radical_inverse(index);
    let radius = u.sqrt();
    let angle = 2.0 * PI * v;
    let local = [radius * angle.cos(), radius * angle.sin(), (1.0 - u).sqrt()];
    let helper = if normal[2].abs() < 0.999 {
        [0.0, 0.0, 1.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let tangent = normalize(cross(helper, normal));
    let bitangent = cross(normal, tangent);
    normalize(add(
        add(mul(tangent, local[0]), mul(bitangent, local[1])),
        mul(normal, local[2]),
    ))
}

fn radical_inverse(mut bits: u32) -> f32 {
    bits = bits.rotate_right(16);
    bits = ((bits & 0x5555_5555) << 1) | ((bits & 0xAAAA_AAAA) >> 1);
    bits = ((bits & 0x3333_3333) << 2) | ((bits & 0xCCCC_CCCC) >> 2);
    bits = ((bits & 0x0F0F_0F0F) << 4) | ((bits & 0xF0F0_F0F0) >> 4);
    bits = ((bits & 0x00FF_00FF) << 8) | ((bits & 0xFF00_FF00) >> 8);
    bits as f32 * 2.328_306_4e-10
}

fn checkpoint(
    observer: &dyn BakeObserver,
    stage: BakeStage,
    completed: u64,
    total: u64,
) -> Result<(), BakeError> {
    if observer.is_cancelled() {
        return Err(BakeError::Cancelled);
    }
    observer.on_progress(BakeProgress {
        stage,
        completed,
        total,
    });
    Ok(())
}

fn filled_rgba(pixel_count: usize, value: [u8; 4]) -> Vec<u8> {
    let mut output = Vec::with_capacity(pixel_count * 4);
    for _ in 0..pixel_count {
        output.extend_from_slice(&value);
    }
    output
}

fn write_gray_or_vector(pixels: &mut [u8], pixel: usize, value: [u8; 3]) {
    pixels[pixel * 4..pixel * 4 + 3].copy_from_slice(&value);
    pixels[pixel * 4 + 3] = 255;
}

fn encode_signed(value: f32) -> u8 {
    encode_unit(value.clamp(-1.0, 1.0) * 0.5 + 0.5)
}

fn encode_unit(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn interpolate1(value: [f32; 3], barycentric: [f32; 3]) -> f32 {
    value[0] * barycentric[0] + value[1] * barycentric[1] + value[2] * barycentric[2]
}

fn interpolate3(a: [f32; 3], b: [f32; 3], c: [f32; 3], barycentric: [f32; 3]) -> [f32; 3] {
    [
        a[0] * barycentric[0] + b[0] * barycentric[1] + c[0] * barycentric[2],
        a[1] * barycentric[0] + b[1] * barycentric[1] + c[1] * barycentric[2],
        a[2] * barycentric[0] + b[2] * barycentric[1] + c[2] * barycentric[2],
    ]
}

fn interpolate4(a: [f32; 4], b: [f32; 4], c: [f32; 4], barycentric: [f32; 3]) -> [f32; 4] {
    [
        a[0] * barycentric[0] + b[0] * barycentric[1] + c[0] * barycentric[2],
        a[1] * barycentric[0] + b[1] * barycentric[1] + c[1] * barycentric[2],
        a[2] * barycentric[0] + b[2] * barycentric[1] + c[2] * barycentric[2],
        a[3] * barycentric[0] + b[3] * barycentric[1] + c[3] * barycentric[2],
    ]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn mul(value: [f32; 3], scalar: f32) -> [f32; 3] {
    [value[0] * scalar, value[1] * scalar, value[2] * scalar]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length2(value: [f32; 3]) -> f32 {
    dot(value, value)
}

fn length(value: [f32; 3]) -> f32 {
    length2(value).sqrt()
}

fn normalize(value: [f32; 3]) -> [f32; 3] {
    let magnitude = length(value);
    if magnitude > 1.0e-12 && magnitude.is_finite() {
        mul(value, 1.0 / magnitude)
    } else {
        [0.0, 0.0, 1.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn plane(z: f32) -> MeshAsset {
        MeshAsset {
            name: "plane".into(),
            primitives: vec![Primitive {
                positions: vec![
                    [-0.5, -0.5, z],
                    [0.5, -0.5, z],
                    [0.5, 0.5, z],
                    [-0.5, 0.5, z],
                ],
                normals: vec![[0.0, 0.0, 1.0]; 4],
                uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                tangents: Vec::new(),
                indices: vec![0, 1, 2, 0, 2, 3],
                material: 0,
                joints: Vec::new(),
                weights: Vec::new(),
            }],
            materials: vec![Material::default()],
            textures: Vec::new(),
            skeleton: None,
        }
    }

    fn fast_config() -> BakeConfig {
        BakeConfig {
            resolution: 16,
            padding: 2,
            cage_distance: 0.2,
            ray_bias: 1.0e-4,
            ao_distance: 0.2,
            ao_samples: 8,
            max_rays: 100_000,
            ..BakeConfig::default()
        }
    }

    #[test]
    fn arbitrary_high_to_low_bake_is_deterministic_and_attaches_all_maps() {
        let mut low = plane(0.0);
        low.primitives[0].uvs = vec![[0.1, 0.1], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9]];
        let high = plane(0.0);
        let sources = [BakeSource {
            asset: &high,
            transform: BakeTransform::IDENTITY,
        }];
        let first = CpuTextureBaker
            .bake_unobserved(&low, BakeTransform::IDENTITY, &sources, &fast_config())
            .expect("bake");
        let second = CpuTextureBaker
            .bake_unobserved(&low, BakeTransform::IDENTITY, &sources, &fast_config())
            .expect("repeat");
        assert_eq!(first, second);
        assert_eq!(first.report.projected_texels, first.report.covered_texels);
        assert_eq!(first.report.projection_misses, 0);
        assert!(first.report.dilated_texels > 0);
        assert!(first.conditioned_target.primitives[0].tangents.len() >= 4);

        let attached = first
            .maps
            .attach_to(&first.conditioned_target)
            .expect("attach");
        let material = &attached.materials[0];
        assert!(material.normal_texture.is_some());
        assert!(material.occlusion_texture.is_some());
        assert!(material.curvature_texture.is_some());
    }

    #[test]
    fn multiple_sources_and_full_affines_share_one_projection_space() {
        let low = plane(0.0);
        let high_a = plane(0.0);
        let high_b = plane(0.0);
        let translated = BakeTransform {
            matrix: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.04],
                [0.0, 0.0, 0.0, 1.0],
            ],
        };
        let sources = [
            BakeSource {
                asset: &high_a,
                transform: translated,
            },
            BakeSource {
                asset: &high_b,
                transform: BakeTransform {
                    matrix: [
                        [1.0, 0.0, 0.0, 3.0],
                        [0.0, 1.0, 0.0, 0.0],
                        [0.0, 0.0, 1.0, 0.0],
                        [0.0, 0.0, 0.0, 1.0],
                    ],
                },
            },
        ];
        let result = CpuTextureBaker
            .bake_unobserved(&low, BakeTransform::IDENTITY, &sources, &fast_config())
            .expect("affine bake");
        assert_eq!(result.report.source_triangles, 4);
        assert_eq!(result.report.projection_misses, 0);
        let normal = result.maps.normal.expect("normal");
        assert!(normal
            .rgba8
            .chunks_exact(4)
            .filter(|pixel| pixel[3] == 255)
            .any(|pixel| pixel[2] > 245));
    }

    #[test]
    fn ao_detects_a_nearby_occluder_without_darkening_missing_pixels() {
        let low = plane(0.0);
        let mut high = plane(0.0);
        let ceiling = plane(0.05).primitives.remove(0);
        high.primitives.push(ceiling);
        let source = [BakeSource {
            asset: &high,
            transform: BakeTransform::IDENTITY,
        }];
        let result = CpuTextureBaker
            .bake_unobserved(&low, BakeTransform::IDENTITY, &source, &fast_config())
            .expect("AO bake");
        assert!(result.report.occluded_rays > 0);
        let ao = result.maps.ambient_occlusion.expect("AO");
        assert!(ao.rgba8.chunks_exact(4).any(|pixel| pixel[0] < 250));
    }

    #[test]
    fn empty_or_mostly_missed_projection_is_never_published_as_maps() {
        let low = plane(0.0);
        let high = plane(0.0);
        let separated = [BakeSource {
            asset: &high,
            transform: BakeTransform {
                matrix: [
                    [1.0, 0.0, 0.0, 5.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ],
            },
        }];
        let error = CpuTextureBaker
            .bake_unobserved(&low, BakeTransform::IDENTITY, &separated, &fast_config())
            .expect_err("a correctly sized blank bake must fail");
        assert!(matches!(
            error,
            BakeError::InsufficientProjection {
                covered_texels: _,
                projected_texels: 0,
                minimum_hit_ratio_millionths: 900_000,
            }
        ));

        let mut partial = plane(0.0);
        for position in &mut partial.primitives[0].positions {
            position[0] *= 0.5;
        }
        let partial_source = [BakeSource {
            asset: &partial,
            transform: BakeTransform::IDENTITY,
        }];
        let error = CpuTextureBaker
            .bake_unobserved(
                &low,
                BakeTransform::IDENTITY,
                &partial_source,
                &BakeConfig {
                    min_projection_hit_ratio: 0.8,
                    ..fast_config()
                },
            )
            .expect_err("a mostly missed bake must fail its configured quality floor");
        assert!(matches!(
            error,
            BakeError::InsufficientProjection {
                covered_texels,
                projected_texels,
                minimum_hit_ratio_millionths: 800_000,
            } if projected_texels > 0 && projected_texels < covered_texels
        ));
    }

    struct Cancelled(AtomicBool);

    impl BakeObserver for Cancelled {
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::Relaxed)
        }
    }

    #[test]
    fn cancellation_and_resource_budgets_fail_before_unbounded_work() {
        let low = plane(0.0);
        let high = plane(0.0);
        let source = [BakeSource {
            asset: &high,
            transform: BakeTransform::IDENTITY,
        }];
        let cancelled = Cancelled(AtomicBool::new(true));
        let error = TextureBaker::bake(
            &CpuTextureBaker,
            &low,
            BakeTransform::IDENTITY,
            &source,
            &fast_config(),
            &cancelled,
        )
        .expect_err("cancelled");
        assert_eq!(error, BakeError::Cancelled);

        let error = CpuTextureBaker
            .bake_unobserved(
                &low,
                BakeTransform::IDENTITY,
                &source,
                &BakeConfig {
                    resolution: 4_096,
                    max_output_bytes: 1_024,
                    max_rays: u64::MAX,
                    ..BakeConfig::default()
                },
            )
            .expect_err("budget");
        assert!(matches!(error, BakeError::InvalidConfig(_)));

        let error = CpuTextureBaker
            .bake_unobserved(
                &low,
                BakeTransform::IDENTITY,
                &source,
                &BakeConfig {
                    min_projection_hit_ratio: 0.0,
                    ..fast_config()
                },
            )
            .expect_err("zero quality floor");
        assert!(matches!(error, BakeError::InvalidConfig(_)));
    }

    #[test]
    fn malformed_uvs_and_singular_source_transforms_are_actionable() {
        let mut low = plane(0.0);
        low.primitives[0].uvs.pop();
        let high = plane(0.0);
        let source = [BakeSource {
            asset: &high,
            transform: BakeTransform::IDENTITY,
        }];
        let error = CpuTextureBaker
            .bake_unobserved(&low, BakeTransform::IDENTITY, &source, &fast_config())
            .expect_err("partial UV");
        assert!(matches!(error, BakeError::InvalidTarget(_)));

        let low = plane(0.0);
        let source = [BakeSource {
            asset: &high,
            transform: BakeTransform {
                matrix: [[0.0; 4]; 4],
            },
        }];
        let error = CpuTextureBaker
            .bake_unobserved(&low, BakeTransform::IDENTITY, &source, &fast_config())
            .expect_err("singular");
        assert!(matches!(error, BakeError::InvalidSource { source: 0, .. }));
    }
}
