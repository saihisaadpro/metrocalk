//! **The tolerance policy.** Every numerical threshold in the scene-interaction subsystem is named
//! here, with the reason it has the value it has. Scattering magic epsilons is how a picker and a
//! gizmo start disagreeing about whether two things are "the same point"; naming them once means a
//! change to the policy is a change in one place, and a reviewer can see what a number is *for*.
//!
//! Two kinds of tolerance live here and they are NOT interchangeable:
//!
//! * **Absolute** — a fixed quantity, correct only near the scene origin at human scale.
//! * **Scale-aware** — a relative quantity, correct at any magnitude. Comparing a coordinate of
//!   10,000,000 against a coordinate of 10,000,000.001 with an absolute 1e-6 epsilon calls them
//!   different (right); comparing two f64 positions a kilometre out with the same epsilon calls
//!   almost-equal values different because the representable spacing there is already ~1e-10.
//!   Prefer [`approx_eq`] / [`approx_eq_vec3`] over hand-rolled `(a - b).abs() < 1e-6`.

/// Below this, a would-be direction vector is treated as having no direction at all.
///
/// Squared length, so callers compare against `length_squared()` and skip a `sqrt`. 1e-24 is
/// (1e-12)², i.e. a vector whose length is at the edge of f64's useful resolution for a normalized
/// quantity — anything shorter is noise, not a direction.
pub const DIRECTION_LEN_SQ: f64 = 1.0e-24;

/// A ray and a plane are treated as parallel when `|dot(normal, dir)|` falls below this.
///
/// This is a **cosine**, so it is an angle threshold: 1e-4 is ~0.0057°. Below it, the intersection
/// parameter `t = dot(n, p - o) / denom` divides by a near-zero and lands anywhere — the classic
/// "the object teleported 40,000 units when I grabbed the handle pointing at the camera" failure.
/// Deliberately far above f64 noise: the goal is to reject *unstable* intersections, not just
/// impossible ones. See [`crate::manip`], which switches to a different drag constraint here rather
/// than returning nothing.
pub const RAY_PLANE_PARALLEL: f64 = 1.0e-4;

/// Two lines (the drag axis and the cursor ray) are treated as parallel above this |cos|.
///
/// 0.9995 is ~1.8°. Inside that cone the closest-approach parameter along the axis is numerically
/// meaningless, so an axis drag must fall back to a screen-space measure instead of producing a
/// huge, sign-unstable delta.
pub const AXIS_RAY_PARALLEL_COS: f64 = 0.9995;

/// The smallest magnitude any scale component may hold after an edit.
///
/// Zero scale makes the object's matrix singular, which makes `inverse()` produce NaN, which then
/// propagates into every descendant's world transform and into the GPU buffer. Clamping the
/// *magnitude* (not the value) preserves an intentional negative/mirrored scale.
pub const MIN_SCALE: f64 = 1.0e-9;

/// Matrix inversion is refused when `|det|` falls below this fraction of the matrix's own natural
/// scale (the product of its three column lengths).
///
/// **Relative, not absolute.** A uniform scale of 1e-6 gives a determinant of 1e-18 while being
/// perfectly well conditioned — its inverse is exactly 1e6 and loses nothing. An absolute threshold
/// rejects that legitimate transform (a millimetre-authored CAD part is routinely at 1e-3, and a
/// micrometre feature at 1e-6) while still accepting a genuinely singular matrix that happens to have
/// large columns. Comparing against the matrix's own scale asks the question that actually matters:
/// are the columns close to linearly dependent?
pub const MIN_DETERMINANT_RATIO: f64 = 1.0e-12;

/// A matrix whose columns are all shorter than this is treated as having collapsed to a point; there
/// is no meaningful direction left to invert.
pub const MIN_MATRIX_SCALE: f64 = 1.0e-150;

/// A quaternion whose squared length strays outside `1 ± this` is renormalized.
///
/// Repeated quaternion products drift off the unit sphere slowly; 1e-9 is tight enough that drift is
/// corrected long before it is visible as a scale artefact in the rotation matrix, and loose enough
/// that we do not renormalize (and so perturb) a quaternion that is already fine.
pub const QUAT_NORM_TOLERANCE: f64 = 1.0e-9;

/// A triangle whose doubled area (the cross-product length) is below this is degenerate and is
/// skipped by the ray test — it has no well-defined normal and its barycentric solve is singular.
pub const DEGENERATE_TRIANGLE_AREA: f64 = 1.0e-20;

/// The minimum ray parameter a hit must have to count as "in front of" the ray origin.
///
/// Not zero: a ray that starts exactly on a surface (a re-cast from a previous hit) would otherwise
/// re-hit that surface at t≈0. Scaled by the ray's own length scale where it matters.
pub const RAY_T_MIN: f64 = 1.0e-9;

/// Relative tolerance for [`approx_eq`] — roughly 12 significant decimal digits, comfortably inside
/// f64's ~15–17 while leaving room for the handful of operations a transform round-trip performs.
pub const RELATIVE: f64 = 1.0e-12;

/// Absolute floor for [`approx_eq`], so values straddling zero still compare sensibly.
pub const ABSOLUTE: f64 = 1.0e-12;

/// Scale-aware equality: equal if within [`ABSOLUTE`], or within [`RELATIVE`] of the larger
/// magnitude. This is the comparison to use on coordinates, because it stays meaningful whether the
/// object is at the origin or ten million units away.
#[must_use]
pub fn approx_eq(a: f64, b: f64) -> bool {
    approx_eq_tol(a, b, RELATIVE, ABSOLUTE)
}

/// [`approx_eq`] with caller-chosen tolerances — for tests that need to state their own budget.
#[must_use]
pub fn approx_eq_tol(a: f64, b: f64, relative: f64, absolute: f64) -> bool {
    if a == b {
        return true; // covers exact equality including both-infinite
    }
    if !a.is_finite() || !b.is_finite() {
        return false; // NaN, or one infinite and the other not
    }
    let diff = (a - b).abs();
    diff <= absolute || diff <= relative * a.abs().max(b.abs())
}

/// Component-wise [`approx_eq`] over a 3-vector.
#[must_use]
pub fn approx_eq_vec3(a: [f64; 3], b: [f64; 3]) -> bool {
    (0..3).all(|i| approx_eq(a[i], b[i]))
}

/// Component-wise [`approx_eq_tol`] over a 3-vector.
#[must_use]
pub fn approx_eq_vec3_tol(a: [f64; 3], b: [f64; 3], relative: f64, absolute: f64) -> bool {
    (0..3).all(|i| approx_eq_tol(a[i], b[i], relative, absolute))
}

/// Whether every component is finite — the gate every value crossing into the scene graph passes.
#[must_use]
pub fn all_finite(v: &[f64]) -> bool {
    v.iter().all(|x| x.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_aware_equality_survives_large_coordinates() {
        // The headline case: two objects a millimetre apart, ten million units from the origin. An
        // absolute 1e-6 epsilon would call these equal at f32 and merely "close" at f64; the point of
        // a RELATIVE comparison is that it still calls them DIFFERENT, because they are.
        assert!(!approx_eq(10_000_000.000, 10_000_000.001));
        // ...while genuine round-trip noise at that magnitude still compares equal.
        assert!(approx_eq(10_000_000.0, 10_000_000.0 + 1.0e-7));
    }

    #[test]
    fn absolute_floor_handles_values_straddling_zero() {
        assert!(approx_eq(0.0, 1.0e-15));
        assert!(!approx_eq(0.0, 1.0e-6));
        assert!(approx_eq(-0.0, 0.0));
    }

    #[test]
    fn nan_and_infinity_are_never_equal_to_anything_finite() {
        assert!(!approx_eq(f64::NAN, f64::NAN));
        assert!(!approx_eq(f64::NAN, 0.0));
        assert!(!approx_eq(f64::INFINITY, 1.0e300));
        // Two identical infinities ARE equal — `a == b` short-circuits before the finite check.
        assert!(approx_eq(f64::INFINITY, f64::INFINITY));
        assert!(!all_finite(&[1.0, f64::NAN, 3.0]));
    }

    #[test]
    fn the_parallel_thresholds_are_angles_not_noise_floors() {
        // Documented intent: these reject UNSTABLE geometry, so they must be far above f64 noise.
        // Expressed as ANGLES, which is what makes the numbers reviewable — "1e-4" says nothing,
        // "0.0057 degrees" says whether the threshold is sane.
        let plane_angle = RAY_PLANE_PARALLEL.asin().to_degrees();
        assert!(
            (0.001..0.5).contains(&plane_angle),
            "the ray/plane rejection cone is {plane_angle}° — far above f64 noise, far below usable"
        );
        // ~1.8 degrees of half-cone for the axis fallback.
        let angle = AXIS_RAY_PARALLEL_COS.acos().to_degrees();
        assert!(
            (1.0..4.0).contains(&angle),
            "axis fallback cone is {angle}°"
        );
    }
}
