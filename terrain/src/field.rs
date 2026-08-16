//! The compiled terrain — the pointwise field everything else is derived from.
//!
//! [`Terrain::compile`] takes a recipe, validates it, bakes the stages that cannot be pointwise (erosion),
//! indexes the strokes and splines spatially, and hands back an object whose central promise is one method:
//!
//! ```text
//! terrain.height(world_x, world_z) -> metres
//! ```
//!
//! Pure, side-effect free, thread-safe, order-independent and bit-reproducible. Meshes, textures,
//! colliders, nav grids and scatter are all *consequences* of that function.
//!
//! ## Evaluation order, and why it is this order
//!
//! ```text
//! layer stack  →  sculpt strokes  →  road/river/pad splines  →  final clamp
//! ```
//!
//! * **Strokes after layers** so a hand edit always beats the procedural stack — otherwise a change to a
//!   noise parameter silently moves the author's work.
//! * **Splines after strokes** so a river follows a hand-sculpted valley and a road grades across it. The
//!   alternative order was tried and is worse: a river resolved before sculpting ignores the valley the
//!   author dug for it, which is the single most confusing thing a terrain tool can do.
//! * **Clamp last** so no combination of layers, strokes and splines can produce a height the collider,
//!   the streaming bounds or the physics world were not sized for.
//!
//! ## One evaluation pass per chunk
//!
//! Meshes, splat textures, colliders and nav grids do *not* each call `height` on their own grid. A chunk
//! is sampled **once** into a [`ChunkSamples`] block at LOD-0 resolution with a one-cell halo, and every
//! artefact is derived from that block. This is worth stating explicitly because three properties fall out
//! of it at once: the cost of a chunk is one field pass rather than four; every LOD of the chunk shares
//! *identical* heights and normals at coincident vertices, so LOD switches cannot shade or shift; and the
//! halo is evaluated with the same expression the neighbouring chunk uses for its interior, so normals
//! match exactly across chunk borders and there is no lighting seam to hide.

use std::collections::BTreeMap;

use crate::biome::{self, BiomeWeights};
use crate::erosion;
use crate::feature::FeatureIndex;
use crate::grid::HeightGrid;
use crate::mesh::ChunkSamples;
use crate::noise;
use crate::recipe::{ChunkCoord, LayerKind, Stroke, TerrainRecipe};
use crate::sculpt;
use crate::spline::SplineIndex;
use crate::validate;
use crate::TerrainError;

/// A spatial index over sculpt strokes: bucketed so a point pays for the strokes that touch it.
///
/// Strokes are *duplicated* into every bucket their radius overlaps and stored contiguously, so a query
/// returns a `&[Stroke]` with no allocation. Recipe order is preserved within each bucket, which is what
/// keeps the ordered semantics (a later flatten beats an earlier raise) exactly intact under filtering.
#[derive(Clone, Debug, Default)]
struct StrokeIndex {
    cell: f32,
    flat: Vec<Stroke>,
    /// bucket key → `(start, len)` into `flat`.
    buckets: BTreeMap<(i32, i32), (u32, u32)>,
}

impl StrokeIndex {
    fn build(strokes: &[Stroke]) -> Self {
        if strokes.is_empty() {
            return Self::default();
        }
        let max_r = strokes.iter().map(|s| s.radius_m).fold(0.0, f32::max);
        // A bucket at least as wide as the largest stroke keeps duplication to at most four buckets each.
        let cell = max_r.max(32.0);
        let mut per_bucket: BTreeMap<(i32, i32), Vec<Stroke>> = BTreeMap::new();
        for s in strokes {
            let r = s.radius_m.max(0.01);
            let (x0, x1) = (
                ((s.x - r) / cell).floor() as i32,
                ((s.x + r) / cell).floor() as i32,
            );
            let (z0, z1) = (
                ((s.z - r) / cell).floor() as i32,
                ((s.z + r) / cell).floor() as i32,
            );
            for cz in z0..=z1 {
                for cx in x0..=x1 {
                    per_bucket.entry((cx, cz)).or_default().push(*s);
                }
            }
        }
        let mut flat = Vec::new();
        let mut buckets = BTreeMap::new();
        for (k, v) in per_bucket {
            let start = flat.len() as u32;
            flat.extend(v);
            buckets.insert(k, (start, flat.len() as u32 - start));
        }
        Self {
            cell,
            flat,
            buckets,
        }
    }

    fn near(&self, x: f32, z: f32) -> &[Stroke] {
        if self.flat.is_empty() {
            return &[];
        }
        let key = (
            (x / self.cell).floor() as i32,
            (z / self.cell).floor() as i32,
        );
        match self.buckets.get(&key) {
            None => &[],
            Some(&(start, len)) => &self.flat[start as usize..(start + len) as usize],
        }
    }
}

/// Everything the material, biome, scatter and nav stages want to know about one point on the surface.
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceSample {
    /// Height in metres.
    pub height: f32,
    /// Unit surface normal.
    pub normal: [f32; 3],
    /// Slope as rise over run (gradient magnitude).
    pub slope: f32,
    /// Moisture `0..1`.
    pub moisture: f32,
    /// Blended biome weights.
    pub biome: BiomeWeights,
    /// Metres of water above this point, or 0 when dry.
    pub water_depth: f32,
}

/// A compiled, immutable terrain. Cheap to share across threads (`Sync`), because every method is a pure
/// read.
#[derive(Clone, Debug)]
pub struct Terrain {
    recipe: TerrainRecipe,
    /// Baked grids by key: caller-supplied heightmaps plus this terrain's own erosion bakes.
    bakes: BTreeMap<String, HeightGrid>,
    /// Per-layer baked-grid key, for `Heightmap`/`Erosion` layers (`None` for pointwise layers).
    layer_bake: Vec<Option<String>>,
    strokes: StrokeIndex,
    splines: SplineIndex,
    /// Spatial index over the recipe's named landforms, so a point consults only the ones covering it.
    features: FeatureIndex,
    /// Cached `[min, max]` clamp, hoisted out of the inner loop.
    clamp: [f32; 2],
}

impl Terrain {
    /// Compile a recipe, baking any erosion layers from scratch.
    ///
    /// `inputs` supplies grids for [`LayerKind::Heightmap`] layers (an imported heightmap the host decoded).
    ///
    /// # Errors
    /// [`TerrainError::InvalidRecipe`] when validation finds a blocking issue, or
    /// [`TerrainError::MissingBakedGrid`] when a `Heightmap` layer's key was not supplied.
    pub fn compile(
        recipe: TerrainRecipe,
        inputs: BTreeMap<String, HeightGrid>,
    ) -> Result<Self, TerrainError> {
        Self::compile_with_bakes(recipe, inputs, &BTreeMap::new())
    }

    /// Compile, reusing any erosion bake already present in `cache` under its content key.
    ///
    /// Erosion is the only expensive part of compiling, and its key is derived from the layers *below* it
    /// plus its own settings — so editing a layer above an erosion pass, or a stroke, or a road, reuses the
    /// bake and recompiles instantly. That is what makes an erosion-heavy terrain still feel live to edit.
    ///
    /// # Errors
    /// As [`Self::compile`].
    pub fn compile_with_bakes(
        recipe: TerrainRecipe,
        mut inputs: BTreeMap<String, HeightGrid>,
        cache: &BTreeMap<String, HeightGrid>,
    ) -> Result<Self, TerrainError> {
        let report = validate::validate(&recipe);
        if report.has_blocking() {
            return Err(TerrainError::InvalidRecipe(report.summary()));
        }

        let mut bakes: BTreeMap<String, HeightGrid> = BTreeMap::new();
        let mut layer_bake: Vec<Option<String>> = vec![None; recipe.layers.len()];

        // Resolve layer bakes in stack order: an erosion pass erodes exactly what sits beneath it, which may
        // include an earlier erosion pass.
        for i in 0..recipe.layers.len() {
            match &recipe.layers[i].kind {
                LayerKind::Heightmap { key, .. } => {
                    // Moved, not cloned: a supplied heightmap can be tens of megabytes.
                    let grid = inputs
                        .remove(key)
                        .or_else(|| cache.get(key).cloned())
                        .ok_or_else(|| TerrainError::MissingBakedGrid {
                            layer: i,
                            key: key.clone(),
                        })?;
                    bakes.insert(key.clone(), grid);
                    layer_bake[i] = Some(key.clone());
                }
                LayerKind::Erosion(settings) => {
                    if !recipe.layers[i].enabled {
                        continue;
                    }
                    let key = erosion::bake_key(&recipe, i);
                    if let Some(g) = cache.get(&key).cloned().or_else(|| inputs.remove(&key)) {
                        bakes.insert(key.clone(), g);
                    } else {
                        // The field below this layer, evaluated with the bakes resolved so far.
                        let below = PartialField {
                            recipe: &recipe,
                            bakes: &bakes,
                            layer_bake: &layer_bake,
                            upto: i,
                        };
                        let grid = erosion::bake(&recipe, settings, &|x, z| below.height(x, z));
                        bakes.insert(key.clone(), grid);
                    }
                    layer_bake[i] = Some(key);
                }
                _ => {}
            }
        }

        let strokes = StrokeIndex::build(&recipe.strokes);
        let features = FeatureIndex::build(&recipe.features);
        // Splines are indexed on XZ geometry first (which needs no heights), then resolved against the
        // layers-features-strokes field — the order the module documents. Resolving them LAST is what lets a
        // road grade across a valley that a feature carved this same compile.
        let mut splines = SplineIndex::build(&recipe.splines, recipe.chunk_size_m.clamp(1.0, 8.0));
        let clamp = [
            recipe.height_clamp[0].min(recipe.height_clamp[1]),
            recipe.height_clamp[1].max(recipe.height_clamp[0]),
        ];
        let pre = PreSplineField {
            recipe: &recipe,
            bakes: &bakes,
            layer_bake: &layer_bake,
            strokes: &strokes,
            features: &features,
        };
        splines.resolve(&|x, z| pre.height(x, z), 8);

        Ok(Self {
            recipe,
            bakes,
            layer_bake,
            strokes,
            splines,
            features,
            clamp,
        })
    }

    /// The recipe this terrain was compiled from.
    #[must_use]
    pub fn recipe(&self) -> &TerrainRecipe {
        &self.recipe
    }

    /// The baked grids, so a host can persist them and skip the bake next session.
    #[must_use]
    pub fn bakes(&self) -> &BTreeMap<String, HeightGrid> {
        &self.bakes
    }

    /// The compiled splines (road ribbons, river surfaces).
    #[must_use]
    pub fn splines(&self) -> &SplineIndex {
        &self.splines
    }

    /// Heap bytes held by the compiled terrain (bakes plus indices) — accounted against the budget.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.bakes.values().map(HeightGrid::bytes).sum::<usize>()
            + self.strokes.flat.len() * std::mem::size_of::<Stroke>()
            + self
                .splines
                .paths
                .iter()
                .map(|p| p.samples.len() * (std::mem::size_of::<[f32; 3]>() + 4))
                .sum::<usize>()
    }

    /// **The** function: terrain height in metres at a world position.
    #[must_use]
    pub fn height(&self, x: f32, z: f32) -> f32 {
        let h = self.height_pre_spline(x, z);
        self.splines
            .apply(h, x, z)
            .clamp(self.clamp[0], self.clamp[1])
    }

    /// Height before splines reshape it — what the splines themselves are resolved against.
    fn height_pre_spline(&self, x: f32, z: f32) -> f32 {
        PreSplineField {
            recipe: &self.recipe,
            bakes: &self.bakes,
            layer_bake: &self.layer_bake,
            strokes: &self.strokes,
            features: &self.features,
        }
        .height(x, z)
    }

    /// Unit surface normal, from a central difference at the LOD-0 cell size.
    ///
    /// The epsilon is deliberately tied to the *finest* cell size and never to the caller's resolution, so
    /// a normal is a property of the terrain rather than of whoever asked — the reason LOD switches do not
    /// shift the shading.
    #[must_use]
    pub fn normal(&self, x: f32, z: f32) -> [f32; 3] {
        let e = self.recipe.cell_size_at_lod(0).max(0.01);
        let dx = self.height(x + e, z) - self.height(x - e, z);
        let dz = self.height(x, z + e) - self.height(x, z - e);
        normalize([-dx, 2.0 * e, -dz])
    }

    /// Slope as rise over run.
    #[must_use]
    pub fn slope(&self, x: f32, z: f32) -> f32 {
        let e = self.recipe.cell_size_at_lod(0).max(0.01);
        let dx = (self.height(x + e, z) - self.height(x - e, z)) / (2.0 * e);
        let dz = (self.height(x, z + e) - self.height(x, z - e)) / (2.0 * e);
        (dx * dx + dz * dz).sqrt()
    }

    /// Moisture `0..1` — wetter in hollows, near water and near rivers; drier high up.
    ///
    /// A cheap proxy for a full hydrology solve, and enough to drive convincing biome placement: the reason
    /// procedural worlds read as artificial is usually uniform vegetation, not imperfect hydrology.
    #[must_use]
    pub fn moisture(&self, x: f32, z: f32) -> f32 {
        self.moisture_at(x, z, self.height(x, z))
    }

    /// [`Self::moisture`] when the caller already knows the height.
    ///
    /// Worth its own entry point: the texture bake and the scatter pass both hold the height already, and
    /// re-deriving it inside moisture doubled the cost of a chunk build. Passing what you already know is a
    /// large part of the difference between a 47 ms chunk and a 20 ms one.
    #[must_use]
    pub fn moisture_at(&self, x: f32, z: f32, h: f32) -> f32 {
        let wl = self.recipe.moisture_wavelength_m.max(1.0);
        let base =
            0.5 + 0.5 * noise::fbm(x / wl, z / wl, self.recipe.seed ^ 0x9F1D_2C33, 4, 2.0, 0.5);
        let sea = self.recipe.water.sea_level_m;
        // Near or below the water line is wet; high ground is dry.
        let low = 1.0 - noise::smoothstep(sea, sea + 60.0, h);
        let alt = noise::smoothstep(sea + 120.0, sea + 600.0, h);
        let river = if self.splines.is_empty() {
            0.0
        } else {
            let c = self.splines.clearance(x, z);
            if c == f32::MAX {
                0.0
            } else {
                1.0 - noise::smoothstep(0.0, 40.0, c)
            }
        };
        (base * 0.6 + low * 0.25 + river * 0.35 - alt * 0.4).clamp(0.0, 1.0)
    }

    /// Water surface height at a point, if any water covers it (sea/lake level, or a river surface).
    #[must_use]
    pub fn water_surface(&self, x: f32, z: f32) -> Option<f32> {
        self.water_surface_at(x, z, self.height(x, z))
    }

    /// [`Self::water_surface`] when the caller already knows the height.
    #[must_use]
    pub fn water_surface_at(&self, x: f32, z: f32, h: f32) -> Option<f32> {
        let sea = if self.recipe.water.enabled && h < self.recipe.water.sea_level_m {
            Some(self.recipe.water.sea_level_m)
        } else {
            None
        };
        let river = self.splines.river_surface(x, z).filter(|&y| y > h);
        match (sea, river) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }

    /// Metres of water above a point (0 when dry).
    #[must_use]
    pub fn water_depth(&self, x: f32, z: f32) -> f32 {
        self.water_depth_at(x, z, self.height(x, z))
    }

    /// [`Self::water_depth`] when the caller already knows the height.
    #[must_use]
    pub fn water_depth_at(&self, x: f32, z: f32, h: f32) -> f32 {
        self.water_surface_at(x, z, h)
            .map_or(0.0, |s| (s - h).max(0.0))
    }

    /// Everything about one point, computing the shared parts once.
    #[must_use]
    pub fn surface(&self, x: f32, z: f32) -> SurfaceSample {
        let height = self.height(x, z);
        let normal = self.normal(x, z);
        // slope from the same normal rather than four more height evaluations.
        let slope = if normal[1].abs() > 1e-6 {
            ((normal[0] * normal[0] + normal[2] * normal[2]).sqrt() / normal[1]).abs()
        } else {
            f32::MAX
        };
        let moisture = self.moisture_at(x, z, height);
        let biome = biome::weights_at(&self.recipe, height, slope, moisture, x, z);
        SurfaceSample {
            height,
            normal,
            slope,
            moisture,
            biome,
            water_depth: self.water_depth_at(x, z, height),
        }
    }

    /// Where a ray meets the surface, or `None` if it never does within `max_distance_m`.
    ///
    /// A coarse march followed by a bisection: the march step is the LOD-0 cell size, which cannot step over
    /// a feature the mesh itself can represent, and eight bisections then land the hit within a millimetre.
    /// This is what a sculpt brush and a route click aim with, so it has to agree with the *drawn* surface
    /// rather than with an approximation of it — evaluating the same `height` the mesh was built from is
    /// what guarantees that.
    #[must_use]
    pub fn raycast(
        &self,
        origin: [f32; 3],
        dir: [f32; 3],
        max_distance_m: f32,
    ) -> Option<[f32; 3]> {
        let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        if len < 1e-6 {
            return None;
        }
        let d = [dir[0] / len, dir[1] / len, dir[2] / len];
        let step = self.recipe.cell_size_at_lod(0).max(0.05);
        let max_t = max_distance_m.max(step);
        let at = |t: f32| {
            [
                origin[0] + d[0] * t,
                origin[1] + d[1] * t,
                origin[2] + d[2] * t,
            ]
        };
        // `above` tracks whether the ray is still over the surface; the first sign change brackets the hit.
        let mut prev_t = 0.0f32;
        let p0 = at(0.0);
        let mut prev_above = p0[1] >= self.height(p0[0], p0[2]);
        let steps = ((max_t / step).ceil() as u32).min(8192);
        for i in 1..=steps {
            let t = (i as f32 * step).min(max_t);
            let p = at(t);
            let above = p[1] >= self.height(p[0], p[2]);
            if above != prev_above {
                let (mut lo, mut hi) = (prev_t, t);
                for _ in 0..8 {
                    let mid = (lo + hi) * 0.5;
                    let q = at(mid);
                    if (q[1] >= self.height(q[0], q[2])) == prev_above {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                let hit = at((lo + hi) * 0.5);
                return Some([hit[0], self.height(hit[0], hit[2]), hit[2]]);
            }
            prev_above = above;
            prev_t = t;
        }
        None
    }

    /// Sample a whole chunk at LOD-0 resolution with a one-cell halo — the single field pass every artefact
    /// for that chunk is derived from.
    ///
    /// # Errors
    /// [`TerrainError::ChunkOutOfBounds`] when the coordinate is outside the world.
    pub fn sample_chunk(&self, coord: ChunkCoord) -> Result<ChunkSamples, TerrainError> {
        if !self.recipe.in_bounds(coord) {
            return Err(TerrainError::ChunkOutOfBounds(coord));
        }
        Ok(self.sample_chunk_unchecked(coord))
    }

    /// [`Self::sample_chunk`] without the bounds check — for callers that already validated, and for
    /// deliberately sampling the apron outside the world when testing seam behaviour.
    #[must_use]
    pub fn sample_chunk_unchecked(&self, coord: ChunkCoord) -> ChunkSamples {
        let verts = self.recipe.verts_at_lod(0);
        let cell = self.recipe.cell_size_at_lod(0);
        let origin = self.recipe.chunk_origin(coord);
        let stride = verts as usize + 2;
        let mut heights = vec![0.0f32; stride * stride];
        let mut min_y = f32::MAX;
        let mut max_y = f32::MIN;
        for jz in 0..stride {
            let wz = origin[1] + (jz as f32 - 1.0) * cell;
            for jx in 0..stride {
                let wx = origin[0] + (jx as f32 - 1.0) * cell;
                let h = self.height(wx, wz);
                heights[jz * stride + jx] = h;
                // Bounds cover the interior only: a halo vertex belongs to the neighbour and must not
                // inflate this chunk's culling box.
                if jx > 0 && jz > 0 && jx <= verts as usize && jz <= verts as usize {
                    min_y = min_y.min(h);
                    max_y = max_y.max(h);
                }
            }
        }
        ChunkSamples {
            coord,
            verts,
            cell,
            origin,
            heights,
            min_y,
            max_y,
        }
    }
}

/// Normalize, tolerating a zero vector (returns +Y).
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len2 = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
    if len2 <= 1e-20 {
        return [0.0, 1.0, 0.0];
    }
    let inv = 1.0 / len2.sqrt();
    [v[0] * inv, v[1] * inv, v[2] * inv]
}

/// The layer stack evaluated up to (but excluding) layer `upto` — what an erosion bake erodes.
struct PartialField<'a> {
    recipe: &'a TerrainRecipe,
    bakes: &'a BTreeMap<String, HeightGrid>,
    layer_bake: &'a [Option<String>],
    upto: usize,
}

impl PartialField<'_> {
    fn height(&self, x: f32, z: f32) -> f32 {
        eval_layers(self.recipe, self.bakes, self.layer_bake, self.upto, x, z)
    }
}

/// Layers plus strokes — the field splines are resolved against.
struct PreSplineField<'a> {
    recipe: &'a TerrainRecipe,
    bakes: &'a BTreeMap<String, HeightGrid>,
    layer_bake: &'a [Option<String>],
    strokes: &'a StrokeIndex,
    features: &'a FeatureIndex,
}

impl PreSplineField<'_> {
    fn height(&self, x: f32, z: f32) -> f32 {
        // Layers first (the base texture), then the named landforms on top of it. Both are inside the
        // closure the strokes sample, so a Smooth or Flatten brush averages the FEATURED ground rather than
        // the bare stack underneath it.
        let stack = |sx: f32, sz: f32| {
            let base = eval_layers(
                self.recipe,
                self.bakes,
                self.layer_bake,
                self.recipe.layers.len(),
                sx,
                sz,
            );
            apply_features(self.recipe, self.features, base, sx, sz)
        };
        let near = self.strokes.near(x, z);
        if near.is_empty() {
            stack(x, z)
        } else {
            sculpt::apply_strokes(&stack, near, x, z, self.recipe.seed)
        }
    }
}

/// Apply every named landform covering a point, in recipe order.
///
/// Order matters and is the author's: a later feature sees what the earlier ones did, so carving a valley
/// through a mountain works if the valley comes after it. The index means a world with a thousand features
/// still costs only the handful whose footprint covers this point.
fn apply_features(recipe: &TerrainRecipe, index: &FeatureIndex, base: f32, x: f32, z: f32) -> f32 {
    if index.is_empty() {
        return base;
    }
    let near = index.near(x, z);
    if near.is_empty() {
        return base;
    }
    let mut h = base;
    // `near` is built in recipe order per bucket, so this preserves authoring order without a sort.
    for &i in near {
        if let Some(f) = recipe.features.get(i as usize) {
            h = f.apply(h, x, z, recipe.seed);
        }
    }
    h
}

/// Evaluate layers `0..upto` of the stack at a world position.
#[allow(clippy::too_many_lines)] // one flat dispatch over the layer vocabulary; splitting it would scatter the stack's semantics across helpers
fn eval_layers(
    recipe: &TerrainRecipe,
    bakes: &BTreeMap<String, HeightGrid>,
    layer_bake: &[Option<String>],
    upto: usize,
    x: f32,
    z: f32,
) -> f32 {
    let mut acc = 0.0f32;
    for (i, layer) in recipe.layers.iter().enumerate().take(upto) {
        if !layer.enabled {
            continue;
        }
        let seed = recipe.seed ^ layer.seed_offset.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let value = match &layer.kind {
            LayerKind::Constant { height } => *height,
            LayerKind::Fbm {
                amplitude,
                wavelength_m,
                octaves,
                lacunarity,
                gain,
                warp_m,
                warp_wavelength_m,
            } => {
                let wl = wavelength_m.max(0.01);
                let (wx, wz) =
                    noise::warp_2d(x, z, seed, *warp_m, 1.0 / warp_wavelength_m.max(0.01));
                noise::fbm(wx / wl, wz / wl, seed, *octaves, *lacunarity, *gain) * amplitude
            }
            LayerKind::Ridged {
                amplitude,
                wavelength_m,
                octaves,
                lacunarity,
                gain,
                warp_m,
                warp_wavelength_m,
            } => {
                let wl = wavelength_m.max(0.01);
                let (wx, wz) =
                    noise::warp_2d(x, z, seed, *warp_m, 1.0 / warp_wavelength_m.max(0.01));
                noise::ridged(wx / wl, wz / wl, seed, *octaves, *lacunarity, *gain) * amplitude
            }
            LayerKind::Billow {
                amplitude,
                wavelength_m,
                octaves,
                lacunarity,
                gain,
            } => {
                let wl = wavelength_m.max(0.01);
                noise::billow(x / wl, z / wl, seed, *octaves, *lacunarity, *gain) * amplitude
            }
            LayerKind::Cellular {
                amplitude,
                wavelength_m,
                invert,
            } => {
                let wl = wavelength_m.max(0.01);
                let v = noise::worley_2d(x / wl, z / wl, seed);
                (if *invert { 1.0 - v } else { v }) * amplitude
            }
            LayerKind::Terrace { step_m, sharpness } => {
                let step = step_m.max(0.01);
                let t = acc / step;
                let base = t.floor();
                let frac = t - base;
                // Blend between the raw ramp and a hard step by `sharpness`; the eased step keeps the
                // riser from aliasing into a stair-step of single pixels.
                let stepped = base + noise::smoothstep(0.35, 0.65, frac);
                noise::lerp(acc, stepped * step, sharpness.clamp(0.0, 1.0))
            }
            LayerKind::Curve { exponent, pivot_m } => {
                // Integer powers only — `powf` is a transcendental and would break bit-reproducibility.
                let d = acc - pivot_m;
                let n = exponent.clamp(1.0, 4.0).round() as u32;
                let mut p = d.abs();
                let mut r = 1.0f32;
                for _ in 0..n {
                    r *= p;
                }
                p = r;
                pivot_m + if d < 0.0 { -p } else { p }
            }
            LayerKind::Heightmap {
                amplitude,
                offset_m,
                ..
            } => match layer_bake
                .get(i)
                .and_then(Option::as_ref)
                .and_then(|k| bakes.get(k))
            {
                Some(g) => g.sample(x, z) * amplitude + offset_m,
                None => 0.0,
            },
            LayerKind::Erosion(settings) => {
                match layer_bake
                    .get(i)
                    .and_then(Option::as_ref)
                    .and_then(|k| bakes.get(k))
                {
                    Some(g) => g.sample(x, z) * settings.amplitude,
                    None => 0.0,
                }
            }
        };
        // Terrace and Curve *replace* the accumulator by construction — they are remaps, not additions, so
        // their authored blend is ignored deliberately rather than producing a doubled height.
        let is_remap = matches!(
            layer.kind,
            LayerKind::Terrace { .. } | LayerKind::Curve { .. }
        );
        let weight = layer_weight(layer, acc, x, z, seed);
        if is_remap {
            acc = noise::lerp(acc, value, weight);
        } else {
            let blended = layer.blend.apply(acc, value * layer.weight);
            acc = noise::lerp(acc, blended, weight);
        }
    }
    acc
}

/// A layer's pointwise mask weight (1.0 when unmasked).
fn layer_weight(layer: &crate::recipe::Layer, acc: f32, x: f32, z: f32, seed: u64) -> f32 {
    let Some(mask) = &layer.mask else {
        return 1.0;
    };
    let mut w = 1.0f32;
    if let Some(band) = mask.height_band {
        w *= band_weight(band, acc);
    }
    if let Some([wl, threshold, softness]) = mask.noise {
        let wl = wl.max(0.01);
        let n = 0.5 + 0.5 * noise::fbm(x / wl, z / wl, seed ^ 0x2C1B_77A5, 3, 2.0, 0.5);
        let s = softness.max(1e-4);
        w *= noise::smoothstep(threshold - s, threshold + s, n);
    }
    if mask.invert {
        w = 1.0 - w;
    }
    w.clamp(0.0, 1.0)
}

/// A four-stop soft band: 0 below `band[0]`, ramping to 1 across `band[0]..band[1]`, 1 through
/// `band[1]..band[2]`, ramping back to 0 across `band[2]..band[3]`.
#[must_use]
pub fn band_weight(band: [f32; 4], v: f32) -> f32 {
    let rise = noise::smoothstep(band[0], band[1], v);
    let fall = 1.0 - noise::smoothstep(band[2], band[3], v);
    (rise * fall).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::{Blend, HeightMask, Layer, LayerKind, SplineDef, SplineKind, StrokeKind};

    fn flat_recipe() -> TerrainRecipe {
        TerrainRecipe {
            world_size_m: 512.0,
            chunk_size_m: 128.0,
            chunk_verts: 33,
            ..TerrainRecipe::default()
        }
    }

    fn hilly_recipe() -> TerrainRecipe {
        let mut r = flat_recipe();
        r.layers.push(Layer::new(
            "Hills",
            LayerKind::Fbm {
                amplitude: 40.0,
                wavelength_m: 220.0,
                octaves: 5,
                lacunarity: 2.0,
                gain: 0.5,
                warp_m: 20.0,
                warp_wavelength_m: 400.0,
            },
        ));
        r
    }

    #[test]
    fn a_flat_recipe_is_exactly_flat() {
        let t = Terrain::compile(flat_recipe(), BTreeMap::new()).expect("compile");
        for i in 0..20 {
            assert_eq!(t.height(i as f32 * 7.3, i as f32 * -3.1), 0.0);
        }
        assert_eq!(t.normal(10.0, 10.0), [0.0, 1.0, 0.0]);
        assert_eq!(t.slope(10.0, 10.0), 0.0);
    }

    #[test]
    fn height_is_bit_reproducible_and_order_independent() {
        let t = Terrain::compile(hilly_recipe(), BTreeMap::new()).expect("compile");
        let forward: Vec<u32> = (0..256)
            .map(|i| t.height(i as f32, 33.0).to_bits())
            .collect();
        let backward: Vec<u32> = (0..256)
            .rev()
            .map(|i| t.height(i as f32, 33.0).to_bits())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        assert_eq!(forward, backward, "sampling order must not matter");

        // Recompiling the same recipe yields the same field, bit for bit.
        let t2 = Terrain::compile(hilly_recipe(), BTreeMap::new()).expect("compile");
        let again: Vec<u32> = (0..256)
            .map(|i| t2.height(i as f32, 33.0).to_bits())
            .collect();
        assert_eq!(forward, again);
    }

    #[test]
    fn neighbouring_chunks_agree_exactly_on_their_shared_edge() {
        // The seamlessness proof: a chunk's halo column is bit-identical to its neighbour's interior column.
        let t = Terrain::compile(hilly_recipe(), BTreeMap::new()).expect("compile");
        let a = t.sample_chunk(ChunkCoord::new(1, 1)).expect("chunk a");
        let b = t.sample_chunk(ChunkCoord::new(2, 1)).expect("chunk b");
        let n = a.verts as i32;
        for iz in 0..n {
            let right_edge_of_a = a.height_at(n - 1, iz);
            let left_edge_of_b = b.height_at(0, iz);
            assert_eq!(
                right_edge_of_a.to_bits(),
                left_edge_of_b.to_bits(),
                "edge heights differ at iz={iz}"
            );
            // And the halo, which is what makes the *normals* match too.
            assert_eq!(a.height_at(n, iz).to_bits(), b.height_at(1, iz).to_bits());
        }
    }

    #[test]
    fn a_stroke_beats_the_procedural_stack() {
        let mut r = hilly_recipe();
        r.strokes.push(Stroke {
            kind: StrokeKind::Flatten,
            x: 100.0,
            z: 100.0,
            radius_m: 20.0,
            strength: 1.0,
            hardness: 0.0,
            target_m: 7.0,
        });
        let t = Terrain::compile(r, BTreeMap::new()).expect("compile");
        assert!((t.height(100.0, 100.0) - 7.0).abs() < 1e-4);
    }

    #[test]
    fn a_spline_beats_a_stroke_and_follows_the_sculpted_shape() {
        // The documented order: sculpt a bowl, then a road across it grades over the sculpted surface.
        let mut r = hilly_recipe();
        r.strokes.push(Stroke {
            kind: StrokeKind::Raise,
            x: 250.0,
            z: 250.0,
            radius_m: 60.0,
            strength: -30.0,
            hardness: 1.0,
            target_m: 0.0,
        });
        r.splines.push(SplineDef {
            name: "Road".into(),
            kind: SplineKind::Road,
            points: vec![[200.0, 0.0, 250.0], [300.0, 0.0, 250.0]],
            width_m: 12.0,
            falloff_m: 8.0,
            ..SplineDef::default()
        });
        let t = Terrain::compile(r, BTreeMap::new()).expect("compile");
        let on_road = t.height(250.0, 250.0);
        let beside = t.height(250.0, 30.0);
        // The road sits in the sculpted bowl (well below the surrounding terrain), i.e. it was resolved
        // AFTER the stroke rather than against the raw procedural stack.
        assert!(
            on_road < beside - 5.0,
            "road did not follow the sculpted bowl: {on_road} vs {beside}"
        );
        // And the corridor is flat across its width.
        let across = t.height(250.0, 254.0);
        assert!(
            (across - on_road).abs() < 0.5,
            "corridor not flat: {across}"
        );
    }

    #[test]
    fn the_final_clamp_is_absolute() {
        let mut r = hilly_recipe();
        r.height_clamp = [-2.0, 5.0];
        r.strokes.push(Stroke {
            kind: StrokeKind::Raise,
            x: 64.0,
            z: 64.0,
            radius_m: 30.0,
            strength: 1000.0,
            hardness: 0.0,
            target_m: 0.0,
        });
        let t = Terrain::compile(r, BTreeMap::new()).expect("compile");
        for i in 0..64 {
            let h = t.height(i as f32 * 4.0, 64.0);
            assert!((-2.0..=5.0).contains(&h), "clamp escaped: {h}");
        }
    }

    #[test]
    fn masks_gate_a_layer_by_height() {
        let mut r = flat_recipe();
        // A base ramp, then snow that only appears above 10 m.
        r.layers.push(Layer::new(
            "Ramp",
            LayerKind::Fbm {
                amplitude: 0.0,
                wavelength_m: 100.0,
                octaves: 1,
                lacunarity: 2.0,
                gain: 0.5,
                warp_m: 0.0,
                warp_wavelength_m: 1.0,
            },
        ));
        r.layers
            .push(Layer::new("Lift", LayerKind::Constant { height: 20.0 }).with_blend(Blend::Add));
        r.layers.push(
            Layer::new("Capped", LayerKind::Constant { height: 5.0 })
                .with_blend(Blend::Add)
                .with_mask(HeightMask {
                    height_band: Some([100.0, 110.0, 500.0, 510.0]),
                    ..HeightMask::default()
                }),
        );
        let t = Terrain::compile(r, BTreeMap::new()).expect("compile");
        // At 20 m accumulated, the 100 m-gated layer contributes nothing.
        assert!((t.height(10.0, 10.0) - 20.0).abs() < 1e-4);
    }

    #[test]
    fn out_of_bounds_chunks_are_refused_not_guessed() {
        let t = Terrain::compile(flat_recipe(), BTreeMap::new()).expect("compile");
        assert!(t.sample_chunk(ChunkCoord::new(99, 0)).is_err());
        assert!(t.sample_chunk(ChunkCoord::new(-1, 0)).is_err());
        assert!(t.sample_chunk(ChunkCoord::new(0, 0)).is_ok());
    }

    #[test]
    fn missing_heightmap_input_is_an_explained_error() {
        let mut r = flat_recipe();
        r.layers.push(Layer::new(
            "Imported",
            LayerKind::Heightmap {
                key: "heightmap:abc".into(),
                amplitude: 1.0,
                offset_m: 0.0,
            },
        ));
        let err = Terrain::compile(r, BTreeMap::new()).expect_err("must refuse");
        assert!(matches!(err, TerrainError::MissingBakedGrid { .. }));
        assert!(err.to_string().contains("heightmap:abc"));
    }

    #[test]
    fn water_depth_is_zero_on_dry_land_and_positive_below_the_line() {
        let mut r = flat_recipe();
        r.water.sea_level_m = 10.0;
        r.layers[0] =
            Layer::new("Base", LayerKind::Constant { height: 4.0 }).with_blend(Blend::Replace);
        let t = Terrain::compile(r, BTreeMap::new()).expect("compile");
        assert!((t.water_depth(5.0, 5.0) - 6.0).abs() < 1e-4);
        let mut r2 = flat_recipe();
        r2.water.sea_level_m = -50.0;
        let t2 = Terrain::compile(r2, BTreeMap::new()).expect("compile");
        assert_eq!(t2.water_depth(5.0, 5.0), 0.0);
    }

    #[test]
    fn moisture_stays_in_range_over_a_real_field() {
        let t = Terrain::compile(hilly_recipe(), BTreeMap::new()).expect("compile");
        for i in 0..64 {
            for j in 0..64 {
                let m = t.moisture(i as f32 * 8.0, j as f32 * 8.0);
                assert!((0.0..=1.0).contains(&m), "moisture out of range: {m}");
            }
        }
    }

    #[test]
    fn band_weight_is_a_soft_window() {
        let b = [0.0, 10.0, 20.0, 30.0];
        assert_eq!(band_weight(b, -5.0), 0.0);
        assert_eq!(band_weight(b, 15.0), 1.0);
        assert_eq!(band_weight(b, 35.0), 0.0);
        let rising = band_weight(b, 5.0);
        assert!(rising > 0.0 && rising < 1.0);
    }
}
