//! Chunk meshing — LODs that cannot crack and do not pop.
//!
//! ## The two failure modes, and how each is closed
//!
//! **Cracks** come from a T-junction: a fine chunk's edge has vertices its coarse neighbour does not, so
//! the surfaces separate and you see through to the sky. Two mechanisms remove them here. First, coarse
//! vertices are an exact *subset* of fine ones (`chunk_verts` is `2^k + 1`, so every LOD subsamples on the
//! lattice) and both chunks evaluate the same pointwise field, so shared vertices coincide **bit-exactly**
//! rather than approximately. Second, every chunk carries a **skirt**: a vertical curtain hanging from its
//! border, deep enough to cover the largest gap any neighbouring LOD could open. The skirt depth is derived
//! from the chunk's own *measured* error at its coarsest level, so it is a proof rather than a guess.
//!
//! **Popping** comes from switching LOD while the geometric change is still visible. The fix is to switch
//! when it is not: each LOD's error is *measured* at build time as the largest vertical deviation from the
//! full-resolution surface, and [`switch_distance`] converts that into the distance at which the deviation
//! projects to the policy's pixel budget. At the default one pixel, the geometry moves by less than a pixel
//! when it switches — there is nothing left to see. This is Ulrich's chunked-LOD criterion, and it is worth
//! preferring over a morphing shader (CDLOD-style vertex interpolation) because it costs no vertex
//! attributes, no per-chunk uniform, no shader variant, and it keeps terrain meshes indistinguishable from
//! every other mesh in the renderer. Vertex morphing remains the named upgrade if a project ever wants to
//! push switches nearer than one pixel of error.
//!
//! ## One sample block, every artefact
//!
//! Everything in this module reads a [`ChunkSamples`] block — one field pass at LOD-0 resolution with a
//! one-cell halo — rather than sampling the field itself. That is why all LODs of a chunk share identical
//! heights *and* identical normals at coincident vertices (so a switch cannot shade differently either), and
//! why normals match across chunk borders (the halo is the neighbour's interior, computed by the same
//! expression).

use crate::recipe::{ChunkCoord, LodPolicy};

/// Raw mesh arrays, in world units. The host converts this into whatever its asset pipeline wants.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshData {
    /// Vertex positions.
    pub positions: Vec<[f32; 3]>,
    /// Vertex normals (unit).
    pub normals: Vec<[f32; 3]>,
    /// Vertex texture coordinates.
    pub uvs: Vec<[f32; 2]>,
    /// Triangle-list indices.
    pub indices: Vec<u32>,
}

impl MeshData {
    /// Triangle count.
    #[must_use]
    pub fn tri_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Vertex count.
    #[must_use]
    pub fn vert_count(&self) -> usize {
        self.positions.len()
    }

    /// Heap bytes — what the mesh budget accounts.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.positions.len() * 12
            + self.normals.len() * 12
            + self.uvs.len() * 8
            + self.indices.len() * 4
    }

    /// Whether the mesh is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// One chunk's field evaluation: LOD-0 heights over the chunk plus a one-cell halo.
///
/// Indices run `-1 ..= verts` on both axes: `0 ..= verts-1` is the chunk itself and `-1`/`verts` are the halo
/// ring, which exists so that border normals are computed from real neighbouring terrain instead of a
/// clamped edge.
#[derive(Clone, Debug, PartialEq)]
pub struct ChunkSamples {
    /// Which chunk this is.
    pub coord: ChunkCoord,
    /// Vertices along a chunk edge at LOD 0.
    pub verts: u32,
    /// LOD-0 cell size in metres.
    pub cell: f32,
    /// World XZ of the chunk's minimum corner.
    pub origin: [f32; 2],
    /// `(verts + 2)²` heights, row-major, offset by one for the halo.
    pub heights: Vec<f32>,
    /// Lowest interior height, in metres.
    pub min_y: f32,
    /// Highest interior height, in metres.
    pub max_y: f32,
}

impl ChunkSamples {
    /// Row stride of [`Self::heights`].
    #[must_use]
    pub fn stride(&self) -> usize {
        self.verts as usize + 2
    }

    /// Height at a grid index, halo-aware and clamped outside it.
    #[must_use]
    pub fn height_at(&self, ix: i32, iz: i32) -> f32 {
        let s = self.stride() as i32;
        let jx = (ix + 1).clamp(0, s - 1);
        let jz = (iz + 1).clamp(0, s - 1);
        self.heights[(jz * s + jx) as usize]
    }

    /// World XZ of a grid index.
    #[must_use]
    pub fn world_xz(&self, ix: i32, iz: i32) -> (f32, f32) {
        (
            self.origin[0] + ix as f32 * self.cell,
            self.origin[1] + iz as f32 * self.cell,
        )
    }

    /// Unit normal at a grid index, central-differenced at the LOD-0 cell size.
    ///
    /// Always at the LOD-0 spacing, whatever LOD is being built — that is what keeps normals identical
    /// across LODs and across chunk borders.
    #[must_use]
    pub fn normal_at(&self, ix: i32, iz: i32) -> [f32; 3] {
        let dx = self.height_at(ix + 1, iz) - self.height_at(ix - 1, iz);
        let dz = self.height_at(ix, iz + 1) - self.height_at(ix, iz - 1);
        let v = [-dx, 2.0 * self.cell, -dz];
        let len2 = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
        if len2 <= 1e-20 {
            return [0.0, 1.0, 0.0];
        }
        let inv = 1.0 / len2.sqrt();
        [v[0] * inv, v[1] * inv, v[2] * inv]
    }

    /// Bilinear height at a *fractional* grid index — how the texture bake and the collider read the block
    /// at their own resolutions without touching the field again.
    #[must_use]
    pub fn height_bilinear(&self, gx: f32, gz: f32) -> f32 {
        let ix = gx.floor();
        let iz = gz.floor();
        let tx = gx - ix;
        let tz = gz - iz;
        let (ix, iz) = (ix as i32, iz as i32);
        let a = self.height_at(ix, iz);
        let b = self.height_at(ix + 1, iz);
        let c = self.height_at(ix, iz + 1);
        let d = self.height_at(ix + 1, iz + 1);
        let top = a + (b - a) * tx;
        let bot = c + (d - c) * tx;
        top + (bot - top) * tz
    }

    /// Slope (rise over run) at a fractional grid index, from the block's own central differences.
    #[must_use]
    pub fn slope_at_grid(&self, gx: f32, gz: f32) -> f32 {
        let dx = (self.height_bilinear(gx + 1.0, gz) - self.height_bilinear(gx - 1.0, gz))
            / (2.0 * self.cell);
        let dz = (self.height_bilinear(gx, gz + 1.0) - self.height_bilinear(gx, gz - 1.0))
            / (2.0 * self.cell);
        (dx * dx + dz * dz).sqrt()
    }

    /// Chunk edge length in metres.
    #[must_use]
    pub fn size_m(&self) -> f32 {
        (self.verts - 1) as f32 * self.cell
    }

    /// World XZ of the chunk centre.
    #[must_use]
    pub fn center_xz(&self) -> [f32; 2] {
        let half = self.size_m() * 0.5;
        [self.origin[0] + half, self.origin[1] + half]
    }

    /// Heap bytes.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.heights.len() * 4
    }
}

/// Interpolate a height inside a quad the way the triangulation actually does, so a measured error is the
/// error of the mesh that will be drawn rather than of a bilinear surface nobody renders.
///
/// The quad is split on the `00`–`11` diagonal: triangle `(00, 10, 11)` covers `tz <= tx` and
/// `(00, 11, 01)` covers the rest.
#[must_use]
pub fn interp_triangulated(h00: f32, h10: f32, h01: f32, h11: f32, tx: f32, tz: f32) -> f32 {
    if tz <= tx {
        h00 + (h10 - h00) * tx + (h11 - h10) * tz
    } else {
        h00 + (h11 - h01) * tx + (h01 - h00) * tz
    }
}

/// A chunk's measured error at one LOD.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LodError {
    /// LOD level.
    pub lod: u8,
    /// Largest vertical deviation, in metres, between this LOD's rendered surface and the LOD-0 surface.
    ///
    /// Measured, not estimated: every LOD-0 vertex is compared against the triangle it would fall inside at
    /// this LOD. A flat chunk therefore reports exactly zero and can be drawn at its coarsest level from any
    /// distance, which is a real and common win.
    pub error_m: f32,
}

/// Measure a LOD's geometric error against the full-resolution surface.
#[must_use]
pub fn measure_lod_error(samples: &ChunkSamples, lod: u8) -> f32 {
    let step = 1i32 << u32::from(lod);
    let n = samples.verts as i32 - 1;
    if step >= n || step <= 0 {
        // Degenerate LOD: compare against the single quad spanning the chunk.
        return max_deviation(samples, n.max(1));
    }
    max_deviation(samples, step)
}

fn max_deviation(samples: &ChunkSamples, step: i32) -> f32 {
    let n = samples.verts as i32 - 1;
    let mut worst = 0.0f32;
    for iz in 0..=n {
        for ix in 0..=n {
            // Skip the vertices this LOD actually keeps — their error is exactly zero.
            if ix % step == 0 && iz % step == 0 {
                continue;
            }
            let qx = (ix / step) * step;
            let qz = (iz / step) * step;
            let qx1 = (qx + step).min(n);
            let qz1 = (qz + step).min(n);
            let span_x = (qx1 - qx).max(1) as f32;
            let span_z = (qz1 - qz).max(1) as f32;
            let tx = (ix - qx) as f32 / span_x;
            let tz = (iz - qz) as f32 / span_z;
            let approx = interp_triangulated(
                samples.height_at(qx, qz),
                samples.height_at(qx1, qz),
                samples.height_at(qx, qz1),
                samples.height_at(qx1, qz1),
                tx,
                tz,
            );
            worst = worst.max((samples.height_at(ix, iz) - approx).abs());
        }
    }
    worst
}

/// A built chunk mesh at one LOD, ready for the host to store and draw.
#[derive(Clone, Debug, PartialEq)]
pub struct ChunkMesh {
    /// Which chunk.
    pub coord: ChunkCoord,
    /// Which LOD.
    pub lod: u8,
    /// Geometry, with positions **local to the chunk centre in XZ and absolute in Y**.
    ///
    /// Local XZ is what lets two identical chunks (two flat plates, two copies of the same plateau) hash to
    /// the same content address and share one GPU upload. Absolute Y keeps the vertical bounds honest
    /// without a second offset to track.
    pub data: MeshData,
    /// World position the instance is placed at: `[centre_x, 0, centre_z]`.
    pub instance_center: [f32; 3],
    /// World-space AABB minimum, including the skirt.
    pub bounds_min: [f32; 3],
    /// World-space AABB maximum.
    pub bounds_max: [f32; 3],
    /// This LOD's measured geometric error, in metres.
    pub geometric_error_m: f32,
    /// How far the skirt hangs below the border, in metres.
    pub skirt_depth_m: f32,
}

impl ChunkMesh {
    /// Heap bytes.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.data.bytes()
    }

    /// Distance beyond which this LOD may be used without exceeding the policy's pixel budget.
    #[must_use]
    pub fn min_distance_m(&self, policy: &LodPolicy) -> f32 {
        switch_distance(self.geometric_error_m, policy)
    }
}

/// Build a chunk's mesh at `lod`, with a crack-proof skirt.
///
/// The skirt is sized from the chunk's error at its *coarsest* level rather than at this one, so the curtain
/// covers the worst gap any neighbour could open regardless of which LODs end up adjacent. Sizing it from
/// the local LOD instead is the subtle bug that produces cracks only when the camera moves fast.
#[must_use]
pub fn build_chunk_mesh(samples: &ChunkSamples, lod: u8, policy: &LodPolicy) -> ChunkMesh {
    let errors = measure_all_lods(samples, policy.levels.max(lod + 1));
    build_chunk_mesh_with_errors(samples, lod, policy, &errors)
}

/// [`build_chunk_mesh`] when the caller has already measured the chunk's errors.
///
/// Measuring is a full sweep of the chunk per level, and a naive build measured the same chunk once for the
/// mesh's own error and again for the skirt depth, at every level — several times the necessary work per
/// chunk. The streaming path measures once and passes the result here.
#[must_use]
pub fn build_chunk_mesh_with_errors(
    samples: &ChunkSamples,
    lod: u8,
    policy: &LodPolicy,
    errors: &[LodError],
) -> ChunkMesh {
    let n = samples.verts as i32 - 1;
    let step = (1i32 << u32::from(lod)).min(n.max(1));
    let side = (n / step) + 1;
    let center = samples.center_xz();
    let size = samples.size_m();

    let mut data = MeshData {
        positions: Vec::with_capacity((side * side) as usize + (side as usize) * 4),
        normals: Vec::with_capacity((side * side) as usize + (side as usize) * 4),
        uvs: Vec::with_capacity((side * side) as usize + (side as usize) * 4),
        indices: Vec::with_capacity(((side - 1) * (side - 1) * 6) as usize + side as usize * 24),
    };

    let mut min_y = f32::MAX;
    let mut max_y = f32::MIN;
    for jz in 0..side {
        for jx in 0..side {
            let (ix, iz) = (jx * step, jz * step);
            let (wx, wz) = samples.world_xz(ix, iz);
            let h = samples.height_at(ix, iz);
            min_y = min_y.min(h);
            max_y = max_y.max(h);
            data.positions.push([wx - center[0], h, wz - center[1]]);
            data.normals.push(samples.normal_at(ix, iz));
            // UV spans the chunk exactly, which is what the baked splat texture is addressed by.
            data.uvs.push([
                (wx - samples.origin[0]) / size,
                (wz - samples.origin[1]) / size,
            ]);
        }
    }

    let idx = |jx: i32, jz: i32| -> u32 { (jz * side + jx) as u32 };
    for jz in 0..side - 1 {
        for jx in 0..side - 1 {
            let (a, b, c, d) = (
                idx(jx, jz),
                idx(jx + 1, jz),
                idx(jx, jz + 1),
                idx(jx + 1, jz + 1),
            );
            // Split on the a–d diagonal, matching `interp_triangulated` exactly.
            data.indices.extend_from_slice(&[a, b, d, a, d, c]);
        }
    }

    // The skirt: repeat the border ring, dropped by the worst-case error, and stitch a wall.
    let coarsest = policy.levels.saturating_sub(1);
    let worst = errors
        .iter()
        .find(|e| e.lod == coarsest)
        .map_or_else(|| measure_lod_error(samples, coarsest), |e| e.error_m);
    let skirt_depth = worst.max(samples.cell) + policy.skirt_margin_m.max(0.0);
    let ring: Vec<u32> = border_ring(side);
    let base = data.positions.len() as u32;
    for &v in &ring {
        let p = data.positions[v as usize];
        data.positions.push([p[0], p[1] - skirt_depth, p[2]]);
        // Outward-ish normal: the skirt is only ever seen edge-on through a crack, so a horizontal normal
        // keeps it from catching the light and drawing attention to itself. Re-normalized — flattening Y out
        // of a unit vector does not leave a unit vector, and a non-unit normal shades wrongly.
        let n = data.normals[v as usize];
        let len = (n[0] * n[0] + n[2] * n[2]).sqrt();
        data.normals.push(if len > 1e-5 {
            [n[0] / len, 0.0, n[2] / len]
        } else {
            // A perfectly flat border vertex has no horizontal component to keep; point the curtain out
            // along +X rather than emitting a zero-length normal.
            [1.0, 0.0, 0.0]
        });
        data.uvs.push(data.uvs[v as usize]);
    }
    for k in 0..ring.len() {
        let k2 = (k + 1) % ring.len();
        let (top_a, top_b) = (ring[k], ring[k2]);
        let (bot_a, bot_b) = (base + k as u32, base + k2 as u32);
        data.indices
            .extend_from_slice(&[top_a, bot_a, top_b, top_b, bot_a, bot_b]);
    }

    let half = size * 0.5;
    ChunkMesh {
        coord: samples.coord,
        lod,
        data,
        instance_center: [center[0], 0.0, center[1]],
        bounds_min: [
            center[0] - half,
            min_y.min(samples.min_y) - skirt_depth,
            center[1] - half,
        ],
        bounds_max: [center[0] + half, max_y.max(samples.max_y), center[1] + half],
        geometric_error_m: errors
            .iter()
            .find(|e| e.lod == lod)
            .map_or_else(|| measure_lod_error(samples, lod), |e| e.error_m),
        skirt_depth_m: skirt_depth,
    }
}

/// The border ring of a `side × side` vertex grid, in order, so the skirt wall is a closed loop.
fn border_ring(side: i32) -> Vec<u32> {
    let idx = |jx: i32, jz: i32| -> u32 { (jz * side + jx) as u32 };
    let mut r = Vec::with_capacity((side as usize) * 4);
    for jx in 0..side {
        r.push(idx(jx, 0));
    }
    for jz in 1..side {
        r.push(idx(side - 1, jz));
    }
    for jx in (0..side - 1).rev() {
        r.push(idx(jx, side - 1));
    }
    for jz in (1..side - 1).rev() {
        r.push(idx(0, jz));
    }
    r
}

/// Every LOD's measured error for a chunk — what the streamer needs to choose one.
#[must_use]
pub fn measure_all_lods(samples: &ChunkSamples, levels: u8) -> Vec<LodError> {
    (0..levels.max(1))
        .map(|lod| LodError {
            lod,
            error_m: measure_lod_error(samples, lod),
        })
        .collect()
}

/// The pixel height one metre of vertical error subtends at a distance.
#[must_use]
pub fn pixels_per_metre(distance_m: f32, policy: &LodPolicy) -> f32 {
    let d = distance_m.max(0.01);
    // Presentation maths, not content maths — a tangent is fine here (see the crate docs on determinism).
    let half_fov = (policy.fov_y_deg.clamp(1.0, 179.0) * 0.5).to_radians();
    let t = half_fov.tan().max(1e-4);
    policy.viewport_height_px as f32 / (2.0 * d * t)
}

/// How many pixels a geometric error projects to at a distance.
#[must_use]
pub fn screen_error_px(geometric_error_m: f32, distance_m: f32, policy: &LodPolicy) -> f32 {
    geometric_error_m * pixels_per_metre(distance_m, policy)
}

/// The nearest distance at which a LOD's error still fits the pixel budget.
///
/// Zero for a LOD with no error, which is why a flat chunk is drawn at its coarsest level everywhere.
#[must_use]
pub fn switch_distance(geometric_error_m: f32, policy: &LodPolicy) -> f32 {
    if geometric_error_m <= 0.0 {
        return 0.0;
    }
    let budget = policy.screen_error_px.max(0.05);
    let half_fov = (policy.fov_y_deg.clamp(1.0, 179.0) * 0.5).to_radians();
    let t = half_fov.tan().max(1e-4);
    geometric_error_m * policy.viewport_height_px as f32 / (2.0 * budget * t)
}

/// Choose a LOD for a distance: the coarsest whose measured error fits the pixel budget.
///
/// `previous` enables hysteresis — a chunk keeps its current level inside a band around the switch, so a
/// camera hovering on a boundary does not thrash between two uploads.
#[must_use]
pub fn choose_lod(
    errors: &[LodError],
    distance_m: f32,
    policy: &LodPolicy,
    previous: Option<u8>,
) -> u8 {
    let mut pick = 0u8;
    for e in errors {
        if switch_distance(e.error_m, policy) <= distance_m {
            pick = pick.max(e.lod);
        }
    }
    if let Some(prev) = previous {
        if prev != pick {
            // The boundary being crossed is the switch distance of the coarser of the two levels. Stay put
            // while the camera is still inside the band around it.
            let coarser = prev.max(pick);
            if let Some(e) = errors.iter().find(|e| e.lod == coarser) {
                let d = switch_distance(e.error_m, policy);
                let band = d * policy.hysteresis.clamp(0.0, 0.9);
                if (distance_m - d).abs() < band {
                    return prev;
                }
            }
        }
    }
    pick
}

/// A flat water quad covering the chunk, when any part of it is submerged. `None` when the chunk is dry, so
/// a mountain range costs nothing.
#[must_use]
pub fn build_water_mesh(samples: &ChunkSamples, sea_level_m: f32) -> Option<MeshData> {
    if samples.min_y >= sea_level_m {
        return None;
    }
    // Chunk-local, like the terrain mesh, so it rides the same instance placement.
    let half = samples.size_m() * 0.5;
    let mut m = MeshData::default();
    for (dx, dz) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        m.positions.push([dx * half, sea_level_m, dz * half]);
        m.normals.push([0.0, 1.0, 0.0]);
        let u = f32::midpoint(dx, 1.0) * half * 0.1;
        let v = f32::midpoint(dz, 1.0) * half * 0.1;
        m.uvs.push([u, v]);
    }
    m.indices.extend_from_slice(&[0, 1, 3, 0, 3, 2]);
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::Terrain;
    use crate::recipe::{Layer, LayerKind, TerrainRecipe};
    use std::collections::BTreeMap;

    fn hilly() -> Terrain {
        let mut r = TerrainRecipe {
            world_size_m: 512.0,
            chunk_size_m: 64.0,
            chunk_verts: 33,
            ..TerrainRecipe::default()
        };
        r.layers.push(Layer::new(
            "Hills",
            LayerKind::Fbm {
                amplitude: 30.0,
                wavelength_m: 90.0,
                octaves: 5,
                lacunarity: 2.0,
                gain: 0.5,
                warp_m: 8.0,
                warp_wavelength_m: 200.0,
            },
        ));
        Terrain::compile(r, BTreeMap::new()).expect("compile")
    }

    fn flat() -> Terrain {
        let r = TerrainRecipe {
            world_size_m: 512.0,
            chunk_size_m: 64.0,
            chunk_verts: 33,
            ..TerrainRecipe::default()
        };
        Terrain::compile(r, BTreeMap::new()).expect("compile")
    }

    #[test]
    fn lod0_is_exact_and_coarser_lods_lose_detail_monotonically() {
        let t = hilly();
        let s = t.sample_chunk(ChunkCoord::new(2, 2)).expect("chunk");
        let errors = measure_all_lods(&s, 5);
        assert_eq!(errors[0].error_m, 0.0, "LOD 0 is the reference");
        for w in errors.windows(2) {
            assert!(
                w[1].error_m >= w[0].error_m - 1e-6,
                "error must not improve as the mesh coarsens: {w:?}"
            );
        }
        assert!(
            errors[3].error_m > 0.05,
            "a hilly chunk must lose something"
        );
    }

    #[test]
    fn a_flat_chunk_has_zero_error_at_every_lod() {
        // Which is why it can be drawn at its coarsest level from a metre away — a real and common win.
        let t = flat();
        let s = t.sample_chunk(ChunkCoord::new(1, 1)).expect("chunk");
        for e in measure_all_lods(&s, 5) {
            assert_eq!(
                e.error_m, 0.0,
                "flat terrain reported error at LOD {}",
                e.lod
            );
            assert_eq!(switch_distance(e.error_m, &LodPolicy::default()), 0.0);
        }
    }

    #[test]
    fn the_mesh_is_well_formed_at_every_lod() {
        let t = hilly();
        let s = t.sample_chunk(ChunkCoord::new(3, 1)).expect("chunk");
        let policy = LodPolicy::default();
        for lod in 0..4u8 {
            let m = build_chunk_mesh(&s, lod, &policy);
            assert_eq!(m.data.indices.len() % 3, 0);
            assert_eq!(m.data.positions.len(), m.data.normals.len());
            assert_eq!(m.data.positions.len(), m.data.uvs.len());
            for i in &m.data.indices {
                assert!((*i as usize) < m.data.positions.len(), "index out of range");
            }
            for n in &m.data.normals {
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                assert!((len - 1.0).abs() < 1e-3, "normal not unit: {len}");
            }
            // The surface (before the skirt) spans exactly one chunk.
            let side = (32 >> lod) + 1;
            let surface = (side * side) as usize;
            let xs: Vec<f32> = m.data.positions[..surface].iter().map(|p| p[0]).collect();
            let lo = xs.iter().copied().fold(f32::MAX, f32::min);
            let hi = xs.iter().copied().fold(f32::MIN, f32::max);
            assert!(
                (hi - lo - 64.0).abs() < 1e-3,
                "chunk span at LOD {lod}: {}",
                hi - lo
            );
        }
    }

    #[test]
    fn shared_edges_are_bit_identical_across_lods_and_chunks() {
        // The crack proof, at the vertex level: a fine chunk's edge vertex and its coarse neighbour's
        // corresponding vertex are the same number, not merely close.
        let t = hilly();
        let a = t.sample_chunk(ChunkCoord::new(2, 2)).expect("a");
        let b = t.sample_chunk(ChunkCoord::new(3, 2)).expect("b");
        let policy = LodPolicy::default();
        let fine = build_chunk_mesh(&a, 0, &policy);
        let coarse = build_chunk_mesh(&b, 2, &policy);
        let n = a.verts as i32 - 1;
        // Every vertex the coarse neighbour keeps on the shared edge must exist, identically, on the fine
        // side of that edge.
        for jz in 0..=(n / 4) {
            let iz = jz * 4;
            let coarse_h = b.height_at(0, iz);
            let fine_h = a.height_at(n, iz);
            assert_eq!(coarse_h.to_bits(), fine_h.to_bits(), "edge vertex differs");
        }
        // And the meshes themselves carry those heights.
        assert!(fine
            .data
            .positions
            .iter()
            .any(|p| p[1].to_bits() == a.height_at(n, 0).to_bits()));
        assert!(coarse
            .data
            .positions
            .iter()
            .any(|p| p[1].to_bits() == b.height_at(0, 0).to_bits()));
    }

    #[test]
    fn the_skirt_covers_the_worst_gap_any_neighbour_lod_could_open() {
        // The crack proof, at the surface level: the deepest point a coarse neighbour's triangle can sag to
        // along a shared edge must still be above this chunk's skirt bottom.
        let t = hilly();
        let policy = LodPolicy::default();
        let s = t.sample_chunk(ChunkCoord::new(2, 3)).expect("chunk");
        let m = build_chunk_mesh(&s, 0, &policy);
        let worst_coarse_error = measure_lod_error(&s, policy.levels - 1);
        assert!(
            m.skirt_depth_m >= worst_coarse_error,
            "skirt {} shallower than the worst neighbour sag {worst_coarse_error}",
            m.skirt_depth_m
        );
        // And the bounds include the skirt, so culling cannot clip a chunk whose skirt is still visible.
        assert!(m.bounds_min[1] <= s.min_y - worst_coarse_error);
    }

    #[test]
    fn skirt_geometry_is_a_closed_wall() {
        let t = hilly();
        let s = t.sample_chunk(ChunkCoord::new(1, 1)).expect("chunk");
        let m = build_chunk_mesh(&s, 1, &policy_1080());
        let side = (32 >> 1) + 1;
        let ring = border_ring(side);
        assert_eq!(
            ring.len(),
            (side as usize - 1) * 4,
            "ring must close exactly once"
        );
        // Skirt vertices are exactly the ring, dropped.
        let surface = (side * side) as usize;
        assert_eq!(m.data.positions.len(), surface + ring.len());
        for (k, &v) in ring.iter().enumerate() {
            let top = m.data.positions[v as usize];
            let bot = m.data.positions[surface + k];
            assert_eq!(top[0], bot[0]);
            assert_eq!(top[2], bot[2]);
            assert!(bot[1] < top[1]);
        }
    }

    fn policy_1080() -> LodPolicy {
        LodPolicy::default()
    }

    #[test]
    fn switching_at_the_chosen_distance_moves_geometry_by_under_a_pixel() {
        // The anti-popping guarantee, measured rather than asserted.
        let t = hilly();
        let policy = LodPolicy {
            screen_error_px: 1.0,
            ..LodPolicy::default()
        };
        for c in [(1, 1), (2, 3), (4, 5), (6, 2)] {
            let s = t.sample_chunk(ChunkCoord::new(c.0, c.1)).expect("chunk");
            for e in measure_all_lods(&s, policy.levels) {
                let d = switch_distance(e.error_m, &policy);
                if d <= 0.0 {
                    continue;
                }
                let px = screen_error_px(e.error_m, d, &policy);
                assert!(
                    px <= policy.screen_error_px + 1e-3,
                    "LOD {} pops by {px} px at its switch distance",
                    e.lod
                );
            }
        }
    }

    #[test]
    fn choose_lod_is_monotone_in_distance_and_respects_hysteresis() {
        let t = hilly();
        let s = t.sample_chunk(ChunkCoord::new(2, 2)).expect("chunk");
        let policy = LodPolicy::default();
        let errors = measure_all_lods(&s, policy.levels);
        let mut last = 0u8;
        for i in 0..200 {
            let d = i as f32 * 40.0;
            let lod = choose_lod(&errors, d, &policy, None);
            assert!(lod >= last, "LOD went finer as the camera retreated");
            last = lod;
        }
        // At a switch boundary, hysteresis keeps the previous level.
        let boundary = switch_distance(errors[1].error_m, &policy);
        let just_past = boundary * 1.02;
        assert_eq!(
            choose_lod(&errors, just_past, &policy, Some(0)),
            0,
            "hysteresis did not hold the previous LOD"
        );
        assert!(choose_lod(&errors, boundary * 1.5, &policy, Some(0)) >= 1);
    }

    #[test]
    fn water_appears_only_where_the_ground_is_below_the_line() {
        let t = hilly();
        let s = t.sample_chunk(ChunkCoord::new(2, 2)).expect("chunk");
        assert!(
            build_water_mesh(&s, s.min_y - 1.0).is_none(),
            "dry chunk got water"
        );
        let w = build_water_mesh(&s, s.min_y + 1.0).expect("submerged chunk needs water");
        assert_eq!(w.indices.len(), 6);
        assert!(w
            .positions
            .iter()
            .all(|p| (p[1] - (s.min_y + 1.0)).abs() < 1e-6));
    }

    #[test]
    fn interpolation_matches_the_triangulation_at_the_corners() {
        for (tx, tz, expect) in [
            (0.0, 0.0, 1.0),
            (1.0, 0.0, 2.0),
            (0.0, 1.0, 3.0),
            (1.0, 1.0, 4.0),
        ] {
            let h = interp_triangulated(1.0, 2.0, 3.0, 4.0, tx, tz);
            assert!((h - expect).abs() < 1e-6, "corner ({tx},{tz}) gave {h}");
        }
    }

    #[test]
    fn mesh_bytes_are_reported_for_the_budget() {
        let t = hilly();
        let s = t.sample_chunk(ChunkCoord::new(0, 0)).expect("chunk");
        let m = build_chunk_mesh(&s, 0, &policy_1080());
        assert!(m.bytes() > 0);
        let coarse = build_chunk_mesh(&s, 3, &policy_1080());
        assert!(coarse.bytes() < m.bytes(), "coarser must be cheaper");
    }
}
