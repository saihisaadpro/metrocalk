//! **The camera and the viewport** — and the one correct conversion from a pointer position to a
//! world-space ray.
//!
//! Getting a picking ray wrong is quiet. The click lands *near* the right object, so it looks like a
//! tolerance problem rather than a coordinate problem, and it gets "fixed" by widening hitboxes until
//! the viewport feels mushy everywhere. The three things that actually go wrong:
//!
//! * **Pixels.** A CSS/logical pixel is not a device pixel. On a 150% display they differ by 1.5×, so
//!   a ray built from logical pixels against a physical-pixel surface misses by half the cursor's
//!   distance from the viewport centre. [`Viewport`] carries the scale factor and the surface origin
//!   explicitly, and refuses to let a caller mix the two by accident.
//! * **Aspect.** If the picker's aspect ratio is not the one the pixels were rasterized with, every
//!   ray off the centre line is wrong, worst at the edges. Here one [`Viewport`] produces both.
//! * **Precision.** Unprojecting through an inverted view-projection matrix mixes the camera's
//!   absolute world position into every arithmetic step, so a ray built ten kilometres from the
//!   origin is quantized before it starts. [`Camera::ray_through_ndc`] builds the direction
//!   *analytically* in the camera's own basis and only then attaches the world-space origin.
//!
//! **Coordinate contract.** Right-handed, **+Y up**, camera looks down **−Z** in view space. Clip
//! depth is `[0, 1]` (the wgpu/D3D convention), not `[-1, 1]`. Distances are metres; angles are
//! radians at every API boundary and degrees only in UI strings.

use glam::{DMat4, DVec3};

use crate::bounds::Aabb;
use crate::epsilon;
use crate::ray::Ray;
use crate::transform::{unm, unv, v, Mat4, Vec3};

/// How the camera flattens the scene.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Projection {
    /// Vertical field of view, radians.
    Perspective { fov_y: f64 },
    /// Half the visible world height at any depth. Parallel: no foreshortening.
    Orthographic { half_height: f64 },
}

impl Projection {
    /// The orthographic projection that shows exactly what the perspective one shows at `distance`.
    ///
    /// Tying them together is deliberate: switching projection then does not move the subject, and
    /// zoom keeps one meaning. An independent "ortho scale" is a second representation of the same
    /// quantity, and two representations of one quantity eventually disagree.
    #[must_use]
    pub fn orthographic_matching(fov_y: f64, distance: f64) -> Self {
        Self::Orthographic {
            half_height: (distance.abs() * (fov_y * 0.5).tan()).max(1.0e-6),
        }
    }

    #[must_use]
    pub fn is_orthographic(self) -> bool {
        matches!(self, Self::Orthographic { .. })
    }
}

/// The rendered surface a camera's image occupies, and how pointer coordinates map onto it.
///
/// `surface_*` is in **physical pixels** (the size the swapchain was configured with). `origin_*` is
/// the surface's top-left in the **logical** coordinate space pointer events arrive in. Keeping both,
/// with the scale factor between them, is what makes a click at the visual centre land on NDC (0,0)
/// on every display.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub surface_width: f64,
    pub surface_height: f64,
    pub origin_x: f64,
    pub origin_y: f64,
    /// Physical pixels per logical pixel (`devicePixelRatio` / winit's `scale_factor`).
    pub dpi_scale: f64,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            surface_width: 1920.0,
            surface_height: 1080.0,
            origin_x: 0.0,
            origin_y: 0.0,
            dpi_scale: 1.0,
        }
    }
}

impl Viewport {
    /// A surface of `width`×`height` **physical** pixels at logical origin `(0, 0)`.
    #[must_use]
    pub fn from_physical(width: f64, height: f64, dpi_scale: f64) -> Self {
        Self {
            surface_width: width.max(1.0),
            surface_height: height.max(1.0),
            origin_x: 0.0,
            origin_y: 0.0,
            dpi_scale: if dpi_scale.is_finite() && dpi_scale > 0.0 {
                dpi_scale
            } else {
                1.0
            },
        }
    }

    /// A surface described in **logical** pixels (what a DOM rect reports), converted internally.
    #[must_use]
    pub fn from_logical(x: f64, y: f64, width: f64, height: f64, dpi_scale: f64) -> Self {
        let s = if dpi_scale.is_finite() && dpi_scale > 0.0 {
            dpi_scale
        } else {
            1.0
        };
        Self {
            surface_width: (width * s).max(1.0),
            surface_height: (height * s).max(1.0),
            origin_x: x,
            origin_y: y,
            dpi_scale: s,
        }
    }

    /// Width ÷ height of the rendered surface — the **only** aspect ratio anything should use, so the
    /// picker and the rasterizer cannot disagree.
    #[must_use]
    pub fn aspect(&self) -> f64 {
        (self.surface_width / self.surface_height).clamp(1.0e-3, 1.0e3)
    }

    #[must_use]
    pub fn logical_width(&self) -> f64 {
        self.surface_width / self.dpi_scale
    }

    #[must_use]
    pub fn logical_height(&self) -> f64 {
        self.surface_height / self.dpi_scale
    }

    /// A pointer position in **logical window coordinates** → normalized device coordinates, where
    /// `(-1, -1)` is bottom-left and `(+1, +1)` is top-right. Values outside `[-1, 1]` mean the
    /// pointer is outside the surface; that is reported, not clamped, so a caller can tell.
    #[must_use]
    pub fn logical_to_ndc(&self, x: f64, y: f64) -> (f64, f64) {
        let px = (x - self.origin_x) * self.dpi_scale;
        let py = (y - self.origin_y) * self.dpi_scale;
        (
            (px / self.surface_width) * 2.0 - 1.0,
            1.0 - (py / self.surface_height) * 2.0,
        )
    }

    /// A pointer position in **physical pixels relative to the surface** → NDC.
    #[must_use]
    pub fn physical_to_ndc(&self, px: f64, py: f64) -> (f64, f64) {
        (
            (px / self.surface_width) * 2.0 - 1.0,
            1.0 - (py / self.surface_height) * 2.0,
        )
    }

    /// A `[0, 1]` surface fraction → NDC. This is the shape the existing IPC boundary already speaks,
    /// so it is supported as a first-class input rather than reconstructed by every caller.
    #[must_use]
    pub fn fraction_to_ndc(&self, fx: f64, fy: f64) -> (f64, f64) {
        (fx * 2.0 - 1.0, 1.0 - fy * 2.0)
    }

    /// NDC → logical window coordinates (the inverse of [`Self::logical_to_ndc`]).
    #[must_use]
    pub fn ndc_to_logical(&self, ndc_x: f64, ndc_y: f64) -> (f64, f64) {
        let px = f64::midpoint(ndc_x, 1.0) * self.surface_width;
        let py = (1.0 - ndc_y) * 0.5 * self.surface_height;
        (
            px / self.dpi_scale + self.origin_x,
            py / self.dpi_scale + self.origin_y,
        )
    }

    /// NDC → `[0, 1]` surface fraction.
    #[must_use]
    pub fn ndc_to_fraction(&self, ndc_x: f64, ndc_y: f64) -> (f64, f64) {
        (f64::midpoint(ndc_x, 1.0), (1.0 - ndc_y) * 0.5)
    }

    /// How many NDC units one physical pixel spans, per axis. The conversion that turns a
    /// "within N pixels" tolerance into geometry, so tolerances are stated in the unit users
    /// perceive and stay constant as the window resizes.
    #[must_use]
    pub fn pixel_in_ndc(&self) -> (f64, f64) {
        (2.0 / self.surface_width, 2.0 / self.surface_height)
    }
}

/// A viewport camera: an eye, a look-at target, an up hint, and how it projects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// World-space eye position, in the canonical f64.
    pub eye: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub projection: Projection,
    pub near: f64,
    pub far: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            eye: [0.0, 0.0, 10.0],
            target: [0.0; 3],
            up: [0.0, 1.0, 0.0],
            projection: Projection::Perspective {
                fov_y: 55.0_f64.to_radians(),
            },
            near: 0.1,
            far: 10_000.0,
        }
    }
}

/// An orbit camera's parameters, kept as the authored state the editor manipulates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Orbit {
    /// Azimuth, radians.
    pub yaw: f64,
    /// Elevation, radians. Clamped away from the poles so `up` never degenerates.
    pub pitch: f64,
    pub distance: f64,
    pub target: Vec3,
}

impl Orbit {
    /// The eye position this orbit implies. Matches the renderer's convention exactly:
    /// `offset = (cos(yaw)·cos(pitch), sin(pitch), sin(yaw)·cos(pitch)) · distance`.
    #[must_use]
    pub fn eye(&self) -> Vec3 {
        let cp = self.pitch.cos();
        [
            self.target[0] + self.yaw.cos() * self.distance * cp,
            self.target[1] + self.distance * self.pitch.sin(),
            self.target[2] + self.yaw.sin() * self.distance * cp,
        ]
    }

    /// Build a camera, choosing near/far from the orbit distance so a scene viewed from far away does
    /// not clip and a scene inspected from centimetres away does not lose its depth resolution.
    ///
    /// A fixed near plane is the usual mistake: 0.1 is far too near when the camera is 5 km out (the
    /// depth buffer's precision is spent on the first millimetre) and far too far when the user has
    /// zoomed into a 2 mm feature (the feature is clipped away entirely).
    #[must_use]
    pub fn to_camera(&self, fov_y: f64, orthographic: bool) -> Camera {
        let d = self.distance.abs().max(1.0e-4);
        let near = (d * 1.0e-3).clamp(1.0e-5, 1.0);
        let far = d * 100.0 + 1000.0;
        Camera {
            eye: self.eye(),
            target: self.target,
            up: [0.0, 1.0, 0.0],
            projection: if orthographic {
                Projection::orthographic_matching(fov_y, d)
            } else {
                Projection::Perspective { fov_y }
            },
            near,
            far,
        }
    }
}

impl Camera {
    /// The camera's orthonormal basis: `(right, up, forward)`, forward pointing **at** the target.
    ///
    /// Falls back to a stable basis when the eye sits on the target or the up hint is parallel to the
    /// view direction (looking straight down, the everyday case) — those are the configurations where
    /// a naive `cross` returns a zero vector and the whole view matrix becomes NaN.
    #[must_use]
    pub fn basis(&self) -> [Vec3; 3] {
        let mut forward = v(self.target) - v(self.eye);
        if forward.length_squared() < epsilon::DIRECTION_LEN_SQ {
            forward = -DVec3::Z;
        }
        let forward = forward.normalize();
        let mut up_hint = v(self.up);
        if up_hint.length_squared() < epsilon::DIRECTION_LEN_SQ {
            up_hint = DVec3::Y;
        }
        let mut right = forward.cross(up_hint);
        if right.length_squared() < epsilon::DIRECTION_LEN_SQ {
            // Looking straight along the up axis: any perpendicular is as good as any other, so pick
            // a deterministic one rather than producing NaN.
            right = forward.any_orthonormal_vector();
        }
        let right = right.normalize();
        let up = right.cross(forward).normalize();
        [unv(right), unv(up), unv(forward)]
    }

    /// The view matrix (world → camera space), right-handed, looking down −Z.
    #[must_use]
    pub fn view_matrix(&self) -> Mat4 {
        let [right, up, forward] = self.basis();
        unm(DMat4::look_to_rh(v(self.eye), v(forward), v(up))).tap_identity(right)
        // `right` is derived inside look_to_rh; named here for readability
    }

    /// The view matrix built **camera-relative**: the eye is at the origin.
    ///
    /// This is the matrix the renderer should use with camera-relative geometry. Because the eye's
    /// large absolute coordinate never enters it, the rotation part is exact regardless of where in
    /// the world the camera is.
    #[must_use]
    pub fn view_matrix_relative(&self) -> Mat4 {
        let [_, up, forward] = self.basis();
        unm(DMat4::look_to_rh(DVec3::ZERO, v(forward), v(up)))
    }

    /// The projection matrix for `aspect`, with clip depth in `[0, 1]`.
    #[must_use]
    pub fn projection_matrix(&self, aspect: f64) -> Mat4 {
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            1.0
        };
        let near = self.near.max(1.0e-9);
        let far = self.far.max(near * (1.0 + 1.0e-6));
        match self.projection {
            Projection::Perspective { fov_y } => {
                let fov = fov_y.clamp(1.0e-3, std::f64::consts::PI - 1.0e-3);
                unm(DMat4::perspective_rh(fov, aspect, near, far))
            }
            Projection::Orthographic { half_height } => {
                let h = half_height.max(1.0e-9);
                let w = h * aspect;
                // The near plane goes BEHIND the eye for a parallel projection: it has no apex to
                // clip against, and starting at the eye slices away everything between the camera and
                // its orbit target — in a top view, most of the scene.
                unm(DMat4::orthographic_rh(-w, w, -h, h, -far, far))
            }
        }
    }

    /// `projection · view` for `aspect`.
    #[must_use]
    pub fn view_projection(&self, aspect: f64) -> Mat4 {
        crate::transform::mat_mul(self.projection_matrix(aspect), self.view_matrix())
    }

    /// **The picking ray.** Built analytically in the camera basis, so the eye's absolute magnitude
    /// never participates in the direction arithmetic.
    ///
    /// For a perspective camera the ray starts at the eye; for an orthographic one it starts on the
    /// near plane, offset across the frustum, because parallel rays do not share an origin.
    #[must_use]
    pub fn ray_through_ndc(&self, ndc_x: f64, ndc_y: f64, aspect: f64) -> Ray {
        let [right, up, forward] = self.basis();
        let (r, u, f) = (v(right), v(up), v(forward));
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            1.0
        };
        match self.projection {
            Projection::Perspective { fov_y } => {
                let t = (fov_y.clamp(1.0e-3, std::f64::consts::PI - 1.0e-3) * 0.5).tan();
                let dir = f + r * (ndc_x * t * aspect) + u * (ndc_y * t);
                Ray::new(self.eye, unv(dir.normalize_or_zero()))
            }
            Projection::Orthographic { half_height } => {
                let h = half_height.max(1.0e-9);
                let w = h * aspect;
                // Start behind the eye plane by `far`, matching the projection's near bound, so that
                // geometry between the camera and the orbit target is reachable — in a top-down
                // orthographic view that is most of the scene.
                let origin = v(self.eye) + r * (ndc_x * w) + u * (ndc_y * h) - f * self.far;
                Ray::new(unv(origin), forward)
            }
        }
    }

    /// The picking ray for a pointer position in logical window coordinates.
    #[must_use]
    pub fn ray_through_logical(&self, viewport: &Viewport, x: f64, y: f64) -> Ray {
        let (nx, ny) = viewport.logical_to_ndc(x, y);
        self.ray_through_ndc(nx, ny, viewport.aspect())
    }

    /// The picking ray for a `[0, 1]` surface fraction (the existing IPC shape).
    #[must_use]
    pub fn ray_through_fraction(&self, viewport: &Viewport, fx: f64, fy: f64) -> Ray {
        let (nx, ny) = viewport.fraction_to_ndc(fx, fy);
        self.ray_through_ndc(nx, ny, viewport.aspect())
    }

    /// Project a world point to NDC, camera-relative so the arithmetic stays exact far from the
    /// origin. Returns `None` when the point is at or behind the eye plane of a perspective camera.
    #[must_use]
    pub fn world_to_ndc(&self, world: Vec3, aspect: f64) -> Option<[f64; 3]> {
        let rel = v(world) - v(self.eye);
        if !rel.is_finite() {
            return None;
        }
        let [right, up, forward] = self.basis();
        // View-space coordinates directly from the basis (right-handed, looking down −Z).
        let view = DVec3::new(rel.dot(v(right)), rel.dot(v(up)), -rel.dot(v(forward)));
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            1.0
        };
        let near = self.near.max(1.0e-9);
        let far = self.far.max(near * (1.0 + 1.0e-6));
        match self.projection {
            Projection::Perspective { fov_y } => {
                let t = (fov_y.clamp(1.0e-3, std::f64::consts::PI - 1.0e-3) * 0.5).tan();
                let w = -view.z;
                if w <= 1.0e-12 {
                    return None; // at or behind the eye: there is no screen position
                }
                // wgpu/D3D depth mapping for `perspective_rh`.
                let depth = (far * near / (near - far) + -view.z * (far / (far - near))) / w;
                Some([view.x / (t * aspect * w), view.y / (t * w), depth])
            }
            Projection::Orthographic { half_height } => {
                let h = half_height.max(1.0e-9);
                let w = h * aspect;
                let depth = (-view.z - (-far)) / (far - (-far));
                Some([view.x / w, view.y / h, depth])
            }
        }
    }

    /// Project a world point to a `[0, 1]` surface fraction. `None` when it is not in front.
    #[must_use]
    pub fn world_to_fraction(&self, viewport: &Viewport, world: Vec3) -> Option<(f64, f64)> {
        let ndc = self.world_to_ndc(world, viewport.aspect())?;
        Some(viewport.ndc_to_fraction(ndc[0], ndc[1]))
    }

    /// How many world units one physical pixel covers at `world` — the conversion behind every
    /// screen-space tolerance and behind constant-on-screen gizmo sizing.
    ///
    /// Under a perspective camera this grows with distance; under an orthographic one it is constant,
    /// which is exactly why a single hard-coded world-space tolerance cannot serve both.
    #[must_use]
    pub fn world_units_per_pixel(&self, viewport: &Viewport, world: Vec3) -> f64 {
        match self.projection {
            Projection::Perspective { fov_y } => {
                let [_, _, forward] = self.basis();
                // Depth ALONG THE VIEW AXIS, not radial distance: a point at the screen edge is
                // farther from the eye than one at the centre but sits on the same image plane, and
                // using the radial distance makes the tolerance grow toward the corners.
                let depth = (v(world) - v(self.eye)).dot(v(forward)).abs().max(1.0e-9);
                let visible_height = 2.0 * depth * (fov_y * 0.5).tan();
                visible_height / viewport.surface_height.max(1.0)
            }
            Projection::Orthographic { half_height } => {
                (2.0 * half_height) / viewport.surface_height.max(1.0)
            }
        }
    }

    /// Distance and target that frame `bounds`, with `margin` as a fraction of the fitted size.
    ///
    /// Handles the cases a naive `radius / tan(fov/2)` gets wrong: a zero-size object (a light, an
    /// empty) would give distance 0 and put the camera inside itself; an enormous object would give a
    /// distance beyond the far plane; and a wide, flat object needs the *horizontal* half-angle,
    /// which on a wide viewport is the tighter constraint.
    #[must_use]
    pub fn frame_bounds(&self, bounds: &Aabb, aspect: f64, margin: f64) -> Option<(Vec3, f64)> {
        if bounds.is_empty() || !bounds.is_finite() {
            return None;
        }
        let center = bounds.center();
        let radius = bounds.radius().max(1.0e-4) * (1.0 + margin.max(0.0));
        let distance = match self.projection {
            Projection::Perspective { fov_y } => {
                let half_v = (fov_y * 0.5).clamp(1.0e-3, 1.5);
                let half_h = (half_v.tan() * aspect.max(1.0e-3)).atan().max(1.0e-3);
                // The tighter of the two constraints, plus the object's own depth so the near face
                // does not end up behind the camera.
                let d = (radius / half_v.sin().max(1.0e-6)).max(radius / half_h.sin().max(1.0e-6));
                d + bounds.half_size()[2].max(0.0)
            }
            Projection::Orthographic { .. } => radius * 2.5,
        };
        // Never inside the geometry, never past the far plane.
        let distance = distance.clamp(radius * 1.05 + self.near.max(1.0e-5), 1.0e12);
        Some((center, distance))
    }
}

/// A no-op used purely to keep `basis()`'s `right` component visibly part of the view derivation.
trait TapIdentity {
    fn tap_identity(self, _unused: Vec3) -> Self;
}
impl TapIdentity for Mat4 {
    fn tap_identity(self, _unused: Vec3) -> Self {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epsilon::approx_eq;

    fn persp() -> Camera {
        Camera {
            eye: [0.0, 0.0, 10.0],
            target: [0.0; 3],
            up: [0.0, 1.0, 0.0],
            projection: Projection::Perspective {
                fov_y: 55.0_f64.to_radians(),
            },
            near: 0.01,
            far: 1000.0,
        }
    }

    #[test]
    fn a_click_at_the_visual_centre_is_ndc_zero_on_every_display() {
        // The DPI case, arithmetically. A 1600×900 LOGICAL window on a 150% display is a 2400×1350
        // physical surface. A pointer event arrives in logical pixels; the surface is physical. The
        // centre must be (0,0) under both, or every off-centre ray is wrong by the scale factor.
        for dpi in [1.0, 1.25, 1.5, 2.0, 3.0] {
            let vp = Viewport::from_logical(0.0, 0.0, 1600.0, 900.0, dpi);
            let (nx, ny) = vp.logical_to_ndc(800.0, 450.0);
            assert!(
                approx_eq(nx, 0.0) && approx_eq(ny, 0.0),
                "dpi {dpi}: got ({nx}, {ny})"
            );
            // Corners map to the NDC corners regardless of scale factor.
            assert_eq!(vp.logical_to_ndc(0.0, 0.0), (-1.0, 1.0));
            assert_eq!(vp.logical_to_ndc(1600.0, 900.0), (1.0, -1.0));
            assert!(
                approx_eq(vp.aspect(), 1600.0 / 900.0),
                "aspect is dpi-independent"
            );
        }
    }

    #[test]
    fn a_viewport_offset_by_a_dock_still_maps_correctly() {
        // A surface that starts 320 logical px from the window's left edge: a pointer at window x=320
        // is at the surface's LEFT edge, not 320 px into it.
        let vp = Viewport::from_logical(320.0, 48.0, 1000.0, 600.0, 2.0);
        assert_eq!(vp.logical_to_ndc(320.0, 48.0), (-1.0, 1.0));
        let (cx, cy) = vp.logical_to_ndc(320.0 + 500.0, 48.0 + 300.0);
        assert!(approx_eq(cx, 0.0) && approx_eq(cy, 0.0));
        // …and the round trip is exact.
        let (lx, ly) = vp.ndc_to_logical(cx, cy);
        assert!(approx_eq(lx, 820.0) && approx_eq(ly, 348.0));
    }

    #[test]
    fn the_centre_ray_points_straight_at_the_target() {
        let c = persp();
        let r = c.ray_through_ndc(0.0, 0.0, 16.0 / 9.0);
        assert!(crate::epsilon::approx_eq_vec3(r.origin, c.eye));
        // Looking from +Z at the origin ⇒ forward is −Z.
        assert!(crate::epsilon::approx_eq_vec3_tol(
            r.direction,
            [0.0, 0.0, -1.0],
            1e-12,
            1e-12
        ));
    }

    #[test]
    fn corner_rays_match_the_projection_they_were_built_from() {
        // The cross-check that catches an aspect/fov mismatch: unproject a corner analytically, then
        // re-project the point it reaches and confirm it lands back on that corner.
        let c = persp();
        let aspect = 21.0 / 9.0; // a deliberately un-square viewport
        for (nx, ny) in [
            (-1.0, -1.0),
            (1.0, -1.0),
            (-1.0, 1.0),
            (1.0, 1.0),
            (0.37, -0.81),
        ] {
            let ray = c.ray_through_ndc(nx, ny, aspect);
            let point = ray.at(25.0);
            let back = c
                .world_to_ndc(point, aspect)
                .expect("in front of the camera");
            assert!(
                approx_eq(back[0], nx) && approx_eq(back[1], ny),
                "({nx}, {ny}) round-tripped to ({}, {})",
                back[0],
                back[1]
            );
            assert!(
                (0.0..=1.0).contains(&back[2]),
                "depth is in the wgpu [0,1] range"
            );
        }
    }

    #[test]
    fn ray_and_projection_agree_with_the_actual_view_projection_matrix() {
        // The analytic ray must describe the SAME frustum the rasterizer uses, or picking drifts from
        // pixels. Compare against the real matrix path rather than against itself.
        let c = persp();
        let aspect = 16.0 / 9.0;
        let vp = c.view_projection(aspect);
        for (nx, ny) in [(0.0, 0.0), (0.9, -0.6), (-0.4, 0.8)] {
            let p = c.ray_through_ndc(nx, ny, aspect).at(12.0);
            let clip = crate::transform::transform_point4(vp, p).expect("in front");
            assert!(
                approx_eq(clip[0], nx) && approx_eq(clip[1], ny),
                "matrix says ({}, {}) for analytic ({nx}, {ny})",
                clip[0],
                clip[1]
            );
        }
    }

    #[test]
    fn orthographic_rays_are_parallel_and_start_across_the_frustum() {
        let c = Camera {
            projection: Projection::Orthographic { half_height: 5.0 },
            ..persp()
        };
        let aspect = 2.0;
        let a = c.ray_through_ndc(-1.0, -1.0, aspect);
        let b = c.ray_through_ndc(1.0, 1.0, aspect);
        assert!(
            crate::epsilon::approx_eq_vec3_tol(a.direction, b.direction, 1e-12, 1e-12),
            "parallel projection ⇒ parallel rays"
        );
        assert!(
            (a.origin[0] - b.origin[0]).abs() > 1.0,
            "…which therefore need DIFFERENT origins ({:?} vs {:?})",
            a.origin,
            b.origin
        );
        // half_width = half_height * aspect = 10, so the corner rays are 20 apart on X.
        assert!(approx_eq((b.origin[0] - a.origin[0]).abs(), 20.0));
        // And the round-trip through the ortho projection still works.
        let p = c.ray_through_ndc(0.5, -0.25, aspect).at(30.0);
        let back = c.world_to_ndc(p, aspect).expect("ortho always projects");
        assert!(approx_eq(back[0], 0.5) && approx_eq(back[1], -0.25));
    }

    #[test]
    fn orthographic_picking_reaches_geometry_between_the_camera_and_its_target() {
        // The top-view failure: an ortho ray that starts at the eye plane clips away everything in
        // front of the orbit target, so clicking the object you are looking down at selects nothing.
        let c = Camera {
            eye: [0.0, 50.0, 0.0],
            target: [0.0; 3],
            projection: Projection::Orthographic { half_height: 20.0 },
            ..persp()
        };
        let ray = c.ray_through_ndc(0.0, 0.0, 1.0);
        // An object floating ABOVE the target (between it and the camera) must still be in front of
        // the ray origin, i.e. at a positive t.
        let hit = [0.0, 40.0, 0.0];
        let t = ray.project_parameter(hit);
        assert!(
            t > 0.0,
            "object between camera and target is reachable (t = {t})"
        );
    }

    #[test]
    fn a_point_behind_a_perspective_camera_has_no_screen_position() {
        let c = persp();
        assert!(
            c.world_to_ndc([0.0, 0.0, 20.0], 1.0).is_none(),
            "behind the eye"
        );
        assert!(
            c.world_to_ndc([0.0, 0.0, 10.0], 1.0).is_none(),
            "exactly at the eye"
        );
        assert!(
            c.world_to_ndc([0.0, 0.0, 9.0], 1.0).is_some(),
            "just in front"
        );
        // An orthographic camera has no apex, so a point "behind" it still projects.
        let o = Camera {
            projection: Projection::Orthographic { half_height: 5.0 },
            ..c
        };
        assert!(o.world_to_ndc([0.0, 0.0, 20.0], 1.0).is_some());
    }

    #[test]
    fn looking_straight_down_does_not_produce_a_nan_basis() {
        // up = +Y and forward = −Y are parallel: the naive cross product is zero and every downstream
        // matrix becomes NaN. This is the default top view, so it is not an edge case.
        let c = Camera {
            eye: [0.0, 100.0, 0.0],
            target: [0.0; 3],
            up: [0.0, 1.0, 0.0],
            ..persp()
        };
        let [right, up, forward] = c.basis();
        for axis in [right, up, forward] {
            assert!(epsilon::all_finite(&axis), "basis axis is finite: {axis:?}");
            let len = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
            assert!(approx_eq(len, 1.0));
        }
        let r = c.ray_through_ndc(0.3, -0.2, 1.5);
        assert!(epsilon::all_finite(&r.direction) && r.direction != [0.0; 3]);
    }

    #[test]
    fn rays_stay_exact_ten_kilometres_from_the_origin() {
        // The precision claim for picking. Build the same relative view twice — once near the origin,
        // once 10 km out — and require the ray DIRECTIONS to be identical to f64 precision. An
        // inverse-view-projection unprojection loses digits here because the eye's magnitude enters
        // every term; the analytic construction does not.
        let near_origin = persp();
        let offset = 10_000.0;
        let far_out = Camera {
            eye: [
                near_origin.eye[0] + offset,
                near_origin.eye[1],
                near_origin.eye[2],
            ],
            target: [
                near_origin.target[0] + offset,
                near_origin.target[1],
                near_origin.target[2],
            ],
            ..near_origin
        };
        for (nx, ny) in [(0.0, 0.0), (0.99, -0.99), (-0.5, 0.25)] {
            let a = near_origin.ray_through_ndc(nx, ny, 1.7);
            let b = far_out.ray_through_ndc(nx, ny, 1.7);
            for i in 0..3 {
                assert!(
                    (a.direction[i] - b.direction[i]).abs() < 1.0e-15,
                    "direction axis {i} drifted by {} at 10 km",
                    (a.direction[i] - b.direction[i]).abs()
                );
            }
            assert!(
                approx_eq(b.origin[0] - a.origin[0], offset),
                "origin carries the offset exactly"
            );
        }
    }

    #[test]
    fn world_units_per_pixel_uses_view_depth_not_radial_distance() {
        let c = persp();
        let vp = Viewport::from_physical(1920.0, 1080.0, 1.0);
        // Two points at the SAME view depth but different screen positions must have the same scale.
        let centre = c.world_units_per_pixel(&vp, [0.0, 0.0, 0.0]);
        let edge = c.world_units_per_pixel(&vp, [8.0, 4.0, 0.0]);
        assert!(approx_eq(centre, edge), "{centre} vs {edge}");
        // Twice the depth ⇒ twice the world size per pixel.
        let far = c.world_units_per_pixel(&vp, [0.0, 0.0, -10.0]);
        assert!(approx_eq(far, centre * 2.0), "{far} vs {}", centre * 2.0);
        // Orthographic is depth-independent, which is why one hard-coded tolerance cannot serve both.
        let o = Camera {
            projection: Projection::Orthographic { half_height: 5.0 },
            ..c
        };
        assert!(approx_eq(
            o.world_units_per_pixel(&vp, [0.0; 3]),
            o.world_units_per_pixel(&vp, [0.0, 0.0, -1000.0])
        ));
    }

    #[test]
    fn framing_handles_a_point_object_and_a_kilometre_object() {
        let c = persp();
        // A light/empty has zero size: a naive fit gives distance 0 and parks the camera inside it.
        let point = Aabb::point([3.0, 4.0, 5.0]);
        let (target, d) = c.frame_bounds(&point, 1.777, 0.1).expect("framable");
        assert_eq!(target, [3.0, 4.0, 5.0]);
        assert!(
            d > 0.0 && d.is_finite(),
            "a zero-size object still gets a usable distance ({d})"
        );

        // A kilometre-wide object must not be framed from inside itself.
        let huge = Aabb::new([-500.0; 3], [500.0; 3]);
        let (_, d) = c.frame_bounds(&huge, 1.777, 0.05).expect("framable");
        assert!(
            d > huge.radius(),
            "camera is outside the geometry ({d} vs r={})",
            huge.radius()
        );
        assert!(d.is_finite());

        // A wide flat object on a narrow viewport is constrained horizontally, so a narrow aspect must
        // pull the camera further back than a wide one.
        let wide = Aabb::new([-50.0, -1.0, -1.0], [50.0, 1.0, 1.0]);
        let (_, narrow_d) = c.frame_bounds(&wide, 0.5, 0.0).expect("framable");
        let (_, wide_d) = c.frame_bounds(&wide, 3.0, 0.0).expect("framable");
        assert!(
            narrow_d > wide_d,
            "narrow viewport needs more distance ({narrow_d} vs {wide_d})"
        );

        assert!(
            c.frame_bounds(&Aabb::EMPTY, 1.0, 0.0).is_none(),
            "nothing to frame"
        );
    }

    #[test]
    fn orbit_near_and_far_scale_with_the_viewing_distance() {
        // A fixed 0.1 near plane is wrong at both ends: it wastes the depth buffer when the camera is
        // kilometres out, and clips the subject away when the user zooms into a millimetre feature.
        let close = Orbit {
            yaw: 0.4,
            pitch: 0.3,
            distance: 0.005,
            target: [0.0; 3],
        }
        .to_camera(55f64.to_radians(), false);
        assert!(
            close.near < 0.005,
            "a 5 mm view is not clipped ({})",
            close.near
        );
        let far = Orbit {
            yaw: 0.4,
            pitch: 0.3,
            distance: 5000.0,
            target: [0.0; 3],
        }
        .to_camera(55f64.to_radians(), false);
        assert!(
            far.near > 0.5,
            "a 5 km view spends its depth range usefully ({})",
            far.near
        );
        assert!(
            far.far > 5000.0,
            "and still reaches the subject ({})",
            far.far
        );
        assert!(
            far.far / far.near < 1.0e6,
            "depth ratio stays inside f32 depth's usable range"
        );
    }

    #[test]
    fn switching_projection_does_not_move_the_subject() {
        let orbit = Orbit {
            yaw: 0.9,
            pitch: 0.35,
            distance: 42.0,
            target: [1.0, 2.0, 3.0],
        };
        let fov = 55f64.to_radians();
        let p = orbit.to_camera(fov, false);
        let o = orbit.to_camera(fov, true);
        // A point at the orbit target projects to the same place under both.
        let a = p.world_to_ndc(orbit.target, 1.6).expect("in front");
        let b = o.world_to_ndc(orbit.target, 1.6).expect("ortho projects");
        assert!(approx_eq(a[0], b[0]) && approx_eq(a[1], b[1]));
        // And a point one unit off-target projects to (very nearly) the same offset.
        let off = [orbit.target[0] + 1.0, orbit.target[1], orbit.target[2]];
        let pa = p.world_to_ndc(off, 1.6).expect("in front");
        let ob = o.world_to_ndc(off, 1.6).expect("ortho");
        assert!((pa[0] - ob[0]).abs() < 1.0e-3, "{} vs {}", pa[0], ob[0]);
    }
}
