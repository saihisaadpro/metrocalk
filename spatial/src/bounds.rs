//! Axis-aligned bounds, and the one correct way to move them through a transform.
//!
//! Bounds are load-bearing: they are the picking broad phase, the framing/focus input, the marquee
//! test, the culling test and the BVH's leaf geometry. Because so much depends on them, the failure
//! mode of getting them wrong is diffuse and hard to trace — a slightly-too-small world AABB makes
//! clicks near an object's edge miss, and nothing in the log says so.
//!
//! The rule enforced here: **never re-derive world bounds from vertices.** Transforming a local AABB
//! by a matrix is eight corner transforms and a min/max, regardless of whether the mesh has 12
//! triangles or 12 million.

use crate::epsilon;
use crate::transform::{m, transform_point4, unv, v, Mat4, Vec3};

/// An axis-aligned bounding box. Always `min <= max` component-wise when non-empty; an empty box is
/// represented by `min > max` so that unioning it is a no-op.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Default for Aabb {
    /// The empty box — the identity of `union`, and the only sane starting value for an accumulator.
    fn default() -> Self {
        Self::EMPTY
    }
}

impl Aabb {
    /// The empty box: inverted bounds, so `union` with anything yields that thing.
    pub const EMPTY: Self = Self {
        min: [f64::INFINITY; 3],
        max: [f64::NEG_INFINITY; 3],
    };

    /// A box from two corners, in either order.
    #[must_use]
    pub fn new(a: Vec3, b: Vec3) -> Self {
        Self {
            min: [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])],
            max: [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])],
        }
    }

    /// A zero-volume box at a point — the bounds of a light, a camera, an empty, a vertex.
    #[must_use]
    pub const fn point(p: Vec3) -> Self {
        Self { min: p, max: p }
    }

    /// A box from a centre and half-extents.
    #[must_use]
    pub fn from_center_half(center: Vec3, half: Vec3) -> Self {
        Self {
            min: [
                center[0] - half[0].abs(),
                center[1] - half[1].abs(),
                center[2] - half[2].abs(),
            ],
            max: [
                center[0] + half[0].abs(),
                center[1] + half[1].abs(),
                center[2] + half[2].abs(),
            ],
        }
    }

    /// Bounds enclosing every point, skipping non-finite ones (an importer can hand us a NaN vertex,
    /// and one NaN would otherwise poison the whole box into never intersecting anything).
    #[must_use]
    pub fn from_points(points: impl IntoIterator<Item = Vec3>) -> Self {
        let mut out = Self::EMPTY;
        for p in points {
            if epsilon::all_finite(&p) {
                out.expand_point(p);
            }
        }
        out
    }

    /// Whether this box contains no volume at all (never been expanded).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        (0..3).any(|i| self.min[i] > self.max[i])
    }

    /// Whether every bound is a usable number.
    #[must_use]
    pub fn is_finite(&self) -> bool {
        epsilon::all_finite(&self.min) && epsilon::all_finite(&self.max)
    }

    #[must_use]
    pub fn center(&self) -> Vec3 {
        [
            f64::midpoint(self.min[0], self.max[0]),
            f64::midpoint(self.min[1], self.max[1]),
            f64::midpoint(self.min[2], self.max[2]),
        ]
    }

    /// Full extents (max − min). Zero on an axis for a flat/degenerate box; empty boxes give zeros.
    #[must_use]
    pub fn size(&self) -> Vec3 {
        if self.is_empty() {
            return [0.0; 3];
        }
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }

    #[must_use]
    pub fn half_size(&self) -> Vec3 {
        let s = self.size();
        [s[0] * 0.5, s[1] * 0.5, s[2] * 0.5]
    }

    /// The radius of the enclosing sphere.
    #[must_use]
    pub fn radius(&self) -> f64 {
        let h = self.half_size();
        (h[0] * h[0] + h[1] * h[1] + h[2] * h[2]).sqrt()
    }

    /// The longest axis extent — the size a framing distance should be computed against.
    #[must_use]
    pub fn max_extent(&self) -> f64 {
        let s = self.size();
        s[0].max(s[1]).max(s[2])
    }

    /// Surface area — the cost metric a SAH BVH build minimizes.
    #[must_use]
    pub fn surface_area(&self) -> f64 {
        if self.is_empty() {
            return 0.0;
        }
        let s = self.size();
        2.0 * s[0].mul_add(s[1], s[1].mul_add(s[2], s[2] * s[0]))
    }

    /// The index of the longest axis (0/1/2) — the BVH split axis.
    #[must_use]
    pub fn longest_axis(&self) -> usize {
        let s = self.size();
        if s[0] >= s[1] && s[0] >= s[2] {
            0
        } else if s[1] >= s[2] {
            1
        } else {
            2
        }
    }

    pub fn expand_point(&mut self, p: Vec3) {
        for i in 0..3 {
            self.min[i] = self.min[i].min(p[i]);
            self.max[i] = self.max[i].max(p[i]);
        }
    }

    pub fn expand(&mut self, other: &Self) {
        if other.is_empty() {
            return;
        }
        for i in 0..3 {
            self.min[i] = self.min[i].min(other.min[i]);
            self.max[i] = self.max[i].max(other.max[i]);
        }
    }

    #[must_use]
    pub fn union(mut self, other: &Self) -> Self {
        self.expand(other);
        self
    }

    /// Grow by `amount` on every axis — the picking tolerance an infinitely thin object needs to be
    /// reachable at all.
    #[must_use]
    pub fn expanded_by(&self, amount: f64) -> Self {
        if self.is_empty() {
            return *self;
        }
        Self {
            min: [
                self.min[0] - amount,
                self.min[1] - amount,
                self.min[2] - amount,
            ],
            max: [
                self.max[0] + amount,
                self.max[1] + amount,
                self.max[2] + amount,
            ],
        }
    }

    #[must_use]
    pub fn contains_point(&self, p: Vec3) -> bool {
        (0..3).all(|i| p[i] >= self.min[i] && p[i] <= self.max[i])
    }

    #[must_use]
    pub fn intersects(&self, other: &Self) -> bool {
        if self.is_empty() || other.is_empty() {
            return false;
        }
        (0..3).all(|i| self.min[i] <= other.max[i] && self.max[i] >= other.min[i])
    }

    /// The eight corners, in a fixed order.
    #[must_use]
    pub fn corners(&self) -> [Vec3; 8] {
        let (lo, hi) = (self.min, self.max);
        [
            [lo[0], lo[1], lo[2]],
            [hi[0], lo[1], lo[2]],
            [lo[0], hi[1], lo[2]],
            [hi[0], hi[1], lo[2]],
            [lo[0], lo[1], hi[2]],
            [hi[0], lo[1], hi[2]],
            [lo[0], hi[1], hi[2]],
            [hi[0], hi[1], hi[2]],
        ]
    }

    /// **The transformed bound.** The AABB of this box after `matrix` is applied.
    ///
    /// Eight corner transforms, not a vertex sweep. The result is the tight AABB of the transformed
    /// *box*, which is a conservative (never too small) bound of the transformed geometry — that
    /// direction matters, because a bound that is too small makes picking miss.
    #[must_use]
    pub fn transformed(&self, matrix: Mat4) -> Self {
        if self.is_empty() {
            return *self;
        }
        let mut out = Self::EMPTY;
        for c in self.corners() {
            let Some(p) = transform_point4(matrix, c) else {
                return Self::EMPTY; // a degenerate/projective matrix has no meaningful world box
            };
            if !epsilon::all_finite(&p) {
                return Self::EMPTY;
            }
            out.expand_point(p);
        }
        out
    }

    /// The closest point inside the box to `p` — used for distance-to-box tests.
    #[must_use]
    pub fn closest_point(&self, p: Vec3) -> Vec3 {
        [
            p[0].clamp(self.min[0], self.max[0]),
            p[1].clamp(self.min[1], self.max[1]),
            p[2].clamp(self.min[2], self.max[2]),
        ]
    }
}

/// An oriented bounding box — a local [`Aabb`] plus the matrix that places it. Kept as a pair rather
/// than baked into a world AABB where the caller needs the *actual* shape (a long thin diagonal part
/// has an enormous world AABB but a small OBB, and treating the AABB as the object is why clicking
/// near such a part selects it from far away).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Obb {
    pub local: Aabb,
    pub matrix: Mat4,
}

impl Obb {
    #[must_use]
    pub fn world_aabb(&self) -> Aabb {
        self.local.transformed(self.matrix)
    }

    /// The eight world-space corners.
    #[must_use]
    pub fn corners(&self) -> [Vec3; 8] {
        let mm = m(self.matrix);
        self.local.corners().map(|c| unv(mm.transform_point3(v(c))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transform::Transform;

    #[test]
    fn empty_bounds_are_the_identity_of_union() {
        let mut e = Aabb::EMPTY;
        assert!(e.is_empty());
        let box_ = Aabb::new([-1.0, -2.0, -3.0], [4.0, 5.0, 6.0]);
        e.expand(&box_);
        assert_eq!(e, box_);
        assert_eq!(box_.union(&Aabb::EMPTY), box_);
    }

    #[test]
    fn a_rotated_box_gets_a_larger_but_never_smaller_world_aabb() {
        // The property that matters for picking: the transformed AABB must ENCLOSE the transformed
        // geometry. A 45° rotation of a unit cube grows the AABB by √2 on the rotated axes.
        let unit = Aabb::new([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0]);
        let t = Transform {
            rotation: Transform::from_axis_angle([0.0, 0.0, 1.0], std::f64::consts::FRAC_PI_4),
            ..Transform::IDENTITY
        };
        let world = unit.transformed(t.to_matrix());
        let root2 = std::f64::consts::SQRT_2;
        assert!((world.max[0] - root2).abs() < 1e-12, "grew to √2 on X");
        assert!((world.max[1] - root2).abs() < 1e-12, "and on Y");
        assert!(
            (world.max[2] - 1.0).abs() < 1e-12,
            "Z is the rotation axis, unchanged"
        );
        // Every original corner must still be inside.
        for c in unit.corners() {
            assert!(world.contains_point(t.transform_point(c)));
        }
    }

    #[test]
    fn transformed_bounds_track_non_uniform_scale_and_translation() {
        let unit = Aabb::new([-1.0; 3], [1.0; 3]);
        let t = Transform {
            translation: [10.0, 0.0, -5.0],
            scale: [3.0, 0.5, 2.0],
            ..Transform::IDENTITY
        };
        let w = unit.transformed(t.to_matrix());
        assert_eq!(w.min, [7.0, -0.5, -7.0]);
        assert_eq!(w.max, [13.0, 0.5, -3.0]);
        assert_eq!(w.center(), [10.0, 0.0, -5.0]);
    }

    #[test]
    fn a_degenerate_matrix_yields_empty_bounds_not_nan() {
        let unit = Aabb::new([-1.0; 3], [1.0; 3]);
        let zero = Transform {
            scale: [0.0; 3],
            ..Transform::IDENTITY
        };
        // A zero-scale matrix collapses to a point, which is still finite and legitimate.
        let collapsed = unit.transformed(zero.to_matrix());
        assert!(collapsed.is_finite());
        assert_eq!(collapsed.size(), [0.0; 3]);
        // A matrix full of NaN must produce EMPTY, never NaN bounds that silently match everything.
        let poisoned = [[f64::NAN; 4]; 4];
        assert!(unit.transformed(poisoned).is_empty());
    }

    #[test]
    fn non_finite_points_are_skipped_rather_than_poisoning_the_box() {
        let b = Aabb::from_points([[0.0; 3], [f64::NAN, 0.0, 0.0], [2.0, 2.0, 2.0]]);
        assert_eq!(b.min, [0.0; 3]);
        assert_eq!(b.max, [2.0; 3]);
    }

    #[test]
    fn surface_area_and_longest_axis_drive_the_bvh_split() {
        let b = Aabb::new([0.0; 3], [1.0, 4.0, 2.0]);
        assert_eq!(b.longest_axis(), 1);
        // 2*(1*4 + 4*2 + 2*1) = 2*14 = 28
        assert!((b.surface_area() - 28.0).abs() < 1e-12);
        assert_eq!(Aabb::EMPTY.surface_area(), 0.0);
    }
}
