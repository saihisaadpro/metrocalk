//! The recipe — the *entire* authored state of a terrain, and the only part of it the document stores.
//!
//! Everything in this module is plain serializable data with no behaviour beyond geometry bookkeeping. A
//! recipe for a 4 km world with a full layer stack, a few hundred sculpt strokes, roads, rivers, six biomes
//! and a dozen scatter rules is a few tens of kilobytes of JSON — which is why terrain edits ride the
//! ordinary commit pipeline, undo one stroke at a time, and merge under CRDT rules like any other component.
//!
//! ## Unit conventions, stated once
//!
//! * Distances are **metres**, heights are **metres above the world origin**, and the world lies in the XZ
//!   plane with +Y up (the renderer's convention).
//! * Noise scale is expressed as a **wavelength in metres**, not a frequency. "Features about 800 m across"
//!   is a sentence an author can reason about; `0.00125` is not.
//! * Slope is **rise over run** (a dimensionless gradient magnitude), never degrees. Degrees would need
//!   `atan`, and this crate admits no transcendentals in the deterministic path — the UI converts for
//!   display, where a last-bit difference cannot change generated content.

use serde::{Deserialize, Serialize};

/// Integer chunk address on the world grid. Chunk `(0, 0)` starts at the world origin and extends towards
/// +X/+Z, so chunk coordinates are simply `floor(world / chunk_size)` with no centring offset to get wrong.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ChunkCoord {
    /// Chunk index along X.
    pub x: i32,
    /// Chunk index along Z.
    pub z: i32,
}

impl ChunkCoord {
    /// Construct a coordinate.
    #[must_use]
    pub fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    /// Chebyshev (king-move) ring distance to another chunk — the integer distance metric the streaming
    /// rings and the *physics* LOD use, because an integer keeps those decisions bit-deterministic.
    #[must_use]
    pub fn ring_distance(self, other: Self) -> i32 {
        (self.x - other.x).abs().max((self.z - other.z).abs())
    }
}

/// How a layer combines with the height accumulated by the layers below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Blend {
    /// `acc + value` — the usual choice.
    Add,
    /// `acc - value`.
    Subtract,
    /// `acc * value` — for shaping an existing silhouette (e.g. a mask-like falloff).
    Multiply,
    /// `min(acc, value)` — carves a ceiling.
    Min,
    /// `max(acc, value)` — raises a floor; how a mountain range sits on top of rolling ground without
    /// doubling its height.
    Max,
    /// `value` — ignores everything below (a base layer, or a deliberate reset).
    Replace,
}

impl Blend {
    /// Apply the blend.
    #[must_use]
    pub fn apply(self, acc: f32, value: f32) -> f32 {
        match self {
            Self::Add => acc + value,
            Self::Subtract => acc - value,
            Self::Multiply => acc * value,
            Self::Min => acc.min(value),
            Self::Max => acc.max(value),
            Self::Replace => value,
        }
    }
}

/// A pointwise weighting applied to a layer, so one stack can express "snow only above the treeline" or
/// "dunes only in the patchy south" without a second terrain.
///
/// Masks read the height accumulated *so far* and a noise field — both pointwise. They deliberately cannot
/// read *slope* or *distance to a road*, because either would need the rest of the pipeline to have already
/// run: slope of the partially-accumulated stack costs extra full-stack evaluations per layer (quadratic in
/// stack depth), and splines are resolved against the finished stack, so consulting them here would be
/// circular. Both live where the finished surface is already known and free: [`crate::material`],
/// [`crate::biome`] and [`crate::scatter`], each of which has its own explicit slope and clearance gates.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HeightMask {
    /// `[fade_in_start, full_start, full_end, fade_out_end]` in metres of accumulated height. `None` ⇒ no
    /// height gate. The four-stop form gives soft edges without a separate "blend" parameter.
    pub height_band: Option<[f32; 4]>,
    /// Patchiness: `(wavelength_m, threshold, softness)`. The mask multiplies by
    /// `smoothstep(threshold - softness, threshold + softness, fbm)`, so raising the threshold shrinks the
    /// patches. `None` ⇒ no patchiness.
    pub noise: Option<[f32; 3]>,
    /// Invert the final weight (`1 - w`).
    pub invert: bool,
}

/// Tunables for the erosion bake. Defaults are the "looks like erosion, costs a few seconds" point.
///
/// Note `talus_slope` is rise/run, not an angle: `0.5` is about 27°, `0.8` about 39°.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErosionSettings {
    /// Bake grid resolution in metres per cell. Erosion shapes *large* features; 2–4 m is plenty and keeps
    /// the bake affordable, while fine detail keeps coming from the noise layers at full resolution.
    pub cell_m: f32,
    /// Droplets released per cell over the whole bake. `0.5` gives clear drainage patterns.
    pub droplets_per_cell: f32,
    /// How much a droplet keeps its previous direction (0 = follows the steepest descent exactly, 1 = flies
    /// straight). Inertia is what makes channels meander instead of snapping to the grid.
    pub inertia: f32,
    /// Metres of sediment a droplet can carry per unit of (slope × speed × water). Slope here is the
    /// dimensionless gradient, so this number means the same thing on a dune field and on a mountain.
    pub capacity: f32,
    /// Fraction of the excess sediment dropped per step when over capacity.
    pub deposition: f32,
    /// Fraction of the deficit eroded per step when under capacity.
    pub erosion_rate: f32,
    /// Water lost per step (`0.01`–`0.05`); higher ⇒ shorter channels.
    pub evaporation: f32,
    /// Gravity term feeding the speed update.
    pub gravity: f32,
    /// Erosion is spread over a disc of this radius in cells, which prevents single-cell pits.
    pub radius_cells: u32,
    /// Hard cap on droplet lifetime in steps. **Load-bearing for seamlessness**: the tiled bake's halo is
    /// sized from this, so a droplet can never influence a tile it did not start within reach of.
    pub max_steps: u32,
    /// Thermal-erosion sweeps: material above `talus_slope` slumps to its neighbours, cutting scree slopes
    /// and softening noise spikes.
    pub thermal_iterations: u32,
    /// Maximum stable slope (rise/run) for thermal erosion.
    pub talus_slope: f32,
    /// Scale applied to the whole erosion delta — the one knob to dial the effect back without re-tuning.
    pub amplitude: f32,
}

impl Default for ErosionSettings {
    fn default() -> Self {
        Self {
            cell_m: 3.0,
            droplets_per_cell: 0.5,
            inertia: 0.06,
            capacity: 0.08,
            deposition: 0.3,
            erosion_rate: 0.3,
            evaporation: 0.02,
            gravity: 0.6,
            radius_cells: 2,
            max_steps: 48,
            thermal_iterations: 4,
            talus_slope: 0.8,
            amplitude: 1.0,
        }
    }
}

/// What a layer computes, before its blend and mask are applied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LayerKind {
    /// A constant height — the usual base layer (sea floor, plateau).
    Constant {
        /// Height in metres.
        height: f32,
    },
    /// Fractal Brownian motion: rolling, natural, no dominant direction. The general-purpose ground layer.
    Fbm {
        /// Peak-to-trough amplitude in metres.
        amplitude: f32,
        /// Size of the largest features, in metres.
        wavelength_m: f32,
        /// Octave count (each one halves the feature size). 5–7 is the usual range.
        octaves: u32,
        /// Feature-size ratio between octaves (2.0 = each octave is half the size).
        lacunarity: f32,
        /// Amplitude ratio between octaves (0.5 = each octave is half as tall).
        gain: f32,
        /// Domain-warp distance in metres — bends features so they read as geology, not noise. 0 disables.
        warp_m: f32,
        /// Wavelength of the warp field in metres.
        warp_wavelength_m: f32,
    },
    /// Ridged multifractal: sharp crests, smooth valleys. Mountains.
    Ridged {
        /// Height of the crests in metres.
        amplitude: f32,
        /// Size of the largest features, in metres.
        wavelength_m: f32,
        /// Octave count.
        octaves: u32,
        /// Feature-size ratio between octaves.
        lacunarity: f32,
        /// Amplitude ratio between octaves.
        gain: f32,
        /// Domain-warp distance in metres.
        warp_m: f32,
        /// Wavelength of the warp field in metres.
        warp_wavelength_m: f32,
    },
    /// Billow noise: rounded lobes. Dunes, rolling hills, cloud-like masses.
    Billow {
        /// Amplitude in metres.
        amplitude: f32,
        /// Size of the largest features, in metres.
        wavelength_m: f32,
        /// Octave count.
        octaves: u32,
        /// Feature-size ratio between octaves.
        lacunarity: f32,
        /// Amplitude ratio between octaves.
        gain: f32,
    },
    /// Cellular (Worley) noise: crater rims, mesas, cracked plates.
    Cellular {
        /// Amplitude in metres.
        amplitude: f32,
        /// Cell size in metres.
        wavelength_m: f32,
        /// Invert so cell interiors rise instead of the rims.
        invert: bool,
    },
    /// Quantize the accumulated height into steps — sedimentary benches, rice terraces, mesa layers.
    Terrace {
        /// Step height in metres.
        step_m: f32,
        /// 0 = untouched, 1 = hard steps.
        sharpness: f32,
    },
    /// Remap the accumulated height by a power curve about `pivot_m`. Exponent > 1 flattens lowlands and
    /// exaggerates peaks; < 1 does the opposite.
    Curve {
        /// Curve exponent, applied via repeated multiplication (never `powf`), so it is rounded to a
        /// sensible set of steps internally.
        exponent: f32,
        /// Height the curve pivots about, in metres.
        pivot_m: f32,
    },
    /// Sample a supplied baked grid — an imported heightmap, or a procedurally baked field.
    Heightmap {
        /// Key into the caller-supplied baked-grid set.
        key: String,
        /// Multiplier applied to the grid's samples.
        amplitude: f32,
        /// Constant added afterwards, in metres.
        offset_m: f32,
    },
    /// A hydraulic + thermal erosion pass. The layer *is* a baked grid at run time: [`crate::erosion::bake`]
    /// produces the delta grid from the stack below this layer, and the field then samples it pointwise.
    Erosion(ErosionSettings),
}

/// One entry in the ordered height stack.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    /// Author-facing name, shown in the layer list.
    pub name: String,
    /// What it computes.
    pub kind: LayerKind,
    /// How it combines with the accumulated height.
    pub blend: Blend,
    /// Extra scale on this layer's contribution (`1.0` = as authored). The soloing/deemphasis knob.
    pub weight: f32,
    /// Optional pointwise gate.
    pub mask: Option<HeightMask>,
    /// Unchecked layers are skipped entirely — the non-destructive way to compare variants.
    pub enabled: bool,
    /// Mixed into the seed so two layers of the same kind do not produce identical fields.
    pub seed_offset: u64,
}

impl Layer {
    /// A layer with the usual defaults (additive, full weight, unmasked, enabled).
    #[must_use]
    pub fn new(name: impl Into<String>, kind: LayerKind) -> Self {
        Self {
            name: name.into(),
            kind,
            blend: Blend::Add,
            weight: 1.0,
            mask: None,
            enabled: true,
            seed_offset: 0,
        }
    }

    /// Builder: set the blend.
    #[must_use]
    pub fn with_blend(mut self, blend: Blend) -> Self {
        self.blend = blend;
        self
    }

    /// Builder: set the seed offset.
    #[must_use]
    pub fn with_seed_offset(mut self, offset: u64) -> Self {
        self.seed_offset = offset;
        self
    }

    /// Builder: set the mask.
    #[must_use]
    pub fn with_mask(mut self, mask: HeightMask) -> Self {
        self.mask = Some(mask);
        self
    }
}

/// What a sculpt brush does to the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrokeKind {
    /// Add `strength` metres at the centre, falling off to zero at the radius. Negative lowers.
    Raise,
    /// Blend towards a *low-passed* copy of the field — a bounded-tap Gaussian of the surrounding surface.
    /// Pointwise (just more taps), so it keeps the field seamless and resolution-independent.
    Smooth,
    /// Blend towards `target_m`. The plateau/pad/level tool.
    Flatten,
    /// Add local detail noise. For roughing up a smoothed patch.
    Noise,
}

/// One recorded brush dab. A drag is many of these; [`crate::sculpt`] coalesces a drag into a stroke run so
/// one gesture is one undo step and a bounded number of ops.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    /// What it does.
    pub kind: StrokeKind,
    /// World X of the centre, in metres.
    pub x: f32,
    /// World Z of the centre, in metres.
    pub z: f32,
    /// Radius of influence in metres.
    pub radius_m: f32,
    /// Metres for [`StrokeKind::Raise`], blend weight `0..1` for `Smooth`/`Flatten`, amplitude in metres
    /// for `Noise`.
    pub strength: f32,
    /// Falloff shape: 0 = linear, 1 = smooth (cosine-like quintic). Values between interpolate.
    pub hardness: f32,
    /// Target height in metres for [`StrokeKind::Flatten`]; noise wavelength in metres for `Noise`;
    /// low-pass radius scale for `Smooth` (0 ⇒ use the stroke radius).
    pub target_m: f32,
}

impl Stroke {
    /// Squared distance from `(x, z)` to this stroke's centre — the spatial index's inner loop.
    #[must_use]
    pub fn dist2(&self, x: f32, z: f32) -> f32 {
        let dx = x - self.x;
        let dz = z - self.z;
        dx * dx + dz * dz
    }
}

/// Whether a spline is a road (flatten and pave) or a river (carve and fill with water).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplineKind {
    /// Flattens the terrain across its width and lays a road material; excludes scatter.
    Road,
    /// Carves a channel below the surrounding terrain and drops a water surface into it.
    River,
    /// Flattens without paving — building pads, plazas, rail beds.
    Pad,
}

/// A road, river or pad: a centre line plus a cross-section, projected onto the terrain.
///
/// The centre line is interpolated with a Catmull-Rom spline through the control points, so authoring is
/// "click a few points" rather than "place tangents". Width and depth are constant along the spline in v1;
/// per-point profiles are a named extension (the point array is already the right shape for it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SplineDef {
    /// Author-facing name.
    pub name: String,
    /// Road, river or pad.
    pub kind: SplineKind,
    /// Control points in world XZ (metres). Y is ignored for rivers (they follow a monotone descent
    /// derived from the terrain) and used as the road surface height when `use_point_height` is set.
    pub points: Vec<[f32; 3]>,
    /// Full width of the flat/carved section, in metres.
    pub width_m: f32,
    /// Blend distance beyond the width where the terrain returns to its natural shape, in metres.
    pub falloff_m: f32,
    /// River channel depth below the natural surface, in metres. Ignored for roads and pads.
    pub depth_m: f32,
    /// Take the surface height from the control points (interpolated) instead of from the terrain. This is
    /// how a road holds a constant grade across a dip instead of following every bump.
    pub use_point_height: bool,
    /// Material layer index painted over the corridor. `None` ⇒ leave the biome material.
    pub material_layer: Option<usize>,
    /// Distance in metres either side of the corridor where scatter is suppressed.
    pub clear_scatter_m: f32,
    /// Enabled flag, for non-destructive comparison.
    pub enabled: bool,
}

impl Default for SplineDef {
    fn default() -> Self {
        Self {
            name: "Road".into(),
            kind: SplineKind::Road,
            points: Vec::new(),
            width_m: 8.0,
            falloff_m: 6.0,
            depth_m: 3.0,
            use_point_height: false,
            material_layer: None,
            clear_scatter_m: 3.0,
            enabled: true,
        }
    }
}

/// Sea/lake water.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WaterConfig {
    /// Draw a water plane and treat everything below it as submerged.
    pub enabled: bool,
    /// Water height in metres.
    pub sea_level_m: f32,
    /// Distance in metres above the water line over which shore materials blend.
    pub shore_blend_m: f32,
    /// Depth in metres below which the underwater material takes over completely.
    pub deep_m: f32,
}

impl Default for WaterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sea_level_m: 0.0,
            shore_blend_m: 2.0,
            deep_m: 8.0,
        }
    }
}

/// A ground material — the paint the automatic blending picks between.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaterialLayer {
    /// Author-facing name ("Grass", "Cliff Rock", "Wet Sand").
    pub name: String,
    /// Linear base colour. Used directly when `texture_key` is absent, and as a tint when it is present.
    pub albedo: [f32; 3],
    /// Perceptual roughness `0..1`.
    pub roughness: f32,
    /// Metalness `0..1` (0 for essentially all ground).
    pub metallic: f32,
    /// Wavelength in metres of the value-noise mottling baked into the splat, which is what stops a
    /// flat-colour material reading as plastic at close range.
    pub detail_wavelength_m: f32,
    /// Strength of that mottling, `0..1`.
    pub detail_strength: f32,
    /// Optional key of a tiling albedo texture supplied by the host. `None` ⇒ colour + mottling only.
    pub texture_key: Option<String>,
    /// World-space tiling size in metres for `texture_key`.
    pub texture_scale_m: f32,
}

impl MaterialLayer {
    /// A plain coloured material with sensible mottling.
    #[must_use]
    pub fn new(name: impl Into<String>, albedo: [f32; 3], roughness: f32) -> Self {
        Self {
            name: name.into(),
            albedo,
            roughness,
            metallic: 0.0,
            detail_wavelength_m: 6.0,
            detail_strength: 0.16,
            texture_key: None,
            texture_scale_m: 4.0,
        }
    }
}

/// A biome rule: soft bands over height, slope and moisture that select a material and a scatter set.
///
/// Rules are evaluated as *weights*, not as a winner-take-all classification, and the weights are blended.
/// That is what gives soft biome transitions for free — a hard classification produces the jagged
/// material boundaries that make procedural terrain look procedural.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BiomeRule {
    /// Author-facing name.
    pub name: String,
    /// `[fade_in_start, full_start, full_end, fade_out_end]` in metres of height.
    pub height_band: [f32; 4],
    /// Same four-stop form over slope (rise/run).
    pub slope_band: [f32; 4],
    /// Same four-stop form over moisture `0..1` (from [`crate::field::Terrain::moisture`]).
    pub moisture_band: [f32; 4],
    /// Index into [`TerrainRecipe::materials`].
    pub material_layer: usize,
    /// Multiplier on this biome's weight — the tie-break between overlapping biomes.
    pub weight: f32,
    /// Patchiness `(wavelength_m, threshold, softness)`, as in [`HeightMask::noise`]. `None` ⇒ smooth.
    pub patchiness: Option<[f32; 3]>,
    /// Enabled flag.
    pub enabled: bool,
}

impl BiomeRule {
    /// A biome that only gates on height, accepting all slopes and moistures.
    #[must_use]
    pub fn by_height(name: impl Into<String>, band: [f32; 4], material_layer: usize) -> Self {
        Self {
            name: name.into(),
            height_band: band,
            slope_band: [-1.0, -1.0, 99.0, 99.0],
            moisture_band: [-1.0, -1.0, 2.0, 2.0],
            material_layer,
            weight: 1.0,
            patchiness: None,
            enabled: true,
        }
    }

    /// Builder: restrict to a slope band.
    #[must_use]
    pub fn with_slope(mut self, band: [f32; 4]) -> Self {
        self.slope_band = band;
        self
    }

    /// Builder: restrict to a moisture band.
    #[must_use]
    pub fn with_moisture(mut self, band: [f32; 4]) -> Self {
        self.moisture_band = band;
        self
    }
}

/// A scatterable prototype — a mesh plus its LOD chain, addressed by opaque host keys.
///
/// The keys are asset handles the host resolves; this crate never loads geometry. `impostor_key` is the far
/// representation — the host bakes a billboard-cloud cross-quad for it (see [`crate::scatter::impostor`]),
/// which is the standard production far-field for foliage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScatterProto {
    /// Author-facing name.
    pub name: String,
    /// Host asset key for the full-detail mesh.
    pub mesh_key: String,
    /// Host asset keys for progressively coarser meshes (nearest first).
    pub lod_keys: Vec<String>,
    /// Host asset key for the impostor/billboard, used beyond the last mesh LOD.
    pub impostor_key: Option<String>,
    /// Approximate horizontal radius in metres — used for spacing and for the far-distance screen-size test.
    pub radius_m: f32,
    /// Approximate height in metres.
    pub height_m: f32,
    /// Whether instances get colliders (trees yes, grass no).
    pub collide: bool,
}

impl ScatterProto {
    /// A prototype with only a full-detail mesh.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        mesh_key: impl Into<String>,
        radius_m: f32,
        height_m: f32,
    ) -> Self {
        Self {
            name: name.into(),
            mesh_key: mesh_key.into(),
            lod_keys: Vec::new(),
            impostor_key: None,
            radius_m,
            height_m,
            collide: false,
        }
    }
}

/// A scatter rule — "this prototype, this dense, in these biomes, on these slopes".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScatterRule {
    /// Author-facing name.
    pub name: String,
    /// Index into [`TerrainRecipe::protos`].
    pub proto: usize,
    /// Instances per hectare (10 000 m²) at full weight. Trees 40–300, grass 20 000+.
    pub density_per_hectare: f32,
    /// Biome indices this rule applies to. Empty ⇒ every biome.
    pub biomes: Vec<usize>,
    /// Maximum slope (rise/run) an instance will stand on.
    pub slope_max: f32,
    /// `[fade_in_start, full_start, full_end, fade_out_end]` height band in metres.
    pub height_band: [f32; 4],
    /// Uniform scale range `[min, max]`.
    pub scale_range: [f32; 2],
    /// Tilt instances towards the surface normal by this fraction (0 = always upright, 1 = fully aligned).
    pub align_to_normal: f32,
    /// Clustering `(wavelength_m, threshold, softness)` — clumps rather than an even carpet. `None` ⇒ even.
    pub cluster: Option<[f32; 3]>,
    /// Suppress within this many metres above the water line.
    pub avoid_water_m: f32,
    /// Cull distance in metres. Ground clutter belongs at 40–80 m, trees at 1 000+ — matching the
    /// per-category streaming distances production open worlds use.
    pub view_distance_m: f32,
    /// Mixed into the seed so two rules do not place instances at identical positions.
    pub seed_offset: u64,
    /// Enabled flag.
    pub enabled: bool,
}

impl ScatterRule {
    /// A rule with the usual defaults for a mid-size prop.
    #[must_use]
    pub fn new(name: impl Into<String>, proto: usize, density_per_hectare: f32) -> Self {
        Self {
            name: name.into(),
            proto,
            density_per_hectare,
            biomes: Vec::new(),
            slope_max: 0.6,
            height_band: [-1.0e6, -1.0e6, 1.0e6, 1.0e6],
            scale_range: [0.85, 1.25],
            align_to_normal: 0.25,
            cluster: None,
            avoid_water_m: 0.5,
            view_distance_m: 600.0,
            seed_offset: 0,
            enabled: true,
        }
    }
}

/// Level-of-detail and view policy. This is *presentation* state: it changes what is drawn, never what the
/// terrain **is**, so unlike the field it is allowed to use a tangent for the projection maths.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LodPolicy {
    /// Number of mesh LODs per chunk, including LOD 0. Each level halves the edge vertex count.
    pub levels: u8,
    /// The switching criterion: a chunk uses the coarsest LOD whose *measured* geometric error projects to
    /// no more than this many pixels. At 1 px, an LOD switch moves no vertex by a visible amount — that is
    /// how popping is eliminated without a morphing shader.
    pub screen_error_px: f32,
    /// Fractional distance band around a switch inside which the previous LOD is kept, so a camera hovering
    /// exactly on a boundary does not oscillate.
    pub hysteresis: f32,
    /// Extra metres added to every skirt beyond the computed worst-case error — cheap insurance against a
    /// crack at a T-junction.
    pub skirt_margin_m: f32,
    /// Assumed viewport height in pixels for the error projection (the host publishes the real one).
    pub viewport_height_px: u32,
    /// Vertical field of view in degrees.
    pub fov_y_deg: f32,
    /// Chunks beyond this distance are not resident at all.
    pub max_view_distance_m: f32,
    /// Splat texture resolution for a LOD-0 chunk. Halved per LOD, floored at 32.
    pub texture_res: u32,
    /// Collider LOD steps out one level every this many chunk rings from the physics focus.
    pub physics_lod_step: i32,
    /// Chunks within this ring distance of the focus get colliders at all. At the default 64 m chunk that
    /// is a square of ground about a kilometre across, which is a long walk rather than a short one.
    pub physics_ring: i32,
    /// Enable the terrain-occludes-terrain horizon test.
    pub horizon_culling: bool,
}

impl Default for LodPolicy {
    fn default() -> Self {
        Self {
            levels: 4,
            screen_error_px: 1.0,
            hysteresis: 0.12,
            skirt_margin_m: 0.5,
            viewport_height_px: 1080,
            fov_y_deg: 55.0,
            // Chosen to AGREE with the budget below: 64 m chunks over a 1 024 m radius is a 33x33 ring, or
            // 1 089 chunks, which fits both ceilings. A default that asks for more than its own budget can
            // hold would make every fresh terrain report itself over budget, which is a bug report rather
            // than a warning.
            max_view_distance_m: 1024.0,
            // 256 texels over a 64 m chunk is 4 per metre — comparable to a production landscape weightmap,
            // and the resolution halves per LOD so distant chunks cost almost nothing.
            texture_res: 256,
            physics_lod_step: 4,
            physics_ring: 8,
            horizon_culling: true,
        }
    }
}

/// Hard memory ceilings, in mebibytes, for the resident terrain working set.
///
/// These are ceilings, not hints: [`crate::stream::Streamer`] evicts to stay under them and *reports* what
/// it dropped rather than quietly exceeding them. A budget that is silently ignored is the reason
/// open-world projects discover their memory problem at certification.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemoryBudget {
    /// Chunk mesh vertex/index data.
    pub mesh_mb: u32,
    /// Baked splat albedo + normal textures.
    pub texture_mb: u32,
    /// Scattered instance arrays.
    pub scatter_mb: u32,
    /// Height-field collider data.
    pub collider_mb: u32,
    /// Hard cap on resident chunks regardless of bytes, so a pathological recipe cannot melt the CPU-side
    /// bookkeeping either.
    pub max_resident_chunks: u32,
}

impl Default for MemoryBudget {
    fn default() -> Self {
        Self {
            mesh_mb: 256,
            texture_mb: 320,
            scatter_mb: 96,
            collider_mb: 64,
            max_resident_chunks: 1200,
        }
    }
}

impl MemoryBudget {
    /// Total ceiling in bytes.
    #[must_use]
    pub fn total_bytes(&self) -> usize {
        (self.mesh_mb as usize
            + self.texture_mb as usize
            + self.scatter_mb as usize
            + self.collider_mb as usize)
            * 1024
            * 1024
    }
}

/// The complete authored terrain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TerrainRecipe {
    /// Format version, so an older document can be migrated rather than misread.
    pub version: u32,
    /// Author-facing name.
    pub name: String,
    /// The plain-language description this recipe was built from, when it was
    /// ([`crate::describe`]). Provenance only — it changes no geometry, and it is kept so that reopening a
    /// project shows what was actually asked for rather than only what came out. Empty for a hand-authored or
    /// preset terrain. `serde(default)` so documents written before descriptions existed still load.
    #[serde(default)]
    pub description: String,
    /// The master seed. Every hashed decision in the system derives from it, so changing it produces a
    /// different world with identical art direction.
    pub seed: u64,
    /// Square world extent in metres. The world spans `[0, world_size_m]` on both axes.
    pub world_size_m: f32,
    /// Chunk edge length in metres.
    pub chunk_size_m: f32,
    /// Vertices along a chunk edge at LOD 0. Must be `2^k + 1` so every LOD subsamples exactly.
    pub chunk_verts: u32,
    /// Final clamp `[min, max]` in metres, applied after every layer, stroke and spline.
    pub height_clamp: [f32; 2],
    /// The ordered height stack — the base texture of the world ("hills this big, everywhere").
    pub layers: Vec<Layer>,
    /// Named landforms with bounded support: the SEMANTIC model (ADR-106). Applied after the stack and
    /// before the sculpt strokes, so a hand edit still sits on top of everything generated.
    ///
    /// `serde(default)` so a document written before features existed still loads.
    #[serde(default)]
    pub features: Vec<crate::feature::WorldFeature>,
    /// Next feature id to hand out. Monotonic and never reused, so a stale reference resolves to nothing
    /// rather than silently to a different landform.
    #[serde(default)]
    pub next_feature_id: crate::feature::FeatureId,
    /// Recorded sculpt strokes, applied after the stack in order.
    pub strokes: Vec<Stroke>,
    /// Roads, rivers and pads.
    pub splines: Vec<SplineDef>,
    /// Water.
    pub water: WaterConfig,
    /// Ground materials.
    pub materials: Vec<MaterialLayer>,
    /// Biome rules.
    pub biomes: Vec<BiomeRule>,
    /// Scatter prototypes.
    pub protos: Vec<ScatterProto>,
    /// Scatter rules.
    pub scatter: Vec<ScatterRule>,
    /// LOD/view policy.
    pub lod: LodPolicy,
    /// Memory ceilings.
    pub budget: MemoryBudget,
    /// Wavelength in metres of the moisture field, which drives biome selection and river-adjacent lushness.
    pub moisture_wavelength_m: f32,
}

impl Default for TerrainRecipe {
    /// An empty but *valid* terrain: a flat plate at height 0 with one material. Every preset builds from
    /// here, and a brand-new terrain is immediately sculptable rather than an error state.
    fn default() -> Self {
        Self {
            version: 1,
            name: "Terrain".into(),
            description: String::new(),
            features: Vec::new(),
            next_feature_id: 1,
            seed: 0x007E_44A1,
            // 64 m chunks at 65 vertices give exactly 1 m cells — fine enough to walk on, and a 2 km world
            // is 32×32 chunks, which the streaming rings and the budget are sized around.
            world_size_m: 2048.0,
            chunk_size_m: 64.0,
            chunk_verts: 65,
            height_clamp: [-256.0, 2048.0],
            layers: vec![
                Layer::new("Base", LayerKind::Constant { height: 0.0 }).with_blend(Blend::Replace)
            ],
            strokes: Vec::new(),
            splines: Vec::new(),
            water: WaterConfig::default(),
            materials: vec![MaterialLayer::new("Ground", [0.32, 0.34, 0.26], 0.85)],
            biomes: vec![BiomeRule::by_height(
                "Ground",
                [-1.0e6, -1.0e6, 1.0e6, 1.0e6],
                0,
            )],
            protos: Vec::new(),
            scatter: Vec::new(),
            lod: LodPolicy::default(),
            budget: MemoryBudget::default(),
            moisture_wavelength_m: 900.0,
        }
    }
}

impl TerrainRecipe {
    /// Chunks along one axis (at least 1).
    #[must_use]
    pub fn chunks_per_axis(&self) -> u32 {
        (self.world_size_m / self.chunk_size_m).ceil().max(1.0) as u32
    }

    /// Total chunk count.
    #[must_use]
    pub fn chunk_count(&self) -> u64 {
        u64::from(self.chunks_per_axis()) * u64::from(self.chunks_per_axis())
    }

    /// Whether a chunk lies inside the world.
    #[must_use]
    pub fn in_bounds(&self, c: ChunkCoord) -> bool {
        let n = self.chunks_per_axis() as i32;
        c.x >= 0 && c.z >= 0 && c.x < n && c.z < n
    }

    /// World XZ of a chunk's minimum corner.
    #[must_use]
    pub fn chunk_origin(&self, c: ChunkCoord) -> [f32; 2] {
        [
            c.x as f32 * self.chunk_size_m,
            c.z as f32 * self.chunk_size_m,
        ]
    }

    /// World XZ of a chunk's centre.
    #[must_use]
    pub fn chunk_center(&self, c: ChunkCoord) -> [f32; 2] {
        let o = self.chunk_origin(c);
        [
            o[0] + self.chunk_size_m * 0.5,
            o[1] + self.chunk_size_m * 0.5,
        ]
    }

    /// The chunk containing a world position (may be out of bounds).
    #[must_use]
    pub fn chunk_at(&self, wx: f32, wz: f32) -> ChunkCoord {
        ChunkCoord::new(
            (wx / self.chunk_size_m).floor() as i32,
            (wz / self.chunk_size_m).floor() as i32,
        )
    }

    /// Edge vertex count at a LOD, floored at 3 so a chunk always has interior geometry.
    #[must_use]
    pub fn verts_at_lod(&self, lod: u8) -> u32 {
        let quads = (self.chunk_verts.max(3) - 1) >> u32::from(lod);
        quads.max(2) + 1
    }

    /// Cell size in metres at a LOD.
    #[must_use]
    pub fn cell_size_at_lod(&self, lod: u8) -> f32 {
        self.chunk_size_m / (self.verts_at_lod(lod) - 1) as f32
    }

    /// Splat texture resolution at a LOD (halved per level, floored at 32).
    #[must_use]
    pub fn texture_res_at_lod(&self, lod: u8) -> u32 {
        (self.lod.texture_res >> u32::from(lod)).max(32)
    }

    /// A stable 64-bit digest of the whole recipe — the cache/content key for bakes and chunk assets. Two
    /// recipes that differ anywhere digest differently; two equal recipes always digest the same, on every
    /// machine, because the digest runs over canonical JSON rather than memory layout.
    #[must_use]
    pub fn digest(&self) -> u64 {
        let json = serde_json::to_vec(self).unwrap_or_default();
        stable_digest(&json)
    }
}

/// A deterministic 64-bit digest of arbitrary bytes (FNV-1a mixing with a SplitMix64 finalizer). Not a
/// cryptographic hash — a cache key. The content-addressed asset store applies the real hash on top.
#[must_use]
pub fn stable_digest(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_recipe_is_a_valid_flat_world() {
        let r = TerrainRecipe::default();
        assert_eq!(r.chunks_per_axis(), 32);
        assert_eq!(r.chunk_count(), 1024);
        assert!(r.in_bounds(ChunkCoord::new(0, 0)));
        assert!(!r.in_bounds(ChunkCoord::new(32, 0)));
        assert!(!r.in_bounds(ChunkCoord::new(-1, 0)));
    }

    #[test]
    fn chunk_geometry_round_trips() {
        let r = TerrainRecipe::default();
        let c = ChunkCoord::new(3, 5);
        let o = r.chunk_origin(c);
        assert_eq!(o, [192.0, 320.0]);
        assert_eq!(r.chunk_at(o[0] + 1.0, o[1] + 1.0), c);
        assert_eq!(r.chunk_center(c), [224.0, 352.0]);
    }

    #[test]
    fn lod_subsampling_stays_aligned() {
        let r = TerrainRecipe::default();
        // 65 verts = 64 quads: every LOD must divide it exactly, which is what makes coarse edge vertices a
        // strict subset of fine ones (and therefore makes shared edges agree).
        assert_eq!(r.verts_at_lod(0), 65);
        assert_eq!(r.verts_at_lod(1), 33);
        assert_eq!(r.verts_at_lod(2), 17);
        assert_eq!(r.verts_at_lod(3), 9);
        assert_eq!(r.cell_size_at_lod(0), 1.0);
        assert_eq!(r.cell_size_at_lod(1), 2.0);
        // Never degenerate, however deep the request.
        assert!(r.verts_at_lod(20) >= 3);
    }

    #[test]
    fn digest_is_stable_and_sensitive() {
        let a = TerrainRecipe::default();
        let mut b = TerrainRecipe::default();
        assert_eq!(a.digest(), b.digest());
        b.seed += 1;
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn blend_modes_do_what_they_say() {
        assert_eq!(Blend::Add.apply(2.0, 3.0), 5.0);
        assert_eq!(Blend::Subtract.apply(2.0, 3.0), -1.0);
        assert_eq!(Blend::Multiply.apply(2.0, 3.0), 6.0);
        assert_eq!(Blend::Min.apply(2.0, 3.0), 2.0);
        assert_eq!(Blend::Max.apply(2.0, 3.0), 3.0);
        assert_eq!(Blend::Replace.apply(2.0, 3.0), 3.0);
    }

    #[test]
    fn recipe_survives_a_json_round_trip() {
        let r = TerrainRecipe::default();
        let json = serde_json::to_string(&r).expect("serialize");
        let back: TerrainRecipe = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(r, back, "the document format must be lossless");
    }
}
