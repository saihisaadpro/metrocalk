//! **Rays and the primitives they hit.** The narrow phase of picking, and the geometry the gizmo's
//! drag constraints are built from.
//!
//! Everything here is f64 and allocation-free. These functions run tens of thousands of times per
//! click and once per pointer-move for hover, so an allocation inside them is a frame spike; they are
//! written to be called from a tight BVH traversal loop.
//!
//! Every intersection reports the ray parameter `t` where `point = origin + direction · t`, with the
//! direction normalized — so `t` **is** the distance from the ray origin, and comparing two hits'
//! `t` is comparing their depth. That property is what makes "select the nearest visible thing"
//! a sort rather than a heuristic.

use crate::bounds::Aabb;
use crate::epsilon;
use crate::transform::{unv, v, Mat4, Vec3};
use glam::DVec3;

/// A ray with a **normalized** direction, so its parameter is a distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    /// Unit length. [`Ray::new`] enforces this.
    pub direction: Vec3,
}

impl Ray {
    /// Build a ray, normalizing the direction. A zero/non-finite direction becomes `-Z`, which is a
    /// deterministic ray that simply hits nothing useful, rather than a NaN that makes every
    /// downstream comparison false and every hit test silently return "no hit".
    #[must_use]
    pub fn new(origin: Vec3, direction: Vec3) -> Self {
        let d = v(direction);
        let d = if d.is_finite() && d.length_squared() > epsilon::DIRECTION_LEN_SQ {
            d.normalize()
        } else {
            -DVec3::Z
        };
        let origin = if epsilon::all_finite(&origin) {
            origin
        } else {
            [0.0; 3]
        };
        Self {
            origin,
            direction: unv(d),
        }
    }

    /// The point at parameter `t`.
    #[must_use]
    pub fn at(&self, t: f64) -> Vec3 {
        unv(v(self.origin) + v(self.direction) * t)
    }

    /// The parameter of the point on the ray closest to `p` (may be negative — behind the origin).
    #[must_use]
    pub fn project_parameter(&self, p: Vec3) -> f64 {
        (v(p) - v(self.origin)).dot(v(self.direction))
    }

    /// The perpendicular distance from `p` to the ray's infinite line.
    #[must_use]
    pub fn distance_to_point(&self, p: Vec3) -> f64 {
        let to = v(p) - v(self.origin);
        let d = v(self.direction);
        (to - d * to.dot(d)).length()
    }

    /// The ray transformed into another space by `matrix`.
    ///
    /// This is how a world ray reaches a mesh's own coordinates: transforming ONE ray into object
    /// space costs two matrix-vector products, while transforming a million vertices into world space
    /// costs a million. The returned `scale` is how much the direction's length changed, so a
    /// caller can convert an object-space `t` back to a world distance — without it, a hit on a
    /// 0.001-scaled CAD part reports a distance a thousand times too large and loses every depth sort.
    #[must_use]
    pub fn transformed(&self, matrix: Mat4) -> Option<(Self, f64)> {
        let mm = crate::transform::m(matrix);
        if !mm.is_finite() {
            return None;
        }
        let origin = mm.transform_point3(v(self.origin));
        let dir = mm.transform_vector3(v(self.direction));
        let len = dir.length();
        if !origin.is_finite() || !dir.is_finite() || len < 1.0e-30 {
            return None;
        }
        Some((
            Self {
                origin: unv(origin),
                direction: unv(dir / len),
            },
            len,
        ))
    }
}

/// A slab test against an AABB. Returns `(t_enter, t_exit)` clipped to `t >= 0`, or `None`.
///
/// Uses the branchless min/max formulation whose infinities cancel: a ray parallel to a slab produces
/// `±inf` bounds that the subsequent `min`/`max` discard correctly. The naive "if direction is zero,
/// special-case it" version is where NaNs get in, because `0 * inf` is NaN and one NaN comparison
/// silently turns the whole test false.
#[must_use]
pub fn ray_aabb(ray: &Ray, aabb: &Aabb, t_max: f64) -> Option<(f64, f64)> {
    if aabb.is_empty() {
        return None;
    }
    let (o, d) = (ray.origin, ray.direction);
    let mut t0 = 0.0f64;
    let mut t1 = t_max;
    for i in 0..3 {
        let inv = 1.0 / d[i]; // ±inf when the ray is parallel to this slab — intended
        let mut near = (aabb.min[i] - o[i]) * inv;
        let mut far = (aabb.max[i] - o[i]) * inv;
        if near > far {
            std::mem::swap(&mut near, &mut far);
        }
        // NaN (origin exactly on a face with a zero direction component) must not widen the interval.
        if near.is_nan() || far.is_nan() {
            continue;
        }
        t0 = t0.max(near);
        t1 = t1.min(far);
        if t0 > t1 {
            return None;
        }
    }
    Some((t0, t1))
}

/// Whether the ray reaches the box at all within `t_max` — **the BVH's node test, deliberately
/// conservative.**
///
/// Each axis is padded by a few ULPs of that axis's own magnitude. A broad phase must never lose a
/// candidate: rejecting one is a silently missed selection, while accepting a spurious one costs only
/// a narrow-phase test that then rejects it honestly. The padding matters in practice because a ray
/// that grazes a box face or passes exactly through a shared vertex is not rare — coplanar faces and
/// welded vertices are what CAD geometry is made of, and an exact test drops those hits to rounding.
#[must_use]
pub fn ray_aabb_hit(ray: &Ray, aabb: &Aabb, t_max: f64) -> bool {
    if aabb.is_empty() {
        return false;
    }
    let (o, d) = (ray.origin, ray.direction);
    let mut t0 = 0.0f64;
    let mut t1 = t_max;
    for i in 0..3 {
        // Relative to the coordinate's own magnitude, so the pad stays meaningful both at the origin
        // and ten kilometres out.
        let pad = 1.0e-9 * (1.0 + aabb.min[i].abs() + aabb.max[i].abs());
        let inv = 1.0 / d[i];
        let mut near = (aabb.min[i] - pad - o[i]) * inv;
        let mut far = (aabb.max[i] + pad - o[i]) * inv;
        if near > far {
            std::mem::swap(&mut near, &mut far);
        }
        if near.is_nan() || far.is_nan() {
            continue;
        }
        t0 = t0.max(near);
        t1 = t1.min(far);
        if t0 > t1 {
            return false;
        }
    }
    true
}

/// A triangle hit: the ray parameter and the barycentric coordinates of the hit point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TriangleHit {
    pub t: f64,
    /// Barycentric weight of vertex `b` (often called `u`).
    pub u: f64,
    /// Barycentric weight of vertex `c` (often called `v`). Vertex `a`'s weight is `1 - u - v`.
    pub v: f64,
    /// `true` when the ray struck the triangle from behind its winding normal.
    pub back_face: bool,
}

/// Möller–Trumbore ray/triangle intersection, **double-sided**.
///
/// Double-sided on purpose: an editor must let a user select a wall from inside a room and a CAD
/// surface whose winding an importer got backwards. Which side was hit is *reported* rather than used
/// to reject, so a caller with a reason to prefer front faces still can.
///
/// Degenerate triangles (zero area — common in imported meshes) are rejected rather than producing a
/// division by a near-zero determinant, which yields a hit at an arbitrary distance.
#[must_use]
pub fn ray_triangle(ray: &Ray, a: Vec3, b: Vec3, c: Vec3, t_max: f64) -> Option<TriangleHit> {
    let (av, bv, cv) = (v(a), v(b), v(c));
    let e1 = bv - av;
    let e2 = cv - av;
    // A zero-area triangle has no normal and no well-posed barycentric solve.
    if e1.cross(e2).length_squared() < epsilon::DEGENERATE_TRIANGLE_AREA {
        return None;
    }
    let d = v(ray.direction);
    let p = d.cross(e2);
    let det = e1.dot(p);
    // **Relative** parallel rejection. `det = e1 · (d × e2)`, and with `|d| = 1` its magnitude is at
    // most `|e1|·|e2|` — so comparing against that product asks "is this ray nearly in the triangle's
    // plane?", which is the actual question. An absolute threshold gets it wrong in both directions:
    // it rejects legitimate hits on millimetre-scale geometry, and it *accepts* a near-parallel hit
    // on a large triangle where `1/det` explodes and `t` comes back as a meaningless huge number —
    // a phantom hit tens of kilometres away that outranks every real one in the depth sort.
    let magnitude = e1.length() * e2.length();
    if det.abs() < 1.0e-12 * magnitude || det.abs() < 1.0e-300 {
        return None;
    }
    let inv_det = 1.0 / det;
    let tvec = v(ray.origin) - av;
    let u = tvec.dot(p) * inv_det;
    if !(-1.0e-12..=1.0 + 1.0e-12).contains(&u) {
        return None;
    }
    let qvec = tvec.cross(e1);
    let vv = d.dot(qvec) * inv_det;
    if vv < -1.0e-12 || u + vv > 1.0 + 1.0e-12 {
        return None;
    }
    let t = e2.dot(qvec) * inv_det;
    if !(epsilon::RAY_T_MIN..=t_max).contains(&t) || !t.is_finite() {
        return None;
    }
    // **Verify the answer.** A ray that grazes the triangle's plane can pass the barycentric bounds
    // through catastrophic cancellation while `t` comes back meaningless — in one measured case a
    // "hit" 4.5×10¹⁰ units away on a triangle 20 units across. Barycentric coordinates alone cannot
    // catch this, because they are computed with the same exploded `1/det`.
    //
    // Reconstructing the point and requiring it to lie in the triangle's own bounding box costs a
    // handful of operations and makes the function self-consistent whatever `t_max` the caller
    // passes. Without it the phantom survives to the depth sort, where being enormously far away
    // does not disqualify it — it just loses to nothing, so a click on empty space selects it.
    let point = v(ray.origin) + d * t;
    let bb = Aabb::from_points([a, b, c]);
    let tolerance = 1.0e-9 * bb.max_extent().max(1.0);
    for i in 0..3 {
        if point[i] < bb.min[i] - tolerance || point[i] > bb.max[i] + tolerance {
            return None;
        }
    }
    Some(TriangleHit {
        t,
        u,
        v: vv,
        back_face: det < 0.0,
    })
}

/// The geometric normal of a triangle (unnormalized winding normal, normalized on return).
#[must_use]
pub fn triangle_normal(a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let n = (v(b) - v(a)).cross(v(c) - v(a));
    if n.length_squared() < epsilon::DEGENERATE_TRIANGLE_AREA {
        [0.0, 1.0, 0.0]
    } else {
        unv(n.normalize())
    }
}

/// Ray/sphere intersection — the proxy shape for point-like objects (lights, cameras, empties,
/// joints) that have no surface to hit but must still be selectable.
#[must_use]
pub fn ray_sphere(ray: &Ray, center: Vec3, radius: f64, t_max: f64) -> Option<f64> {
    let oc = v(ray.origin) - v(center);
    let d = v(ray.direction);
    let b = oc.dot(d);
    let c = oc.length_squared() - radius * radius;
    if c > 0.0 && b > 0.0 {
        return None; // origin outside and pointing away
    }
    let disc = b * b - c;
    if disc < 0.0 {
        return None;
    }
    let sqrt_d = disc.sqrt();
    let near = -b - sqrt_d;
    let far = -b + sqrt_d;
    // Prefer the near root; fall back to the far one when the origin is inside the sphere (the camera
    // is inside the object — selecting it is still the right answer).
    let t = if near >= epsilon::RAY_T_MIN {
        near
    } else {
        far
    };
    (t >= epsilon::RAY_T_MIN && t <= t_max).then_some(t)
}

/// Ray/plane intersection.
///
/// Returns `None` when the ray is (near-)parallel — the gizmo's degeneracy case. Deliberately does
/// **not** reject a negative `t`: a drag plane behind the pointer is still the right plane, and
/// rejecting it is what makes a handle freeze when the user drags past the camera's own plane.
#[must_use]
pub fn ray_plane(ray: &Ray, point_on_plane: Vec3, normal: Vec3) -> Option<(Vec3, f64)> {
    let n = v(normal);
    if n.length_squared() < epsilon::DIRECTION_LEN_SQ {
        return None;
    }
    let n = n.normalize();
    let d = v(ray.direction);
    let denom = n.dot(d);
    if denom.abs() < epsilon::RAY_PLANE_PARALLEL {
        return None;
    }
    let t = n.dot(v(point_on_plane) - v(ray.origin)) / denom;
    if !t.is_finite() {
        return None;
    }
    Some((ray.at(t), t))
}

/// The closest points between a ray and an infinite line, as `(line_parameter, ray_parameter)`.
///
/// `None` when the two are within [`epsilon::AXIS_RAY_PARALLEL_COS`] of parallel: the solution there
/// is a division by a vanishing determinant, and its sign flips with numerical noise — which is
/// exactly the "the object shot off to infinity when I grabbed the handle pointing at me" failure.
/// Returning `None` instead of a number lets the caller pick a different constraint (see
/// [`crate::manip`]), which is the only correct response.
#[must_use]
pub fn closest_ray_line(ray: &Ray, line_point: Vec3, line_dir: Vec3) -> Option<(f64, f64)> {
    let u = v(line_dir);
    if u.length_squared() < epsilon::DIRECTION_LEN_SQ {
        return None;
    }
    let u = u.normalize();
    let d = v(ray.direction);
    let cos = u.dot(d).abs();
    if cos >= epsilon::AXIS_RAY_PARALLEL_COS {
        return None;
    }
    // Minimize |w + s·u − t·d|² where w = line_point − ray.origin. Setting both partials to zero:
    //   w·u + s − t·b = 0        (∂/∂s)
    //   w·d + s·b − t = 0        (∂/∂t)
    // with b = u·d. Both directions are unit length, so the determinant is 1 − b².
    let w = v(line_point) - v(ray.origin);
    let b = u.dot(d);
    let denom = 1.0 - b * b;
    if denom.abs() < 1.0e-12 {
        return None;
    }
    let t = (w.dot(d) - b * w.dot(u)) / denom; // along the ray
    let s = t.mul_add(b, -w.dot(u)); // along the line
    (s.is_finite() && t.is_finite()).then_some((s, t))
}

/// The shortest distance from a ray to a finite segment — the hit test for a line/wire/edge, and for
/// a gizmo axis handle.
#[must_use]
pub fn ray_segment_distance(ray: &Ray, a: Vec3, b: Vec3) -> f64 {
    let seg = v(b) - v(a);
    let len = seg.length();
    if len < 1.0e-12 {
        return ray.distance_to_point(a);
    }
    let dir = seg / len;
    let s = closest_ray_line(ray, a, unv(dir)).map_or_else(
        || {
            // Parallel: every point on the segment is equidistant from the line, so measure from its
            // midpoint's projection rather than giving up.
            ray.project_parameter(unv(v(a) + dir * (len * 0.5)))
        },
        |(s, _)| s,
    );
    let clamped = s.clamp(0.0, len);
    ray.distance_to_point(unv(v(a) + dir * clamped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epsilon::approx_eq;

    fn ray(o: Vec3, d: Vec3) -> Ray {
        Ray::new(o, d)
    }

    #[test]
    fn a_normalized_direction_makes_t_a_distance() {
        let r = ray([0.0; 3], [0.0, 0.0, -7.0]); // deliberately unnormalized input
        assert!(approx_eq(
            (r.direction[0].powi(2) + r.direction[1].powi(2) + r.direction[2].powi(2)).sqrt(),
            1.0
        ));
        assert!(crate::epsilon::approx_eq_vec3(r.at(5.0), [0.0, 0.0, -5.0]));
        assert!(approx_eq(r.project_parameter([0.0, 0.0, -5.0]), 5.0));
    }

    #[test]
    fn a_zero_direction_becomes_a_deterministic_ray_not_a_nan() {
        let r = ray([1.0, 2.0, 3.0], [0.0; 3]);
        assert!(epsilon::all_finite(&r.direction));
        let bad = ray([f64::NAN, 0.0, 0.0], [f64::NAN; 3]);
        assert!(epsilon::all_finite(&bad.origin) && epsilon::all_finite(&bad.direction));
    }

    #[test]
    fn aabb_slab_test_handles_axis_parallel_rays_without_nan() {
        let box_ = Aabb::new([-1.0; 3], [1.0; 3]);
        // Straight down the X axis, exactly on the Y=0/Z=0 centreline: two of the three slabs have a
        // zero direction component, which is where the naive formulation produces 0*inf = NaN.
        let r = ray([-10.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        let (t0, t1) = ray_aabb(&r, &box_, f64::INFINITY).expect("hits");
        assert!(approx_eq(t0, 9.0) && approx_eq(t1, 11.0));

        // Parallel and OUTSIDE the slab must miss.
        let miss = ray([-10.0, 5.0, 0.0], [1.0, 0.0, 0.0]);
        assert!(ray_aabb(&miss, &box_, f64::INFINITY).is_none());

        // Grazing exactly along a face.
        let graze = ray([-10.0, 1.0, 0.0], [1.0, 0.0, 0.0]);
        assert!(ray_aabb(&graze, &box_, f64::INFINITY).is_some());

        // Origin inside: t_enter is clamped to 0, so "the camera is inside the box" still hits.
        let inside = ray([0.0; 3], [0.0, 1.0, 0.0]);
        let (t0, _) = ray_aabb(&inside, &box_, f64::INFINITY).expect("hits from inside");
        assert!(approx_eq(t0, 0.0));

        // Behind the ray entirely.
        let behind = ray([10.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        assert!(ray_aabb(&behind, &box_, f64::INFINITY).is_none());
        assert!(ray_aabb(&r, &Aabb::EMPTY, f64::INFINITY).is_none());
    }

    #[test]
    fn triangle_hits_are_double_sided_and_report_which_side() {
        let (a, b, c) = ([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]);
        let front = ray([0.4, 0.4, 5.0], [0.0, 0.0, -1.0]);
        let h = ray_triangle(&front, a, b, c, f64::INFINITY).expect("front hit");
        assert!(approx_eq(h.t, 5.0));
        assert!(
            approx_eq(h.u, 0.2) && approx_eq(h.v, 0.2),
            "barycentrics: {h:?}"
        );
        // The barycentric weights reconstruct the hit point — the property that makes UV and
        // vertex-attribute interpolation at the hit possible at all.
        let w = 1.0 - h.u - h.v;
        for i in 0..3 {
            let recon = w * a[i] + h.u * b[i] + h.v * c[i];
            assert!(approx_eq(recon, front.at(h.t)[i]));
        }

        // From behind: still a hit (you can select a wall from inside a room), flagged as back-facing.
        let back = ray([0.4, 0.4, -5.0], [0.0, 0.0, 1.0]);
        let hb = ray_triangle(&back, a, b, c, f64::INFINITY).expect("back hit");
        assert!(
            hb.back_face != h.back_face,
            "the two sides are distinguished"
        );

        // Outside the triangle, on its plane's far side.
        let miss = ray([1.9, 1.9, 5.0], [0.0, 0.0, -1.0]);
        assert!(ray_triangle(&miss, a, b, c, f64::INFINITY).is_none());
        // Beyond t_max.
        assert!(ray_triangle(&front, a, b, c, 4.0).is_none());
    }

    #[test]
    fn degenerate_triangles_are_rejected_rather_than_hit_at_an_arbitrary_distance() {
        let r = ray([0.0, 0.0, 5.0], [0.0, 0.0, -1.0]);
        // Zero area: all three vertices collinear. Imported meshes are full of these.
        assert!(ray_triangle(
            &r,
            [0.0; 3],
            [1.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            f64::INFINITY
        )
        .is_none());
        // All three coincident.
        assert!(ray_triangle(&r, [0.0; 3], [0.0; 3], [0.0; 3], f64::INFINITY).is_none());
        // A sliver that is thin but genuinely non-degenerate must still be hittable.
        let sliver = ray_triangle(
            &r,
            [-1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0e-7, 0.0],
            f64::INFINITY,
        );
        assert!(
            sliver.is_some(),
            "a thin-but-real triangle is still selectable"
        );
    }

    #[test]
    fn a_grazing_ray_cannot_produce_a_phantom_hit_far_from_the_triangle() {
        // The failure this guards: a ray almost in the triangle's plane divides by a near-zero
        // determinant. The barycentric coordinates can still land inside [0,1] through cancellation
        // while `t` comes back as a meaningless huge number — a "hit" tens of kilometres away on a
        // triangle a few units across, which then wins nothing and loses to nothing, so a click on
        // empty space selects it.
        let a = [0.0, 0.0, 0.0];
        let b = [10.0, 0.0, 0.0];
        let c = [0.0, 10.0, 0.0];
        // Start just above the plane and travel almost parallel to it.
        for slope in [1.0e-7, 1.0e-9, 1.0e-11, 0.0] {
            let r = ray([5.0, 2.0, 1.0e-6], [1.0, 0.3, -slope]);
            if let Some(hit) = ray_triangle(&r, a, b, c, f64::INFINITY) {
                let p = r.at(hit.t);
                assert!(
                    p[0] >= -1.0e-6 && p[0] <= 10.000_001 && p[1] >= -1.0e-6 && p[1] <= 10.000_001,
                    "slope {slope}: reported hit at {p:?} (t = {}) is not on the triangle",
                    hit.t
                );
            }
        }
        // And an honest, well-conditioned hit still works.
        let straight = ray([2.0, 2.0, 5.0], [0.0, 0.0, -1.0]);
        let hit = ray_triangle(&straight, a, b, c, f64::INFINITY).expect("a real hit survives");
        assert!(approx_eq(hit.t, 5.0));
    }

    #[test]
    fn sphere_proxy_selects_point_objects_from_inside_and_out() {
        let r = ray([0.0, 0.0, 10.0], [0.0, 0.0, -1.0]);
        let t = ray_sphere(&r, [0.0; 3], 1.0, f64::INFINITY).expect("hit");
        assert!(approx_eq(t, 9.0), "near root");
        // Origin inside the sphere: the far root, so the object under a camera sitting inside it is
        // still selectable rather than mysteriously unclickable.
        let inside = ray([0.0; 3], [0.0, 0.0, -1.0]);
        assert!(approx_eq(
            ray_sphere(&inside, [0.0; 3], 1.0, f64::INFINITY).unwrap(),
            1.0
        ));
        // Pointing away.
        let away = ray([0.0, 0.0, 10.0], [0.0, 0.0, 1.0]);
        assert!(ray_sphere(&away, [0.0; 3], 1.0, f64::INFINITY).is_none());
        assert!(ray_sphere(&r, [5.0, 0.0, 0.0], 1.0, f64::INFINITY).is_none());
    }

    #[test]
    fn plane_intersection_refuses_near_parallel_instead_of_exploding() {
        let plane_point = [0.0; 3];
        let normal = [0.0, 1.0, 0.0];
        let straight = ray([0.0, 5.0, 0.0], [0.0, -1.0, 0.0]);
        let (p, t) = ray_plane(&straight, plane_point, normal).expect("hit");
        assert!(approx_eq(t, 5.0) && approx_eq(p[1], 0.0));

        // Exactly parallel.
        let parallel = ray([0.0, 5.0, 0.0], [1.0, 0.0, 0.0]);
        assert!(ray_plane(&parallel, plane_point, normal).is_none());
        // Near-parallel: 0.001° off. A naive implementation returns a hit ~286,000 units away here.
        let angle = 0.001_f64.to_radians();
        let grazing = ray([0.0, 5.0, 0.0], [angle.cos(), -angle.sin(), 0.0]);
        assert!(
            ray_plane(&grazing, plane_point, normal).is_none(),
            "near-parallel must be refused, not answered with a wild number"
        );

        // A plane BEHIND the ray still intersects — a drag plane does not stop existing because the
        // pointer moved past it, and rejecting negative t is what makes a handle freeze mid-drag.
        let behind = ray([0.0, 5.0, 0.0], [0.0, 1.0, 0.0]);
        let (_, t) = ray_plane(&behind, plane_point, normal).expect("still solvable");
        assert!(t < 0.0, "reported as behind (t = {t}), not discarded");
    }

    #[test]
    fn closest_ray_line_refuses_the_degenerate_cone() {
        // The axis is well presented: a clean solve.
        let r = ray([0.0, 0.0, 10.0], [0.0, 0.0, -1.0]);
        let (s, _) = closest_ray_line(&r, [0.0; 3], [1.0, 0.0, 0.0]).expect("solvable");
        assert!(approx_eq(s, 0.0));
        let r2 = ray([3.0, 0.0, 10.0], [0.0, 0.0, -1.0]);
        let (s2, _) = closest_ray_line(&r2, [0.0; 3], [1.0, 0.0, 0.0]).expect("solvable");
        assert!(
            approx_eq(s2, 3.0),
            "the cursor 3 units along X reads as s = 3"
        );

        // A case with a non-zero axis/ray dot product, which is where a sign slip hides: a 45° ray
        // that passes exactly through (10, 0, 0) must report BOTH parameters landing on that point.
        let diag = ray([0.0, 0.0, 10.0], [1.0, 0.0, -1.0]);
        let (s3, t3) = closest_ray_line(&diag, [0.0; 3], [1.0, 0.0, 0.0]).expect("solvable");
        assert!(approx_eq(s3, 10.0), "line parameter is 10 (got {s3})");
        let on_ray = diag.at(t3);
        assert!(
            crate::epsilon::approx_eq_vec3_tol(on_ray, [10.0, 0.0, 0.0], 1e-9, 1e-9),
            "both parameters name the same point (got {on_ray:?})"
        );

        // The axis points almost exactly at the camera: refused.
        let head_on = ray([0.0, 0.0, 10.0], [0.0, 0.0, -1.0]);
        assert!(
            closest_ray_line(&head_on, [0.0; 3], [0.0, 0.0, 1.0]).is_none(),
            "an axis pointing at the eye has no stable parameter"
        );
        // And 1° off-axis is still refused (inside the documented ~1.8° cone).
        let one_deg = 1.0_f64.to_radians();
        let nearly = ray([0.0, 0.0, 10.0], [one_deg.sin(), 0.0, -one_deg.cos()]);
        assert!(closest_ray_line(&nearly, [0.0; 3], [0.0, 0.0, 1.0]).is_none());
        // 10° off-axis is fine.
        let ten_deg = 10.0_f64.to_radians();
        let ok = ray([0.0, 0.0, 10.0], [ten_deg.sin(), 0.0, -ten_deg.cos()]);
        assert!(closest_ray_line(&ok, [0.0; 3], [0.0, 0.0, 1.0]).is_some());
    }

    #[test]
    fn segment_distance_is_clamped_to_the_segment() {
        let r = ray([0.0, 1.0, 10.0], [0.0, 0.0, -1.0]);
        // The ray passes 1 unit above the segment's interior.
        assert!(approx_eq(
            ray_segment_distance(&r, [-5.0, 0.0, 0.0], [5.0, 0.0, 0.0]),
            1.0
        ));
        // Past the end: the distance is measured to the ENDPOINT, not to the infinite line.
        let far = ray([100.0, 0.0, 10.0], [0.0, 0.0, -1.0]);
        assert!(approx_eq(
            ray_segment_distance(&far, [-5.0, 0.0, 0.0], [5.0, 0.0, 0.0]),
            95.0
        ));
        // A zero-length segment degrades to a point distance rather than dividing by zero.
        assert!(approx_eq(ray_segment_distance(&r, [0.0; 3], [0.0; 3]), 1.0));
    }

    #[test]
    fn transforming_a_ray_into_object_space_reports_the_scale_needed_to_get_back() {
        use crate::transform::{mat_inverse, Transform};
        // A CAD part at 0.001 scale: an object-space t must be convertible back to a world distance,
        // or every depth comparison between a millimetre part and a metre part is wrong by 1000×.
        let t = Transform {
            translation: [10.0, 0.0, 0.0],
            scale: [0.001, 0.001, 0.001],
            ..Transform::IDENTITY
        };
        let inv = mat_inverse(t.to_matrix()).expect("invertible");
        let world_ray = ray([10.0, 0.0, 5.0], [0.0, 0.0, -1.0]);
        let (local_ray, scale) = world_ray.transformed(inv).expect("transformable");
        // The object-space hit at local z = 0 is at local t = 5000; divided by the scale that is 5
        // world units — the true distance.
        let local_t = 5.0 / 0.001;
        assert!(approx_eq(local_t / scale, 5.0), "world distance recovered");
        let local_point = local_ray.at(local_t);
        assert!(
            approx_eq(local_point[2], 0.0),
            "and the local hit point is where expected"
        );
        assert!(world_ray.transformed([[f64::NAN; 4]; 4]).is_none());
    }

    #[test]
    fn intersections_stay_exact_far_from_the_origin() {
        // Picking a 1 mm feature ten million units out: the hit parameter must still resolve the
        // millimetre. This is the whole argument for f64 rays, stated as a test.
        let base = 10_000_000.0;
        let r = ray([base, 0.0, 100.0], [0.0, 0.0, -1.0]);
        let box_ = Aabb::new(
            [base - 0.0005, -0.0005, -0.0005],
            [base + 0.0005, 0.0005, 0.0005],
        );
        let (t0, t1) = ray_aabb(&r, &box_, f64::INFINITY).expect("hits the millimetre box");
        assert!(
            approx_eq(t0, 99.9995) && approx_eq(t1, 100.0005),
            "t = ({t0}, {t1})"
        );
        assert!(
            t1 - t0 > 0.0009,
            "the millimetre thickness survives ({})",
            t1 - t0
        );
    }
}
