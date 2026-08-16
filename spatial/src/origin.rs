//! **The render origin** — how an f64 world reaches an f32 GPU without jitter, and without the
//! renderer's limits leaking back into the authored scene.
//!
//! An f32 has ~24 bits of mantissa. At a coordinate of 100,000 the gap between representable values
//! is about 0.0078; at 1,000,000 it is 0.0625; at 10,000,000 it is 1.0. So a vertex a kilometre from
//! the origin snaps to a grid coarse enough to see, and — worse — the *camera* snaps too, so the
//! quantization changes as you orbit and the whole scene shimmers. This is the "large world jitter"
//! every engine hits.
//!
//! The fix is not to make the scene smaller. It is to subtract a nearby reference point before the
//! cast: `render = (world − origin) as f32`. Near the camera the subtraction happens in f64 (exact
//! for these magnitudes), and the result is a small number f32 represents precisely. Far from the
//! camera the error grows again — but those objects are far away, so the error is far below a pixel.
//!
//! Two invariants this module exists to keep:
//!
//! * **Authored coordinates never change.** Rebasing moves the *reference*, not the scene. Nothing
//!   here writes to a transform. An implementation that walks the scene subtracting an offset from
//!   every authored position has corrupted the document to work around a float type.
//! * **Rebasing is not continuous.** The origin snaps to a coarse grid and only moves when the camera
//!   has left the current cell, so consecutive frames overwhelmingly share a reference. A reference
//!   that follows the camera exactly would change the rounding of every vertex every frame — the
//!   jitter you were trying to remove, reintroduced.

use crate::bounds::Aabb;
use crate::epsilon;
use crate::transform::{Mat4, Vec3};

/// The camera-relative reference point the GPU's f32 coordinates are measured from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderOrigin {
    origin: Vec3,
    /// The rebase cell size. The origin is always a multiple of this on each axis.
    cell: f64,
    /// How many times the origin has moved — a consumer that caches baked f32 geometry compares this
    /// to know its cache is stale.
    epoch: u64,
}

impl Default for RenderOrigin {
    fn default() -> Self {
        Self::new(1024.0)
    }
}

impl RenderOrigin {
    /// A reference at the world origin with the given rebase cell size.
    ///
    /// 1024 m is the default: large enough that ordinary orbiting never crosses a boundary (so the
    /// epoch is stable and caches survive), small enough that f32 relative coordinates inside a cell
    /// keep sub-millimetre resolution — the worst case inside a 1024-unit cell is a spacing of about
    /// 6×10⁻⁵.
    #[must_use]
    pub fn new(cell: f64) -> Self {
        Self {
            origin: [0.0; 3],
            cell: if cell.is_finite() && cell > 0.0 {
                cell
            } else {
                1024.0
            },
            epoch: 0,
        }
    }

    #[must_use]
    pub fn origin(&self) -> Vec3 {
        self.origin
    }

    #[must_use]
    pub fn cell(&self) -> f64 {
        self.cell
    }

    /// Increments whenever [`Self::follow`] actually moved the reference.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Move the reference toward `camera` if it has left the current cell. Returns whether it moved.
    ///
    /// Snapping to a grid rather than tracking the camera continuously is the whole point: within a
    /// cell every frame shares a reference, so nothing re-quantizes and nothing shimmers.
    pub fn follow(&mut self, camera: Vec3) -> bool {
        if !epsilon::all_finite(&camera) {
            return false;
        }
        let snapped = [
            (camera[0] / self.cell).round() * self.cell,
            (camera[1] / self.cell).round() * self.cell,
            (camera[2] / self.cell).round() * self.cell,
        ];
        if snapped == self.origin {
            return false;
        }
        self.origin = snapped;
        self.epoch += 1;
        true
    }

    /// World (f64, authoritative) → render (f32, camera-relative). The only sanctioned narrowing.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)] // the narrowing IS the operation; it is safe post-subtraction
    pub fn to_render(&self, world: Vec3) -> [f32; 3] {
        [
            (world[0] - self.origin[0]) as f32,
            (world[1] - self.origin[1]) as f32,
            (world[2] - self.origin[2]) as f32,
        ]
    }

    /// Render (f32, camera-relative) → world (f64). Exact: the origin is an f64 and the addition
    /// re-widens before it is applied.
    #[must_use]
    pub fn to_world(&self, render: [f32; 3]) -> Vec3 {
        [
            f64::from(render[0]) + self.origin[0],
            f64::from(render[1]) + self.origin[1],
            f64::from(render[2]) + self.origin[2],
        ]
    }

    /// A world-space matrix rebased for the GPU: the rotation/scale part is unchanged, only the
    /// translation column is shifted. Returned as f32 columns, ready to upload.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn to_render_matrix(&self, world: Mat4) -> [[f32; 4]; 4] {
        let mut out = [[0.0f32; 4]; 4];
        for c in 0..4 {
            for r in 0..4 {
                let value = if c == 3 && r < 3 {
                    world[c][r] - self.origin[r]
                } else {
                    world[c][r]
                };
                out[c][r] = value as f32;
            }
        }
        out
    }

    /// Bounds rebased for the GPU.
    #[must_use]
    pub fn to_render_bounds(&self, world: &Aabb) -> ([f32; 3], [f32; 3]) {
        (self.to_render(world.min), self.to_render(world.max))
    }

    /// The worst-case position error, in world units, that the f32 cast introduces for a point this
    /// far from the reference. Surfaced by the diagnostics overlay so "is this jitter or is this a
    /// bug" is a question with an answer.
    #[must_use]
    pub fn quantization_at(&self, world: Vec3) -> f64 {
        let d = (0..3)
            .map(|i| (world[i] - self.origin[i]).abs())
            .fold(0.0, f64::max);
        if d == 0.0 {
            return 0.0;
        }
        // f32 has 24 mantissa bits; spacing at magnitude d is 2^(exp(d)) * 2^-23.
        let exp = d.log2().floor();
        exp.exp2() * f64::from(f32::EPSILON)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epsilon::approx_eq;

    #[test]
    fn rebasing_recovers_precision_that_a_direct_cast_destroys() {
        // The claim, measured. Two objects a millimetre apart, ten million units from the origin.
        let a = [10_000_000.000, 0.0, 0.0];
        let b = [10_000_000.001, 0.0, 0.0];

        // Direct cast (what the renderer did before): the distinction is gone.
        #[allow(clippy::cast_possible_truncation)]
        let (na, nb) = (a[0] as f32, b[0] as f32);
        assert_eq!(na, nb, "a direct f32 cast collapses them — this is the bug");

        // Camera-relative: the same two points, referenced near the camera, stay distinct.
        let mut origin = RenderOrigin::new(1024.0);
        origin.follow(a);
        let (ra, rb) = (origin.to_render(a), origin.to_render(b));
        assert_ne!(ra[0], rb[0], "camera-relative keeps them apart");
        // The recovered separation is the millimetre, resolved to f32's spacing at the RELATIVE
        // magnitude (a few hundred units from the reference), not at 1e7. That is the whole gain:
        // ~6e-5 of quantization instead of 1.0.
        let recovered = f64::from(rb[0] - ra[0]);
        assert!(
            (recovered - 0.001).abs() < 1.0e-4,
            "and keeps the millimetre (got {recovered})"
        );
        assert!(
            origin.quantization_at(b) < 1.0e-3,
            "reported quantization ({}) is below the distinction being preserved",
            origin.quantization_at(b)
        );
    }

    #[test]
    fn authored_coordinates_are_never_mutated_by_a_rebase() {
        let world = [123_456.75, -9_876.5, 42.0];
        let mut origin = RenderOrigin::new(1024.0);
        origin.follow([123_000.0, 0.0, 0.0]);
        let round_trip = origin.to_world(origin.to_render(world));
        // A rebase is a lens, not an edit: the world value comes back (to f32's resolution at the
        // RELATIVE magnitude, which is tiny) and the caller's own copy is untouched.
        for i in 0..3 {
            assert!(
                (round_trip[i] - world[i]).abs() < 1.0e-3,
                "axis {i}: {} vs {}",
                round_trip[i],
                world[i]
            );
        }
        assert_eq!(
            world,
            [123_456.75, -9_876.5, 42.0],
            "the input is unchanged"
        );
    }

    #[test]
    fn the_origin_snaps_to_a_grid_and_holds_still_inside_a_cell() {
        let mut o = RenderOrigin::new(1024.0);
        assert_eq!(o.origin(), [0.0; 3]);
        assert_eq!(o.epoch(), 0);
        // A camera anywhere inside cell 0 (|x| < 512) rounds back to the reference already in use, so
        // there is nothing to do — a reference that tracked the camera continuously would re-quantize
        // every vertex every frame, which is the jitter itself.
        for x in [1.0, 50.0, 300.0, 511.0, -400.0] {
            assert!(!o.follow([x, 0.0, 0.0]), "no rebase at x={x}");
        }
        assert_eq!(o.origin(), [0.0; 3]);
        assert_eq!(o.epoch(), 0, "and the epoch is stable, so caches survive");
        let epoch = o.epoch();
        assert!(
            o.follow([2000.0, 0.0, 0.0]),
            "crossing a cell boundary does rebase"
        );
        assert_eq!(o.origin(), [2048.0, 0.0, 0.0]);
        assert_eq!(o.epoch(), epoch + 1);
    }

    #[test]
    fn matrix_rebase_shifts_only_the_translation_column() {
        use crate::transform::Transform;
        let t = Transform {
            translation: [5000.0, 10.0, -20.0],
            rotation: Transform::from_axis_angle([0.0, 1.0, 0.0], 0.7),
            scale: [2.0, 3.0, 4.0],
        };
        let mut o = RenderOrigin::new(1024.0);
        o.follow([5000.0, 0.0, 0.0]);
        let world = t.to_matrix();
        let render = o.to_render_matrix(world);
        for c in 0..3 {
            for r in 0..4 {
                assert!(
                    (f64::from(render[c][r]) - world[c][r]).abs() < 1.0e-5,
                    "the basis is untouched at [{c}][{r}]"
                );
            }
        }
        let expected = o.to_render([world[3][0], world[3][1], world[3][2]]);
        for r in 0..3 {
            assert!(
                (render[3][r] - expected[r]).abs() < 1.0e-5,
                "translation rebased"
            );
        }
    }

    #[test]
    fn quantization_reporting_matches_reality() {
        let o = RenderOrigin::new(1024.0);
        // At ~1e7 from the reference, f32 spacing is about 1.0 — the diagnostic must say so, because
        // "the viewport looks wrong" and "you are 10,000 km from the render origin" are the same fact.
        let q = o.quantization_at([10_000_000.0, 0.0, 0.0]);
        assert!((0.5..=2.0).contains(&q), "reported spacing {q} at 1e7");
        // Inside a cell it is sub-millimetre.
        assert!(o.quantization_at([500.0, 0.0, 0.0]) < 1.0e-4);
        assert!(approx_eq(o.quantization_at([0.0; 3]), 0.0));
    }

    #[test]
    fn a_non_finite_camera_never_moves_the_reference() {
        let mut o = RenderOrigin::new(1024.0);
        o.follow([4096.0, 0.0, 0.0]);
        let before = o.origin();
        assert!(!o.follow([f64::NAN, 0.0, 0.0]));
        assert_eq!(o.origin(), before);
    }
}
