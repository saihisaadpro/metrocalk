//! **The manipulator** — the translate/rotate/scale gizmo's mathematics.
//!
//! Four rules this module exists to enforce, each fixing a specific way gizmos go wrong:
//!
//! 1. **The drag is geometric, not pixel-based.** A handle defines a constraint (a line, a plane, a
//!    ring); the cursor ray is intersected with it; the movement is whatever that intersection says.
//!    Screen-space pixel deltas scaled by a fudge factor feel approximately right at one zoom level
//!    and wrong at every other.
//!
//! 2. **Everything is measured from the state recorded at pointer-down.** [`DragState`] holds the
//!    start transform, the start intersection and the constraint, frozen for the gesture. So the
//!    object cannot jump on the first move (the delta starts at exactly zero), snapping cannot drift
//!    (see [`crate::snap`]), and a 200-frame drag is one delta rather than 200 accumulated ones.
//!
//! 3. **Degenerate camera angles are detected, not suffered.** When an axis points nearly at the
//!    camera, the closest-approach parameter along it divides by a vanishing determinant and its
//!    sign flips with noise — the "I grabbed the handle and the object shot to infinity" failure.
//!    The constraint chosen at pointer-down records which method is numerically sound *for that
//!    camera*, and a degenerate axis falls back to a view-plane projection that degrades smoothly to
//!    "no movement" instead of exploding. The method is fixed for the gesture, because switching
//!    methods mid-drag is itself a jump.
//!
//! 4. **Hit geometry is separate from visual geometry.** Handles are drawn thin and picked fat, with
//!    the tolerance expressed in **pixels** so it is the same physical size at any zoom. Making the
//!    drawn gizmo bigger to make it easier to grab trades one problem for another.

use crate::camera::{Camera, Viewport};
use crate::epsilon;
use crate::ray::{closest_ray_line, ray_plane, ray_segment_distance, Ray};
use crate::snap::{snap_scalar, SnapMode, SnapSettings};
use crate::transform::{quat_mul, Quat, Transform, Vec3};

/// What the gizmo manipulates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GizmoMode {
    #[default]
    Translate,
    Rotate,
    Scale,
}

/// The frame the handles align to.
///
/// `Parent` and `View` exist because they are the two frames a user cannot reconstruct by hand:
/// parent space is what a child's stored numbers are actually in, and view space is what "move it
/// left on my screen" means.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GizmoSpace {
    #[default]
    World,
    /// The object's own rotated axes.
    Local,
    /// The parent's axes — the frame the child's local transform is expressed in.
    Parent,
    /// The camera's right/up/forward.
    View,
}

/// Where the gizmo sits and what a rotation or scale happens about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PivotMode {
    /// The active object's own origin.
    #[default]
    ActiveOrigin,
    /// The average of the selection's origins.
    Median,
    /// The centre of the selection's combined bounds. Different from `Median` whenever objects have
    /// different sizes, which is most of the time.
    BoundsCenter,
    /// Each object transforms about its own origin — a multi-object rotate that spins each part in
    /// place rather than swinging them around a shared centre.
    IndividualOrigins,
    /// A user-placed 3D cursor.
    Cursor,
}

/// A grabbable part of the gizmo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GizmoHandle {
    AxisX,
    AxisY,
    AxisZ,
    PlaneXY,
    PlaneYZ,
    PlaneZX,
    /// The view-plane handle: free translation, uniform scale, or a view-aligned rotation ring.
    Screen,
    /// Uniform scale (the centre box of a scale gizmo).
    Uniform,
}

impl GizmoHandle {
    /// The axis index this handle is bound to, or `None` for the planar and screen handles.
    #[must_use]
    pub fn axis(self) -> Option<usize> {
        match self {
            Self::AxisX => Some(0),
            Self::AxisY => Some(1),
            Self::AxisZ => Some(2),
            _ => None,
        }
    }

    /// The two axis indices a planar handle spans.
    #[must_use]
    pub fn plane_axes(self) -> Option<(usize, usize)> {
        match self {
            Self::PlaneXY => Some((0, 1)),
            Self::PlaneYZ => Some((1, 2)),
            Self::PlaneZX => Some((2, 0)),
            _ => None,
        }
    }

    /// The universal X = red, Y = green, Z = blue; neutral for the rest.
    #[must_use]
    pub fn color(self) -> [f32; 3] {
        match self {
            Self::AxisX | Self::PlaneYZ => [0.92, 0.26, 0.28],
            Self::AxisY | Self::PlaneZX => [0.36, 0.83, 0.34],
            Self::AxisZ | Self::PlaneXY => [0.30, 0.52, 0.96],
            Self::Screen | Self::Uniform => [0.86, 0.86, 0.55],
        }
    }
}

/// How much bigger, in pixels, the hit geometry is than the drawn geometry.
///
/// 9 px of grab radius on a 2 px line is the difference between a gizmo that feels precise and one
/// that feels like it is dodging the cursor. Measured in pixels so it is the same physical target at
/// any zoom and on any display density.
const HANDLE_GRAB_PX: f64 = 9.0;
/// The gizmo's on-screen size, in pixels, from centre to axis tip.
pub const GIZMO_SIZE_PX: f64 = 96.0;

/// The geometric constraint a drag is confined to, chosen at pointer-down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragConstraint {
    /// Slide along a line. The stored `axis` is already in world space.
    Axis { axis: Vec3 },
    /// Slide along a line whose closest-approach solve is ill-conditioned for this camera: the
    /// cursor is intersected with the view plane instead and the result projected onto the axis, so
    /// movement fades to zero as the axis turns toward the eye rather than exploding.
    AxisViaViewPlane { axis: Vec3, view_normal: Vec3 },
    /// Slide in a plane.
    Plane { normal: Vec3 },
    /// Rotate about an axis, measured in the plane perpendicular to it.
    Ring { axis: Vec3 },
    /// Rotate about an axis whose ring is edge-on to the camera: the angle is measured in screen
    /// space around the gizmo's projected centre instead.
    RingViaScreen { axis: Vec3, toward_camera: bool },
    /// A radial screen-space measure, for uniform scale and the view-plane handle.
    Radial { normal: Vec3 },
}

/// The state frozen at pointer-down. Everything the drag reports is derived from this, which is what
/// makes the interaction jump-free and drift-free.
#[derive(Clone, Copy, Debug)]
pub struct DragState {
    pub handle: GizmoHandle,
    pub mode: GizmoMode,
    pub constraint: DragConstraint,
    /// The gizmo's world origin at pointer-down.
    pub origin: Vec3,
    /// The gizmo's world axes at pointer-down.
    pub axes: [Vec3; 3],
    /// The world size of one gizmo unit — the reference a scale drag is measured against.
    pub world_size: f64,
    /// The intersection of the pointer-down ray with the constraint.
    pub start_hit: Vec3,
    /// Parameter along the axis at pointer-down (axis constraints).
    pub start_param: f64,
    /// The angle of the pointer-down hit around the ring, radians.
    pub start_angle: f64,
    /// Radial screen distance at pointer-down, in pixels (radial constraints).
    pub start_radius_px: f64,
    /// The gizmo centre's NDC position, for screen-space fallbacks.
    pub origin_ndc: [f64; 2],
    /// The pointer's NDC position at pointer-down.
    pub start_ndc: [f64; 2],
    /// Accumulated rotation, so a drag past 180° keeps turning instead of wrapping backwards.
    turns: f64,
    last_angle: f64,
}

/// The change a drag has produced, relative to the transform recorded at pointer-down.
///
/// A delta rather than an absolute transform, so it can be applied to a whole multi-selection about a
/// shared pivot without each object needing its own drag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransformDelta {
    /// World-space translation.
    pub translation: Vec3,
    /// World-space rotation, applied about the pivot.
    pub rotation: Quat,
    /// Multiplicative scale factor, per gizmo axis.
    pub scale: Vec3,
}

impl Default for TransformDelta {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl TransformDelta {
    pub const IDENTITY: Self = Self {
        translation: [0.0; 3],
        rotation: crate::transform::IDENTITY_QUAT,
        scale: [1.0; 3],
    };

    /// Whether this delta changes anything — used to skip an undo entry for a click that did not move.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.translation == [0.0; 3]
            && crate::transform::quat_approx_eq(self.rotation, crate::transform::IDENTITY_QUAT)
            && self.scale == [1.0; 3]
    }

    /// Apply to a world transform, rotating and scaling about `pivot`.
    ///
    /// Scale is applied along the **gizmo's** axes (`axes`), not the object's — otherwise scaling a
    /// rotated object in World mode stretches it along its own axes while the handles say otherwise.
    #[must_use]
    pub fn apply(&self, start: &Transform, pivot: Vec3, axes: [Vec3; 3]) -> Transform {
        let rel = sub(start.translation, pivot);
        // Scale the pivot-relative offset in the gizmo's frame.
        let scaled = if self.scale == [1.0; 3] {
            rel
        } else {
            let mut out = [0.0; 3];
            for (i, axis) in axes.iter().enumerate() {
                let along = dot(rel, *axis) * self.scale[i];
                out = add(out, mul(*axis, along));
            }
            out
        };
        let rotated = rotate_vec(self.rotation, scaled);
        let translation = add(add(pivot, rotated), self.translation);

        // The object's own scale changes only by the component of the gizmo scale along each of its
        // own axes, so a uniform gizmo scale is uniform on the object whatever its rotation.
        let object_axes = start.basis();
        let mut scale = start.scale;
        if self.scale != [1.0; 3] {
            for (i, oa) in object_axes.iter().enumerate() {
                let mut factor = 0.0;
                let mut weight = 0.0;
                for (k, ga) in axes.iter().enumerate() {
                    let w = dot(*oa, *ga).abs();
                    factor += w * self.scale[k];
                    weight += w;
                }
                if weight > 1.0e-12 {
                    scale[i] *= factor / weight;
                }
            }
        }
        Transform {
            translation,
            rotation: quat_mul(self.rotation, start.rotation),
            scale,
        }
        .sanitized()
        .unwrap_or(*start)
    }
}

/// The gizmo. Holds mode/space/pivot and the in-flight drag; owns no scene state.
#[derive(Clone, Debug, Default)]
pub struct Manipulator {
    mode: GizmoMode,
    space: GizmoSpace,
    pivot: PivotMode,
    drag: Option<DragState>,
    hover: Option<GizmoHandle>,
}

impl Manipulator {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn mode(&self) -> GizmoMode {
        self.mode
    }
    pub fn set_mode(&mut self, mode: GizmoMode) {
        self.mode = mode;
    }
    #[must_use]
    pub fn space(&self) -> GizmoSpace {
        self.space
    }
    pub fn set_space(&mut self, space: GizmoSpace) {
        self.space = space;
    }
    #[must_use]
    pub fn pivot(&self) -> PivotMode {
        self.pivot
    }
    pub fn set_pivot(&mut self, pivot: PivotMode) {
        self.pivot = pivot;
    }
    #[must_use]
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }
    #[must_use]
    pub fn drag_state(&self) -> Option<&DragState> {
        self.drag.as_ref()
    }
    #[must_use]
    pub fn hover(&self) -> Option<GizmoHandle> {
        self.hover
    }
    pub fn set_hover(&mut self, handle: Option<GizmoHandle>) {
        self.hover = handle;
    }

    /// **The gizmo's world axes for the current space.**
    ///
    /// The bug this replaces: the call site passed an identity rotation regardless of space, so the
    /// UI could say "Local" while every handle moved along world axes. The frame is derived here,
    /// from real inputs, so the label and the mathematics cannot disagree.
    #[must_use]
    pub fn axes(&self, object_rotation: Quat, parent_rotation: Quat, camera: &Camera) -> [Vec3; 3] {
        match self.space {
            GizmoSpace::World => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            GizmoSpace::Local => Transform {
                rotation: object_rotation,
                ..Transform::IDENTITY
            }
            .basis(),
            GizmoSpace::Parent => Transform {
                rotation: parent_rotation,
                ..Transform::IDENTITY
            }
            .basis(),
            GizmoSpace::View => {
                let [right, up, forward] = camera.basis();
                // Forward points AT the target; the gizmo's Z should point at the viewer, so negate.
                [right, up, [-forward[0], -forward[1], -forward[2]]]
            }
        }
    }

    /// The gizmo's world size so it occupies [`GIZMO_SIZE_PX`] pixels regardless of distance or
    /// projection. Correct under orthographic too, where on-screen size does **not** depend on the
    /// eye-to-object distance — a perspective-only formula silently shrinks the gizmo to nothing in
    /// the axis views.
    #[must_use]
    pub fn world_size(&self, camera: &Camera, viewport: &Viewport, origin: Vec3) -> f64 {
        (camera.world_units_per_pixel(viewport, origin) * GIZMO_SIZE_PX).max(1.0e-9)
    }

    /// Which handle the ray grabs, using **hit** geometry (fatter than what is drawn).
    ///
    /// Returns the closest handle in *pixels*, so a cursor between two handles takes the one it is
    /// visually nearer to rather than whichever the loop happened to reach first.
    #[must_use]
    pub fn hit_test(
        &self,
        ray: &Ray,
        origin: Vec3,
        axes: [Vec3; 3],
        world_size: f64,
        camera: &Camera,
        viewport: &Viewport,
    ) -> Option<GizmoHandle> {
        let upp = camera.world_units_per_pixel(viewport, origin).max(1.0e-12);
        let grab = HANDLE_GRAB_PX * upp;
        let mut best: Option<(GizmoHandle, f64)> = None;
        let mut consider = |h: GizmoHandle, d: f64| {
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((h, d));
            }
        };

        match self.mode {
            GizmoMode::Rotate => {
                for (i, handle) in [GizmoHandle::AxisX, GizmoHandle::AxisY, GizmoHandle::AxisZ]
                    .into_iter()
                    .enumerate()
                {
                    // Distance from the ray to the ring, measured as |distance-to-centre − radius| in
                    // the ring's own plane. When the ring is edge-on the plane solve is refused, and
                    // the ring is then genuinely unreachable — which is correct: you cannot grab a
                    // circle you are looking at edge-on, and pretending otherwise is what causes a
                    // wild rotation.
                    if let Some((hit, _)) = ray_plane(ray, origin, axes[i]) {
                        let d = (length(sub(hit, origin)) - world_size).abs();
                        if d < grab {
                            consider(handle, d);
                        }
                    }
                }
                // The view-aligned outer ring is always grabbable — the escape hatch when every axis
                // ring is edge-on.
                let view_normal = neg(ray.direction);
                if let Some((hit, _)) = ray_plane(ray, origin, view_normal) {
                    let d = (length(sub(hit, origin)) - world_size * 1.18).abs();
                    if d < grab {
                        consider(GizmoHandle::Screen, d);
                    }
                }
            }
            GizmoMode::Translate | GizmoMode::Scale => {
                // Planar handles first: their quads sit near the origin, and testing them before the
                // axes means the quad wins where they overlap — matching what is drawn on top.
                if matches!(self.mode, GizmoMode::Translate | GizmoMode::Scale) {
                    for handle in [
                        GizmoHandle::PlaneXY,
                        GizmoHandle::PlaneYZ,
                        GizmoHandle::PlaneZX,
                    ] {
                        let (ai, bi) = handle.plane_axes().expect("planar handle");
                        let normal = cross(axes[ai], axes[bi]);
                        if let Some((hit, _)) = ray_plane(ray, origin, normal) {
                            let local = sub(hit, origin);
                            let (a, b) = (dot(local, axes[ai]), dot(local, axes[bi]));
                            let (lo, hi) = (world_size * 0.18, world_size * 0.46);
                            if a > lo && a < hi && b > lo && b < hi {
                                consider(handle, 0.0);
                            }
                        }
                    }
                }
                for (i, handle) in [GizmoHandle::AxisX, GizmoHandle::AxisY, GizmoHandle::AxisZ]
                    .into_iter()
                    .enumerate()
                {
                    let tip = add(origin, mul(axes[i], world_size));
                    // Start the grab segment away from the centre so the axes do not fight the
                    // centre handle for the same pixels.
                    let base = add(origin, mul(axes[i], world_size * 0.14));
                    let d = ray_segment_distance(ray, base, tip);
                    if d < grab {
                        consider(handle, d);
                    }
                }
                // The centre handle: free-move (translate) or uniform scale.
                let centre = if matches!(self.mode, GizmoMode::Scale) {
                    GizmoHandle::Uniform
                } else {
                    GizmoHandle::Screen
                };
                if ray_point_distance(ray, origin) < world_size * 0.13 + grab {
                    consider(centre, 0.0);
                }
            }
        }
        best.map(|(h, _)| h)
    }

    /// Begin a drag. Records the constraint that is numerically sound **for this camera**, plus the
    /// reference intersection every later update is measured against.
    #[allow(clippy::too_many_arguments)]
    pub fn begin(
        &mut self,
        handle: GizmoHandle,
        ray: &Ray,
        cursor_ndc: [f64; 2],
        origin: Vec3,
        axes: [Vec3; 3],
        world_size: f64,
        camera: &Camera,
        viewport: &Viewport,
    ) -> bool {
        let constraint = self.choose_constraint(handle, ray, origin, axes, camera);
        let origin_ndc = camera
            .world_to_ndc(origin, viewport.aspect())
            .map_or([0.0, 0.0], |n| [n[0], n[1]]);
        // A constraint the pointer-down ray cannot meet is not a usable drag: refusing to start is
        // better than starting one whose first update teleports the object.
        let Some((start_hit, start_param, start_angle)) =
            reference(&constraint, ray, origin, cursor_ndc, origin_ndc)
        else {
            return false;
        };
        let (px, py) = viewport.pixel_in_ndc();
        let start_radius_px = (((cursor_ndc[0] - origin_ndc[0]) / px.max(1e-12)).powi(2)
            + ((cursor_ndc[1] - origin_ndc[1]) / py.max(1e-12)).powi(2))
        .sqrt();
        self.drag = Some(DragState {
            handle,
            mode: self.mode,
            constraint,
            origin,
            axes,
            world_size,
            start_hit,
            start_param,
            start_angle,
            start_radius_px,
            origin_ndc,
            start_ndc: cursor_ndc,
            turns: 0.0,
            last_angle: start_angle,
        });
        true
    }

    fn choose_constraint(
        &self,
        handle: GizmoHandle,
        ray: &Ray,
        origin: Vec3,
        axes: [Vec3; 3],
        _camera: &Camera,
    ) -> DragConstraint {
        let view_normal = neg(ray.direction);
        match (self.mode, handle) {
            (GizmoMode::Rotate, _) => {
                let axis = match handle.axis() {
                    Some(i) => axes[i],
                    None => view_normal, // the view-aligned ring
                };
                // A ring whose plane the ray cannot meet stably is measured in screen space instead.
                if ray_plane(ray, origin, axis).is_some() {
                    DragConstraint::Ring { axis }
                } else {
                    DragConstraint::RingViaScreen {
                        axis,
                        toward_camera: dot(axis, view_normal) >= 0.0,
                    }
                }
            }
            (GizmoMode::Scale, GizmoHandle::Uniform) => DragConstraint::Radial {
                normal: view_normal,
            },
            (_, GizmoHandle::PlaneXY | GizmoHandle::PlaneYZ | GizmoHandle::PlaneZX) => {
                let (ai, bi) = handle.plane_axes().expect("planar handle");
                DragConstraint::Plane {
                    normal: cross(axes[ai], axes[bi]),
                }
            }
            (_, GizmoHandle::AxisX | GizmoHandle::AxisY | GizmoHandle::AxisZ) => {
                let axis = axes[handle.axis().expect("axis handle")];
                // THE degeneracy decision, made once, at pointer-down: is the closest-approach solve
                // well conditioned for this camera? If not, use the view plane for the whole drag.
                // Deciding per frame would switch methods mid-gesture, which is itself a jump.
                if closest_ray_line(ray, origin, axis).is_some() {
                    DragConstraint::Axis { axis }
                } else {
                    DragConstraint::AxisViaViewPlane { axis, view_normal }
                }
            }
            // The view-plane handle, and anything else, drag in the plane facing the viewer.
            (_, GizmoHandle::Screen | GizmoHandle::Uniform) => DragConstraint::Plane {
                normal: view_normal,
            },
        }
    }

    /// Update the drag and return the **total** delta since pointer-down.
    ///
    /// Total, never incremental: that is what makes the result independent of frame rate, and what
    /// lets snapping quantize the whole movement without accumulating rounding error.
    pub fn update(
        &mut self,
        ray: &Ray,
        cursor_ndc: [f64; 2],
        snap: &SnapSettings,
        viewport: &Viewport,
    ) -> TransformDelta {
        let Some(state) = self.drag.as_mut() else {
            return TransformDelta::IDENTITY;
        };
        match state.mode {
            GizmoMode::Translate => translate_delta(state, ray, cursor_ndc, snap),
            GizmoMode::Rotate => rotate_delta(state, ray, cursor_ndc, snap),
            GizmoMode::Scale => scale_delta(state, ray, cursor_ndc, snap, viewport),
        }
    }

    /// End the drag and return the state it held, so the caller can build one undo entry from the
    /// gesture rather than one per frame.
    pub fn end(&mut self) -> Option<DragState> {
        self.drag.take()
    }

    /// Abandon the drag (Escape). The caller restores the transform recorded at pointer-down.
    pub fn cancel(&mut self) -> Option<DragState> {
        self.drag.take()
    }

    /// The gizmo's **drawn** geometry as world-space line segment pairs, with colours. Thin on
    /// purpose — [`Self::hit_test`] is what makes it easy to grab.
    #[must_use]
    pub fn geometry(
        &self,
        origin: Vec3,
        axes: [Vec3; 3],
        world_size: f64,
    ) -> Vec<(Vec3, Vec3, [f32; 3])> {
        let mut out = Vec::new();
        match self.mode {
            GizmoMode::Translate | GizmoMode::Scale => {
                for (i, handle) in [GizmoHandle::AxisX, GizmoHandle::AxisY, GizmoHandle::AxisZ]
                    .into_iter()
                    .enumerate()
                {
                    let color = handle.color();
                    let tip = add(origin, mul(axes[i], world_size));
                    out.push((origin, tip, color));
                    // A tip marker: a cross for translate (arrow-ish), a box tick for scale.
                    let perp = mul(axes[(i + 1) % 3], world_size * 0.07);
                    let perp2 = mul(axes[(i + 2) % 3], world_size * 0.07);
                    out.push((sub(tip, perp), add(tip, perp), color));
                    out.push((sub(tip, perp2), add(tip, perp2), color));
                }
                for handle in [
                    GizmoHandle::PlaneXY,
                    GizmoHandle::PlaneYZ,
                    GizmoHandle::PlaneZX,
                ] {
                    let (ai, bi) = handle.plane_axes().expect("planar handle");
                    let color = handle.color();
                    let (lo, hi) = (world_size * 0.18, world_size * 0.46);
                    let a_lo = mul(axes[ai], lo);
                    let a_hi = mul(axes[ai], hi);
                    let b_lo = mul(axes[bi], lo);
                    let b_hi = mul(axes[bi], hi);
                    out.push((
                        add(add(origin, a_hi), b_lo),
                        add(add(origin, a_hi), b_hi),
                        color,
                    ));
                    out.push((
                        add(add(origin, a_lo), b_hi),
                        add(add(origin, a_hi), b_hi),
                        color,
                    ));
                }
            }
            GizmoMode::Rotate => {
                const SEGMENTS: usize = 64;
                for (i, handle) in [GizmoHandle::AxisX, GizmoHandle::AxisY, GizmoHandle::AxisZ]
                    .into_iter()
                    .enumerate()
                {
                    let color = handle.color();
                    let u = axes[(i + 1) % 3];
                    let v = axes[(i + 2) % 3];
                    let mut prev = add(origin, mul(u, world_size));
                    for k in 1..=SEGMENTS {
                        let t = (k as f64) / (SEGMENTS as f64) * std::f64::consts::TAU;
                        let p = add(
                            origin,
                            add(mul(u, world_size * t.cos()), mul(v, world_size * t.sin())),
                        );
                        out.push((prev, p, color));
                        prev = p;
                    }
                }
            }
        }
        out
    }
}

// ── the reference recorded at pointer-down ───────────────────────────────────────────────────────

fn reference(
    constraint: &DragConstraint,
    ray: &Ray,
    origin: Vec3,
    cursor_ndc: [f64; 2],
    origin_ndc: [f64; 2],
) -> Option<(Vec3, f64, f64)> {
    match constraint {
        DragConstraint::Axis { axis } => {
            let (s, _) = closest_ray_line(ray, origin, *axis)?;
            Some((add(origin, mul(*axis, s)), s, 0.0))
        }
        DragConstraint::AxisViaViewPlane { axis, view_normal } => {
            let (hit, _) = ray_plane(ray, origin, *view_normal)?;
            let s = dot(sub(hit, origin), *axis);
            Some((hit, s, 0.0))
        }
        DragConstraint::Plane { normal } | DragConstraint::Radial { normal } => {
            // A plane the ray cannot meet is not a usable constraint; refusing to start the drag is
            // better than starting one whose first update teleports the object.
            let (hit, _) = ray_plane(ray, origin, *normal)?;
            Some((hit, 0.0, 0.0))
        }
        DragConstraint::Ring { axis } => {
            let (hit, _) = ray_plane(ray, origin, *axis)?;
            let angle = ring_angle(sub(hit, origin), *axis);
            Some((hit, 0.0, angle))
        }
        DragConstraint::RingViaScreen { .. } => {
            let angle = (cursor_ndc[1] - origin_ndc[1]).atan2(cursor_ndc[0] - origin_ndc[0]);
            Some((origin, 0.0, angle))
        }
    }
}

fn translate_delta(
    state: &mut DragState,
    ray: &Ray,
    _cursor_ndc: [f64; 2],
    snap: &SnapSettings,
) -> TransformDelta {
    let translation = match state.constraint {
        DragConstraint::Axis { axis } => {
            let s = closest_ray_line(ray, state.origin, axis).map_or(state.start_param, |(s, _)| s);
            axis_translation(axis, s - state.start_param, snap)
        }
        DragConstraint::AxisViaViewPlane { axis, view_normal } => {
            let s = ray_plane(ray, state.origin, view_normal)
                .map_or(state.start_param, |(hit, _)| {
                    dot(sub(hit, state.origin), axis)
                });
            axis_translation(axis, s - state.start_param, snap)
        }
        DragConstraint::Plane { normal } | DragConstraint::Radial { normal } => {
            // A plane that has become unsolvable holds position rather than snapping the object to
            // the origin — freezing is recoverable, a teleport is not.
            let hit = ray_plane(ray, state.origin, normal).map_or(state.start_hit, |(h, _)| h);
            let raw = sub(hit, state.start_hit);
            match snap.mode {
                SnapMode::Off => raw,
                SnapMode::Relative | SnapMode::Absolute => {
                    crate::snap::snap_vec3(raw, snap.translate)
                }
            }
        }
        DragConstraint::Ring { .. } | DragConstraint::RingViaScreen { .. } => [0.0; 3],
    };
    TransformDelta {
        translation,
        ..TransformDelta::IDENTITY
    }
}

fn axis_translation(axis: Vec3, distance: f64, snap: &SnapSettings) -> Vec3 {
    let d = if snap.is_on() {
        snap_scalar(distance, snap.translate)
    } else {
        distance
    };
    if !d.is_finite() {
        return [0.0; 3];
    }
    mul(axis, d)
}

fn rotate_delta(
    state: &mut DragState,
    ray: &Ray,
    cursor_ndc: [f64; 2],
    snap: &SnapSettings,
) -> TransformDelta {
    let (axis, raw_angle) = match state.constraint {
        DragConstraint::Ring { axis } => {
            let Some((hit, _)) = ray_plane(ray, state.origin, axis) else {
                return current_rotation(state, snap);
            };
            (axis, ring_angle(sub(hit, state.origin), axis))
        }
        DragConstraint::RingViaScreen {
            axis,
            toward_camera,
        } => {
            let a =
                (cursor_ndc[1] - state.origin_ndc[1]).atan2(cursor_ndc[0] - state.origin_ndc[0]);
            // A screen-space angle runs anticlockwise on screen; the world sense flips when the axis
            // points away from the viewer, or the object would rotate the wrong way from behind.
            (axis, if toward_camera { a } else { -a })
        }
        _ => return TransformDelta::IDENTITY,
    };

    // Turn accumulation: an angle read from atan2 lives in (−π, π], so a drag past half a turn wraps
    // and the object springs backwards. Tracking the crossings makes a multi-turn drag work.
    let mut step = raw_angle - state.last_angle;
    if step > std::f64::consts::PI {
        step -= std::f64::consts::TAU;
    } else if step < -std::f64::consts::PI {
        step += std::f64::consts::TAU;
    }
    state.turns += step;
    state.last_angle = raw_angle;

    let total = if snap.is_on() {
        snap_scalar(state.turns, snap.rotate)
    } else {
        state.turns
    };
    TransformDelta {
        rotation: Transform::from_axis_angle(axis, total),
        ..TransformDelta::IDENTITY
    }
}

fn current_rotation(state: &DragState, snap: &SnapSettings) -> TransformDelta {
    let axis = match state.constraint {
        DragConstraint::Ring { axis } | DragConstraint::RingViaScreen { axis, .. } => axis,
        _ => [0.0, 1.0, 0.0],
    };
    let total = if snap.is_on() {
        snap_scalar(state.turns, snap.rotate)
    } else {
        state.turns
    };
    TransformDelta {
        rotation: Transform::from_axis_angle(axis, total),
        ..TransformDelta::IDENTITY
    }
}

fn scale_delta(
    state: &mut DragState,
    ray: &Ray,
    cursor_ndc: [f64; 2],
    snap: &SnapSettings,
    viewport: &Viewport,
) -> TransformDelta {
    let reference = state.world_size.max(1.0e-9);
    let mut scale = [1.0; 3];
    match state.constraint {
        DragConstraint::Axis { axis } => {
            let s = closest_ray_line(ray, state.origin, axis).map_or(state.start_param, |(s, _)| s);
            apply_axis_scale(
                &mut scale,
                state.axes,
                axis,
                1.0 + (s - state.start_param) / reference,
                snap,
            );
        }
        DragConstraint::AxisViaViewPlane { axis, view_normal } => {
            let s = ray_plane(ray, state.origin, view_normal)
                .map_or(state.start_param, |(hit, _)| {
                    dot(sub(hit, state.origin), axis)
                });
            apply_axis_scale(
                &mut scale,
                state.axes,
                axis,
                1.0 + (s - state.start_param) / reference,
                snap,
            );
        }
        DragConstraint::Plane { normal } => {
            // Planar scale: the two spanned axes grow together by the in-plane distance ratio.
            let hit = ray_plane(ray, state.origin, normal).map_or(state.start_hit, |(h, _)| h);
            let f = ratio(
                length(sub(hit, state.origin)),
                length(sub(state.start_hit, state.origin)).max(reference * 0.05),
                snap,
            );
            if let Some((ai, bi)) = state.handle.plane_axes() {
                scale[ai] = f;
                scale[bi] = f;
            } else {
                scale = [f; 3];
            }
        }
        DragConstraint::Radial { .. } => {
            // Uniform scale from the cursor's radial distance to the gizmo centre, in pixels — the
            // one place a screen-space measure is the RIGHT model, because "how far out have I
            // dragged" has no world-space meaning for a uniform scale.
            let (px, py) = viewport.pixel_in_ndc();
            let r = (((cursor_ndc[0] - state.origin_ndc[0]) / px.max(1e-12)).powi(2)
                + ((cursor_ndc[1] - state.origin_ndc[1]) / py.max(1e-12)).powi(2))
            .sqrt();
            let f = ratio(r, state.start_radius_px.max(8.0), snap);
            scale = [f; 3];
        }
        DragConstraint::Ring { .. } | DragConstraint::RingViaScreen { .. } => {}
    }
    TransformDelta {
        scale,
        ..TransformDelta::IDENTITY
    }
}

fn apply_axis_scale(
    scale: &mut Vec3,
    axes: [Vec3; 3],
    axis: Vec3,
    factor: f64,
    snap: &SnapSettings,
) {
    let f = guard_factor(if snap.is_on() {
        snap_scalar(factor, snap.scale)
    } else {
        factor
    });
    // Write the factor onto the gizmo axis it belongs to, so scaling X really is X — the previous
    // implementation collapsed all three handles onto one component, which made two of the three
    // drawn scale handles silent no-ops and non-uniform scale unreachable.
    for (i, a) in axes.iter().enumerate() {
        if dot(*a, axis).abs() > 0.999 {
            scale[i] = f;
            return;
        }
    }
    *scale = [f; 3];
}

fn ratio(current: f64, start: f64, snap: &SnapSettings) -> f64 {
    let raw = if start.abs() < 1.0e-9 {
        1.0
    } else {
        current / start
    };
    guard_factor(if snap.is_on() {
        snap_scalar(raw, snap.scale)
    } else {
        raw
    })
}

/// Keep a scale factor finite and away from zero — a zero factor makes the object's matrix singular
/// and every descendant's world transform NaN.
fn guard_factor(f: f64) -> f64 {
    if !f.is_finite() {
        return 1.0;
    }
    if f.abs() < 1.0e-4 {
        if f < 0.0 {
            -1.0e-4
        } else {
            1.0e-4
        }
    } else {
        f
    }
}

/// The angle of `v` around `axis`, in the plane perpendicular to it.
fn ring_angle(v: Vec3, axis: Vec3) -> f64 {
    let n = normalize_or(axis, [0.0, 1.0, 0.0]);
    // A deterministic reference direction in the ring's plane, so the angle is stable frame to frame.
    let reference = if n[0].abs() < 0.9 {
        cross(n, [1.0, 0.0, 0.0])
    } else {
        cross(n, [0.0, 1.0, 0.0])
    };
    let u = normalize_or(reference, [1.0, 0.0, 0.0]);
    let w = cross(n, u);
    dot(v, w).atan2(dot(v, u))
}

fn ray_point_distance(ray: &Ray, p: Vec3) -> f64 {
    ray.distance_to_point(p)
}

// ── small vector helpers (kept local so the crate's public surface stays plain arrays) ────────────

fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: Vec3, s: f64) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn neg(a: Vec3) -> Vec3 {
    [-a[0], -a[1], -a[2]]
}
fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn length(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}
fn normalize_or(a: Vec3, fallback: Vec3) -> Vec3 {
    let len_sq = dot(a, a);
    if !len_sq.is_finite() || len_sq < epsilon::DIRECTION_LEN_SQ {
        return fallback;
    }
    mul(a, len_sq.sqrt().recip())
}
fn rotate_vec(q: Quat, v: Vec3) -> Vec3 {
    Transform {
        rotation: q,
        ..Transform::IDENTITY
    }
    .transform_vector(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{Projection, Viewport};
    use crate::epsilon::{approx_eq, approx_eq_vec3_tol};

    fn camera_at(eye: Vec3) -> Camera {
        Camera {
            eye,
            target: [0.0; 3],
            up: [0.0, 1.0, 0.0],
            projection: Projection::Perspective {
                fov_y: 55.0_f64.to_radians(),
            },
            near: 0.01,
            far: 1000.0,
        }
    }

    fn viewport() -> Viewport {
        Viewport::from_physical(1920.0, 1080.0, 1.0)
    }

    const WORLD_AXES: [Vec3; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    /// A ray from the camera through a world point, plus that point's NDC — the shape a real pointer
    /// event produces.
    fn aim(c: &Camera, vp: &Viewport, at: Vec3) -> (Ray, [f64; 2]) {
        let ndc = c
            .world_to_ndc(at, vp.aspect())
            .expect("in front of the camera");
        (
            c.ray_through_ndc(ndc[0], ndc[1], vp.aspect()),
            [ndc[0], ndc[1]],
        )
    }

    #[test]
    fn local_space_really_uses_the_objects_axes() {
        // The bug this replaces: the call site passed an identity basis regardless of space, so the
        // UI said "Local" while every handle moved along WORLD axes.
        let mut m = Manipulator::new();
        let c = camera_at([0.0, 0.0, 20.0]);
        let spin = Transform::from_axis_angle([0.0, 1.0, 0.0], 90f64.to_radians());

        m.set_space(GizmoSpace::World);
        let world = m.axes(spin, crate::transform::IDENTITY_QUAT, &c);
        assert!(approx_eq_vec3_tol(world[0], [1.0, 0.0, 0.0], 1e-12, 1e-12));

        m.set_space(GizmoSpace::Local);
        let local = m.axes(spin, crate::transform::IDENTITY_QUAT, &c);
        assert!(
            approx_eq_vec3_tol(local[0], [0.0, 0.0, -1.0], 1e-9, 1e-9),
            "an object turned 90° about Y has its local X along world −Z (got {:?})",
            local[0]
        );

        m.set_space(GizmoSpace::Parent);
        let parent = m.axes(crate::transform::IDENTITY_QUAT, spin, &c);
        assert!(approx_eq_vec3_tol(parent[0], [0.0, 0.0, -1.0], 1e-9, 1e-9));

        m.set_space(GizmoSpace::View);
        let view = m.axes(spin, crate::transform::IDENTITY_QUAT, &c);
        assert!(
            approx_eq_vec3_tol(view[2], [0.0, 0.0, 1.0], 1e-9, 1e-9),
            "view Z faces the viewer"
        );
    }

    #[test]
    fn a_drag_does_not_jump_on_the_first_update() {
        // Pointer-down then a move of exactly zero must produce exactly zero movement. Any
        // implementation that measures against something other than the recorded start hit shows up
        // here as an instant offset.
        let mut m = Manipulator::new();
        let (c, vp) = (camera_at([6.0, 5.0, 14.0]), viewport());
        let origin = [0.0; 3];
        let size = m.world_size(&c, &vp, origin);
        for handle in [
            GizmoHandle::AxisX,
            GizmoHandle::AxisY,
            GizmoHandle::AxisZ,
            GizmoHandle::PlaneXY,
            GizmoHandle::Screen,
        ] {
            let grab_point = match handle.axis() {
                Some(i) => add(origin, mul(WORLD_AXES[i], size * 0.7)),
                None => add(origin, [size * 0.3, size * 0.3, 0.0]),
            };
            let (ray, ndc) = aim(&c, &vp, grab_point);
            assert!(
                m.begin(handle, &ray, ndc, origin, WORLD_AXES, size, &c, &vp),
                "{handle:?}"
            );
            let delta = m.update(&ray, ndc, &SnapSettings::default(), &vp);
            assert!(
                approx_eq_vec3_tol(delta.translation, [0.0; 3], 1e-9, 1e-9),
                "{handle:?} jumped by {:?}",
                delta.translation
            );
            m.end();
        }
    }

    #[test]
    fn axis_translation_moves_exactly_along_that_axis() {
        let mut m = Manipulator::new();
        let (c, vp) = (camera_at([4.0, 6.0, 18.0]), viewport());
        let origin = [0.0; 3];
        let size = m.world_size(&c, &vp, origin);
        let (ray0, ndc0) = aim(&c, &vp, [size * 0.7, 0.0, 0.0]);
        m.begin(
            GizmoHandle::AxisX,
            &ray0,
            ndc0,
            origin,
            WORLD_AXES,
            size,
            &c,
            &vp,
        );
        // Aim at a point 3 units further along X, at the same height.
        let (ray1, ndc1) = aim(&c, &vp, [size * 0.7 + 3.0, 0.0, 0.0]);
        let d = m.update(&ray1, ndc1, &SnapSettings::default(), &vp);
        assert!(
            approx_eq(d.translation[0], 3.0),
            "moved {} along X",
            d.translation[0]
        );
        assert!(
            d.translation[1].abs() < 1e-9 && d.translation[2].abs() < 1e-9,
            "and not at all off-axis: {:?}",
            d.translation
        );
    }

    #[test]
    fn an_axis_pointing_at_the_camera_neither_explodes_nor_reverses() {
        // THE degeneracy case. The camera looks straight down −Z and the user grabs the Z handle,
        // which projects to a single point. A naive closest-approach solve divides by a vanishing
        // determinant: the object shoots off by thousands of units and the direction flips with
        // noise. The requirement is that movement stays bounded and small.
        let mut m = Manipulator::new();
        let (c, vp) = (camera_at([0.0, 0.0, 20.0]), viewport());
        let origin = [0.0; 3];
        let size = m.world_size(&c, &vp, origin);
        let (ray0, ndc0) = aim(&c, &vp, [0.0, 0.0, 0.0]);
        assert!(m.begin(
            GizmoHandle::AxisZ,
            &ray0,
            ndc0,
            origin,
            WORLD_AXES,
            size,
            &c,
            &vp
        ));
        assert!(
            matches!(
                m.drag_state().unwrap().constraint,
                DragConstraint::AxisViaViewPlane { .. }
            ),
            "the degenerate case must be detected at pointer-down, not suffered per frame"
        );
        // Sweep the cursor across a wide arc; the object must stay bounded throughout.
        let mut max_move: f64 = 0.0;
        for k in 0..80 {
            let f = f64::from(k) / 79.0;
            let (nx, ny) = (-0.9 + 1.8 * f, -0.9 + 1.8 * f);
            let ray = c.ray_through_ndc(nx, ny, vp.aspect());
            let d = m.update(&ray, [nx, ny], &SnapSettings::default(), &vp);
            assert!(
                crate::epsilon::all_finite(&d.translation),
                "no NaN at ndc ({nx}, {ny}): {:?}",
                d.translation
            );
            max_move = max_move.max(length(d.translation));
        }
        assert!(
            max_move < size * 8.0,
            "movement stayed bounded ({max_move} world units for a {size}-unit gizmo)"
        );
    }

    #[test]
    fn rotation_accumulates_past_half_a_turn() {
        // atan2 lives in (−π, π]. Without turn accumulation a drag past 180° springs backwards.
        let mut m = Manipulator::new();
        m.set_mode(GizmoMode::Rotate);
        // Camera on +Y looking down, so the XZ ring (about Y) faces us squarely.
        let c = Camera {
            eye: [0.0, 20.0, 0.0],
            target: [0.0; 3],
            up: [0.0, 0.0, -1.0],
            ..camera_at([0.0, 20.0, 0.0])
        };
        let vp = viewport();
        let origin = [0.0; 3];
        let size = m.world_size(&c, &vp, origin);
        let start = [size, 0.0, 0.0];
        let (ray0, ndc0) = aim(&c, &vp, start);
        assert!(m.begin(
            GizmoHandle::AxisY,
            &ray0,
            ndc0,
            origin,
            WORLD_AXES,
            size,
            &c,
            &vp
        ));

        // Walk 270° around the ring in small steps.
        let mut last = TransformDelta::IDENTITY;
        for k in 1..=270 {
            let a = f64::from(k).to_radians();
            let p = [size * a.cos(), 0.0, -size * a.sin()];
            let (ray, ndc) = aim(&c, &vp, p);
            last = m.update(&ray, ndc, &SnapSettings::default(), &vp);
        }
        let turns = m.drag_state().expect("dragging").turns;
        assert!(
            approx_eq_tol_angle(turns.abs(), 270f64.to_radians()),
            "accumulated {}° rather than wrapping",
            turns.to_degrees()
        );
        assert!(!last.is_identity());
    }

    fn approx_eq_tol_angle(a: f64, b: f64) -> bool {
        (a - b).abs() < 2f64.to_radians()
    }

    #[test]
    fn snapping_quantizes_the_total_drag_and_never_accumulates() {
        let mut m = Manipulator::new();
        let (c, vp) = (camera_at([4.0, 6.0, 18.0]), viewport());
        let origin = [0.0; 3];
        let size = m.world_size(&c, &vp, origin);
        let snap = SnapSettings::relative(1.0, 15f64.to_radians(), 0.1);
        let (ray0, ndc0) = aim(&c, &vp, [size * 0.7, 0.0, 0.0]);
        m.begin(
            GizmoHandle::AxisX,
            &ray0,
            ndc0,
            origin,
            WORLD_AXES,
            size,
            &c,
            &vp,
        );

        // Deliver the SAME final position two ways: one big move, and 500 small ones.
        let target = [size * 0.7 + 4.3, 0.0, 0.0];
        let (ray_end, ndc_end) = aim(&c, &vp, target);
        let one_shot = m.update(&ray_end, ndc_end, &snap, &vp);

        for k in 1..=500 {
            let f = f64::from(k) / 500.0;
            let p = [size * 0.7 + 4.3 * f, 0.0, 0.0];
            let (r, n) = aim(&c, &vp, p);
            m.update(&r, n, &snap, &vp);
        }
        let stepped = m.update(&ray_end, ndc_end, &snap, &vp);
        assert_eq!(
            one_shot.translation, stepped.translation,
            "500 small steps and one big step must agree exactly"
        );
        assert!(
            approx_eq(one_shot.translation[0], 4.0),
            "4.3 snapped to 4, got {}",
            one_shot.translation[0]
        );
    }

    #[test]
    fn each_scale_handle_drives_its_own_axis() {
        // Two of the three drawn scale handles used to be silent no-ops, and the third scaled all
        // three axes — which made non-uniform scale unreachable from the viewport entirely.
        let (c, vp) = (camera_at([12.0, 9.0, 16.0]), viewport());
        let origin = [0.0; 3];
        for (handle, index) in [
            (GizmoHandle::AxisX, 0usize),
            (GizmoHandle::AxisY, 1),
            (GizmoHandle::AxisZ, 2),
        ] {
            let mut m = Manipulator::new();
            m.set_mode(GizmoMode::Scale);
            let size = m.world_size(&c, &vp, origin);
            let axis = WORLD_AXES[index];
            let grab = mul(axis, size * 0.8);
            let (ray0, ndc0) = aim(&c, &vp, grab);
            assert!(m.begin(handle, &ray0, ndc0, origin, WORLD_AXES, size, &c, &vp));
            let (ray1, ndc1) = aim(&c, &vp, mul(axis, size * 0.8 + size * 0.5));
            let d = m.update(&ray1, ndc1, &SnapSettings::default(), &vp);
            assert!(
                d.scale[index] > 1.01,
                "{handle:?} scaled its axis: {:?}",
                d.scale
            );
            for other in 0..3 {
                if other != index {
                    assert!(
                        approx_eq(d.scale[other], 1.0),
                        "{handle:?} left axis {other} alone: {:?}",
                        d.scale
                    );
                }
            }
        }
    }

    #[test]
    fn scale_can_never_reach_zero() {
        let mut m = Manipulator::new();
        m.set_mode(GizmoMode::Scale);
        let (c, vp) = (camera_at([10.0, 8.0, 14.0]), viewport());
        let origin = [0.0; 3];
        let size = m.world_size(&c, &vp, origin);
        let (ray0, ndc0) = aim(&c, &vp, [size * 0.8, 0.0, 0.0]);
        m.begin(
            GizmoHandle::AxisX,
            &ray0,
            ndc0,
            origin,
            WORLD_AXES,
            size,
            &c,
            &vp,
        );
        // Drag far past the origin, which naively gives a large negative factor.
        let (ray1, ndc1) = aim(&c, &vp, [-size * 40.0, 0.0, 0.0]);
        let d = m.update(&ray1, ndc1, &SnapSettings::default(), &vp);
        let applied = d.apply(&Transform::IDENTITY, origin, WORLD_AXES);
        assert!(
            crate::transform::mat_inverse(applied.to_matrix()).is_some(),
            "the resulting transform stays invertible: scale {:?}",
            applied.scale
        );
        assert!(applied.scale.iter().all(|s| s.abs() >= epsilon::MIN_SCALE));
    }

    #[test]
    fn hit_geometry_is_easier_to_grab_than_the_drawn_line_but_still_specific() {
        let m = Manipulator::new();
        let (c, vp) = (camera_at([0.0, 0.0, 20.0]), viewport());
        let origin = [0.0; 3];
        let size = m.world_size(&c, &vp, origin);
        let upp = c.world_units_per_pixel(&vp, origin);

        // Six pixels off the X axis: a thin-line test misses, the grab region catches it.
        let near = [size * 0.7, 6.0 * upp, 0.0];
        let (ray, _) = aim(&c, &vp, near);
        assert_eq!(
            m.hit_test(&ray, origin, WORLD_AXES, size, &c, &vp),
            Some(GizmoHandle::AxisX)
        );

        // Forty pixels off, and nothing is grabbed — the tolerance must not swallow the viewport.
        let far = [size * 0.7, 40.0 * upp, 0.0];
        let (ray, _) = aim(&c, &vp, far);
        assert!(m
            .hit_test(&ray, origin, WORLD_AXES, size, &c, &vp)
            .is_none());

        // Between two axes, the nearer one wins rather than whichever was tested first.
        let toward_y = [4.0 * upp, size * 0.7, 0.0];
        let (ray, _) = aim(&c, &vp, toward_y);
        assert_eq!(
            m.hit_test(&ray, origin, WORLD_AXES, size, &c, &vp),
            Some(GizmoHandle::AxisY)
        );
    }

    #[test]
    fn the_gizmo_keeps_its_pixel_size_at_any_distance_and_in_orthographic() {
        let m = Manipulator::new();
        let vp = viewport();
        // Perspective: world size scales with view depth, so the ON-SCREEN size is constant.
        let near = camera_at([0.0, 0.0, 5.0]);
        let far = camera_at([0.0, 0.0, 500.0]);
        let sn = m.world_size(&near, &vp, [0.0; 3]);
        let sf = m.world_size(&far, &vp, [0.0; 3]);
        assert!(
            approx_eq(sf / sn, 100.0),
            "100× the distance ⇒ 100× the world size ({sn} → {sf})"
        );

        // Orthographic: on-screen size does NOT depend on distance, so the world size must not
        // either. A perspective-only formula shrinks the gizmo to nothing in the axis views.
        let ortho = Camera {
            projection: Projection::Orthographic { half_height: 10.0 },
            ..far
        };
        let a = m.world_size(&ortho, &vp, [0.0; 3]);
        let b = m.world_size(&ortho, &vp, [0.0, 0.0, -400.0]);
        assert!(
            approx_eq(a, b),
            "ortho gizmo size is depth-independent ({a} vs {b})"
        );
    }

    #[test]
    fn a_delta_applied_about_a_shared_pivot_moves_a_multi_selection_coherently() {
        // Multi-object transform: one delta, applied about a shared pivot, must swing every object
        // around that pivot rather than spinning each in place.
        let delta = TransformDelta {
            rotation: Transform::from_axis_angle([0.0, 1.0, 0.0], 90f64.to_radians()),
            ..TransformDelta::IDENTITY
        };
        let pivot = [0.0; 3];
        let a = Transform::from_translation([2.0, 0.0, 0.0]);
        let b = Transform::from_translation([4.0, 0.0, 0.0]);
        let ra = delta.apply(&a, pivot, WORLD_AXES);
        let rb = delta.apply(&b, pivot, WORLD_AXES);
        assert!(
            approx_eq_vec3_tol(ra.translation, [0.0, 0.0, -2.0], 1e-9, 1e-9),
            "{:?}",
            ra.translation
        );
        assert!(
            approx_eq_vec3_tol(rb.translation, [0.0, 0.0, -4.0], 1e-9, 1e-9),
            "{:?}",
            rb.translation
        );
        // Each object's own orientation also turns.
        assert!(crate::transform::quat_approx_eq(
            ra.rotation,
            delta.rotation
        ));

        // With each object as its own pivot, they spin in place instead.
        let in_place = delta.apply(&a, a.translation, WORLD_AXES);
        assert!(approx_eq_vec3_tol(
            in_place.translation,
            [2.0, 0.0, 0.0],
            1e-9,
            1e-9
        ));
    }

    #[test]
    fn a_uniform_gizmo_scale_stays_uniform_on_a_rotated_object() {
        // Scaling in WORLD space along the gizmo's axes must not shear a rotated object into a
        // different shape than the handles promised.
        let delta = TransformDelta {
            scale: [2.0; 3],
            ..TransformDelta::IDENTITY
        };
        let object = Transform {
            rotation: Transform::from_axis_angle([0.3, 0.7, 0.2], 41f64.to_radians()),
            scale: [1.0; 3],
            ..Transform::IDENTITY
        };
        let out = delta.apply(&object, [0.0; 3], WORLD_AXES);
        for s in out.scale {
            assert!(approx_eq(s, 2.0), "uniform stays uniform: {:?}", out.scale);
        }
    }

    #[test]
    fn an_identity_delta_reports_itself_so_a_click_makes_no_undo_entry() {
        assert!(TransformDelta::IDENTITY.is_identity());
        assert!(!TransformDelta {
            translation: [0.001, 0.0, 0.0],
            ..TransformDelta::IDENTITY
        }
        .is_identity());
    }

    #[test]
    fn the_drawn_geometry_is_finite_and_centred_on_the_gizmo() {
        for mode in [GizmoMode::Translate, GizmoMode::Rotate, GizmoMode::Scale] {
            let mut m = Manipulator::new();
            m.set_mode(mode);
            let geo = m.geometry([10.0, -4.0, 2.0], WORLD_AXES, 3.0);
            assert!(!geo.is_empty(), "{mode:?} draws something");
            for (a, b, _) in &geo {
                assert!(epsilon::all_finite(a) && epsilon::all_finite(b));
                for p in [a, b] {
                    assert!(
                        length(sub(*p, [10.0, -4.0, 2.0])) <= 3.0 * 1.3,
                        "{mode:?} geometry stays within the gizmo's radius"
                    );
                }
            }
        }
    }
}
