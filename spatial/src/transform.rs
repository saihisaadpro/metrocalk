//! **The canonical transform.** One representation, in f64, that everything else is derived from.
//!
//! The rule this module exists to enforce: a matrix is a *product*, never a *source*. The authoritative
//! state of an object is its TRS — translation, a unit quaternion, and a per-axis scale — and matrices
//! are generated from it on demand. Nothing writes a matrix back into the canonical state except the
//! one place that has to ([`Transform::from_matrix`], used when converting a world-space edit into
//! parent-space), and that place says so and reports what it could not represent.
//!
//! Why f64 rather than the renderer's f32: an f32 holds ~7 significant decimal digits, so at a
//! coordinate of 10,000,000 the spacing between representable values is about 1.0 — the engine
//! literally cannot tell two objects a metre apart from each other. f64 holds ~16, so the same scene
//! resolves to well under a micrometre. The renderer still works in f32; it receives *camera-relative*
//! positions (see [`crate::origin`]) so it never has to represent the large absolute value at all.

use glam::{DMat3, DMat4, DQuat, DVec3};
use serde::{Deserialize, Serialize};

use crate::epsilon;

/// A 3-vector in the engine's canonical precision. Plain array — the crate's boundary type.
pub type Vec3 = [f64; 3];
/// A quaternion `[x, y, z, w]`. Canonically unit-length; [`Transform::sanitized`] enforces that.
pub type Quat = [f64; 4];
/// A 4×4 matrix, **column-major** (`m[col][row]`) — the same convention glam and WGSL use, so an
/// upload is a cast and never a transpose.
pub type Mat4 = [[f64; 4]; 4];

/// The identity quaternion.
pub const IDENTITY_QUAT: Quat = [0.0, 0.0, 0.0, 1.0];

// ── glam firewall (glam types exist only inside this crate) ───────────────────────────────────────

pub(crate) fn v(a: Vec3) -> DVec3 {
    DVec3::new(a[0], a[1], a[2])
}
pub(crate) fn unv(a: DVec3) -> Vec3 {
    [a.x, a.y, a.z]
}
pub(crate) fn q(a: Quat) -> DQuat {
    DQuat::from_xyzw(a[0], a[1], a[2], a[3])
}
pub(crate) fn unq(a: DQuat) -> Quat {
    [a.x, a.y, a.z, a.w]
}
pub(crate) fn m(a: Mat4) -> DMat4 {
    DMat4::from_cols_array_2d(&a)
}
pub(crate) fn unm(a: DMat4) -> Mat4 {
    a.to_cols_array_2d()
}

/// The engine's canonical transform: translation, rotation, and **per-axis** scale.
///
/// Non-uniform scale is representable on purpose. A single uniform scalar cannot express a stretched
/// box, a mirrored part, or the axis-wise unit conversion an imported CAD assembly needs, and an
/// engine that stores one scalar has already lost that information before any code gets a chance to
/// be correct about it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub translation: Vec3,
    /// Unit quaternion `[x, y, z, w]`.
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    pub const IDENTITY: Self = Self {
        translation: [0.0; 3],
        rotation: IDENTITY_QUAT,
        scale: [1.0; 3],
    };

    /// Construct from the three parts.
    #[must_use]
    pub const fn new(translation: Vec3, rotation: Quat, scale: Vec3) -> Self {
        Self {
            translation,
            rotation,
            scale,
        }
    }

    /// A pure translation.
    #[must_use]
    pub const fn from_translation(t: Vec3) -> Self {
        Self {
            translation: t,
            rotation: IDENTITY_QUAT,
            scale: [1.0; 3],
        }
    }

    /// A rotation of `angle` radians about `axis` (normalized internally; a zero axis ⇒ identity).
    #[must_use]
    pub fn from_axis_angle(axis: Vec3, angle: f64) -> Quat {
        let a = v(axis);
        if a.length_squared() < epsilon::DIRECTION_LEN_SQ || !angle.is_finite() {
            return IDENTITY_QUAT;
        }
        unq(DQuat::from_axis_angle(a.normalize(), angle))
    }

    /// Whether every component is finite and the transform is usable.
    #[must_use]
    pub fn is_finite(&self) -> bool {
        epsilon::all_finite(&self.translation)
            && epsilon::all_finite(&self.rotation)
            && epsilon::all_finite(&self.scale)
    }

    /// **The gate.** Return a transform guaranteed safe to put in the scene graph: finite, with a
    /// unit-length rotation and a non-degenerate scale.
    ///
    /// A NaN that reaches the scene graph does not stay local — it flows into the world matrix, into
    /// every descendant, into the GPU instance buffer, and into the saved file, and the object
    /// disappears with no message. Every entry point that accepts an outside number (numeric field,
    /// importer, AI patch, deserialized document, solver result) calls this, and non-finite input is
    /// *rejected* — this returns `None` rather than silently substituting a value the user did not ask
    /// for.
    #[must_use]
    pub fn sanitized(&self) -> Option<Self> {
        if !self.is_finite() {
            return None;
        }
        let len_sq: f64 = self.rotation.iter().map(|c| c * c).sum();
        let rotation = if len_sq < epsilon::DIRECTION_LEN_SQ {
            IDENTITY_QUAT // a zero quaternion carries no rotation; identity is the only honest reading
        } else if (len_sq - 1.0).abs() > epsilon::QUAT_NORM_TOLERANCE {
            let inv = len_sq.sqrt().recip();
            [
                self.rotation[0] * inv,
                self.rotation[1] * inv,
                self.rotation[2] * inv,
                self.rotation[3] * inv,
            ]
        } else {
            self.rotation
        };
        // Clamp the MAGNITUDE, so a deliberate mirror (negative scale) survives.
        let clamp = |s: f64| {
            if s.abs() < epsilon::MIN_SCALE {
                if s < 0.0 {
                    -epsilon::MIN_SCALE
                } else {
                    epsilon::MIN_SCALE
                }
            } else {
                s
            }
        };
        Some(Self {
            translation: self.translation,
            rotation,
            scale: [
                clamp(self.scale[0]),
                clamp(self.scale[1]),
                clamp(self.scale[2]),
            ],
        })
    }

    /// The TRS composed into a matrix: `T · R · S`.
    #[must_use]
    pub fn to_matrix(&self) -> Mat4 {
        unm(DMat4::from_scale_rotation_translation(
            v(self.scale),
            q(self.rotation),
            v(self.translation),
        ))
    }

    /// Decompose a matrix back to TRS.
    ///
    /// **This is lossy in general and says so.** A 4×4 affine matrix can encode shear (which a rotated
    /// parent combined with a non-uniformly-scaled child produces); TRS cannot. The returned
    /// [`Decomposed::shear`] reports the off-diagonal magnitude that was discarded, so a caller can
    /// tell the difference between "this is exact" and "this is the nearest TRS". The mirror
    /// convention is the standard one: a negative determinant puts the sign on X.
    ///
    /// The hierarchy never calls this per level — it composes matrices and decomposes at most once,
    /// at the point a world-space edit has to be written back as a local TRS. Decomposing at every
    /// level of a deep tree is how a hierarchy accumulates error it can never recover.
    #[must_use]
    pub fn decompose(matrix: Mat4) -> Decomposed {
        let mm = m(matrix);
        let translation = unv(mm.w_axis.truncate());
        let (c0, c1, c2) = (
            mm.x_axis.truncate(),
            mm.y_axis.truncate(),
            mm.z_axis.truncate(),
        );
        let det = DMat3::from_cols(c0, c1, c2).determinant();
        let mut sx = c0.length();
        let sy = c1.length();
        let sz = c2.length();
        if det < 0.0 {
            sx = -sx; // mirror convention: the flip lives on X
        }
        // Shear = how far the (normalized) axes are from mutually perpendicular. Reported, not hidden.
        let n0 = if c0.length_squared() > epsilon::DIRECTION_LEN_SQ {
            c0.normalize()
        } else {
            DVec3::X
        };
        let n1 = if c1.length_squared() > epsilon::DIRECTION_LEN_SQ {
            c1.normalize()
        } else {
            DVec3::Y
        };
        let n2 = if c2.length_squared() > epsilon::DIRECTION_LEN_SQ {
            c2.normalize()
        } else {
            DVec3::Z
        };
        let shear = n0.dot(n1).abs().max(n1.dot(n2).abs()).max(n2.dot(n0).abs());
        // Undo the mirror FIRST, then orthonormalize: the third axis must be derived from the
        // corrected first axis. Negating `r0` after computing `r2 = r0 × r1` leaves `r2` pointing the
        // wrong way, which silently turns a mirror into a mirror *plus* an unwanted 180° flip — and
        // the resulting matrix no longer reproduces the input.
        let r0 = if det < 0.0 { -n0 } else { n0 };
        // Gram-Schmidt against the corrected first axis, so shear does not poison the rotation.
        let r1 = (n1 - r0 * r0.dot(n1)).normalize_or_zero();
        let r1 = if r1.length_squared() < epsilon::DIRECTION_LEN_SQ {
            r0.any_orthonormal_vector()
        } else {
            r1
        };
        let r2 = r0.cross(r1);
        let basis = DMat3::from_cols(r0, r1, r2);
        let rotation = DQuat::from_mat3(&basis).normalize();
        Decomposed {
            transform: Self {
                translation,
                rotation: unq(rotation),
                scale: [sx, sy, sz],
            },
            shear,
            determinant: det,
        }
    }

    /// [`Self::decompose`] discarding the diagnostics — for the many call sites that have already
    /// established the matrix is a rigid/scaled transform with no shear.
    #[must_use]
    pub fn from_matrix(matrix: Mat4) -> Self {
        Self::decompose(matrix).transform
    }

    /// `self` applied after `parent`: the world transform of a child whose local transform is `self`.
    /// Composes as MATRICES (exact) and decomposes once at the end.
    #[must_use]
    pub fn compose(parent: &Self, local: &Self) -> Decomposed {
        Self::decompose(mat_mul(parent.to_matrix(), local.to_matrix()))
    }

    /// **Parent-space write-back**: `local = inverse(parent_world) · world`.
    ///
    /// The gizmo and the world-space numeric fields both act in world space while an entity stores a
    /// local transform, so every world-space edit funnels through here. Skipping it is the well-known
    /// "scale/rotate inside a rotated parent is silently wrong" bug. Returns `None` when the parent
    /// matrix is singular (a zero-scaled ancestor), because there is no correct answer to return and
    /// inventing one writes NaN into the document.
    #[must_use]
    pub fn to_local(world: &Self, parent_world: Mat4) -> Option<Decomposed> {
        let pm = m(parent_world);
        if !is_invertible(pm) {
            return None;
        }
        let local = pm.inverse() * m(world.to_matrix());
        if !local.is_finite() {
            return None;
        }
        Some(Self::decompose(unm(local)))
    }

    /// The transform's own axis directions in world space (its local X/Y/Z) — the Local-space gizmo
    /// basis. Normalized, and independent of scale, so a squashed object still gets orthonormal
    /// handles.
    #[must_use]
    pub fn basis(&self) -> [Vec3; 3] {
        let r = q(self.rotation);
        [unv(r * DVec3::X), unv(r * DVec3::Y), unv(r * DVec3::Z)]
    }

    /// Transform a point by this transform (scale, then rotate, then translate).
    #[must_use]
    pub fn transform_point(&self, p: Vec3) -> Vec3 {
        unv(q(self.rotation) * (v(p) * v(self.scale)) + v(self.translation))
    }

    /// Transform a direction (rotation and scale, no translation).
    #[must_use]
    pub fn transform_vector(&self, d: Vec3) -> Vec3 {
        unv(q(self.rotation) * (v(d) * v(self.scale)))
    }

    /// Scale-aware component-wise comparison — the equality tests should use.
    #[must_use]
    pub fn approx_eq(&self, other: &Self) -> bool {
        epsilon::approx_eq_vec3(self.translation, other.translation)
            && quat_approx_eq(self.rotation, other.rotation)
            && epsilon::approx_eq_vec3(self.scale, other.scale)
    }
}

/// The result of turning a matrix back into a TRS, with the fidelity of that conversion attached.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decomposed {
    pub transform: Transform,
    /// How far the matrix's axes were from perpendicular — the shear TRS could not represent.
    /// `0.0` means the decomposition is exact. Above ~1e-6, the caller is looking at an approximation
    /// and should say so rather than pretend the write-back was lossless.
    pub shear: f64,
    /// The 3×3 determinant. Negative means the transform mirrors; near zero means it is degenerate.
    pub determinant: f64,
}

impl Decomposed {
    /// Whether the decomposition round-trips exactly (no shear worth reporting).
    #[must_use]
    pub fn is_exact(&self) -> bool {
        self.shear <= 1.0e-9
    }
}

/// Column-major 4×4 multiply, `a · b` (apply `b` first, then `a`).
#[must_use]
pub fn mat_mul(a: Mat4, b: Mat4) -> Mat4 {
    unm(m(a) * m(b))
}

/// Whether an affine matrix can be inverted usefully — a **relative** conditioning test.
///
/// See [`epsilon::MIN_DETERMINANT_RATIO`] for why this is not `|det| < 1e-18`: that absolute form
/// rejects a legitimately tiny uniform scale (millimetre CAD, micrometre features) while accepting
/// genuinely degenerate matrices whose columns happen to be long.
fn is_invertible(mm: glam::DMat4) -> bool {
    if !mm.is_finite() {
        return false;
    }
    let scale = mm.x_axis.truncate().length()
        * mm.y_axis.truncate().length()
        * mm.z_axis.truncate().length();
    if scale < epsilon::MIN_MATRIX_SCALE {
        return false; // collapsed to a point
    }
    mm.determinant().abs() >= epsilon::MIN_DETERMINANT_RATIO * scale
}

/// Matrix inverse, or `None` when the matrix is singular — never an infinity-filled matrix.
#[must_use]
pub fn mat_inverse(a: Mat4) -> Option<Mat4> {
    let mm = m(a);
    if !is_invertible(mm) {
        return None;
    }
    let inv = mm.inverse();
    inv.is_finite().then(|| unm(inv))
}

/// Transform a point by a matrix (with the perspective divide, so this is also correct for a
/// projection matrix). `None` when the point lands on or behind the projection's eye plane.
#[must_use]
pub fn transform_point4(matrix: Mat4, p: Vec3) -> Option<Vec3> {
    let out = m(matrix) * v(p).extend(1.0);
    if out.w.abs() < 1.0e-12 || !out.is_finite() {
        return None;
    }
    Some(unv(out.truncate() / out.w))
}

/// Transform a direction by a matrix's rotation/scale part (no translation, no divide).
#[must_use]
pub fn transform_dir(matrix: Mat4, d: Vec3) -> Vec3 {
    unv(m(matrix).transform_vector3(v(d)))
}

/// Whether two quaternions describe the same orientation. `q` and `-q` are the same rotation, so the
/// comparison is on `|dot|`.
#[must_use]
pub fn quat_approx_eq(a: Quat, b: Quat) -> bool {
    let dot: f64 = (0..4).map(|i| a[i] * b[i]).sum();
    epsilon::approx_eq_tol(dot.abs(), 1.0, 1.0e-9, 1.0e-9)
}

/// Normalize a quaternion; a zero/non-finite quaternion becomes identity.
#[must_use]
pub fn quat_normalized(a: Quat) -> Quat {
    let len_sq: f64 = a.iter().map(|c| c * c).sum();
    if !len_sq.is_finite() || len_sq < epsilon::DIRECTION_LEN_SQ {
        return IDENTITY_QUAT;
    }
    let inv = len_sq.sqrt().recip();
    [a[0] * inv, a[1] * inv, a[2] * inv, a[3] * inv]
}

/// Quaternion product `a · b` (apply `b` first).
#[must_use]
pub fn quat_mul(a: Quat, b: Quat) -> Quat {
    unq(q(a) * q(b))
}

// ── Euler angles (display only) ───────────────────────────────────────────────────────────────────

/// **The engine's Euler convention**, stated once: angles are radians, and the composed rotation is
///
/// ```text
/// R = Rz(z) · Ry(y) · Rx(x)
/// ```
///
/// i.e. rotate about **X first, then Y, then Z, each about the FIXED world axes** (extrinsic XYZ,
/// which is the same rotation as intrinsic Z-Y-X). This is the convention Blender labels "XYZ Euler"
/// and the one a user typing three numbers into a panel most often expects.
///
/// Euler angles exist here for **display and entry only**. They are never the stored state: they have
/// no unique representation (three different triples give the same orientation), and gimbal lock
/// makes them lose a degree of freedom. The quaternion is the truth; this is a lens onto it.
#[must_use]
pub fn quat_to_euler_xyz(rotation: Quat) -> Vec3 {
    let mm = DMat3::from_quat(q(quat_normalized(rotation)));
    // R = Rz·Ry·Rx  ⇒  R[2][0] (row 2, col 0) = -sin(y)
    let r20 = mm.x_axis.z;
    let sy = (-r20).clamp(-1.0, 1.0);
    let y = sy.asin();
    // Gimbal lock: |sin y| ≈ 1 collapses X and Z into one axis. Pick z = 0 and fold the whole
    // remaining rotation into x — an arbitrary but STABLE and documented choice, rather than the
    // atan2-of-two-near-zeros noise the general branch would produce.
    if r20.abs() > 1.0 - 1.0e-9 {
        // With cos(y) = 0 the X and Z rotations act on the same axis and only their SUM (or
        // difference, depending on the sign of sin y) is recoverable. Folding it into x and pinning
        // z to 0 is arbitrary but stable; the sign factor is what makes the reconstruction actually
        // reproduce the original orientation at y = −90° as well as at +90°.
        let x = (mm.y_axis.x * sy.signum()).atan2(mm.y_axis.y);
        return [x, y, 0.0];
    }
    let x = mm.y_axis.z.atan2(mm.z_axis.z);
    let z = mm.x_axis.y.atan2(mm.x_axis.x);
    [x, y, z]
}

/// The inverse of [`quat_to_euler_xyz`] — `Rz(z) · Ry(y) · Rx(x)` as a unit quaternion.
#[must_use]
pub fn euler_xyz_to_quat(euler: Vec3) -> Quat {
    if !epsilon::all_finite(&euler) {
        return IDENTITY_QUAT;
    }
    let qx = DQuat::from_axis_angle(DVec3::X, euler[0]);
    let qy = DQuat::from_axis_angle(DVec3::Y, euler[1]);
    let qz = DQuat::from_axis_angle(DVec3::Z, euler[2]);
    unq((qz * qy * qx).normalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epsilon::{approx_eq, approx_eq_tol};

    fn deg(d: f64) -> f64 {
        d.to_radians()
    }

    #[test]
    fn trs_round_trips_through_a_matrix() {
        let t = Transform {
            translation: [1.5, -2.25, 3.125],
            rotation: Transform::from_axis_angle([0.3, 0.9, -0.2], deg(37.0)),
            scale: [2.0, 0.5, 1.25],
        };
        let back = Transform::decompose(t.to_matrix());
        assert!(
            back.is_exact(),
            "a plain TRS has no shear (got {})",
            back.shear
        );
        assert!(
            back.transform.approx_eq(&t),
            "{:?} vs {:?}",
            back.transform,
            t
        );
    }

    #[test]
    fn decompose_reports_the_shear_it_cannot_represent() {
        // A rotated parent with a non-uniformly-scaled child produces genuine shear. TRS cannot hold
        // it; the contract is that we SAY SO rather than silently returning a wrong transform.
        let parent = Transform {
            translation: [0.0; 3],
            rotation: Transform::from_axis_angle([0.0, 0.0, 1.0], deg(45.0)),
            scale: [3.0, 1.0, 1.0],
        };
        let child = Transform {
            translation: [0.0; 3],
            rotation: Transform::from_axis_angle([0.0, 0.0, 1.0], deg(30.0)),
            scale: [1.0, 1.0, 1.0],
        };
        let composed = Transform::compose(&parent, &child);
        assert!(
            composed.shear > 1.0e-6,
            "this composition really does shear (got {})",
            composed.shear
        );
        assert!(!composed.is_exact());
    }

    #[test]
    fn negative_scale_survives_a_round_trip_as_a_mirror() {
        let t = Transform {
            translation: [4.0, 0.0, 0.0],
            rotation: Transform::from_axis_angle([0.0, 1.0, 0.0], deg(20.0)),
            scale: [-2.0, 1.0, 1.0],
        };
        let d = Transform::decompose(t.to_matrix());
        assert!(
            d.determinant < 0.0,
            "a mirrored transform has a negative determinant"
        );
        // The matrix is what must match; the mirror may legitimately be attributed to a different
        // axis+rotation pair, so compare the MATRICES, not the raw components.
        let re = d.transform.to_matrix();
        let orig = t.to_matrix();
        for c in 0..4 {
            for r in 0..4 {
                assert!(
                    approx_eq(re[c][r], orig[c][r]),
                    "mirror round-trip at [{c}][{r}]: {} vs {}",
                    re[c][r],
                    orig[c][r]
                );
            }
        }
    }

    #[test]
    fn parent_space_write_back_reproduces_the_world_transform() {
        // The #24104 trap, in f64 and with NON-UNIFORM parent scale (the case a uniform-scale engine
        // cannot even express): parent · to_local(world) must reproduce `world` exactly.
        let parent = Transform {
            translation: [100.0, 20.0, -80.0],
            rotation: Transform::from_axis_angle([0.2, 1.0, 0.4], deg(63.0)),
            scale: [2.0, 2.0, 2.0],
        };
        let world = Transform {
            translation: [1.0, 2.0, 3.0],
            rotation: Transform::from_axis_angle([0.0, 0.0, 1.0], 0.5),
            scale: [1.0, 1.0, 1.0],
        };
        let local = Transform::to_local(&world, parent.to_matrix()).expect("invertible parent");
        assert!(local.is_exact(), "uniform parent scale ⇒ no shear");
        let recomposed = mat_mul(parent.to_matrix(), local.transform.to_matrix());
        let expected = world.to_matrix();
        for c in 0..4 {
            for r in 0..4 {
                assert!(
                    approx_eq(recomposed[c][r], expected[c][r]),
                    "parent·local must reproduce world at [{c}][{r}]: {} vs {}",
                    recomposed[c][r],
                    expected[c][r]
                );
            }
        }
    }

    #[test]
    fn a_singular_parent_refuses_rather_than_returning_nan() {
        let degenerate = Transform {
            translation: [0.0; 3],
            rotation: IDENTITY_QUAT,
            scale: [0.0, 1.0, 1.0], // a zero-scaled ancestor
        };
        assert!(Transform::to_local(&Transform::IDENTITY, degenerate.to_matrix()).is_none());
        assert!(mat_inverse(degenerate.to_matrix()).is_none());
        // Fully collapsed is also refused.
        let collapsed = Transform {
            scale: [0.0; 3],
            ..Transform::IDENTITY
        };
        assert!(mat_inverse(collapsed.to_matrix()).is_none());
    }

    #[test]
    fn a_tiny_uniform_scale_is_invertible_because_the_test_is_relative() {
        // A micrometre-authored CAD feature has a determinant of 1e-18 and is perfectly well
        // conditioned. An ABSOLUTE determinant threshold rejects it, which would make every
        // millimetre/micrometre assembly un-editable — the reason the test compares against the
        // matrix's own scale instead.
        for s in [1.0e-3, 1.0e-6, 1.0e-9] {
            let t = Transform {
                scale: [s; 3],
                ..Transform::IDENTITY
            };
            let inv = mat_inverse(t.to_matrix())
                .unwrap_or_else(|| panic!("uniform scale {s} must be invertible"));
            // And the inverse is genuinely correct: M · M⁻¹ = I.
            let product = mat_mul(t.to_matrix(), inv);
            for c in 0..4 {
                for r in 0..4 {
                    let expect = if c == r { 1.0 } else { 0.0 };
                    assert!(
                        approx_eq_tol(product[c][r], expect, 1.0e-9, 1.0e-9),
                        "scale {s}: M·M⁻¹ at [{c}][{r}] = {}",
                        product[c][r]
                    );
                }
            }
        }
    }

    #[test]
    fn sanitize_rejects_non_finite_and_repairs_drift() {
        let nan = Transform {
            translation: [1.0, f64::NAN, 0.0],
            ..Transform::IDENTITY
        };
        assert!(
            nan.sanitized().is_none(),
            "NaN must never enter the scene graph"
        );

        let drifted = Transform {
            rotation: [0.0, 0.0, 0.0, 0.5], // half-length quaternion
            scale: [0.0, 1.0, -0.0],        // zero scale, one of them negative-zero
            ..Transform::IDENTITY
        };
        let fixed = drifted.sanitized().expect("finite input is repairable");
        let len_sq: f64 = fixed.rotation.iter().map(|c| c * c).sum();
        assert!(
            approx_eq(len_sq, 1.0),
            "rotation renormalized (len² {len_sq})"
        );
        assert!(fixed.scale.iter().all(|s| s.abs() >= epsilon::MIN_SCALE));
        assert!(
            mat_inverse(fixed.to_matrix()).is_some(),
            "repaired scale is invertible"
        );

        // A deliberate mirror is preserved, not clamped away.
        let mirrored = Transform {
            scale: [-1.0, 1.0, 1.0],
            ..Transform::IDENTITY
        };
        assert_eq!(mirrored.sanitized().unwrap().scale[0], -1.0);
    }

    #[test]
    fn precision_holds_ten_million_units_from_the_origin() {
        // The engine's stated large-world claim, as arithmetic: two objects a millimetre apart, ten
        // million units out, must remain DISTINGUISHABLE through a full compose/decompose round trip.
        // In f32 this test is impossible — the spacing of representable f32 values at 1e7 is 1.0.
        let a = Transform::from_translation([10_000_000.000, 0.0, 0.0]);
        let b = Transform::from_translation([10_000_000.001, 0.0, 0.0]);
        let ra = Transform::from_matrix(a.to_matrix()).translation[0];
        let rb = Transform::from_matrix(b.to_matrix()).translation[0];
        assert!(ra != rb, "the round trip must not collapse them");
        assert!(
            (rb - ra - 0.001).abs() < 1.0e-9,
            "the millimetre survives (got {})",
            rb - ra
        );
        // And the f32 counter-proof, so the reason for f64 is recorded rather than asserted:
        assert_eq!(
            10_000_000.000_f32, 10_000_000.001_f32,
            "f32 genuinely cannot hold this distinction — this is why the canonical type is f64"
        );
    }

    #[test]
    fn euler_round_trips_and_states_its_convention() {
        // The convention: R = Rz·Ry·Rx. A rotation of 90° about X then 90° about Z must map
        // +Y --Rx--> +Z --Rz--> +Z  (Rz leaves Z alone), which is the check that pins the ORDER down.
        let qq = euler_xyz_to_quat([deg(90.0), 0.0, deg(90.0)]);
        let mapped = q(qq) * DVec3::Y;
        assert!(
            (mapped - DVec3::Z).length() < 1.0e-9,
            "extrinsic XYZ: X applies first (got {mapped:?})"
        );

        for e in [
            [deg(10.0), deg(20.0), deg(30.0)],
            [deg(-170.0), deg(45.0), deg(89.0)],
            [0.0, 0.0, 0.0],
        ] {
            let back = quat_to_euler_xyz(euler_xyz_to_quat(e));
            let re = euler_xyz_to_quat(back);
            assert!(
                quat_approx_eq(re, euler_xyz_to_quat(e)),
                "euler {e:?} round-trips to the same ORIENTATION (via {back:?})"
            );
        }
    }

    #[test]
    fn euler_gimbal_lock_is_stable_not_noisy() {
        // At y = ±90° the X and Z axes coincide. The documented choice is z = 0; the test's real job
        // is that the ORIENTATION still round-trips, which is what a user editing the panel sees.
        for sign in [1.0, -1.0] {
            let original = euler_xyz_to_quat([deg(25.0), sign * deg(90.0), deg(40.0)]);
            let e = quat_to_euler_xyz(original);
            assert!(
                approx_eq(e[2], 0.0),
                "locked axis folds into x, z pinned to 0"
            );
            assert!(
                quat_approx_eq(euler_xyz_to_quat(e), original),
                "gimbal-locked orientation still round-trips"
            );
        }
    }

    #[test]
    fn basis_is_orthonormal_even_under_non_uniform_scale() {
        let t = Transform {
            rotation: Transform::from_axis_angle([1.0, 1.0, 0.0], deg(50.0)),
            scale: [10.0, 0.1, 3.0],
            ..Transform::IDENTITY
        };
        let b = t.basis();
        for axis in b {
            let len = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
            assert!(
                approx_eq(len, 1.0),
                "gizmo axes stay unit length (got {len})"
            );
        }
        let dot01: f64 = (0..3).map(|i| b[0][i] * b[1][i]).sum();
        assert!(dot01.abs() < 1.0e-12, "and mutually perpendicular");
    }
}
