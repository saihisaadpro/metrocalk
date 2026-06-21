//! `metrocalk-gizmo` — the universal translate / rotate / scale manipulator (M9.1 / G1), built from
//! scratch behind the project-owned [`Gizmo`] trait (invariant 5). The PUBLIC surface is our own plain
//! arrays ([`Vec3`]/[`Quat`]/[`Mat4`]) — **glam is an internal math detail that never crosses the trait**
//! (the /physics boundary discipline; CI grep-gated, so no egui/`transform-gizmo`/glam type leaks).
//!
//! The interaction math is small + fully documented + headless-tested. The headline correctness bit is
//! **parent-space write-back** ([`to_local`]): the gizmo acts in WORLD space, but an entity stores its
//! LOCAL transform, so `local = inverse(parent_world) · world` — skipping this is the well-known
//! "scale-in-a-rotated-parent silently wrong" bug (Bevy #24104), which a test here covers head-on.

// Math-heavy crate: x/y/z/w + i/k/u/v are the canonical names, `a`/`b`/`ai`/`bi` are local pairs, the
// precise float constants in tests are clearer un-separated, and step/index → f32 loses no precision here.
#![allow(
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::unreadable_literal,
    clippy::cast_precision_loss,
    clippy::float_cmp // snap/median produce EXACT multiples — exact assert_eq is correct in those tests
)]

mod math;

pub use math::{
    axis_angle, mat_mul, median, pixel_scale, quat_basis, snap_angle, snap_vec3, to_local, Mat4,
    Quat, Ray, Transform, Vec3,
};

use serde::{Deserialize, Serialize};

/// What the gizmo manipulates.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum GizmoMode {
    #[default]
    Translate,
    Rotate,
    Scale,
}

/// The frame the handles align to — world axes or the entity's own (local) axes. Mandatory toggle.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum GizmoSpace {
    #[default]
    World,
    Local,
}

/// Where the gizmo sits + rotates about — the entity origin, or the (multi-)selection bounds centre.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum GizmoPivot {
    #[default]
    Origin,
    Center,
}

/// A pickable handle. Per-axis (arrow/ring/box), planar (2-axis), and screen-space centre — the
/// table-stakes set every engine shares. X=red, Y=green, Z=blue is applied at draw time.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Handle {
    AxisX,
    AxisY,
    AxisZ,
    PlaneXY,
    PlaneYZ,
    PlaneZX,
    /// Screen-space centre: free-move (translate) / uniform (scale) / view-aligned ring (rotate).
    Screen,
}

impl Handle {
    /// The handle's primary axis index (0=X,1=Y,2=Z), or `None` for the screen/planar-ambiguous handles.
    #[must_use]
    pub fn axis(self) -> Option<usize> {
        match self {
            Self::AxisX => Some(0),
            Self::AxisY => Some(1),
            Self::AxisZ => Some(2),
            _ => None,
        }
    }
    /// The universal X/Y/Z colour (red/green/blue); the planar/screen handles use a neutral highlight.
    #[must_use]
    pub fn color(self) -> [f32; 3] {
        match self {
            Self::AxisX | Self::PlaneYZ => [0.9, 0.25, 0.25],
            Self::AxisY | Self::PlaneZX => [0.3, 0.85, 0.3],
            Self::AxisZ | Self::PlaneXY => [0.3, 0.5, 0.95],
            Self::Screen => [0.85, 0.85, 0.5],
        }
    }
}

/// A line vertex of the gizmo's drawn geometry — `pos` in WORLD space + a `color` — the wgpu pass uploads
/// these (the only render coupling; the gizmo never names a GPU type).
#[derive(Clone, Copy, Debug)]
pub struct GizmoVertex {
    pub pos: Vec3,
    pub color: [f32; 3],
}

/// In-flight drag state (recorded at `drag_start`).
#[derive(Clone, Copy)]
struct Drag {
    handle: Handle,
    /// The gizmo origin (world) + axis basis (world or local per `space`) + pixel scale, frozen for the drag.
    origin: Vec3,
    basis: Quat,
    scale: f32,
    /// The entity's transform when the drag began (the delta is applied to this — coalesced into one commit).
    start: Transform,
    /// The ray-hit on the drag reference plane at `drag_start` (the motion is measured from here).
    start_hit: Vec3,
    /// The parameter along the handle axis at start (for axis translate/scale).
    start_param: f32,
}

/// Snap increments (Ctrl-hold) — grid for translate, angle (radians) for rotate, ratio step for scale.
#[derive(Clone, Copy, Debug)]
pub struct SnapConfig {
    pub grid: f32,
    pub angle: f32,
    pub scale_step: f32,
}

impl Default for SnapConfig {
    fn default() -> Self {
        Self {
            grid: 0.5,
            angle: 15.0_f32.to_radians(),
            scale_step: 0.1,
        }
    }
}

/// The project-owned gizmo seam — the one boundary the editor manipulates a transform through. No foreign
/// (egui / glam / `transform-gizmo`) type appears here (invariant 5).
pub trait Gizmo {
    fn mode(&self) -> GizmoMode;
    fn set_mode(&mut self, mode: GizmoMode);
    fn space(&self) -> GizmoSpace;
    fn toggle_space(&mut self);
    fn pivot(&self) -> GizmoPivot;
    fn toggle_pivot(&mut self);
    /// Which handle the `ray` hits, given the gizmo's world `origin`, axis `basis`, and on-screen `scale`.
    fn pick(&self, ray: Ray, origin: Vec3, basis: Quat, scale: f32) -> Option<Handle>;
    /// Begin a drag on `handle`; `current` is the entity's transform now (the delta coalesces onto it).
    fn drag_start(
        &mut self,
        handle: Handle,
        ray: Ray,
        origin: Vec3,
        basis: Quat,
        scale: f32,
        current: Transform,
    );
    /// Update the drag with a new `ray`; returns the entity's new **WORLD** transform (snapped if `snap`).
    /// The caller converts to local via [`to_local`] before committing.
    fn drag_update(&mut self, ray: Ray, snap: bool) -> Transform;
    /// Whether a drag is active.
    fn dragging(&self) -> bool;
    fn drag_end(&mut self);
    /// The gizmo's line geometry (handle segments + colours) for the wgpu overlay pass.
    fn geometry(&self, origin: Vec3, basis: Quat, scale: f32) -> Vec<GizmoVertex>;
}

/// The from-scratch [`Gizmo`] implementation (the only one; the trait lets `transform-gizmo` slot in later
/// without touching call sites).
#[derive(Clone)]
pub struct TransformGizmo {
    mode: GizmoMode,
    space: GizmoSpace,
    pivot: GizmoPivot,
    snap: SnapConfig,
    drag: Option<Drag>,
}

impl Default for TransformGizmo {
    fn default() -> Self {
        Self {
            mode: GizmoMode::Translate,
            space: GizmoSpace::World,
            pivot: GizmoPivot::Origin,
            snap: SnapConfig::default(),
            drag: None,
        }
    }
}

impl TransformGizmo {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    #[must_use]
    pub fn with_snap(mut self, snap: SnapConfig) -> Self {
        self.snap = snap;
        self
    }

    /// The world-space axis directions for the current `space`, given the entity `basis` (its world
    /// rotation). World space ⇒ the global axes; Local space ⇒ the entity's rotated axes.
    fn axes(&self, basis: Quat) -> [Vec3; 3] {
        match self.space {
            GizmoSpace::World => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            GizmoSpace::Local => math::quat_basis(basis),
        }
    }
}

impl Gizmo for TransformGizmo {
    fn mode(&self) -> GizmoMode {
        self.mode
    }
    fn set_mode(&mut self, mode: GizmoMode) {
        self.mode = mode;
    }
    fn space(&self) -> GizmoSpace {
        self.space
    }
    fn toggle_space(&mut self) {
        self.space = match self.space {
            GizmoSpace::World => GizmoSpace::Local,
            GizmoSpace::Local => GizmoSpace::World,
        };
    }
    fn pivot(&self) -> GizmoPivot {
        self.pivot
    }
    fn toggle_pivot(&mut self) {
        self.pivot = match self.pivot {
            GizmoPivot::Origin => GizmoPivot::Center,
            GizmoPivot::Center => GizmoPivot::Origin,
        };
    }

    fn pick(&self, ray: Ray, origin: Vec3, basis: Quat, scale: f32) -> Option<Handle> {
        math::pick(self.mode, ray, origin, self.axes(basis), scale)
    }

    fn drag_start(
        &mut self,
        handle: Handle,
        ray: Ray,
        origin: Vec3,
        basis: Quat,
        scale: f32,
        current: Transform,
    ) {
        let axes = self.axes(basis);
        let (start_hit, start_param) =
            math::drag_reference(self.mode, handle, ray, origin, axes, scale);
        self.drag = Some(Drag {
            handle,
            origin,
            basis,
            scale,
            start: current,
            start_hit,
            start_param,
        });
    }

    fn drag_update(&mut self, ray: Ray, snap: bool) -> Transform {
        let Some(d) = self.drag else {
            return Transform::IDENTITY;
        };
        let axes = self.axes(d.basis);
        math::drag_update(
            self.mode,
            d.handle,
            ray,
            d.origin,
            axes,
            d.scale,
            &d.start,
            d.start_hit,
            d.start_param,
            snap.then_some(self.snap),
        )
    }

    fn dragging(&self) -> bool {
        self.drag.is_some()
    }
    fn drag_end(&mut self) {
        self.drag = None;
    }

    fn geometry(&self, origin: Vec3, basis: Quat, scale: f32) -> Vec<GizmoVertex> {
        math::geometry(self.mode, origin, self.axes(basis), scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOV: f32 = std::f32::consts::FRAC_PI_3; // 60°

    /// A ray from a camera at `cam` through a world point `target` (straight at it).
    fn ray_to(cam: Vec3, target: Vec3) -> Ray {
        let d = [target[0] - cam[0], target[1] - cam[1], target[2] - cam[2]];
        Ray {
            origin: cam,
            dir: d,
        }
    }

    #[test]
    fn pixel_scale_grows_with_camera_distance() {
        // Constant on-screen size ⇒ world size scales linearly with distance (the single-source formula).
        let near = pixel_scale([0.0, 0.0, 5.0], [0.0; 3], FOV, 0.15);
        let far = pixel_scale([0.0, 0.0, 50.0], [0.0; 3], FOV, 0.15);
        assert!(
            far > near * 8.0,
            "10× distance ⇒ ~10× world size ({near} → {far})"
        );
    }

    #[test]
    fn translate_axis_drag_moves_along_that_axis() {
        let mut g = TransformGizmo::new();
        let origin = [0.0; 3];
        let basis = [0.0, 0.0, 0.0, 1.0];
        let scale = 1.0;
        // Camera looking down -Z from above the scene so the X axis is well-presented.
        let cam = [0.0, 2.0, 10.0];
        g.drag_start(
            Handle::AxisX,
            ray_to(cam, [0.0, 0.0, 0.0]),
            origin,
            basis,
            scale,
            Transform::IDENTITY,
        );
        // Move the cursor ray to aim at x=3 → the entity should translate ~+3 along X, ~0 on Y/Z.
        let out = g.drag_update(ray_to(cam, [3.0, 0.0, 0.0]), false);
        assert!(
            (out.translation[0] - 3.0).abs() < 0.2,
            "moved along X (got {})",
            out.translation[0]
        );
        assert!(
            out.translation[1].abs() < 1e-4 && out.translation[2].abs() < 1e-4,
            "no off-axis drift"
        );
    }

    #[test]
    fn rotate_drag_produces_a_rotation_about_the_axis() {
        let mut g = TransformGizmo::new();
        g.set_mode(GizmoMode::Rotate);
        let origin = [0.0; 3];
        let basis = [0.0, 0.0, 0.0, 1.0];
        // Rotate about Y: camera on +Y looking down so the XZ ring faces us.
        let cam = [0.0, 12.0, 0.0];
        g.drag_start(
            Handle::AxisY,
            ray_to(cam, [1.0, 0.0, 0.0]),
            origin,
            basis,
            1.0,
            Transform::IDENTITY,
        );
        let out = g.drag_update(ray_to(cam, [0.0, 0.0, -1.0]), false); // 90° around Y (x→ -z)
                                                                       // The resulting quaternion should be a ~90° rotation about Y (w ≈ cos45, y ≈ ±sin45).
        let (w, y) = (out.rotation[3].abs(), out.rotation[1].abs());
        assert!(
            (w - 0.70710677).abs() < 0.05 && (y - 0.70710677).abs() < 0.05,
            "≈90° about Y (got {:?})",
            out.rotation
        );
    }

    #[test]
    fn scale_uniform_drag_scales_all_axes() {
        let mut g = TransformGizmo::new();
        g.set_mode(GizmoMode::Scale);
        let cam = [0.0, 0.0, 10.0];
        g.drag_start(
            Handle::Screen,
            ray_to(cam, [0.0, 0.0, 0.0]),
            [0.0; 3],
            [0.0, 0.0, 0.0, 1.0],
            1.0,
            Transform::IDENTITY,
        );
        let out = g.drag_update(ray_to(cam, [1.0, 1.0, 0.0]), false); // drag outward
        assert!(
            out.scale[0] > 1.0
                && (out.scale[0] - out.scale[1]).abs() < 1e-4
                && (out.scale[0] - out.scale[2]).abs() < 1e-4,
            "uniform scale (got {:?})",
            out.scale
        );
    }

    #[test]
    fn parent_space_write_back_under_a_rotated_scaled_parent() {
        // THE #24104 trap: a parent rotated 90° about Y + scaled 2×. We set the child's WORLD transform;
        // `to_local` must produce a LOCAL transform that, composed back through the parent, reproduces the
        // world transform exactly — i.e. parent_world · local == world.
        let parent = Transform {
            translation: [5.0, 0.0, 0.0],
            rotation: math::axis_angle([0.0, 1.0, 0.0], std::f32::consts::FRAC_PI_2),
            scale: [2.0, 2.0, 2.0],
        };
        let world_new = Transform {
            translation: [1.0, 2.0, 3.0],
            rotation: math::axis_angle([0.0, 0.0, 1.0], 0.5),
            scale: [1.0, 1.0, 1.0],
        };
        let local = to_local(&world_new, parent.to_matrix());
        // Recompose: parent_world · local should equal world_new (within fp tolerance).
        let recomposed = math::mat_mul(parent.to_matrix(), local.to_matrix());
        let expected = world_new.to_matrix();
        for c in 0..4 {
            for r in 0..4 {
                assert!(
                    (recomposed[c][r] - expected[c][r]).abs() < 1e-4,
                    "parent·local must reproduce world at [{c}][{r}]: {} vs {}",
                    recomposed[c][r],
                    expected[c][r]
                );
            }
        }
    }

    #[test]
    fn snapping_quantizes_translation_and_angle() {
        assert_eq!(snap_vec3([0.62, -0.18, 1.27], 0.5), [0.5, -0.0, 1.5]);
        let inc = 15.0_f32.to_radians();
        let snapped = snap_angle(20.0_f32.to_radians(), inc);
        assert!((snapped - inc).abs() < 1e-5, "20° snaps to 15°");
    }

    #[test]
    fn multi_select_pivots_at_the_median() {
        // bounds midpoint: x ∈ [0,4] → 2, y ∈ [0,6] → 3, z = 0.
        let m = median(&[[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [4.0, 6.0, 0.0]]);
        assert_eq!(m, [2.0, 3.0, 0.0]);
    }

    #[test]
    fn drag_is_coalesced_not_per_frame() {
        // The gizmo holds the START transform across many updates — a 100-step drag yields ONE delta from
        // the original, never accumulating frame-over-frame (the undo-storm guard).
        let mut g = TransformGizmo::new();
        let cam = [0.0, 2.0, 10.0];
        g.drag_start(
            Handle::AxisX,
            ray_to(cam, [0.0; 3]),
            [0.0; 3],
            [0.0, 0.0, 0.0, 1.0],
            1.0,
            Transform::IDENTITY,
        );
        let mut last = Transform::IDENTITY;
        for i in 1..=100u8 {
            last = g.drag_update(ray_to(cam, [f32::from(i) * 0.03, 0.0, 0.0]), false);
        }
        assert!(
            (last.translation[0] - 3.0).abs() < 0.3,
            "final == total drag from start, not summed"
        );
    }
}
