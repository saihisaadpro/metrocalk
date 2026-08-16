//! **Snapping**, and the one distinction that decides whether it drifts.
//!
//! There are two snapping policies and they are not interchangeable:
//!
//! ```text
//! RELATIVE:  final = initial + snap(total_delta)      ← the delta lands on a multiple
//! ABSOLUTE:  final = snap(initial + total_delta)      ← the RESULT lands on a multiple
//! ```
//!
//! Both are legitimate and mature tools offer both: relative preserves an object's existing offset
//! (an object at x = 3.7 moved one grid unit lands at 4.7), absolute aligns things to a shared grid
//! (that object lands at 4.0). What is *never* legitimate is the third form:
//!
//! ```text
//! WRONG:     final = snap(previous_frame + frame_delta)
//! ```
//!
//! Applied per frame, that accumulates: each frame's rounding error becomes the next frame's input,
//! so a slow drag quantizes differently from a fast one and a long drag ends up somewhere neither
//! policy predicts. The defence is structural rather than careful — every function here takes the
//! **total** delta measured from the drag's recorded start, so there is no per-frame state to
//! accumulate into. The manipulator holds the start transform for the whole gesture for the same
//! reason.

use crate::transform::{Quat, Transform, Vec3};

/// Which snapping policy applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SnapMode {
    #[default]
    Off,
    /// `final = initial + snap(total_delta)` — the movement is quantized, the starting offset kept.
    Relative,
    /// `final = snap(initial + total_delta)` — the result lands on the global grid.
    Absolute,
}

/// The increments a snap quantizes to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapSettings {
    pub mode: SnapMode,
    /// World units per translation step.
    pub translate: f64,
    /// **Radians** per rotation step. Degrees appear only in UI strings.
    pub rotate: f64,
    /// Scale-factor step.
    pub scale: f64,
}

impl Default for SnapSettings {
    fn default() -> Self {
        Self {
            mode: SnapMode::Off,
            translate: 0.1,
            rotate: 15.0_f64.to_radians(),
            scale: 0.1,
        }
    }
}

impl SnapSettings {
    /// The common "hold a modifier to snap" configuration.
    #[must_use]
    pub fn relative(translate: f64, rotate_radians: f64, scale: f64) -> Self {
        Self {
            mode: SnapMode::Relative,
            translate,
            rotate: rotate_radians,
            scale,
        }
    }

    #[must_use]
    pub fn is_on(&self) -> bool {
        !matches!(self.mode, SnapMode::Off)
    }

    /// Apply the translation policy. `start` is the transform recorded at pointer-down and `delta` is
    /// the **total** movement since then — never a per-frame increment.
    #[must_use]
    pub fn apply_translation(&self, start: Vec3, delta: Vec3) -> Vec3 {
        match self.mode {
            SnapMode::Off => add(start, delta),
            SnapMode::Relative => add(start, snap_vec3(delta, self.translate)),
            SnapMode::Absolute => snap_vec3(add(start, delta), self.translate),
        }
    }

    /// Apply the translation policy along a single axis, leaving the other components of `delta`
    /// alone. Snapping the whole vector during an axis drag would quantize the two axes the user is
    /// not touching, which reads as the object jumping sideways.
    #[must_use]
    pub fn apply_axis_translation(&self, start: Vec3, axis: Vec3, distance: f64) -> Vec3 {
        match self.mode {
            SnapMode::Off => add(start, scale(axis, distance)),
            SnapMode::Relative => add(start, scale(axis, snap_scalar(distance, self.translate))),
            SnapMode::Absolute => {
                // Project the start onto the axis, snap the resulting coordinate, and put the object
                // back — so an absolute axis drag lands on the grid ALONG THAT AXIS without
                // disturbing the perpendicular offset.
                let along = dot(start, axis);
                let snapped = snap_scalar(along + distance, self.translate);
                add(start, scale(axis, snapped - along))
            }
        }
    }

    /// Apply the rotation policy to a **total** angle in radians.
    #[must_use]
    pub fn apply_angle(&self, start_angle: f64, delta_angle: f64) -> f64 {
        match self.mode {
            SnapMode::Off => start_angle + delta_angle,
            SnapMode::Relative => start_angle + snap_scalar(delta_angle, self.rotate),
            SnapMode::Absolute => snap_scalar(start_angle + delta_angle, self.rotate),
        }
    }

    /// Apply the scale policy to a **total** multiplicative factor.
    ///
    /// Relative snaps the factor (a 1.37× drag becomes 1.4×); absolute snaps the resulting scale (an
    /// object at 0.93 scaled by 1.37 lands on 1.3). Both clamp away from zero, because a zero scale
    /// makes the object's matrix singular and its inverse NaN.
    #[must_use]
    pub fn apply_scale(&self, start: Vec3, factor: Vec3) -> Vec3 {
        let raw = [
            start[0] * factor[0],
            start[1] * factor[1],
            start[2] * factor[2],
        ];
        let out = match self.mode {
            SnapMode::Off => raw,
            SnapMode::Relative => {
                let f = snap_vec3(factor, self.scale);
                [start[0] * f[0], start[1] * f[1], start[2] * f[2]]
            }
            SnapMode::Absolute => snap_vec3(raw, self.scale),
        };
        [
            guard_scale(out[0]),
            guard_scale(out[1]),
            guard_scale(out[2]),
        ]
    }
}

/// Round `value` to the nearest multiple of `step`. A non-positive or non-finite step is a no-op —
/// snapping to "every 0 units" would divide by zero and return NaN.
#[must_use]
pub fn snap_scalar(value: f64, step: f64) -> f64 {
    if !step.is_finite() || step <= 0.0 || !value.is_finite() {
        return value;
    }
    (value / step).round() * step
}

/// Component-wise [`snap_scalar`].
#[must_use]
pub fn snap_vec3(v: Vec3, step: f64) -> Vec3 {
    [
        snap_scalar(v[0], step),
        snap_scalar(v[1], step),
        snap_scalar(v[2], step),
    ]
}

/// Keep a scale component away from zero while preserving an intentional mirror.
fn guard_scale(s: f64) -> f64 {
    if !s.is_finite() {
        return 1.0;
    }
    if s.abs() < crate::epsilon::MIN_SCALE {
        if s < 0.0 {
            -crate::epsilon::MIN_SCALE
        } else {
            crate::epsilon::MIN_SCALE
        }
    } else {
        s
    }
}

fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: Vec3, s: f64) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// A snap target the scene offers — a grid line, a vertex, an edge point, a face point, a pivot.
///
/// One vocabulary for all of them so that adding vertex or face snapping later is a new *producer*
/// rather than a second, parallel snapping implementation with its own arithmetic. The manipulator
/// consumes candidates through this type and does not care where they came from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapCandidate {
    pub position: Vec3,
    pub kind: SnapKind,
    /// Distance from the query point, in world units.
    pub distance: f64,
    /// The object the candidate belongs to (`u64::MAX` for a pure grid/world candidate).
    pub key: u64,
}

/// What a snap candidate is, and — through [`SnapKind::priority`] — how strongly it should win.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SnapKind {
    Grid,
    Surface,
    Edge,
    Pivot,
    Vertex,
}

impl SnapKind {
    /// Higher wins a tie on distance. A vertex is the most specific thing a user can mean, then a
    /// pivot, then an edge, then a surface; the grid is the fallback everything else outranks.
    #[must_use]
    pub fn priority(self) -> u8 {
        match self {
            Self::Grid => 0,
            Self::Surface => 1,
            Self::Edge => 2,
            Self::Pivot => 3,
            Self::Vertex => 4,
        }
    }
}

/// Choose among snap candidates: nearest wins, ties broken by [`SnapKind::priority`], and the final
/// tie broken by `key` so the result is **deterministic** — the same click never yields two answers.
#[must_use]
pub fn best_candidate(candidates: &[SnapCandidate], radius: f64) -> Option<SnapCandidate> {
    candidates
        .iter()
        .filter(|c| c.distance <= radius && crate::epsilon::all_finite(&c.position))
        .copied()
        .reduce(|a, b| {
            // A near-tie on distance is decided by kind, not by float noise or array order.
            let tie = (a.distance - b.distance).abs() <= radius * 1.0e-3;
            let b_wins = if tie {
                match b.kind.priority().cmp(&a.kind.priority()) {
                    std::cmp::Ordering::Greater => true,
                    std::cmp::Ordering::Less => false,
                    std::cmp::Ordering::Equal => b.key < a.key,
                }
            } else {
                b.distance < a.distance
            };
            if b_wins {
                b
            } else {
                a
            }
        })
}

/// Snap a whole transform's translation, for callers that hold a [`Transform`] rather than parts.
#[must_use]
pub fn snap_transform_translation(
    settings: &SnapSettings,
    start: &Transform,
    delta: Vec3,
) -> Transform {
    Transform {
        translation: settings.apply_translation(start.translation, delta),
        ..*start
    }
}

/// Snap a rotation about `axis` by a **total** angle, composed onto `start`.
#[must_use]
pub fn snap_rotation(settings: &SnapSettings, start: Quat, axis: Vec3, total_angle: f64) -> Quat {
    let angle = match settings.mode {
        SnapMode::Off => total_angle,
        // For rotation the two policies coincide in practice: an object's authored orientation is
        // rarely on the increment grid, and users expect "hold to rotate in 15° steps" to mean steps
        // OF THE DRAG. Absolute is offered for parity and behaves the same here because the delta is
        // measured from a zero base.
        SnapMode::Relative | SnapMode::Absolute => snap_scalar(total_angle, settings.rotate),
    };
    crate::transform::quat_mul(Transform::from_axis_angle(axis, angle), start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epsilon::approx_eq;

    #[test]
    fn relative_snapping_preserves_an_off_grid_starting_offset() {
        let s = SnapSettings::relative(1.0, 0.1, 0.1);
        // An object at 3.7 dragged 1.05 units: the DELTA snaps to 1.0, so it lands at 4.7 and keeps
        // its authored 0.7 offset. This is the behaviour that lets a user nudge a deliberately
        // off-grid object without destroying its placement.
        let out = s.apply_translation([3.7, 0.0, 0.0], [1.05, 0.0, 0.0]);
        assert!(approx_eq(out[0], 4.7), "got {}", out[0]);
    }

    #[test]
    fn absolute_snapping_lands_on_the_grid() {
        let s = SnapSettings {
            mode: SnapMode::Absolute,
            ..SnapSettings::relative(1.0, 0.1, 0.1)
        };
        let out = s.apply_translation([3.7, 0.0, 0.0], [1.05, 0.0, 0.0]);
        assert!(approx_eq(out[0], 5.0), "4.75 rounds to 5, got {}", out[0]);
    }

    #[test]
    fn snapping_from_the_start_transform_cannot_accumulate_drift() {
        // THE drift test. Simulate a 10,000-step drag delivered as tiny increments — the shape that
        // makes a per-frame `snap(previous + increment)` implementation wander. Because every call
        // takes the TOTAL delta from the recorded start, the answer depends only on where the pointer
        // ended up, so a slow drag and a fast one agree exactly.
        let s = SnapSettings::relative(0.25, 0.1, 0.1);
        let start = [1.0, 2.0, 3.0];
        let mut result = start;
        let mut total = 0.0;
        for _ in 0..10_000 {
            total += 0.001_37; // an increment deliberately incommensurate with the 0.25 step
            result = s.apply_translation(start, [total, 0.0, 0.0]);
        }
        // 13.7 total movement snaps to 13.75 (55 steps of 0.25).
        assert!(approx_eq(result[0], 1.0 + 13.75), "got {}", result[0]);
        // The one-shot answer for the same total is bit-identical.
        let one_shot = s.apply_translation(start, [total, 0.0, 0.0]);
        assert_eq!(result, one_shot, "10,000 small steps == one big step");
        // And the untouched axes are exactly untouched.
        assert_eq!(result[1], 2.0);
        assert_eq!(result[2], 3.0);
    }

    #[test]
    fn axis_snapping_leaves_the_perpendicular_offset_alone() {
        let s = SnapSettings::relative(1.0, 0.1, 0.1);
        // Dragging along X must not quantize the object's Y and Z, which the user is not touching.
        let out = s.apply_axis_translation([0.3, 7.77, -2.22], [1.0, 0.0, 0.0], 2.4);
        assert!(
            approx_eq(out[0], 2.3),
            "the X delta snapped to 2, got {}",
            out[0]
        );
        assert_eq!(out[1], 7.77, "Y untouched");
        assert_eq!(out[2], -2.22, "Z untouched");

        // Absolute along an axis lands the AXIS COORDINATE on the grid, still without touching the
        // perpendicular offset.
        let abs = SnapSettings {
            mode: SnapMode::Absolute,
            ..s
        };
        let out = abs.apply_axis_translation([0.3, 7.77, -2.22], [1.0, 0.0, 0.0], 2.4);
        assert!(
            approx_eq(out[0], 3.0),
            "0.3 + 2.4 = 2.7 → 3, got {}",
            out[0]
        );
        assert_eq!(out[1], 7.77);
    }

    #[test]
    fn angle_snapping_quantizes_the_drag_not_the_frame() {
        let s = SnapSettings::relative(1.0, 15.0_f64.to_radians(), 0.1);
        let start = 0.3; // an arbitrary authored orientation
        let out = s.apply_angle(start, 20.0_f64.to_radians());
        assert!(
            approx_eq(out, start + 15.0_f64.to_radians()),
            "20° of drag snaps to 15°, added to the authored 0.3 rad"
        );
        // Accumulating in small increments gives the identical answer.
        let mut total = 0.0;
        let mut last = start;
        for _ in 0..2000 {
            total += 20.0_f64.to_radians() / 2000.0;
            last = s.apply_angle(start, total);
        }
        assert_eq!(last, out);
    }

    #[test]
    fn scale_snapping_never_produces_a_singular_transform() {
        let s = SnapSettings::relative(1.0, 0.1, 0.25);
        // A drag toward zero must not land ON zero: a zero scale makes the matrix singular, its
        // inverse NaN, and every descendant's world transform NaN with it.
        let out = s.apply_scale([1.0; 3], [0.01, 0.01, 0.01]);
        assert!(
            out.iter().all(|v| v.abs() >= crate::epsilon::MIN_SCALE),
            "got {out:?}"
        );
        assert!(crate::transform::mat_inverse(
            Transform {
                scale: out,
                ..Transform::IDENTITY
            }
            .to_matrix()
        )
        .is_some());
        // A mirror survives snapping.
        let mirrored = s.apply_scale([-2.0, 1.0, 1.0], [1.0; 3]);
        assert!(mirrored[0] < 0.0, "the mirror is preserved: {mirrored:?}");
        // And a genuine snap still happens: 1.37× on a 1.0 start with a 0.25 step ⇒ 1.25.
        let snapped = s.apply_scale([1.0; 3], [1.37, 1.37, 1.37]);
        assert!(approx_eq(snapped[0], 1.25), "got {}", snapped[0]);
    }

    #[test]
    fn a_zero_or_nonsense_step_is_a_no_op_rather_than_a_nan() {
        assert!(approx_eq(snap_scalar(3.7, 0.0), 3.7));
        assert!(approx_eq(snap_scalar(3.7, -1.0), 3.7));
        assert!(approx_eq(snap_scalar(3.7, f64::NAN), 3.7));
        assert!(
            snap_scalar(f64::NAN, 1.0).is_nan(),
            "a NaN input stays a NaN, not a fake 0"
        );
        let s = SnapSettings {
            mode: SnapMode::Relative,
            translate: 0.0,
            ..SnapSettings::default()
        };
        let out = s.apply_translation([1.0; 3], [0.37, 0.0, 0.0]);
        assert!(
            approx_eq(out[0], 1.37),
            "a zero step must not swallow the movement"
        );
    }

    #[test]
    fn candidate_selection_is_deterministic_under_ties() {
        // Two candidates at exactly the same distance must not depend on array order — a click that
        // sometimes snaps to a vertex and sometimes to a grid line is worse than no snapping.
        let a = SnapCandidate {
            position: [1.0, 0.0, 0.0],
            kind: SnapKind::Grid,
            distance: 0.5,
            key: 9,
        };
        let b = SnapCandidate {
            position: [0.0, 1.0, 0.0],
            kind: SnapKind::Vertex,
            distance: 0.5,
            key: 3,
        };
        assert_eq!(best_candidate(&[a, b], 1.0).unwrap().kind, SnapKind::Vertex);
        assert_eq!(
            best_candidate(&[b, a], 1.0).unwrap().kind,
            SnapKind::Vertex,
            "order-independent"
        );

        // Same kind and distance ⇒ the lower key wins, in either order.
        let c = SnapCandidate { key: 7, ..b };
        assert_eq!(best_candidate(&[b, c], 1.0).unwrap().key, 3);
        assert_eq!(best_candidate(&[c, b], 1.0).unwrap().key, 3);

        // Genuinely nearer beats higher priority.
        let near_grid = SnapCandidate { distance: 0.1, ..a };
        assert_eq!(
            best_candidate(&[near_grid, b], 1.0).unwrap().kind,
            SnapKind::Grid
        );
        // Outside the radius ⇒ nothing.
        assert!(best_candidate(&[a, b], 0.1).is_none());
        assert!(best_candidate(&[], 1.0).is_none());
    }
}
