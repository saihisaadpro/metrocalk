//! A world-aligned scalar grid — the storage for everything that cannot be evaluated pointwise: imported
//! heightmaps, erosion bakes, flow-accumulation/moisture fields, and the sampled masks the material and
//! scatter stages share.
//!
//! The grid is *world-aligned*, not chunk-aligned: it stores its own origin and cell size in metres and is
//! always sampled by world position. That is the property that keeps the field seamless — two chunks
//! sampling the same baked grid at a shared edge hit the identical four texels with the identical weights,
//! so they cannot disagree, and a chunk never needs to know which grid tile it fell into.

use serde::{Deserialize, Serialize};

/// A regular grid of `f32` samples over a rectangle of world space, sampled bilinearly with clamped edges.
///
/// Sample `(ix, iz)` sits at world `(origin.x + ix·cell, origin.z + iz·cell)` — samples are on cell
/// *corners*, not centres, so a grid of `n+1` samples exactly spans `n` cells with no half-cell offset to
/// forget. Edge clamping (rather than wrapping or zeroing) is deliberate: a terrain that fades to zero at
/// the edge of an imported heightmap produces a visible cliff, while clamping extends the border plausibly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeightGrid {
    /// World XZ position of sample `(0, 0)`, in metres.
    pub origin: [f32; 2],
    /// Distance between adjacent samples, in metres.
    pub cell: f32,
    /// Samples along X.
    pub width: u32,
    /// Samples along Z.
    pub height: u32,
    /// `width * height` samples, row-major (X fastest).
    pub data: Vec<f32>,
}

impl HeightGrid {
    /// A zero-filled grid. `width`/`height` are sample counts and are clamped to at least 1 so that
    /// sampling is always well defined.
    #[must_use]
    pub fn new(origin: [f32; 2], cell: f32, width: u32, height: u32) -> Self {
        let (width, height) = (width.max(1), height.max(1));
        Self {
            origin,
            cell: if cell > 0.0 { cell } else { 1.0 },
            width,
            height,
            data: vec![0.0; (width as usize) * (height as usize)],
        }
    }

    /// Fill a grid from a function of *world* position — the bridge from a pointwise field to a bake.
    pub fn from_fn(
        origin: [f32; 2],
        cell: f32,
        width: u32,
        height: u32,
        mut f: impl FnMut(f32, f32) -> f32,
    ) -> Self {
        let mut g = Self::new(origin, cell, width, height);
        for iz in 0..g.height {
            let wz = g.origin[1] + iz as f32 * g.cell;
            for ix in 0..g.width {
                let wx = g.origin[0] + ix as f32 * g.cell;
                let idx = (iz * g.width + ix) as usize;
                g.data[idx] = f(wx, wz);
            }
        }
        g
    }

    /// Number of samples.
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Always false — a grid has at least one sample by construction. Present because clippy asks for it
    /// next to `len`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Heap bytes this grid occupies — the number the memory budget accounts.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.data.len() * std::mem::size_of::<f32>()
    }

    /// Sample at integer indices, clamped to the grid.
    #[must_use]
    pub fn get(&self, ix: i64, iz: i64) -> f32 {
        let x = ix.clamp(0, i64::from(self.width) - 1) as usize;
        let z = iz.clamp(0, i64::from(self.height) - 1) as usize;
        self.data[z * self.width as usize + x]
    }

    /// Overwrite a sample. Out-of-range indices are ignored (callers working with halos rely on this).
    pub fn set(&mut self, ix: i64, iz: i64, v: f32) {
        if ix < 0 || iz < 0 || ix >= i64::from(self.width) || iz >= i64::from(self.height) {
            return;
        }
        let idx = iz as usize * self.width as usize + ix as usize;
        self.data[idx] = v;
    }

    /// Add to a sample, clamped-ignored like [`Self::set`].
    pub fn add(&mut self, ix: i64, iz: i64, v: f32) {
        if ix < 0 || iz < 0 || ix >= i64::from(self.width) || iz >= i64::from(self.height) {
            return;
        }
        let idx = iz as usize * self.width as usize + ix as usize;
        self.data[idx] += v;
    }

    /// Bilinear sample at a world position, with clamped edges.
    #[must_use]
    pub fn sample(&self, wx: f32, wz: f32) -> f32 {
        let gx = (wx - self.origin[0]) / self.cell;
        let gz = (wz - self.origin[1]) / self.cell;
        let fx = gx.floor();
        let fz = gz.floor();
        let tx = gx - fx;
        let tz = gz - fz;
        let (ix, iz) = (fx as i64, fz as i64);
        let h00 = self.get(ix, iz);
        let h10 = self.get(ix + 1, iz);
        let h01 = self.get(ix, iz + 1);
        let h11 = self.get(ix + 1, iz + 1);
        let a = h00 + (h10 - h00) * tx;
        let b = h01 + (h11 - h01) * tx;
        a + (b - a) * tz
    }

    /// World-space gradient `(∂h/∂x, ∂h/∂z)` by central difference over one cell.
    #[must_use]
    pub fn gradient(&self, wx: f32, wz: f32) -> [f32; 2] {
        let e = self.cell;
        [
            (self.sample(wx + e, wz) - self.sample(wx - e, wz)) / (2.0 * e),
            (self.sample(wx, wz + e) - self.sample(wx, wz - e)) / (2.0 * e),
        ]
    }

    /// Smallest and largest sample.
    #[must_use]
    pub fn min_max(&self) -> (f32, f32) {
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for &v in &self.data {
            lo = lo.min(v);
            hi = hi.max(v);
        }
        (lo, hi)
    }

    /// Copy a rectangle of samples from `src` into `self`, both addressed in *their own* sample indices.
    /// Out-of-range writes are dropped. This is how a tiled bake commits a tile's interior (excluding its
    /// halo) into the global grid.
    pub fn blit_from(
        &mut self,
        src: &Self,
        src_x: i64,
        src_z: i64,
        w: u32,
        h: u32,
        dst_x: i64,
        dst_z: i64,
    ) {
        for jz in 0..i64::from(h) {
            for jx in 0..i64::from(w) {
                let v = src.get(src_x + jx, src_z + jz);
                self.set(dst_x + jx, dst_z + jz, v);
            }
        }
    }

    /// Resample onto a new cell size covering the same world rectangle (bilinear). Used to trade quality
    /// for memory when a bake would blow the budget — the honest degradation path.
    #[must_use]
    pub fn resampled(&self, cell: f32) -> Self {
        let span_x = (self.width - 1) as f32 * self.cell;
        let span_z = (self.height - 1) as f32 * self.cell;
        let cell = if cell > 0.0 { cell } else { self.cell };
        let w = (span_x / cell).round().max(1.0) as u32 + 1;
        let h = (span_z / cell).round().max(1.0) as u32 + 1;
        Self::from_fn(self.origin, cell, w, h, |x, z| self.sample(x, z))
    }

    /// Deterministic content bytes (dimensions + LE sample bits) for content addressing a bake, so an
    /// unchanged recipe re-resolves the same handle instead of re-baking.
    #[must_use]
    pub fn content_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(self.data.len() * 4 + 24);
        b.extend_from_slice(&self.width.to_le_bytes());
        b.extend_from_slice(&self.height.to_le_bytes());
        b.extend_from_slice(&self.cell.to_le_bytes());
        b.extend_from_slice(&self.origin[0].to_le_bytes());
        b.extend_from_slice(&self.origin[1].to_le_bytes());
        for v in &self.data {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp() -> HeightGrid {
        // h = x (in metres), so every interpolation has an exactly predictable answer.
        HeightGrid::from_fn([0.0, 0.0], 2.0, 5, 5, |x, _| x)
    }

    #[test]
    fn samples_on_corners_exactly_and_interpolates_between() {
        let g = ramp();
        assert_eq!(g.sample(0.0, 0.0), 0.0);
        assert_eq!(g.sample(4.0, 4.0), 4.0);
        assert_eq!(g.sample(3.0, 1.0), 3.0, "linear field reproduced exactly");
    }

    #[test]
    fn edges_clamp_rather_than_fall_to_zero() {
        let g = ramp();
        assert_eq!(g.sample(-50.0, 0.0), 0.0);
        assert_eq!(g.sample(999.0, 0.0), 8.0, "clamped to the last sample");
    }

    #[test]
    fn gradient_recovers_the_slope() {
        let g = ramp();
        let [dx, dz] = g.gradient(4.0, 4.0);
        assert!((dx - 1.0).abs() < 1e-5, "dh/dx should be 1: {dx}");
        assert!(dz.abs() < 1e-5, "dh/dz should be 0: {dz}");
    }

    #[test]
    fn blit_copies_an_interior_region() {
        let src = HeightGrid::from_fn([0.0, 0.0], 1.0, 8, 8, |x, z| x + z * 10.0);
        let mut dst = HeightGrid::new([0.0, 0.0], 1.0, 8, 8);
        dst.blit_from(&src, 2, 2, 3, 3, 5, 5);
        assert_eq!(dst.get(5, 5), src.get(2, 2));
        assert_eq!(dst.get(7, 7), src.get(4, 4));
        assert_eq!(dst.get(0, 0), 0.0, "outside the blit stays untouched");
    }

    #[test]
    fn resample_preserves_a_linear_field_and_the_extent() {
        let g = ramp();
        let c = g.resampled(1.0);
        assert_eq!(c.width, 9);
        assert_eq!(c.sample(5.0, 3.0), 5.0);
    }

    #[test]
    fn content_bytes_change_with_content_and_not_otherwise() {
        let a = ramp();
        let b = ramp();
        assert_eq!(a.content_bytes(), b.content_bytes());
        let mut c = ramp();
        c.set(1, 1, 99.0);
        assert_ne!(a.content_bytes(), c.content_bytes());
    }
}
