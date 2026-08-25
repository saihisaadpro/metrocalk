//! Vegetation and prop scattering — hash-placed, so it needs no storage and no order.
//!
//! Every instance's existence, position, yaw, tilt and scale come from hashing its **world** cell with the
//! recipe seed. Three properties follow, and each of them removes a whole class of problem:
//!
//! * **Nothing is stored in the document.** A million trees are a rule, not a million transforms. The recipe
//!   stays kilobytes and merges cleanly.
//! * **Chunk boundaries are invisible.** Candidate cells are addressed in world space and a candidate is
//!   kept by whichever chunk contains it, so no tree is duplicated at a seam and none is missing. The same
//!   forest appears whether you approach it from the north or the south.
//! * **Streaming is free of state.** Re-entering a chunk regenerates byte-identical instances, so nothing
//!   needs to be saved when a chunk is evicted, and there is no "did the trees move?" bug to chase.
//!
//! ## Distance handling
//!
//! Instance LOD is chosen by **projected pixel height**, not raw distance: a 30 m tree and a 0.3 m tuft
//! should not switch representation at the same range, and a pixel threshold expresses that directly. Below
//! the last mesh LOD an instance becomes an impostor — a view-independent billboard cloud of intersecting
//! quads, which is the standard production far-field for foliage — and below a couple of pixels it is culled
//! outright. [`impostor_cross_mesh`] builds that geometry; the host bakes its texture from the prototype.
//!
//! No transcendental functions appear here either: yaw comes from a hashed unit vector and the quaternions
//! are built with half-angle identities, so placement is bit-identical across platforms.

use crate::field::{band_weight, Terrain};
use crate::mesh::{ChunkSamples, MeshData};
use crate::noise;
use crate::recipe::{ChunkCoord, LodPolicy, ScatterProto};

/// Marks an instance drawn as an impostor rather than a mesh LOD.
pub const LOD_IMPOSTOR: u8 = 255;

/// Projected pixel heights at which an instance drops to the next mesh LOD.
///
/// Chosen so the switch happens while the instance is still large enough for the coarser mesh to be
/// indistinguishable, and rare enough that the transition is not what the eye is drawn to.
pub const LOD_PIXEL_STEPS: [f32; 4] = [96.0, 40.0, 16.0, 7.0];

/// Below this projected height an impostor is not worth drawing at all.
pub const CULL_PIXEL_HEIGHT: f32 = 2.0;

/// Hard cap on instances one rule may place in one chunk, so a mistyped density degrades instead of hanging.
pub const MAX_PER_RULE_PER_CHUNK: usize = 16_384;

/// One placed instance. 32 bytes, so a dense chunk's array stays cache-friendly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScatterInstance {
    /// Index into [`crate::recipe::TerrainRecipe::protos`].
    pub proto: u16,
    /// Index into [`crate::recipe::TerrainRecipe::scatter`] — kept so a rule can be toggled or restyled
    /// without regenerating the chunk.
    pub rule: u16,
    /// World position of the base.
    pub position: [f32; 3],
    /// Orientation as a unit quaternion (xyzw): hashed yaw, optionally tilted towards the surface normal.
    pub rotation: [f32; 4],
    /// Uniform scale.
    pub scale: f32,
}

/// What a chunk's scatter pass produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScatterOutput {
    /// Which chunk.
    pub coord: ChunkCoord,
    /// Instances, grouped by prototype so the host can batch draws without sorting.
    pub instances: Vec<ScatterInstance>,
    /// Instance count per prototype, parallel to the recipe's prototype list.
    pub counts: Vec<u32>,
}

impl ScatterOutput {
    /// Heap bytes — what the scatter budget accounts.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.instances.len() * std::mem::size_of::<ScatterInstance>() + self.counts.len() * 4
    }

    /// Total instances.
    #[must_use]
    pub fn len(&self) -> usize {
        self.instances.len()
    }

    /// Whether the chunk scattered nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }
}

/// One instance selected for drawing this frame, with the representation to draw it as.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScatterDraw {
    /// Prototype index.
    pub proto: u16,
    /// Mesh LOD index, or [`LOD_IMPOSTOR`].
    pub lod: u8,
    /// World position.
    pub position: [f32; 3],
    /// Orientation (xyzw).
    pub rotation: [f32; 4],
    /// Uniform scale.
    pub scale: f32,
}

/// Choose how to draw an instance at a distance, or `None` to cull it.
///
/// `view_distance_m` is the rule's hard cut; within it, the representation follows projected pixel height.
#[must_use]
pub fn instance_lod(
    proto: &ScatterProto,
    scale: f32,
    distance_m: f32,
    view_distance_m: f32,
    policy: &LodPolicy,
) -> Option<u8> {
    if distance_m > view_distance_m {
        return None;
    }
    let px = proto.height_m.max(0.01) * scale * crate::mesh::pixels_per_metre(distance_m, policy);
    if px < CULL_PIXEL_HEIGHT {
        return None;
    }
    let mesh_lods = proto.lod_keys.len().min(LOD_PIXEL_STEPS.len() - 1);
    for (i, threshold) in LOD_PIXEL_STEPS.iter().take(mesh_lods + 1).enumerate() {
        if px >= *threshold {
            return Some(i as u8);
        }
    }
    if proto.impostor_key.is_some() {
        Some(LOD_IMPOSTOR)
    } else {
        // No impostor authored: hold the coarsest mesh rather than popping the instance out of existence.
        Some(mesh_lods as u8)
    }
}

impl ScatterOutput {
    /// Append this chunk's visible instances to `out`, with their chosen representation.
    ///
    /// Pushing into a caller-owned buffer rather than returning a `Vec` keeps the per-frame path
    /// allocation-free, which matters because this runs for every resident chunk every frame.
    pub fn select_into(&self, terrain: &Terrain, eye: [f32; 3], out: &mut Vec<ScatterDraw>) {
        let recipe = terrain.recipe();
        for inst in &self.instances {
            let Some(proto) = recipe.protos.get(inst.proto as usize) else {
                continue;
            };
            let rule_dist = recipe
                .scatter
                .get(inst.rule as usize)
                .map_or(600.0, |r| r.view_distance_m);
            let dx = inst.position[0] - eye[0];
            let dy = inst.position[1] - eye[1];
            let dz = inst.position[2] - eye[2];
            let d = (dx * dx + dy * dy + dz * dz).sqrt();
            if let Some(lod) = instance_lod(proto, inst.scale, d, rule_dist, &recipe.lod) {
                out.push(ScatterDraw {
                    proto: inst.proto,
                    lod,
                    position: inst.position,
                    rotation: inst.rotation,
                    scale: inst.scale,
                });
            }
        }
    }
}

/// Scatter one chunk.
///
/// Reads heights, slopes and normals from the chunk's [`ChunkSamples`] block, so placement costs a handful
/// of cheap lattice reads per candidate rather than a field evaluation.
#[must_use]
#[allow(clippy::too_many_lines)] // one candidate loop with a sequence of cheap gates; splitting it would hide the ordering that makes it fast
pub fn scatter_chunk(terrain: &Terrain, samples: &ChunkSamples) -> ScatterOutput {
    let recipe = terrain.recipe();
    let mut out = ScatterOutput {
        coord: samples.coord,
        instances: Vec::new(),
        counts: vec![0; recipe.protos.len()],
    };
    if recipe.protos.is_empty() {
        return out;
    }
    let size = samples.size_m();
    let (ox, oz) = (samples.origin[0], samples.origin[1]);

    for (ri, rule) in recipe.scatter.iter().enumerate() {
        if !rule.enabled || rule.density_per_hectare <= 0.0 {
            continue;
        }
        if rule.proto >= recipe.protos.len() {
            continue;
        }
        // A prototype with no asset bound places nothing — a preset should not fail because the project has
        // no pine tree, and it should not litter the world with invisible instances either.
        if recipe.protos[rule.proto].mesh_key.is_empty() {
            continue;
        }
        // One candidate per cell; cell area is the mean area per instance.
        let cell = (10_000.0 / rule.density_per_hectare).sqrt().max(0.25);
        let seed = recipe.seed
            ^ rule.seed_offset.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ ((ri as u64 + 1).wrapping_mul(0xD1B5_4A32_D192_ED03));
        // World-space cell range covering this chunk (inclusive, so a cell straddling the edge is
        // considered exactly once — by the chunk that ends up containing its jittered point).
        let cx0 = (ox / cell).floor() as i32;
        let cx1 = ((ox + size) / cell).ceil() as i32;
        let cz0 = (oz / cell).floor() as i32;
        let cz1 = ((oz + size) / cell).ceil() as i32;
        let mut placed = 0usize;

        for cz in cz0..=cz1 {
            for cx in cx0..=cx1 {
                if placed >= MAX_PER_RULE_PER_CHUNK {
                    break;
                }
                let jx = noise::hash_unit(cx, cz, seed);
                let jz = noise::hash_unit_3(cx, cz, 1, seed);
                let wx = (cx as f32 + jx) * cell;
                let wz = (cz as f32 + jz) * cell;
                // Half-open ownership: exactly one chunk claims each candidate.
                if wx < ox || wx >= ox + size || wz < oz || wz >= oz + size {
                    continue;
                }
                // The keep/reject roll comes first — it is the cheapest gate and rejects most candidates.
                let roll = noise::hash_unit_3(cx, cz, 2, seed);

                let gx = (wx - ox) / samples.cell;
                let gz = (wz - oz) / samples.cell;
                let height = samples.height_bilinear(gx, gz);
                let slope = samples.slope_at_grid(gx, gz);
                if slope > rule.slope_max {
                    continue;
                }
                let hband = band_weight(rule.height_band, height);
                if hband <= 0.0 {
                    continue;
                }
                // Above water only, with a margin.
                if rule.avoid_water_m > 0.0 {
                    let depth = terrain.water_depth_at(wx, wz, height);
                    if depth > 0.0 || height < recipe.water.sea_level_m + rule.avoid_water_m {
                        continue;
                    }
                }
                // Keep clear of road/river corridors.
                if !terrain.splines().is_empty() && terrain.splines().clearance(wx, wz) <= 0.0 {
                    continue;
                }
                let mut accept = hband;
                if !rule.biomes.is_empty() {
                    let moisture = terrain.moisture_at(wx, wz, height);
                    let biomes = crate::biome::weights_at(recipe, height, slope, moisture, wx, wz);
                    let w: f32 = rule.biomes.iter().map(|b| biomes.weight_of(*b)).sum();
                    if w <= 0.0 {
                        continue;
                    }
                    accept *= w;
                }
                if let Some([wl, threshold, softness]) = rule.cluster {
                    let wl = wl.max(0.01);
                    let n =
                        0.5 + 0.5 * noise::fbm(wx / wl, wz / wl, seed ^ 0x77C1_3B95, 3, 2.0, 0.5);
                    accept *= noise::smoothstep(
                        threshold - softness.max(1e-4),
                        threshold + softness.max(1e-4),
                        n,
                    );
                }
                if roll >= accept {
                    continue;
                }

                let normal = samples.normal_at(gx.round() as i32, gz.round() as i32);
                let scale_t = noise::hash_unit_3(cx, cz, 3, seed);
                let scale = noise::lerp(
                    rule.scale_range[0].max(0.01),
                    rule.scale_range[1].max(rule.scale_range[0]),
                    scale_t,
                );
                let rotation = orientation(cx, cz, seed, normal, rule.align_to_normal);
                out.instances.push(ScatterInstance {
                    proto: rule.proto as u16,
                    rule: ri as u16,
                    position: [wx, height, wz],
                    rotation,
                    scale,
                });
                out.counts[rule.proto] += 1;
                placed += 1;
            }
        }
    }
    // Group by prototype so the host can draw one instanced batch per prototype without re-sorting.
    out.instances.sort_by_key(|i| (i.proto, i.rule));
    out
}

/// A hashed yaw, optionally tilted towards the surface normal — built from half-angle identities so no
/// trigonometric function is involved.
#[must_use]
pub fn orientation(cx: i32, cz: i32, seed: u64, normal: [f32; 3], align: f32) -> [f32; 4] {
    // A hashed point on the unit circle gives (cos θ, sin θ) without calling `cos`/`sin`.
    let mut ax = noise::hash_unit_3(cx, cz, 7, seed) * 2.0 - 1.0;
    let mut az = noise::hash_unit_3(cx, cz, 8, seed) * 2.0 - 1.0;
    let len = (ax * ax + az * az).sqrt();
    if len < 1e-4 {
        ax = 1.0;
        az = 0.0;
    } else {
        ax /= len;
        az /= len;
    }
    let yaw = quat_from_cos_sin_y(ax, az);
    let a = align.clamp(0.0, 1.0);
    if a <= 0.0 {
        return quat_normalize(yaw);
    }
    let tilt = quat_tilt_to_normal(normal, a);
    // Normalized once at the end: the half-angle identities are exact in real arithmetic but lose a little
    // precision near 180°, and a renderer handed a 1.001-length quaternion scales the instance with it.
    quat_normalize(quat_mul(tilt, yaw))
}

/// Renormalize a quaternion, falling back to identity if it has collapsed.
fn quat_normalize(q: [f32; 4]) -> [f32; 4] {
    let len2 = q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3];
    if len2 <= 1e-12 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let inv = 1.0 / len2.sqrt();
    [q[0] * inv, q[1] * inv, q[2] * inv, q[3] * inv]
}

/// Quaternion for a rotation about +Y given `(cos θ, sin θ)`.
fn quat_from_cos_sin_y(c: f32, s: f32) -> [f32; 4] {
    // Half-angle: cos(θ/2) = √((1+cos θ)/2), sin(θ/2) = sin θ / (2 cos(θ/2)).
    let cos_half = f32::midpoint(1.0, c.clamp(-1.0, 1.0)).max(0.0).sqrt();
    if cos_half < 1e-5 {
        // θ ≈ 180°.
        return [0.0, 1.0, 0.0, 0.0];
    }
    [0.0, s / (2.0 * cos_half), 0.0, cos_half]
}

/// Quaternion tilting +Y towards `normal` by `amount` (0 = upright, 1 = fully aligned).
fn quat_tilt_to_normal(normal: [f32; 3], amount: f32) -> [f32; 4] {
    let n = normal;
    // Axis = Y × N, which for Y = (0,1,0) is (n.z, 0, -n.x).
    let mut ax = n[2];
    let mut az = -n[0];
    let alen = (ax * ax + az * az).sqrt();
    if alen < 1e-5 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    ax /= alen;
    az /= alen;
    // Partial tilt: interpolate the cosine of the angle rather than the angle, which needs no `acos`.
    let cos_full = n[1].clamp(-1.0, 1.0);
    let cos_partial = noise::lerp(1.0, cos_full, amount);
    let cos_half = f32::midpoint(1.0, cos_partial).max(0.0).sqrt();
    let sin_half = ((1.0 - cos_partial) * 0.5).max(0.0).sqrt();
    [ax * sin_half, 0.0, az * sin_half, cos_half]
}

/// Hamilton product (xyzw).
fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

/// Unit direction pairs for evenly spaced vertical planes, tabulated so no trigonometry is needed.
const PLANE_DIRS: [[[f32; 2]; 4]; 3] = [
    // 2 planes at 0° and 90°.
    [[1.0, 0.0], [0.0, 1.0], [0.0, 0.0], [0.0, 0.0]],
    // 3 planes at 0°, 60°, 120°.
    [
        [1.0, 0.0],
        [0.5, 0.866_025_4],
        [-0.5, 0.866_025_4],
        [0.0, 0.0],
    ],
    // 4 planes at 0°, 45°, 90°, 135°.
    [
        [1.0, 0.0],
        [0.707_106_77, 0.707_106_77],
        [0.0, 1.0],
        [-0.707_106_77, 0.707_106_77],
    ],
];

/// Build an impostor: a billboard cloud of `planes` intersecting vertical quads plus an optional top quad.
///
/// Intersecting planes rather than one camera-facing quad is what makes the impostor *view-independent* — it
/// reads as a volume from any angle and needs no per-frame orientation, so it draws through the ordinary
/// instanced mesh path with the ordinary shader. The same billboard atlas is mapped onto every plane, which
/// is the standard foliage treatment.
#[must_use]
pub fn impostor_cross_mesh(radius_m: f32, height_m: f32, planes: u32, top_quad: bool) -> MeshData {
    let planes = planes.clamp(2, 4);
    let dirs = &PLANE_DIRS[(planes - 2) as usize];
    let r = radius_m.max(0.01);
    let h = height_m.max(0.01);
    let mut m = MeshData::default();
    for d in dirs.iter().take(planes as usize) {
        let d = *d;
        let base = m.positions.len() as u32;
        // A quad standing on the ground, centred on the axis, facing perpendicular to `d`.
        let (nx, nz) = (-d[1], d[0]);
        for (sx, sy, u, v) in [
            (-1.0f32, 0.0f32, 0.0f32, 1.0f32),
            (1.0, 0.0, 1.0, 1.0),
            (-1.0, 1.0, 0.0, 0.0),
            (1.0, 1.0, 1.0, 0.0),
        ] {
            m.positions.push([d[0] * r * sx, h * sy, d[1] * r * sx]);
            m.normals.push([nx, 0.0, nz]);
            m.uvs.push([u, v]);
        }
        m.indices
            .extend_from_slice(&[base, base + 1, base + 3, base, base + 3, base + 2]);
    }
    if top_quad {
        // A horizontal quad at two thirds height, so looking straight down still shows canopy.
        let base = m.positions.len() as u32;
        let y = h * 0.66;
        for (dx, dz, u, v) in [
            (-1.0f32, -1.0f32, 0.0f32, 0.0f32),
            (1.0, -1.0, 1.0, 0.0),
            (-1.0, 1.0, 0.0, 1.0),
            (1.0, 1.0, 1.0, 1.0),
        ] {
            m.positions.push([dx * r, y, dz * r]);
            m.normals.push([0.0, 1.0, 0.0]);
            m.uvs.push([u, v]);
        }
        m.indices
            .extend_from_slice(&[base, base + 1, base + 3, base, base + 3, base + 2]);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::{
        BiomeRule, Layer, LayerKind, MaterialLayer, ScatterRule, SplineDef, SplineKind,
        TerrainRecipe,
    };
    use std::collections::BTreeMap;

    fn wooded() -> Terrain {
        let mut r = TerrainRecipe {
            world_size_m: 256.0,
            chunk_size_m: 64.0,
            chunk_verts: 33,
            ..TerrainRecipe::default()
        };
        r.layers.push(Layer::new(
            "Hills",
            LayerKind::Fbm {
                amplitude: 25.0,
                wavelength_m: 140.0,
                octaves: 4,
                lacunarity: 2.0,
                gain: 0.5,
                warp_m: 0.0,
                warp_wavelength_m: 1.0,
            },
        ));
        r.water.sea_level_m = -1000.0;
        r.materials = vec![MaterialLayer::new("Grass", [0.2, 0.35, 0.15], 0.85)];
        r.biomes = vec![BiomeRule::by_height(
            "All",
            [-1.0e6, -1.0e6, 1.0e6, 1.0e6],
            0,
        )];
        r.protos = vec![ScatterProto {
            name: "Pine".into(),
            mesh_key: "mesh:pine".into(),
            lod_keys: vec!["mesh:pine1".into(), "mesh:pine2".into()],
            impostor_key: Some("mesh:pine_imp".into()),
            radius_m: 2.0,
            height_m: 14.0,
            collide: true,
        }];
        r.scatter = vec![ScatterRule {
            avoid_water_m: 0.0,
            ..ScatterRule::new("Pines", 0, 120.0)
        }];
        Terrain::compile(r, BTreeMap::new()).expect("compile")
    }

    #[test]
    fn scatter_is_bit_reproducible() {
        let t = wooded();
        let s = t.sample_chunk(ChunkCoord::new(1, 1)).expect("chunk");
        let a = scatter_chunk(&t, &s);
        let b = scatter_chunk(&t, &s);
        assert_eq!(a, b);
        assert!(!a.is_empty(), "nothing was scattered");
    }

    #[test]
    fn every_candidate_is_claimed_by_exactly_one_chunk() {
        // The seam proof: no instance is duplicated across a chunk boundary and none is lost.
        let t = wooded();
        let mut all: Vec<[u32; 2]> = Vec::new();
        for cz in 0..4 {
            for cx in 0..4 {
                let s = t.sample_chunk(ChunkCoord::new(cx, cz)).expect("chunk");
                let out = scatter_chunk(&t, &s);
                for i in &out.instances {
                    // Positions must be inside the chunk that produced them.
                    assert!(
                        i.position[0] >= s.origin[0] && i.position[0] < s.origin[0] + s.size_m()
                    );
                    assert!(
                        i.position[2] >= s.origin[1] && i.position[2] < s.origin[1] + s.size_m()
                    );
                    all.push([i.position[0].to_bits(), i.position[2].to_bits()]);
                }
            }
        }
        let before = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(before, all.len(), "an instance was placed twice");
        assert!(
            before > 100,
            "the test world scattered too little to prove anything"
        );
    }

    #[test]
    fn instances_sit_on_the_surface() {
        let t = wooded();
        let s = t.sample_chunk(ChunkCoord::new(2, 2)).expect("chunk");
        for i in scatter_chunk(&t, &s).instances {
            let h = t.height(i.position[0], i.position[2]);
            assert!(
                (i.position[1] - h).abs() < 0.35,
                "instance floating or buried: {} vs terrain {h}",
                i.position[1]
            );
        }
    }

    #[test]
    fn slope_and_height_gates_are_respected() {
        let mut r = wooded().recipe().clone();
        r.scatter[0].slope_max = 0.15;
        r.scatter[0].height_band = [8.0, 10.0, 18.0, 20.0];
        let t = Terrain::compile(r, BTreeMap::new()).expect("compile");
        let mut n = 0;
        for cz in 0..4 {
            for cx in 0..4 {
                let s = t.sample_chunk(ChunkCoord::new(cx, cz)).expect("chunk");
                for i in scatter_chunk(&t, &s).instances {
                    let slope = t.slope(i.position[0], i.position[2]);
                    assert!(slope <= 0.16, "placed on a {slope} slope");
                    assert!(
                        i.position[1] >= 7.9 && i.position[1] <= 20.1,
                        "outside the height band: {}",
                        i.position[1]
                    );
                    n += 1;
                }
            }
        }
        assert!(n > 0, "the gates rejected everything — test proves nothing");
    }

    #[test]
    fn roads_are_kept_clear() {
        let mut r = wooded().recipe().clone();
        r.scatter[0].density_per_hectare = 900.0;
        r.splines.push(SplineDef {
            name: "Road".into(),
            kind: SplineKind::Road,
            points: vec![[0.0, 0.0, 96.0], [256.0, 0.0, 96.0]],
            width_m: 10.0,
            falloff_m: 4.0,
            clear_scatter_m: 2.0,
            ..SplineDef::default()
        });
        let t = Terrain::compile(r, BTreeMap::new()).expect("compile");
        for cx in 0..4 {
            let s = t.sample_chunk(ChunkCoord::new(cx, 1)).expect("chunk");
            for i in scatter_chunk(&t, &s).instances {
                let from_road = (i.position[2] - 96.0).abs();
                assert!(from_road > 6.9, "tree in the road at {from_road} m");
            }
        }
    }

    #[test]
    fn density_scales_the_instance_count() {
        let t = wooded();
        let s = t.sample_chunk(ChunkCoord::new(1, 2)).expect("chunk");
        let sparse = scatter_chunk(&t, &s).len();
        let mut r = t.recipe().clone();
        r.scatter[0].density_per_hectare = 480.0;
        let dense = {
            let t2 = Terrain::compile(r, BTreeMap::new()).expect("compile");
            let s2 = t2.sample_chunk(ChunkCoord::new(1, 2)).expect("chunk");
            scatter_chunk(&t2, &s2).len()
        };
        assert!(
            dense > sparse * 2,
            "4x density gave {dense} against {sparse}"
        );
    }

    #[test]
    fn a_runaway_density_is_capped_not_fatal() {
        let mut r = wooded().recipe().clone();
        r.scatter[0].density_per_hectare = 5_000_000.0;
        let t = Terrain::compile(r, BTreeMap::new()).expect("compile");
        let s = t.sample_chunk(ChunkCoord::new(0, 0)).expect("chunk");
        let out = scatter_chunk(&t, &s);
        assert!(
            out.len() <= MAX_PER_RULE_PER_CHUNK,
            "cap not enforced: {}",
            out.len()
        );
    }

    #[test]
    fn orientations_are_unit_quaternions() {
        for i in 0..500 {
            let n = crate::mesh::ChunkSamples {
                coord: ChunkCoord::new(0, 0),
                verts: 3,
                cell: 1.0,
                origin: [0.0, 0.0],
                heights: vec![0.0; 25],
                min_y: 0.0,
                max_y: 0.0,
            }
            .normal_at(1, 1);
            let q = orientation(i, i * 3, 42, n, 0.6);
            let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
            assert!((len - 1.0).abs() < 1e-4, "not unit: {len}");
        }
        // Full alignment on a tilted normal actually tilts.
        let tilted = orientation(1, 1, 7, [0.6, 0.8, 0.0], 1.0);
        let upright = orientation(1, 1, 7, [0.0, 1.0, 0.0], 1.0);
        assert!(
            (tilted[0] - upright[0]).abs() + (tilted[2] - upright[2]).abs() > 0.05,
            "alignment had no effect"
        );
    }

    #[test]
    fn lod_follows_projected_size_not_raw_distance() {
        let policy = LodPolicy::default();
        let tree = ScatterProto {
            lod_keys: vec!["a".into(), "b".into()],
            impostor_key: Some("i".into()),
            ..ScatterProto::new("Tree", "m", 2.0, 20.0)
        };
        let tuft = ScatterProto {
            lod_keys: vec!["a".into()],
            impostor_key: None,
            ..ScatterProto::new("Tuft", "m", 0.2, 0.3)
        };
        // At the same distance, the big thing keeps a finer representation than the small one.
        let d = 60.0;
        let big = instance_lod(&tree, 1.0, d, 1000.0, &policy).expect("visible");
        let small = instance_lod(&tuft, 1.0, d, 1000.0, &policy).expect("visible");
        assert!(
            big < small || small == LOD_IMPOSTOR,
            "big {big} small {small}"
        );
        // Far enough that a 20 m tree projects below the last mesh threshold, it becomes an impostor; past
        // the rule's own cut it is dropped entirely. (At 900 m a 20 m tree is still ~23 px tall, which
        // legitimately deserves a mesh — the pixel criterion is doing its job.)
        let px_at = |d: f32| 20.0 * crate::mesh::pixels_per_metre(d, &policy);
        assert!(
            px_at(3000.0) < LOD_PIXEL_STEPS[2],
            "test distance is not far enough: {} px",
            px_at(3000.0)
        );
        assert_eq!(
            instance_lod(&tree, 1.0, 3000.0, 4000.0, &policy),
            Some(LOD_IMPOSTOR)
        );
        assert_eq!(
            instance_lod(&tree, 1.0, 4200.0, 4000.0, &policy),
            None,
            "past the cut"
        );
        // A prototype with no impostor holds its coarsest mesh instead of vanishing.
        let far = instance_lod(&tuft, 1.0, 30.0, 1000.0, &policy);
        assert_eq!(far, Some(1));
    }

    #[test]
    fn selection_is_allocation_stable_and_distance_filtered() {
        let t = wooded();
        let s = t.sample_chunk(ChunkCoord::new(1, 1)).expect("chunk");
        let out = scatter_chunk(&t, &s);
        let mut draws = Vec::new();
        out.select_into(&t, [96.0, 40.0, 96.0], &mut draws);
        let near = draws.len();
        draws.clear();
        out.select_into(&t, [96.0, 4000.0, 96.0], &mut draws);
        assert!(draws.len() < near, "distance did not thin the selection");
    }

    #[test]
    fn impostor_geometry_is_a_valid_billboard_cloud() {
        for planes in 2..=4u32 {
            let m = impostor_cross_mesh(1.5, 8.0, planes, true);
            let quads = planes as usize + 1;
            assert_eq!(m.positions.len(), quads * 4);
            assert_eq!(m.indices.len(), quads * 6);
            for i in &m.indices {
                assert!((*i as usize) < m.positions.len());
            }
            // Stands on the ground and reaches the full height.
            let lo = m.positions.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
            let hi = m.positions.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
            assert_eq!(lo, 0.0);
            assert!((hi - 8.0).abs() < 1e-5);
            // UVs cover the atlas.
            assert!(m.uvs.iter().any(|uv| uv[0] == 0.0 && uv[1] == 0.0));
            assert!(m.uvs.iter().any(|uv| uv[0] == 1.0 && uv[1] == 1.0));
        }
    }
}
