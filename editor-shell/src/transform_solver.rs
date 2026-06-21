//! M9.4 (G4) — the intent-powered transform solver: **the reveal/rank/explain engine applied to SPACE.**
//! Selecting *where* something goes becomes "declare the spatial relationship that must hold; the engine
//! wires it, every 'no' explained" — the same move as binding-by-intent (M3.1) + describe-to-create
//! (M3.2), now on transforms. This module is the **deterministic core** (the live wiring + the
//! AI-as-constraint-compiler + physics-aware feedback layer on top):
//!
//! - **Semantic snapping** ([`snap_candidates`]) — a snap-graph of meaningful targets ranked by the
//!   **shared [`crate::reveal::intent_order`]** (proximity·affinity·recency), with a ghost position + an
//!   explained "why this". It REUSES the ADR-011 ranker — never a parallel heuristic (the adversarial guard).
//! - **A pragmatic constraint palette** ([`Constraint`] / [`solve`]) — align-to-surface, snap-to-point,
//!   coplanar, coaxial, clearance, symmetry — each a **deterministic** geometric solve; a blocked one
//!   **explains itself** ([`Blocked`], ADR-016). NOT a parametric CAD solver (the scope fence).
//! - **Screw / hinge / orbit presets** ([`MotionPreset`] / [`constrain_drag`]) — 6-DoF motion as
//!   constraint presets. The **"4D" trajectory gizmo is DEFERRED** (it needs an animation/timeline tier —
//!   the M8.4 sim-replay seam).
//!
//! Everything is deterministic (same inputs → same result — the M8.1 cross-platform path for results that
//! feed gameplay) and pure (no glam past the gizmo's plain-array boundary; no per-frame alloc) — so the
//! solved [`Transform`] commits through the **one** transform pipeline (M9.1 `set_transform` / M9.2
//! `set_part_local`), undoable.

// The `recency` map is app-owned + default-hasher (the same one the reveal feeds); generalizing the
// snap-graph query over the hasher `S` adds noise for no caller benefit (mirrors `reveal.rs`).
#![allow(clippy::implicit_hasher)]

use std::collections::HashMap;

use metrocalk_ecs::Entity;
use metrocalk_gizmo::{axis_angle, Quat, Transform, Vec3};

use crate::reveal::intent_order;

// ── tiny plain-array vector math (glam stays behind the gizmo trait; editor-shell/src is glam-free) ────

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn length(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}
fn dist(a: Vec3, b: Vec3) -> f32 {
    length(sub(a, b))
}
/// Normalize, or `None` if (near-)zero-length — the degenerate cases each constraint reports as blocked.
fn normalize(a: Vec3) -> Option<Vec3> {
    let len = length(a);
    if len < 1e-6 {
        None
    } else {
        Some([a[0] / len, a[1] / len, a[2] / len])
    }
}

// ── semantic snapping (deliverable 1 — reuse the ADR-011 ranker) ──────────────────────────────────────

/// A meaningful snap-target's kind, carrying its semantic **affinity** for the shared ranker — a socket
/// is a stronger spatial intent than a bare origin, so it wins the affinity tiebreak at equal distance
/// (the snap-graph is the registry/ECS made spatial; this is the per-kind intent weight).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SnapKind {
    Socket,
    Pivot,
    Bone,
    Surface,
    Edge,
    Vertex,
    Collider,
    Origin,
}

impl SnapKind {
    #[must_use]
    pub fn affinity(self) -> u32 {
        match self {
            SnapKind::Socket => 7,
            SnapKind::Pivot => 6,
            SnapKind::Bone => 5,
            SnapKind::Surface => 4,
            SnapKind::Edge => 3,
            SnapKind::Vertex => 2,
            SnapKind::Collider => 1,
            SnapKind::Origin => 0,
        }
    }
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            SnapKind::Socket => "socket",
            SnapKind::Pivot => "pivot",
            SnapKind::Bone => "bone",
            SnapKind::Surface => "surface",
            SnapKind::Edge => "edge",
            SnapKind::Vertex => "vertex",
            SnapKind::Collider => "collider",
            SnapKind::Origin => "origin",
        }
    }
}

/// One node of the snap-graph: a meaningful target on some entity, at a world position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapTarget {
    pub entity: Entity,
    pub kind: SnapKind,
    pub position: Vec3,
}

/// A ranked snap candidate — the ghost-preview target + the explained "why this" (the reveal/rank/explain
/// pattern applied to space).
#[derive(Clone, Debug, PartialEq)]
pub struct SnapCandidate {
    pub entity: Entity,
    pub kind: SnapKind,
    pub position: Vec3,
    pub distance: f32,
    /// "snap to the socket of e7 — 0.12 m" — the magnetic-intent "why this snapped" surface.
    pub why: String,
}

/// The **snap-graph query**: rank the targets within `radius` of `from` by the **shared ADR-011 intent
/// ordering** ([`intent_order`]: proximity·affinity·recency·stable-id) — *the same ranker the bind reveal
/// uses*, not a parallel heuristic. Returns ranked candidates each carrying a ghost position + an
/// explained "why this". Deterministic (same scene → same order).
#[must_use]
pub fn snap_candidates(
    targets: &[SnapTarget],
    from: Vec3,
    radius: f32,
    recency: &HashMap<Entity, u64>,
) -> Vec<SnapCandidate> {
    let mut out: Vec<SnapCandidate> = targets
        .iter()
        .filter_map(|t| {
            let d = dist(from, t.position);
            if d > radius {
                return None;
            }
            Some(SnapCandidate {
                entity: t.entity,
                kind: t.kind,
                position: t.position,
                distance: d,
                why: format!(
                    "snap to the {} of {:#x} — {d:.2} m",
                    t.kind.label(),
                    t.entity.0
                ),
            })
        })
        .collect();
    out.sort_by(|a, b| {
        intent_order(
            (
                a.distance,
                a.kind.affinity(),
                recency.get(&a.entity).copied().unwrap_or(0),
                a.entity.0,
            ),
            (
                b.distance,
                b.kind.affinity(),
                recency.get(&b.entity).copied().unwrap_or(0),
                b.entity.0,
            ),
        )
    });
    out
}

// ── the pragmatic constraint palette (deliverable 2 — deterministic, explained) ───────────────────────

/// A declarable spatial relationship — the **pragmatic, deterministic subset** (NOT a parametric CAD
/// solver). Each [`solve`]s to a [`Transform`] deterministically; a blocked one explains itself.
#[derive(Clone, Debug, PartialEq)]
pub enum Constraint {
    /// Snap the entity's origin onto a point (a socket / pivot / vertex from the snap-graph).
    SnapToPoint { target: Vec3 },
    /// Place ON a surface: translate the origin to `point` and orient the entity's local +Y to `normal`.
    AlignToSurface { point: Vec3, normal: Vec3 },
    /// Keep the origin **coplanar** with a plane (point + normal): project the current translation onto it.
    Coplanar { point: Vec3, normal: Vec3 },
    /// Keep the origin **coaxial** with a line (point + direction): project the current translation onto it.
    Coaxial { point: Vec3, dir: Vec3 },
    /// Hold a fixed **clearance** distance from a point, along the current offset direction.
    Clearance { from: Vec3, distance: f32 },
    /// Mirror the origin across a **symmetry** plane (point + normal).
    Symmetry { point: Vec3, normal: Vec3 },
}

/// A constraint that can't hold, with a specific, helpful reason (ADR-016 "every 'no' explained").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blocked {
    pub reason: String,
}

fn blocked(reason: &str) -> Blocked {
    Blocked {
        reason: reason.to_string(),
    }
}

/// Solve a constraint → the entity's new **world** [`Transform`] (deterministic; reuses the gizmo's
/// plain-array math). `Err(Blocked)` — with an explanation — when the constraint is degenerate (a
/// zero-length normal/axis). The result commits through the one transform pipeline (undoable); only the
/// fields a constraint governs change.
///
/// # Errors
/// [`Blocked`] when the constraint is geometrically degenerate (explained).
pub fn solve(constraint: &Constraint, current: &Transform) -> Result<Transform, Blocked> {
    let mut out = *current;
    match constraint {
        Constraint::SnapToPoint { target } => {
            out.translation = *target;
        }
        Constraint::AlignToSurface { point, normal } => {
            let n = normalize(*normal).ok_or_else(|| {
                blocked("surface normal is degenerate (zero-length) — can't align")
            })?;
            out.translation = *point;
            out.rotation = rotation_to_up(n);
        }
        Constraint::Coplanar { point, normal } => {
            let n = normalize(*normal)
                .ok_or_else(|| blocked("plane normal is degenerate — can't keep coplanar"))?;
            // project: p - n·dot(p - point, n)
            let off = dot(sub(current.translation, *point), n);
            out.translation = sub(current.translation, scale(n, off));
        }
        Constraint::Coaxial { point, dir } => {
            let d = normalize(*dir)
                .ok_or_else(|| blocked("axis direction is degenerate — can't keep coaxial"))?;
            // closest point on the line through `point` along `d`
            let t = dot(sub(current.translation, *point), d);
            out.translation = add(*point, scale(d, t));
        }
        Constraint::Clearance { from, distance } => {
            let dir = normalize(sub(current.translation, *from)).unwrap_or([0.0, 1.0, 0.0]);
            out.translation = add(*from, scale(dir, *distance));
        }
        Constraint::Symmetry { point, normal } => {
            let n = normalize(*normal)
                .ok_or_else(|| blocked("mirror plane normal is degenerate — can't mirror"))?;
            let signed = dot(sub(current.translation, *point), n);
            out.translation = sub(current.translation, scale(n, 2.0 * signed));
        }
    }
    Ok(out)
}

/// The residual error of a solved transform against its constraint — `0` when satisfied — for the G1
/// precision-HUD's constraint read-out (deliverable 6). Cheap, deterministic.
#[must_use]
pub fn residual(constraint: &Constraint, t: &Transform) -> f32 {
    match constraint {
        Constraint::SnapToPoint { target } => dist(t.translation, *target),
        Constraint::AlignToSurface { point, .. } => dist(t.translation, *point),
        Constraint::Coplanar { point, normal } => {
            normalize(*normal).map_or(0.0, |n| dot(sub(t.translation, *point), n).abs())
        }
        Constraint::Coaxial { point, dir } => normalize(*dir).map_or(0.0, |d| {
            let s = dot(sub(t.translation, *point), d);
            dist(t.translation, add(*point, scale(d, s)))
        }),
        Constraint::Clearance { from, distance } => (dist(t.translation, *from) - distance).abs(),
        Constraint::Symmetry { .. } => 0.0,
    }
}

// ── screw / hinge / orbit presets (deliverable 3 — 4D deferred) ───────────────────────────────────────

/// A 6-DoF motion preset (theme 5): a free drag restricted to one kind of motion about an axis. The
/// **"4D" trajectory gizmo (a motion field over time) is DEFERRED** — it needs an animation/timeline tier
/// we haven't built (the nearest substrate is the M8.4 sim-replay channel).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MotionPreset {
    #[default]
    Free,
    /// Rotate about a fixed axis only (a door/elbow).
    Hinge,
    /// Couple rotation about an axis with translation along it (a bolt).
    Screw,
    /// Revolve the position around an axis at a fixed radius (an orbit).
    Orbit,
}

/// Restrict a free-drag world transform to a [`MotionPreset`] about `axis` (unit) through `pivot`,
/// relative to the `start` transform. Deterministic; `Free`/degenerate-axis pass the drag through. The
/// `screw_lead` couples translation-per-radian for `Screw`.
#[must_use]
pub fn constrain_drag(
    preset: MotionPreset,
    axis: Vec3,
    pivot: Vec3,
    start: &Transform,
    dragged: &Transform,
    screw_lead: f32,
) -> Transform {
    let Some(a) = normalize(axis) else {
        return *dragged; // degenerate axis → no constraint (free)
    };
    match preset {
        MotionPreset::Free => *dragged,
        MotionPreset::Hinge => {
            // Keep the START position; take only the rotation component about the axis.
            let angle = signed_angle_about(start.translation, dragged.translation, pivot, a);
            Transform {
                translation: start.translation,
                rotation: compose(axis_angle(a, angle), start.rotation),
                scale: start.scale,
            }
        }
        MotionPreset::Screw => {
            let angle = signed_angle_about(start.translation, dragged.translation, pivot, a);
            // translate along the axis by lead·angle (a bolt advancing as it turns)
            let along = scale(a, screw_lead * angle);
            Transform {
                translation: add(start.translation, along),
                rotation: compose(axis_angle(a, angle), start.rotation),
                scale: start.scale,
            }
        }
        MotionPreset::Orbit => {
            // Revolve the START position around the axis by the dragged angle; orientation unchanged.
            let angle = signed_angle_about(start.translation, dragged.translation, pivot, a);
            Transform {
                translation: rotate_point_about(start.translation, pivot, a, angle),
                rotation: start.rotation,
                scale: start.scale,
            }
        }
    }
}

/// The signed angle (about unit `axis` through `pivot`) between the `from`→ and `to`→ radial directions.
fn signed_angle_about(from: Vec3, to: Vec3, pivot: Vec3, axis: Vec3) -> f32 {
    // radial components (perpendicular to the axis)
    let rf = perp(sub(from, pivot), axis);
    let rt = perp(sub(to, pivot), axis);
    let (Some(u), Some(v)) = (normalize(rf), normalize(rt)) else {
        return 0.0;
    };
    let c = dot(u, v).clamp(-1.0, 1.0);
    let s = dot(cross(u, v), axis);
    s.atan2(c)
}

/// The component of `v` perpendicular to unit `axis`.
fn perp(v: Vec3, axis: Vec3) -> Vec3 {
    sub(v, scale(axis, dot(v, axis)))
}

/// Rotate `p` about unit `axis` through `pivot` by `angle` (Rodrigues).
fn rotate_point_about(p: Vec3, pivot: Vec3, axis: Vec3, angle: f32) -> Vec3 {
    let r = sub(p, pivot);
    let (s, c) = angle.sin_cos();
    // r·cos + (axis×r)·sin + axis·(axis·r)·(1-cos)
    let term1 = scale(r, c);
    let term2 = scale(cross(axis, r), s);
    let term3 = scale(axis, dot(axis, r) * (1.0 - c));
    add(pivot, add(add(term1, term2), term3))
}

/// Quaternion multiply `a · b` (xyzw) — composing a correction `a` onto a base `b`.
fn compose(a: Quat, b: Quat) -> Quat {
    let (ax, ay, az, aw) = (a[0], a[1], a[2], a[3]);
    let (bx, by, bz, bw) = (b[0], b[1], b[2], b[3]);
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// The rotation taking local +Y onto unit `n` — the "place upright on a surface" orientation.
fn rotation_to_up(n: Vec3) -> Quat {
    const UP: Vec3 = [0.0, 1.0, 0.0];
    let d = dot(UP, n).clamp(-1.0, 1.0);
    if d > 1.0 - 1e-6 {
        return [0.0, 0.0, 0.0, 1.0]; // already aligned
    }
    if d < -1.0 + 1e-6 {
        return axis_angle([1.0, 0.0, 0.0], std::f32::consts::PI); // antiparallel → flip about X
    }
    let axis = normalize(cross(UP, n)).unwrap_or([1.0, 0.0, 0.0]);
    axis_angle(axis, d.acos())
}

#[cfg(test)]
mod tests {
    // The asserts compare values a constraint COPIES verbatim (SnapToPoint→target, Free→drag,
    // orbit-keeps-rotation) — exact float equality is correct there (no arithmetic to drift).
    #![allow(clippy::float_cmp)]
    use super::*;
    use metrocalk_gizmo::quat_basis;

    fn target(id: u64, kind: SnapKind, pos: Vec3) -> SnapTarget {
        SnapTarget {
            entity: Entity(id),
            kind,
            position: pos,
        }
    }
    fn ident() -> Transform {
        Transform::IDENTITY
    }

    // ── snapping reuses the ADR-011 ranker (proximity·affinity·recency) ───────

    #[test]
    fn snap_ranks_nearest_first_then_affinity_reusing_intent_order() {
        let recency = HashMap::new();
        let targets = [
            target(1, SnapKind::Origin, [10.0, 0.0, 0.0]), // far origin
            target(2, SnapKind::Origin, [1.0, 0.0, 0.0]),  // near origin
            target(3, SnapKind::Socket, [1.0, 0.0, 0.0]),  // near socket (same dist as #2)
        ];
        let ranked = snap_candidates(&targets, [0.0; 3], 20.0, &recency);
        // Proximity is PRIMARY: the two near targets (dist 1) precede the far one (dist 10).
        assert_eq!(ranked[2].entity, Entity(1), "the far target ranks last");
        // At EQUAL distance, higher affinity wins (socket > origin) — the ADR-011 affinity tiebreak.
        assert_eq!(
            ranked[0].entity,
            Entity(3),
            "near socket beats near origin at equal distance"
        );
        assert_eq!(ranked[1].entity, Entity(2));
        // The candidate carries an explained "why this".
        assert!(ranked[0].why.contains("socket"), "why: {}", ranked[0].why);
    }

    #[test]
    fn snap_proximity_dominates_affinity_and_recency_breaks_ties() {
        // A nearer ORIGIN beats a farther SOCKET — distance is primary (ADR-011), not kind.
        let recency = HashMap::new();
        let targets = [
            target(1, SnapKind::Socket, [5.0, 0.0, 0.0]),
            target(2, SnapKind::Origin, [1.0, 0.0, 0.0]),
        ];
        let ranked = snap_candidates(&targets, [0.0; 3], 20.0, &recency);
        assert_eq!(
            ranked[0].entity,
            Entity(2),
            "nearer origin beats the farther socket"
        );

        // Recency breaks an exact distance+affinity tie (more-recent first) — the ADR-011 recency signal.
        let mut rec = HashMap::new();
        rec.insert(Entity(11), 1u64);
        rec.insert(Entity(12), 9u64);
        let tied = [
            target(11, SnapKind::Origin, [2.0, 0.0, 0.0]),
            target(12, SnapKind::Origin, [2.0, 0.0, 0.0]),
        ];
        let ranked = snap_candidates(&tied, [0.0; 3], 20.0, &rec);
        assert_eq!(
            ranked[0].entity,
            Entity(12),
            "more-recently-touched wins the tie"
        );
    }

    #[test]
    fn snap_radius_excludes_far_targets() {
        let recency = HashMap::new();
        let targets = [
            target(1, SnapKind::Origin, [0.5, 0.0, 0.0]),
            target(2, SnapKind::Origin, [50.0, 0.0, 0.0]),
        ];
        let ranked = snap_candidates(&targets, [0.0; 3], 1.0, &recency);
        assert_eq!(ranked.len(), 1, "only the in-radius target");
        assert_eq!(ranked[0].entity, Entity(1));
    }

    // ── the constraint palette: deterministic solves + explained blocks ───────

    #[test]
    fn snap_to_point_moves_the_origin() {
        let out = solve(
            &Constraint::SnapToPoint {
                target: [3.0, 4.0, 5.0],
            },
            &ident(),
        )
        .unwrap();
        assert_eq!(out.translation, [3.0, 4.0, 5.0]);
    }

    #[test]
    fn align_to_surface_places_and_orients_up_to_the_normal() {
        // A 45° surface normal: the entity lands on the point and its local +Y points along the normal.
        let n = [0.0, 0.707_106_77, 0.707_106_77];
        let out = solve(
            &Constraint::AlignToSurface {
                point: [1.0, 2.0, 3.0],
                normal: n,
            },
            &ident(),
        )
        .unwrap();
        assert_eq!(out.translation, [1.0, 2.0, 3.0]);
        let up = quat_basis(out.rotation)[1]; // the entity's local +Y in world
        for k in 0..3 {
            assert!(
                (up[k] - n[k]).abs() < 1e-3,
                "local +Y aligned to the surface normal"
            );
        }
    }

    #[test]
    fn coplanar_and_coaxial_project_onto_plane_and_line() {
        let mut t = ident();
        t.translation = [3.0, 7.0, 2.0];
        // Coplanar with the y=0 plane (normal +Y): y is zeroed, x/z kept.
        let cp = solve(
            &Constraint::Coplanar {
                point: [0.0, 0.0, 0.0],
                normal: [0.0, 1.0, 0.0],
            },
            &t,
        )
        .unwrap();
        assert!((cp.translation[1]).abs() < 1e-4 && (cp.translation[0] - 3.0).abs() < 1e-4);
        // Coaxial with the X axis through the origin: y,z zeroed, x kept.
        let ax = solve(
            &Constraint::Coaxial {
                point: [0.0, 0.0, 0.0],
                dir: [1.0, 0.0, 0.0],
            },
            &t,
        )
        .unwrap();
        assert!((ax.translation[0] - 3.0).abs() < 1e-4);
        assert!(ax.translation[1].abs() < 1e-4 && ax.translation[2].abs() < 1e-4);
    }

    #[test]
    fn clearance_and_symmetry_hold_their_relationships() {
        let mut t = ident();
        t.translation = [3.0, 0.0, 0.0];
        let cl = solve(
            &Constraint::Clearance {
                from: [0.0; 3],
                distance: 2.0,
            },
            &t,
        )
        .unwrap();
        assert!(
            (cl.translation[0] - 2.0).abs() < 1e-4,
            "held 2 m clearance from the origin"
        );
        let sym = solve(
            &Constraint::Symmetry {
                point: [0.0; 3],
                normal: [1.0, 0.0, 0.0],
            },
            &t,
        )
        .unwrap();
        assert!(
            (sym.translation[0] + 3.0).abs() < 1e-4,
            "mirrored across the YZ plane"
        );
    }

    #[test]
    fn a_degenerate_constraint_is_blocked_with_an_explanation() {
        let err = solve(
            &Constraint::AlignToSurface {
                point: [0.0; 3],
                normal: [0.0, 0.0, 0.0], // zero-length
            },
            &ident(),
        )
        .unwrap_err();
        assert!(err.reason.contains("normal"), "explained: {}", err.reason);
    }

    #[test]
    fn solves_are_deterministic() {
        let c = Constraint::AlignToSurface {
            point: [1.0, 2.0, 3.0],
            normal: [0.3, 0.9, 0.1],
        };
        let a = solve(&c, &ident()).unwrap();
        let b = solve(&c, &ident()).unwrap();
        assert_eq!(
            a, b,
            "same inputs → identical solve (deterministic — the M8.1 boundary)"
        );
        assert!(
            residual(&c, &a) < 1e-3,
            "the solved transform satisfies its constraint"
        );
    }

    // ── screw / hinge / orbit presets (4D deferred) ──────────────────────────

    #[test]
    fn hinge_keeps_position_and_orbit_revolves_it() {
        let axis = [0.0, 1.0, 0.0];
        let pivot = [0.0; 3];
        let mut start = ident();
        start.translation = [1.0, 0.0, 0.0];
        // The free drag swung the point a quarter-turn about +Y (x=1 → z direction).
        let mut dragged = ident();
        dragged.translation = [0.0, 0.0, 1.0];

        // HINGE: position pinned at the start; only a rotation about the axis.
        let hinge = constrain_drag(MotionPreset::Hinge, axis, pivot, &start, &dragged, 0.0);
        assert_eq!(
            hinge.translation, start.translation,
            "hinge pins the position"
        );
        assert!(
            hinge.rotation[1].abs() > 0.1,
            "hinge produced a rotation about +Y"
        );

        // ORBIT: the position revolves about the axis (x=1 → ~ +Z at a quarter turn); orientation kept.
        let orbit = constrain_drag(MotionPreset::Orbit, axis, pivot, &start, &dragged, 0.0);
        assert!(
            (orbit.translation[2] - 1.0).abs() < 1e-2 && orbit.translation[0].abs() < 1e-2,
            "orbit revolved the position to +Z, got {:?}",
            orbit.translation
        );
        assert_eq!(
            orbit.rotation, start.rotation,
            "orbit keeps the orientation"
        );

        // SCREW: couples translation along the axis with the turn. The +X→+Z drag is a -90° turn about
        // +Y, so with lead 0.5 the bolt advances lead·angle = 0.5·(-π/2) ≈ -0.785 along the axis.
        let screw = constrain_drag(MotionPreset::Screw, axis, pivot, &start, &dragged, 0.5);
        let expected = 0.5 * -std::f32::consts::FRAC_PI_2;
        assert!(
            (screw.translation[1] - expected).abs() < 0.02,
            "screw coupled translation to the turn (lead·angle), got {}",
            screw.translation[1]
        );

        // Deterministic.
        let again = constrain_drag(MotionPreset::Orbit, axis, pivot, &start, &dragged, 0.0);
        assert_eq!(orbit.translation, again.translation);
    }

    #[test]
    fn free_and_degenerate_axis_pass_the_drag_through() {
        let mut dragged = ident();
        dragged.translation = [9.0, 9.0, 9.0];
        let free = constrain_drag(
            MotionPreset::Free,
            [0.0, 1.0, 0.0],
            [0.0; 3],
            &ident(),
            &dragged,
            0.0,
        );
        assert_eq!(
            free.translation, dragged.translation,
            "Free passes the drag through"
        );
        let degen = constrain_drag(
            MotionPreset::Hinge,
            [0.0; 3],
            [0.0; 3],
            &ident(),
            &dragged,
            0.0,
        );
        assert_eq!(
            degen.translation, dragged.translation,
            "a degenerate axis falls back to free"
        );
    }
}
