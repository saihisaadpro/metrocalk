//! Terrain collision — a height field, not a triangle mesh, and it has a level of detail too.
//!
//! A 64 m chunk at 1 m cells is 4 225 heights: 17 kB as a height field, against roughly 200 kB as a
//! triangle mesh with its BVH. The height field is also faster to query (a point lookup is two floors and a
//! bilinear read, with no tree descent) and it can never be non-manifold. There is no case for a tri-mesh
//! terrain collider unless the terrain has caves, which a height field cannot express and which this
//! subsystem does not claim to.
//!
//! ## Physics LOD, and why it is integer
//!
//! Colliders coarsen with distance like everything else, but the level is chosen from an **integer chunk
//! ring distance** rather than from a float camera distance. That is deliberate: collider resolution changes
//! what the simulation computes, so the choice must be bit-deterministic and identical on every machine in a
//! networked or replayed session. A float distance compared against a threshold is not — two machines a
//! rounding error apart would build different colliders and diverge. Rings cannot.
//!
//! Beyond the physics ring there is no collider at all, because nothing is there to collide with; the ring
//! is sized by the recipe rather than by the render distance for the same reason.

use crate::mesh::ChunkSamples;
use crate::recipe::{ChunkCoord, LodPolicy};

/// A height-field collider for one chunk, in the neutral form the physics boundary accepts.
///
/// `heights` is row-major with **rows along +Z and columns along +X**, spanning `size_m` on both axes and
/// centred on [`Self::center`]. Stating the convention here rather than in the physics backend is
/// deliberate: a transposed height field produces a terrain that is subtly wrong in a way that only shows up
/// as objects sinking on slopes.
#[derive(Clone, Debug, PartialEq)]
pub struct ChunkCollider {
    /// Which chunk.
    pub coord: ChunkCoord,
    /// Which LOD the samples were taken at.
    pub lod: u8,
    /// Samples along Z.
    pub rows: u32,
    /// Samples along X.
    pub cols: u32,
    /// `rows * cols` heights in metres.
    pub heights: Vec<f32>,
    /// Chunk edge length in metres (the field spans this on both axes).
    pub size_m: f32,
    /// World position of the field centre, with `y` at the mean height.
    pub center: [f32; 3],
}

impl ChunkCollider {
    /// Heap bytes — what the collider budget accounts.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.heights.len() * 4
    }

    /// Where the collider **body** belongs: the chunk centre in XZ with `y = 0`.
    ///
    /// [`Self::heights`] are absolute world metres, so a body placed at [`Self::center`] (whose `y` is the
    /// chunk's mean height, kept for display) would add that height a second time and float the terrain.
    /// Returning the right translation here rather than documenting it in prose is the difference between a
    /// contract and a hope.
    #[must_use]
    pub fn body_translation(&self) -> [f32; 3] {
        [self.center[0], 0.0, self.center[2]]
    }

    /// Height at a world position by bilinear interpolation, clamped to the field.
    ///
    /// Useful on its own: placing a prop, snapping a character, or answering "what is the ground here?"
    /// without entering the physics world at all.
    #[must_use]
    pub fn height_at(&self, wx: f32, wz: f32) -> f32 {
        let half = self.size_m * 0.5;
        let cell_x = self.size_m / (self.cols - 1).max(1) as f32;
        let cell_z = self.size_m / (self.rows - 1).max(1) as f32;
        let gx = ((wx - (self.center[0] - half)) / cell_x).clamp(0.0, (self.cols - 1) as f32);
        let gz = ((wz - (self.center[2] - half)) / cell_z).clamp(0.0, (self.rows - 1) as f32);
        let ix = gx.floor() as u32;
        let iz = gz.floor() as u32;
        let ix1 = (ix + 1).min(self.cols - 1);
        let iz1 = (iz + 1).min(self.rows - 1);
        let tx = gx - ix as f32;
        let tz = gz - iz as f32;
        let at = |x: u32, z: u32| self.heights[(z * self.cols + x) as usize];
        let top = at(ix, iz) + (at(ix1, iz) - at(ix, iz)) * tx;
        let bot = at(ix, iz1) + (at(ix1, iz1) - at(ix, iz1)) * tx;
        top + (bot - top) * tz
    }
}

/// Build a chunk's collider at `lod` by subsampling its already-evaluated height block.
///
/// Because it subsamples the same block the mesh is built from, the collider and the visual surface agree
/// exactly at every shared sample — the alternative (evaluating the field again at collider resolution) is
/// how "the character stands slightly above/below the ground" bugs get in.
#[must_use]
pub fn chunk_collider(samples: &ChunkSamples, lod: u8) -> ChunkCollider {
    let n = samples.verts as i32 - 1;
    let step = (1i32 << u32::from(lod)).min(n.max(1));
    let side = (n / step) + 1;
    let mut heights = Vec::with_capacity((side * side) as usize);
    let mut sum = 0.0f64;
    for jz in 0..side {
        for jx in 0..side {
            let h = samples.height_at(jx * step, jz * step);
            heights.push(h);
            sum += f64::from(h);
        }
    }
    let mean = (sum / f64::from(side * side)) as f32;
    let c = samples.center_xz();
    ChunkCollider {
        coord: samples.coord,
        lod,
        rows: side as u32,
        cols: side as u32,
        heights,
        size_m: samples.size_m(),
        center: [c[0], mean, c[1]],
    }
}

/// Which collider LOD a chunk should have, given the chunk the physics focus is in. `None` ⇒ no collider.
///
/// Integer rings only — see the module docs on why this must not read a float distance.
#[must_use]
pub fn physics_lod(coord: ChunkCoord, focus: ChunkCoord, policy: &LodPolicy) -> Option<u8> {
    let ring = coord.ring_distance(focus);
    if ring > policy.physics_ring.max(0) {
        return None;
    }
    let step = policy.physics_lod_step.max(1);
    let lod = (ring / step) as u8;
    Some(lod.min(policy.levels.saturating_sub(1)))
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
                amplitude: 20.0,
                wavelength_m: 100.0,
                octaves: 4,
                lacunarity: 2.0,
                gain: 0.5,
                warp_m: 0.0,
                warp_wavelength_m: 1.0,
            },
        ));
        Terrain::compile(r, BTreeMap::new()).expect("compile")
    }

    #[test]
    fn the_collider_matches_the_visual_surface_at_its_samples() {
        let t = hilly();
        let s = t.sample_chunk(ChunkCoord::new(2, 2)).expect("chunk");
        let c = chunk_collider(&s, 0);
        assert_eq!(c.rows, 33);
        assert_eq!(c.cols, 33);
        for jz in 0..33i32 {
            for jx in 0..33i32 {
                let (wx, wz) = s.world_xz(jx, jz);
                let expect = s.height_at(jx, jz);
                let got = c.heights[(jz * 33 + jx) as usize];
                assert_eq!(expect.to_bits(), got.to_bits(), "sample mismatch");
                // And the interpolating query agrees at the sample points.
                assert!((c.height_at(wx, wz) - expect).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn the_convention_is_rows_along_z_columns_along_x() {
        // A transposed height field is the classic silent terrain-collision bug. This pins the layout with a
        // field that is deliberately asymmetric between the axes.
        let mut r = TerrainRecipe {
            world_size_m: 128.0,
            chunk_size_m: 64.0,
            chunk_verts: 9,
            ..TerrainRecipe::default()
        };
        r.layers.push(
            Layer::new(
                "RampZ",
                LayerKind::Cellular {
                    amplitude: 0.0,
                    wavelength_m: 1.0,
                    invert: false,
                },
            )
            .with_blend(crate::recipe::Blend::Add),
        );
        r.strokes.push(crate::recipe::Stroke {
            kind: crate::recipe::StrokeKind::Raise,
            // A bump near the +Z edge, at mid X.
            x: 32.0,
            z: 60.0,
            radius_m: 8.0,
            strength: 30.0,
            hardness: 0.0,
            target_m: 0.0,
        });
        let t = Terrain::compile(r, BTreeMap::new()).expect("compile");
        let s = t.sample_chunk(ChunkCoord::new(0, 0)).expect("chunk");
        let c = chunk_collider(&s, 0);
        let last_row = c.rows - 1;
        let mid_col = c.cols / 2;
        let at = |row: u32, col: u32| c.heights[(row * c.cols + col) as usize];
        assert!(
            at(last_row, mid_col) > at(0, mid_col) + 10.0,
            "the bump is at high Z, so it must be in the LAST row: {} vs {}",
            at(last_row, mid_col),
            at(0, mid_col)
        );
        assert!(
            at(last_row, mid_col) > at(last_row, 0) + 10.0,
            "and at mid X, so it must not be in the first column"
        );
    }

    #[test]
    fn coarser_collider_lods_are_cheaper_and_still_plausible() {
        let t = hilly();
        let s = t.sample_chunk(ChunkCoord::new(1, 3)).expect("chunk");
        let fine = chunk_collider(&s, 0);
        let coarse = chunk_collider(&s, 2);
        assert!(coarse.bytes() * 8 <= fine.bytes());
        // The coarse field still spans the chunk and stays within the fine field's range.
        assert_eq!(coarse.size_m, fine.size_m);
        let (flo, fhi) = fine
            .heights
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
        for h in &coarse.heights {
            assert!(*h >= flo - 1e-3 && *h <= fhi + 1e-3);
        }
    }

    #[test]
    fn physics_lod_is_integer_ringed_and_bounded() {
        let policy = LodPolicy {
            physics_ring: 6,
            physics_lod_step: 3,
            levels: 4,
            ..LodPolicy::default()
        };
        let focus = ChunkCoord::new(10, 10);
        assert_eq!(physics_lod(focus, focus, &policy), Some(0));
        assert_eq!(
            physics_lod(ChunkCoord::new(12, 10), focus, &policy),
            Some(0)
        );
        assert_eq!(
            physics_lod(ChunkCoord::new(13, 10), focus, &policy),
            Some(1)
        );
        assert_eq!(
            physics_lod(ChunkCoord::new(16, 10), focus, &policy),
            Some(2)
        );
        assert_eq!(physics_lod(ChunkCoord::new(17, 10), focus, &policy), None);
        // Diagonals use the same ring metric, so the collider shell is square, not a cross.
        assert_eq!(
            physics_lod(ChunkCoord::new(12, 12), focus, &policy),
            Some(0)
        );
    }

    #[test]
    fn height_query_clamps_outside_the_chunk_instead_of_reading_out_of_bounds() {
        let t = hilly();
        let s = t.sample_chunk(ChunkCoord::new(0, 0)).expect("chunk");
        let c = chunk_collider(&s, 0);
        let h = c.height_at(-1000.0, -1000.0);
        assert_eq!(h, c.heights[0]);
        let h2 = c.height_at(1e6, 1e6);
        assert_eq!(h2, *c.heights.last().unwrap());
    }
}
