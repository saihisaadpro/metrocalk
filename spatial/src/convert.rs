//! **Importer adapters.** The one place a foreign coordinate convention is turned into the engine's,
//! so no format's assumptions leak into the scene graph.
//!
//! Every interchange format makes different choices. glTF is Y-up, right-handed, metres. USD is
//! usually Y-up but carries an explicit `upAxis` that is often Z. STEP and most CAD is Z-up and
//! millimetres. URDF is Z-up, metres, and states rotation as roll-pitch-yaw. FBX is a coin toss.
//!
//! The failure mode of handling this per importer is not that models arrive rotated — that is
//! obvious and gets fixed. It is that they arrive rotated *inconsistently*, so "up" becomes a
//! property of which file an object came from, and every downstream assumption (gravity, ground
//! plane, camera framing, the Y axis of the gizmo) is right for some objects and wrong for others.
//!
//! An importer's contract: build a [`AxisConvention`] and a [`UnitScale`] describing what the *file*
//! says, then push every transform through [`AxisConvention::to_engine`]. Nothing downstream needs
//! to know the format existed.

use crate::transform::{mat_mul, Mat4, Quat, Transform, Vec3};

/// A source coordinate convention, described by where its axes point in engine space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxisConvention {
    /// Already the engine's convention: right-handed, +Y up, −Z forward. A no-op.
    YUpRightHanded,
    /// Z is up and Y is forward (CAD, USD with `upAxis = "Z"`, URDF, most STEP). Converted by
    /// rotating −90° about X, which maps source +Z to engine +Y and source +Y to engine −Z.
    ZUpRightHanded,
    /// Y up, but left-handed (+Z points *into* the screen): DirectX-lineage assets and some FBX
    /// exports. Converted by negating Z, which also flips winding — reported so the importer can
    /// reverse its index order rather than shipping inside-out normals.
    YUpLeftHanded,
}

impl AxisConvention {
    /// The 4×4 that maps a point in this convention into engine space.
    #[must_use]
    pub fn to_engine_matrix(self) -> Mat4 {
        match self {
            Self::YUpRightHanded => Transform::IDENTITY.to_matrix(),
            // −90° about X: (x, y, z) ↦ (x, z, −y). Source +Z (up) becomes engine +Y (up).
            Self::ZUpRightHanded => [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 0.0, -1.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            // Mirror Z. This is a reflection, so it inverts winding.
            Self::YUpLeftHanded => [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, -1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }

    /// Whether this conversion reverses triangle winding. An importer that ignores this ships meshes
    /// whose normals point inward: they light incorrectly and back-face culling removes the wrong
    /// half.
    #[must_use]
    pub fn flips_winding(self) -> bool {
        matches!(self, Self::YUpLeftHanded)
    }

    /// Convert a point.
    #[must_use]
    pub fn point_to_engine(self, p: Vec3) -> Vec3 {
        match self {
            Self::YUpRightHanded => p,
            Self::ZUpRightHanded => [p[0], p[2], -p[1]],
            Self::YUpLeftHanded => [p[0], p[1], -p[2]],
        }
    }

    /// Convert a full transform: `engine = C · source · C⁻¹`, the similarity transform that
    /// re-expresses the *same* placement in the engine's basis.
    ///
    /// The common bug is applying `C · source` instead — that converts the position correctly and
    /// leaves the rotation expressed in the source basis, so an imported assembly's parts are in the
    /// right places but individually mis-rotated. It looks like a modelling error rather than an
    /// importer one.
    #[must_use]
    pub fn to_engine(self, source: &Transform) -> Transform {
        if matches!(self, Self::YUpRightHanded) {
            return *source;
        }
        let c = self.to_engine_matrix();
        // These conventions are their own inverse up to sign; compute it properly rather than
        // assuming, so adding a convention later cannot silently break the round trip.
        let c_inv = crate::transform::mat_inverse(c).unwrap_or(c);
        Transform::decompose(mat_mul(mat_mul(c, source.to_matrix()), c_inv)).transform
    }

    /// Convert a rotation on its own.
    #[must_use]
    pub fn quat_to_engine(self, q: Quat) -> Quat {
        self.to_engine(&Transform {
            rotation: q,
            ..Transform::IDENTITY
        })
        .rotation
    }
}

/// The source file's linear unit, as a factor onto engine metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnitScale(f64);

impl UnitScale {
    pub const METRES: Self = Self(1.0);
    pub const CENTIMETRES: Self = Self(0.01);
    pub const MILLIMETRES: Self = Self(0.001);
    pub const INCHES: Self = Self(0.0254);
    pub const FEET: Self = Self(0.3048);

    /// An arbitrary factor (a USD `metersPerUnit`, a STEP length-unit conversion).
    #[must_use]
    pub fn from_metres_per_unit(factor: f64) -> Self {
        Self(if factor.is_finite() && factor > 0.0 {
            factor
        } else {
            1.0
        })
    }

    #[must_use]
    pub fn factor(self) -> f64 {
        self.0
    }

    /// Scale a position into engine metres. **Positions only** — applying this to a transform's
    /// scale as well is the double-conversion that makes a millimetre assembly arrive a thousand
    /// times too small *and* a thousand times too small again.
    #[must_use]
    pub fn point_to_engine(self, p: Vec3) -> Vec3 {
        [p[0] * self.0, p[1] * self.0, p[2] * self.0]
    }

    /// Apply to a transform: translation is converted, rotation is untouched, and scale is untouched
    /// (it is a ratio, and ratios are unitless).
    #[must_use]
    pub fn to_engine(self, t: &Transform) -> Transform {
        Transform {
            translation: self.point_to_engine(t.translation),
            rotation: t.rotation,
            scale: t.scale,
        }
    }
}

/// A complete importer adapter: convention plus units, applied in the right order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImportAdapter {
    pub axes: AxisConvention,
    pub units: UnitScale,
}

impl ImportAdapter {
    #[must_use]
    pub fn new(axes: AxisConvention, units: UnitScale) -> Self {
        Self { axes, units }
    }

    /// Units first, then axes — scaling is uniform so the two commute for positions, but stating the
    /// order keeps a future non-uniform unit conversion from becoming order-dependent by accident.
    #[must_use]
    pub fn to_engine(&self, source: &Transform) -> Transform {
        self.axes.to_engine(&self.units.to_engine(source))
    }

    #[must_use]
    pub fn point_to_engine(&self, p: Vec3) -> Vec3 {
        self.axes.point_to_engine(self.units.point_to_engine(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epsilon::{approx_eq, approx_eq_vec3_tol};
    use crate::transform::quat_approx_eq;

    #[test]
    fn z_up_becomes_y_up() {
        let c = AxisConvention::ZUpRightHanded;
        // The source's up axis lands on the engine's up axis.
        assert!(approx_eq_vec3_tol(
            c.point_to_engine([0.0, 0.0, 1.0]),
            [0.0, 1.0, 0.0],
            1e-12,
            1e-12
        ));
        // …and the source's forward lands on the engine's forward (−Z).
        assert!(approx_eq_vec3_tol(
            c.point_to_engine([0.0, 1.0, 0.0]),
            [0.0, 0.0, -1.0],
            1e-12,
            1e-12
        ));
        // Right is shared.
        assert!(approx_eq_vec3_tol(
            c.point_to_engine([1.0, 0.0, 0.0]),
            [1.0, 0.0, 0.0],
            1e-12,
            1e-12
        ));
        assert!(!c.flips_winding(), "a rotation preserves winding");
    }

    #[test]
    fn a_z_up_rotation_is_converted_as_a_similarity_not_a_pre_multiply() {
        // The bug this test exists for: converting only the position leaves the ROTATION in the
        // source basis. Take a Z-up part rotated 90° about ITS OWN up axis (+Z). In engine space that
        // must read as 90° about +Y.
        let source = Transform {
            translation: [1.0, 2.0, 3.0],
            rotation: Transform::from_axis_angle([0.0, 0.0, 1.0], std::f64::consts::FRAC_PI_2),
            scale: [1.0; 3],
        };
        let engine = AxisConvention::ZUpRightHanded.to_engine(&source);
        assert!(
            approx_eq_vec3_tol(engine.translation, [1.0, 3.0, -2.0], 1e-12, 1e-12),
            "position converted: {:?}",
            engine.translation
        );
        let expected = Transform::from_axis_angle([0.0, 1.0, 0.0], std::f64::consts::FRAC_PI_2);
        assert!(
            quat_approx_eq(engine.rotation, expected),
            "a spin about the source's up is a spin about the engine's up (got {:?})",
            engine.rotation
        );

        // The property that actually matters: converting a point through the transform, and
        // converting the transform then applying it, must agree.
        let local_point = [0.5, -0.25, 2.0];
        let a = AxisConvention::ZUpRightHanded.point_to_engine(source.transform_point(local_point));
        let b = engine.transform_point(AxisConvention::ZUpRightHanded.point_to_engine(local_point));
        assert!(approx_eq_vec3_tol(a, b, 1e-9, 1e-9), "{a:?} vs {b:?}");
    }

    #[test]
    fn a_left_handed_source_reports_that_it_flips_winding() {
        let c = AxisConvention::YUpLeftHanded;
        assert!(
            c.flips_winding(),
            "an importer must reverse its indices for this"
        );
        assert_eq!(c.point_to_engine([1.0, 2.0, 3.0]), [1.0, 2.0, -3.0]);
    }

    #[test]
    fn identity_convention_is_exactly_a_no_op() {
        let t = Transform {
            translation: [7.0, -3.0, 0.5],
            rotation: Transform::from_axis_angle([0.3, 0.5, 0.8], 1.1),
            scale: [2.0, 0.5, 1.5],
        };
        assert_eq!(AxisConvention::YUpRightHanded.to_engine(&t), t);
    }

    #[test]
    fn millimetre_cad_arrives_at_the_right_size_exactly_once() {
        // A 1000 mm part is 1 m. Scale is a RATIO and must not be converted — doing so is the
        // classic double conversion that lands an assembly a million times too small.
        let source = Transform {
            translation: [1000.0, 0.0, 0.0],
            scale: [2.0, 2.0, 2.0],
            ..Transform::IDENTITY
        };
        let out = UnitScale::MILLIMETRES.to_engine(&source);
        assert!(approx_eq(out.translation[0], 1.0), "1000 mm is 1 m");
        assert_eq!(out.scale, [2.0; 3], "scale is unitless and untouched");
        assert!(approx_eq(UnitScale::INCHES.factor(), 0.0254));
        assert!(
            approx_eq(UnitScale::from_metres_per_unit(0.0).factor(), 1.0),
            "invalid ⇒ 1"
        );
    }

    #[test]
    fn a_full_cad_adapter_places_a_part_correctly() {
        // The real case: Z-up millimetres (STEP), a part 500 mm along the source's up axis.
        let adapter = ImportAdapter::new(AxisConvention::ZUpRightHanded, UnitScale::MILLIMETRES);
        let p = adapter.point_to_engine([0.0, 0.0, 500.0]);
        assert!(
            approx_eq_vec3_tol(p, [0.0, 0.5, 0.0], 1e-12, 1e-12),
            "half a metre UP in engine space, got {p:?}"
        );
    }
}
