//! The gizmo interaction math — glam-backed, but **glam never escapes this module** (the public types are
//! the plain arrays re-exported by `lib.rs`). All of it is small + documented + headless-tested: ray
//! unproject/pick, the three drag deltas, the pixel-constant sizing, snapping, and the parent-space
//! write-back ([`to_local`]) that keeps a child correct under a rotated/scaled parent.

use glam::{Mat4 as GMat4, Quat as GQuat, Vec3 as GVec3};
use serde::{Deserialize, Serialize};

use crate::{GizmoMode, GizmoVertex, Handle, SnapConfig};

/// A 3-vector (world units). Plain array — the gizmo's boundary type.
pub type Vec3 = [f32; 3];
/// A unit quaternion `[x, y, z, w]`.
pub type Quat = [f32; 4];
/// A 4×4 matrix, **column-major** (glam's convention): `m[col][row]`.
pub type Mat4 = [[f32; 4]; 4];

/// A picking / drag ray (camera origin + direction; direction need not be normalized).
#[derive(Clone, Copy, Debug)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

/// A decomposed transform (the entity's TRS) — what a drag produces and what the editor commits.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Transform {
    pub const IDENTITY: Self = Self {
        translation: [0.0; 3],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0; 3],
    };
    #[must_use]
    pub fn to_matrix(&self) -> Mat4 {
        unm(GMat4::from_scale_rotation_translation(
            gv(self.scale),
            gq(self.rotation),
            gv(self.translation),
        ))
    }
    #[must_use]
    pub fn from_matrix(m: Mat4) -> Self {
        let (s, r, t) = gm(m).to_scale_rotation_translation();
        Self {
            translation: t.to_array(),
            rotation: r.to_array(),
            scale: s.to_array(),
        }
    }
}

// ── glam conversions (the firewall: glam types live only inside this module) ─────────────────────────

fn gv(v: Vec3) -> GVec3 {
    GVec3::from_array(v)
}
fn gq(q: Quat) -> GQuat {
    GQuat::from_xyzw(q[0], q[1], q[2], q[3])
}
fn gm(m: Mat4) -> GMat4 {
    GMat4::from_cols_array_2d(&m)
}
fn unm(m: GMat4) -> Mat4 {
    m.to_cols_array_2d()
}

/// `axis`-angle (radians) → our `[x,y,z,w]` quaternion.
#[must_use]
pub fn axis_angle(axis: Vec3, angle: f32) -> Quat {
    GQuat::from_axis_angle(gv(axis).normalize_or_zero(), angle).to_array()
}

/// The three world-space axis directions of a rotation `q` (its local X/Y/Z) — the Local-space gizmo basis.
#[must_use]
pub fn quat_basis(q: Quat) -> [Vec3; 3] {
    let r = gq(q);
    [
        (r * GVec3::X).to_array(),
        (r * GVec3::Y).to_array(),
        (r * GVec3::Z).to_array(),
    ]
}

/// Column-major 4×4 multiply `a · b` (exposed so a test can recompose `parent · local`).
#[must_use]
pub fn mat_mul(a: Mat4, b: Mat4) -> Mat4 {
    unm(gm(a) * gm(b))
}

/// **Parent-space write-back** (M9.1 deliverable 4): the gizmo acts in WORLD space, but the entity stores
/// its LOCAL transform, so `local = inverse(parent_world) · world`. Skipping this is the "scale in a
/// rotated parent silently wrong" bug (Bevy #24104). `parent_world` is the parent's world matrix
/// (identity for a root).
#[must_use]
pub fn to_local(world_new: &Transform, parent_world: Mat4) -> Transform {
    let local = gm(parent_world).inverse() * gm(world_new.to_matrix());
    Transform::from_matrix(unm(local))
}

/// Constant on-screen pixel size: the gizmo's world size scales with camera distance ÷ `tan(fovY/2)`
/// (the single-source formula). `k` is the on-screen fraction; the result is clamped to stay sane.
#[must_use]
pub fn pixel_scale(cam: Vec3, gizmo: Vec3, fov_y: f32, k: f32) -> f32 {
    let dist = (gv(gizmo) - gv(cam)).length();
    (k * dist / (fov_y * 0.5).tan()).clamp(0.01, 1.0e4)
}

/// Snap each component of a translation to the nearest `grid` multiple (Ctrl-hold).
#[must_use]
pub fn snap_vec3(v: Vec3, grid: f32) -> Vec3 {
    if grid <= 0.0 {
        return v;
    }
    [
        (v[0] / grid).round() * grid,
        (v[1] / grid).round() * grid,
        (v[2] / grid).round() * grid,
    ]
}

/// Snap an angle (radians) to the nearest `inc` multiple.
#[must_use]
pub fn snap_angle(a: f32, inc: f32) -> f32 {
    if inc <= 0.0 {
        a
    } else {
        (a / inc).round() * inc
    }
}

/// The median (per-component midpoint of the bounds) of a selection — a multi-select gizmo's pivot.
#[must_use]
pub fn median(points: &[Vec3]) -> Vec3 {
    if points.is_empty() {
        return [0.0; 3];
    }
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    [
        (lo[0] + hi[0]) * 0.5,
        (lo[1] + hi[1]) * 0.5,
        (lo[2] + hi[2]) * 0.5,
    ]
}

// ── ray helpers ──────────────────────────────────────────────────────────────────────────────────────

fn ray_dir(ray: &Ray) -> GVec3 {
    gv(ray.dir).normalize_or_zero()
}

/// Intersect a ray with the plane through `p` with normal `n`. `None` if (near-)parallel or behind.
fn ray_plane(ray: &Ray, p: GVec3, n: GVec3) -> Option<GVec3> {
    let d = ray_dir(ray);
    let denom = n.dot(d);
    if denom.abs() < 1.0e-6 {
        return None;
    }
    let t = n.dot(p - gv(ray.origin)) / denom;
    if t < 0.0 {
        return None;
    }
    Some(gv(ray.origin) + d * t)
}

/// Parameter `s` along the (unit) `axis` from `origin` at the closest approach to `ray` — the heart of
/// axis translate/scale (line-vs-line closest point).
fn closest_param_on_axis(ray: &Ray, origin: GVec3, axis: GVec3) -> f32 {
    let (d1, d2) = (axis.normalize_or_zero(), ray_dir(ray));
    let r = origin - gv(ray.origin);
    let (b, c, dd, e) = (d1.dot(d2), d2.dot(d2), d1.dot(r), d2.dot(r));
    let denom = d1.dot(d1) * c - b * b;
    if denom.abs() < 1.0e-6 {
        0.0
    } else {
        (b * e - c * dd) / denom
    }
}

/// Shortest distance from a ray to a finite segment `[a, b]` — axis-handle picking.
fn ray_segment_dist(ray: &Ray, a: GVec3, b: GVec3) -> f32 {
    let axis = b - a;
    let len = axis.length().max(1.0e-6);
    let s = closest_param_on_axis(ray, a, axis / len).clamp(0.0, len);
    let on_axis = a + (axis / len) * s;
    // distance from that axis point to the ray
    let to = on_axis - gv(ray.origin);
    let d = ray_dir(ray);
    (to - d * to.dot(d)).length()
}

fn ray_point_dist(ray: &Ray, p: GVec3) -> f32 {
    let to = p - gv(ray.origin);
    let d = ray_dir(ray);
    (to - d * to.dot(d)).length()
}

/// The plane normal for a planar handle (the cross of its two axes).
fn plane_normal(axes: [Vec3; 3], handle: Handle) -> GVec3 {
    let (ai, bi) = match handle {
        Handle::PlaneYZ => (1, 2),
        Handle::PlaneZX => (2, 0),
        _ => (0, 1), // PlaneXY (+ any non-planar fallback)
    };
    gv(axes[ai]).cross(gv(axes[bi])).normalize_or_zero()
}

// ── pick ─────────────────────────────────────────────────────────────────────────────────────────────

/// Which handle a ray hits for `mode`, given the gizmo world `origin`, axis `basis`, and on-screen `scale`.
#[must_use]
pub fn pick(
    mode: GizmoMode,
    ray: Ray,
    origin: Vec3,
    axes: [Vec3; 3],
    scale: f32,
) -> Option<Handle> {
    let o = gv(origin);
    let thresh = scale * 0.18;
    let mut best: Option<(Handle, f32)> = None;
    let mut consider = |h: Handle, d: f32| {
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((h, d));
        }
    };

    match mode {
        GizmoMode::Rotate => {
            for (i, h) in [Handle::AxisX, Handle::AxisY, Handle::AxisZ]
                .into_iter()
                .enumerate()
            {
                let n = gv(axes[i]).normalize_or_zero();
                if let Some(hit) = ray_plane(&ray, o, n) {
                    let d = ((hit - o).length() - scale).abs();
                    if d < thresh {
                        consider(h, d);
                    }
                }
            }
        }
        GizmoMode::Translate | GizmoMode::Scale => {
            for (i, h) in [Handle::AxisX, Handle::AxisY, Handle::AxisZ]
                .into_iter()
                .enumerate()
            {
                let axis = gv(axes[i]).normalize_or_zero();
                let d = ray_segment_dist(&ray, o, o + axis * scale);
                if d < thresh {
                    consider(h, d);
                }
            }
            if mode == GizmoMode::Translate {
                for (h, ai, bi) in [
                    (Handle::PlaneXY, 0, 1),
                    (Handle::PlaneYZ, 1, 2),
                    (Handle::PlaneZX, 2, 0),
                ] {
                    let n = gv(axes[ai]).cross(gv(axes[bi])).normalize_or_zero();
                    if let Some(hit) = ray_plane(&ray, o, n) {
                        let local = hit - o;
                        let (a, b) = (local.dot(gv(axes[ai])), local.dot(gv(axes[bi])));
                        let q = scale * 0.45;
                        if a > scale * 0.08 && a < q && b > scale * 0.08 && b < q {
                            consider(h, 0.0);
                        }
                    }
                }
            }
            if ray_point_dist(&ray, o) < thresh {
                consider(Handle::Screen, 0.0);
            }
        }
    }
    best.map(|(h, _)| h)
}

// ── drag ────────────────────────────────────────────────────────────────────────────────────────────

/// The reference hit + axis parameter recorded at `drag_start` — the motion is measured relative to these.
#[must_use]
pub fn drag_reference(
    mode: GizmoMode,
    handle: Handle,
    ray: Ray,
    origin: Vec3,
    axes: [Vec3; 3],
    _scale: f32,
) -> (Vec3, f32) {
    let o = gv(origin);
    match (mode, handle) {
        (
            GizmoMode::Translate | GizmoMode::Scale,
            Handle::AxisX | Handle::AxisY | Handle::AxisZ,
        ) => {
            let axis = gv(axes[handle.axis().unwrap()]).normalize_or_zero();
            let s = closest_param_on_axis(&ray, o, axis);
            ((o + axis * s).to_array(), s)
        }
        (GizmoMode::Translate, Handle::PlaneXY | Handle::PlaneYZ | Handle::PlaneZX) => {
            let hit = ray_plane(&ray, o, plane_normal(axes, handle)).unwrap_or(o);
            (hit.to_array(), 0.0)
        }
        (GizmoMode::Rotate, _) => {
            let n = gv(axes[handle.axis().unwrap_or(1)]).normalize_or_zero();
            let hit = ray_plane(&ray, o, n).unwrap_or(o);
            (hit.to_array(), 0.0)
        }
        (_, Handle::Screen) => {
            // a view-facing plane through the origin (normal = toward the camera)
            let n = -ray_dir(&ray);
            let hit = ray_plane(&ray, o, n).unwrap_or(o);
            (hit.to_array(), (hit - o).length())
        }
        _ => (origin, 0.0),
    }
}

/// Apply a drag update for `mode`/`handle` → the entity's new **WORLD** transform (snapped if `snap`).
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn drag_update(
    mode: GizmoMode,
    handle: Handle,
    ray: Ray,
    origin: Vec3,
    axes: [Vec3; 3],
    scale: f32,
    start: &Transform,
    start_hit: Vec3,
    start_param: f32,
    snap: Option<SnapConfig>,
) -> Transform {
    let o = gv(origin);
    let mut out = *start;
    match mode {
        GizmoMode::Translate => {
            let delta = match handle {
                Handle::AxisX | Handle::AxisY | Handle::AxisZ => {
                    let axis = gv(axes[handle.axis().unwrap()]).normalize_or_zero();
                    let s = closest_param_on_axis(&ray, o, axis);
                    axis * (s - start_param)
                }
                Handle::PlaneXY | Handle::PlaneYZ | Handle::PlaneZX => {
                    let hit =
                        ray_plane(&ray, o, plane_normal(axes, handle)).unwrap_or(gv(start_hit));
                    hit - gv(start_hit)
                }
                Handle::Screen => {
                    let n = -ray_dir(&ray);
                    let hit = ray_plane(&ray, o, n).unwrap_or(gv(start_hit));
                    hit - gv(start_hit)
                }
            };
            let mut t = gv(start.translation) + delta;
            if let Some(s) = snap {
                t = gv(snap_vec3(t.to_array(), s.grid));
            }
            out.translation = t.to_array();
        }
        GizmoMode::Rotate => {
            let i = handle.axis().unwrap_or(1);
            let n = gv(axes[i]).normalize_or_zero();
            let hit = ray_plane(&ray, o, n).unwrap_or(gv(start_hit));
            let (a, b) = (
                (gv(start_hit) - o).normalize_or_zero(),
                (hit - o).normalize_or_zero(),
            );
            if a.length_squared() > 1.0e-8 && b.length_squared() > 1.0e-8 {
                let mut angle = a.cross(b).dot(n).atan2(a.dot(b)); // signed angle about n
                if let Some(s) = snap {
                    angle = snap_angle(angle, s.angle);
                }
                let r = GQuat::from_axis_angle(n, angle) * gq(start.rotation);
                out.rotation = r.normalize().to_array();
            }
        }
        GizmoMode::Scale => {
            let factor_axis = |i: usize| -> f32 {
                let axis = gv(axes[i]).normalize_or_zero();
                let s = closest_param_on_axis(&ray, o, axis);
                1.0 + (s - start_param) / scale.max(1.0e-3)
            };
            let mut sc = start.scale;
            match handle {
                Handle::AxisX | Handle::AxisY | Handle::AxisZ => {
                    let i = handle.axis().unwrap();
                    sc[i] = (start.scale[i] * factor_axis(i)).max(1.0e-3);
                }
                _ => {
                    let n = -ray_dir(&ray);
                    let hit = ray_plane(&ray, o, n).unwrap_or(gv(start_hit));
                    let f =
                        (1.0 + ((hit - o).length() - start_param) / scale.max(1.0e-3)).max(1.0e-3);
                    sc = [start.scale[0] * f, start.scale[1] * f, start.scale[2] * f];
                }
            }
            if let Some(s) = snap {
                let step = s.scale_step.max(1.0e-3);
                sc = [
                    (sc[0] / step).round() * step,
                    (sc[1] / step).round() * step,
                    (sc[2] / step).round() * step,
                ];
            }
            out.scale = sc;
        }
    }
    out
}

// ── drawn geometry ───────────────────────────────────────────────────────────────────────────────────

fn seg(out: &mut Vec<GizmoVertex>, a: GVec3, b: GVec3, color: [f32; 3]) {
    out.push(GizmoVertex {
        pos: a.to_array(),
        color,
    });
    out.push(GizmoVertex {
        pos: b.to_array(),
        color,
    });
}

/// The gizmo's line geometry (handle segments) in WORLD space — axis arrows for translate/scale, three
/// rings for rotate. X=red, Y=green, Z=blue (universal).
#[must_use]
pub fn geometry(mode: GizmoMode, origin: Vec3, axes: [Vec3; 3], scale: f32) -> Vec<GizmoVertex> {
    const RGB: [[f32; 3]; 3] = [[0.9, 0.25, 0.25], [0.3, 0.85, 0.3], [0.3, 0.5, 0.95]];
    let o = gv(origin);
    let mut out = Vec::new();
    match mode {
        GizmoMode::Translate | GizmoMode::Scale => {
            for i in 0..3 {
                let a = gv(axes[i]).normalize_or_zero() * scale;
                seg(&mut out, o, o + a, RGB[i]);
                // a small box/arrow tip marker (a short cross) so scale (box) vs translate (arrow) reads
                let tip = o + a;
                let perp = gv(axes[(i + 1) % 3]).normalize_or_zero() * (scale * 0.06);
                seg(&mut out, tip - perp, tip + perp, RGB[i]);
            }
            // planar corner ticks (translate)
            if matches!(mode, GizmoMode::Translate) {
                for (ai, bi) in [(0, 1), (1, 2), (2, 0)] {
                    let a = gv(axes[ai]).normalize_or_zero() * (scale * 0.3);
                    let b = gv(axes[bi]).normalize_or_zero() * (scale * 0.3);
                    let col = [0.8, 0.8, 0.5];
                    seg(&mut out, o + a, o + a + b, col);
                    seg(&mut out, o + b, o + a + b, col);
                }
            }
        }
        GizmoMode::Rotate => {
            const N: usize = 48;
            for i in 0..3 {
                let u = gv(axes[(i + 1) % 3]).normalize_or_zero();
                let v = gv(axes[(i + 2) % 3]).normalize_or_zero();
                let mut prev = o + u * scale;
                for k in 1..=N {
                    let t = (k as f32) / (N as f32) * std::f32::consts::TAU;
                    let p = o + (u * t.cos() + v * t.sin()) * scale;
                    seg(&mut out, prev, p, RGB[i]);
                    prev = p;
                }
            }
        }
    }
    out
}
