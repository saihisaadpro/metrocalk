//! Ground sketch — the Build sub-engine's outline, drawn **in the viewport at world scale**.
//!
//! The Build sub-engine could already turn a drawn outline into a solid ([`crate::shape_forge`]), but
//! only from a sketch pad inside a panel: a 100×70 canvas at a hard-coded 0.1 m per unit, so **every
//! drawing was at most 10 m × 7 m**, and the solid it became landed on a golden-angle scatter spot
//! rather than anywhere the author had pointed. Neither limit is a tuning knob — a 40 m building
//! footprint could not be expressed, and a footprint drawn *around* something already in the scene
//! could not stay there. This module is the other surface: the outline is a list of **world points**,
//! so its size is the size of the world and its place is where it was drawn.
//!
//! **Everything here is a pure function of the points and the cursor.** The renderer owns the cursor
//! ray and the overlay; the shell owns the commands; this owns the arithmetic — snapping, the metrics
//! the readout shows, and the profile+origin handed to the shape baker. That split is what lets the
//! interesting part be tested without a GPU, a window, or a pointer.
//!
//! **Coordinate contract.** Points are world-space `[x, y, z]` in metres, all sharing one `y` (a
//! sketch is planar by construction — the first point sets the ground and the rest stay level with
//! it). The profile handed to the extruder is `[x, z]` **relative to the polygon's centroid**, which
//! is also the entity's spawn position: the mesh builder rests every shape on `y = 0`, so a centroid
//! origin puts the solid exactly where the outline was drawn and nowhere else.

// EVERY GEOMETRIC QUANTITY HERE IS AN `f32` IN METRES, and that is the renderer's contract, not a
// shortcut: the points this module produces are uploaded as `Instance.center` and handed to the shape
// baker, both of which are `f32`. The one place the arithmetic is wider is the shoelace sum (an area
// is a product of two coordinates, so it loses twice the precision a length does), and it narrows once
// at the boundary — deliberately, not accidentally.
#![allow(clippy::cast_possible_truncation)]

use std::f32::consts::PI;

use serde::{Deserialize, Serialize};

/// Default snap pitch, metres. Fine enough for a doorway, coarse enough that a hand lands on it.
pub const DEFAULT_GRID_M: f32 = 0.25;

/// Angle-lock increment, radians (15°). Every architectural angle a beginner reaches for — 0°, 45°,
/// 90° — is a multiple of it, and it is coarse enough that the lock is a decision rather than a
/// tremor.
const ANGLE_STEP: f32 = PI / 12.0;

/// How close (as a fraction of `tol_m`) a direction must be to a multiple of [`ANGLE_STEP`] before
/// the lock takes it. Below 1.0 so a deliberately oblique wall stays oblique.
const ANGLE_TOL: f32 = 0.55;

/// What decided the point under the cursor. The UI says this word, so a snapped point is a *claim the
/// author can check* rather than a coordinate that happens to be round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapKind {
    /// Nothing snapped — the raw plane hit, rounded to the millimetre.
    Free,
    /// Rounded onto the construction grid.
    Grid,
    /// Landed exactly on a point already in this outline.
    Vertex,
    /// Landed on the FIRST point: taking it closes the loop.
    Close,
    /// The direction from the previous point locked to a multiple of 15°.
    Axis,
}

impl SnapKind {
    /// The word the readout shows. Plain language, no engine vocabulary (`<ux_quality>` 4).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Grid => "grid",
            Self::Vertex => "on a corner",
            Self::Close => "closes the shape",
            Self::Axis => "locked angle",
        }
    }
}

/// Where the next point would land, and why.
#[derive(Clone, Copy, Debug)]
pub struct Snapped {
    pub point: [f32; 3],
    pub kind: SnapKind,
    /// Taking this point would close the outline onto its first point.
    pub closes: bool,
}

/// Round to the millimetre. Every point that leaves this module goes through it, so a readout, an
/// overlay and a baked profile can never disagree in the sixth decimal place.
fn mm(v: f32) -> f32 {
    (v * 1000.0).round() / 1000.0
}

fn dist_xz(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dx, dz) = (a[0] - b[0], a[2] - b[2]);
    dx.hypot(dz)
}

/// Resolve the raw plane hit into the point that would actually be placed.
///
/// The order is the whole design, and it is the order of what the author is trying to *say*:
///
/// 1. **Close the loop.** With three points down, landing near the first one means "finish" — and it
///    must win over the grid, or a first point that is not on the grid becomes unreachable.
/// 2. **Land on a corner.** An existing vertex is an exact place the author already chose.
/// 3. **Lock the angle.** With a previous point, a direction within tolerance of a multiple of 15°
///    snaps to it, and the *length* along that direction snaps to the grid. This is what makes a
///    square corner square and 7.25 m long at the same time.
/// 4. **Land on the grid.**
///
/// `tol_m` is a world-space tolerance the caller derives from the camera distance, so the snap feels
/// the same size on screen whether the outline is a doorway or a runway. `grid_m <= 0` turns the grid
/// off (freehand); the angle lock is independent of it.
#[must_use]
pub fn snap_cursor(
    raw: [f32; 3],
    points: &[[f32; 3]],
    grid_m: f32,
    angle_snap: bool,
    tol_m: f32,
) -> Snapped {
    let y = points.first().map_or(raw[1], |p| p[1]);
    let raw = [raw[0], y, raw[2]];
    let tol = tol_m.max(1.0e-3);

    // 1. Closing the loop outranks everything: it is the only snap that ENDS something.
    if points.len() >= 3 {
        let first = points[0];
        if dist_xz(raw, first) <= tol * 1.75 {
            return Snapped {
                point: first,
                kind: SnapKind::Close,
                closes: true,
            };
        }
    }
    // 2. Any other corner already placed.
    for p in points.iter().skip(1) {
        if dist_xz(raw, *p) <= tol {
            return Snapped {
                point: *p,
                kind: SnapKind::Vertex,
                closes: false,
            };
        }
    }
    // 3. The angle lock, measured from the last point placed.
    if angle_snap {
        if let Some(last) = points.last() {
            let (dx, dz) = (raw[0] - last[0], raw[2] - last[2]);
            let len = dx.hypot(dz);
            if len > 1.0e-4 {
                let angle = dz.atan2(dx);
                let steps = (angle / ANGLE_STEP).round();
                let locked = steps * ANGLE_STEP;
                // The perpendicular distance from the raw hit to the locked ray — the honest measure
                // of "how far off the line is the cursor", independent of how long the segment is.
                let off = (angle - locked).sin().abs() * len;
                if off <= tol * ANGLE_TOL {
                    let snapped_len = if grid_m > 0.0 {
                        (len / grid_m).round().max(1.0) * grid_m
                    } else {
                        len
                    };
                    return Snapped {
                        point: [
                            mm(last[0] + locked.cos() * snapped_len),
                            y,
                            mm(last[2] + locked.sin() * snapped_len),
                        ],
                        kind: SnapKind::Axis,
                        closes: false,
                    };
                }
            }
        }
    }
    // 4. The grid, or nothing.
    if grid_m > 0.0 {
        return Snapped {
            point: [
                mm((raw[0] / grid_m).round() * grid_m),
                y,
                mm((raw[2] / grid_m).round() * grid_m),
            ],
            kind: SnapKind::Grid,
            closes: false,
        };
    }
    Snapped {
        point: [mm(raw[0]), y, mm(raw[2])],
        kind: SnapKind::Free,
        closes: false,
    }
}

/// A point exactly `length_m` from the last one, along the direction the cursor is indicating.
///
/// This is the typed-dimension half of precision drawing: aim roughly, type `7.5`, get a wall that is
/// 7.5 m long to the millimetre. The *direction* still comes from the cursor (angle-locked when the
/// author asked for it), because a length without a direction does not describe a wall.
///
/// `None` when there is no previous point, when the cursor is not indicating a direction, or when the
/// length is not a positive finite number — a refusal the caller turns into a sentence.
#[must_use]
pub fn point_at_length(
    points: &[[f32; 3]],
    cursor: [f32; 3],
    length_m: f32,
    angle_snap: bool,
) -> Option<[f32; 3]> {
    if !length_m.is_finite() || length_m <= 0.0 {
        return None;
    }
    let last = *points.last()?;
    let (dx, dz) = (cursor[0] - last[0], cursor[2] - last[2]);
    let len = dx.hypot(dz);
    if len <= 1.0e-4 {
        return None;
    }
    let mut angle = dz.atan2(dx);
    if angle_snap {
        angle = (angle / ANGLE_STEP).round() * ANGLE_STEP;
    }
    Some([
        mm(last[0] + angle.cos() * length_m),
        last[1],
        mm(last[2] + angle.sin() * length_m),
    ])
}

/// Everything the readout says about the outline so far. Derived, never stored — two representations
/// of one quantity eventually disagree.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Metrics {
    /// Length of the segment being drawn right now (last point → cursor), metres. `0` with no cursor.
    pub segment_m: f32,
    /// Total length of the placed segments, metres — plus the closing edge once it can close.
    pub perimeter_m: f32,
    /// Enclosed area, m². `0` below three points.
    pub area_m2: f32,
    /// Bounding-box extent along world X, metres.
    pub width_m: f32,
    /// Bounding-box extent along world Z, metres.
    pub depth_m: f32,
}

/// Measure the outline. The perimeter includes the closing edge only when there is a shape to close
/// (three points), so the number never claims an edge that is not drawn.
#[must_use]
pub fn metrics(points: &[[f32; 3]], cursor: Option<[f32; 3]>) -> Metrics {
    let mut m = Metrics::default();
    if let (Some(last), Some(c)) = (points.last(), cursor) {
        m.segment_m = mm(dist_xz(*last, c));
    }
    if points.is_empty() {
        return m;
    }
    let (mut lo_x, mut hi_x) = (f32::INFINITY, f32::NEG_INFINITY);
    let (mut lo_z, mut hi_z) = (f32::INFINITY, f32::NEG_INFINITY);
    for p in points {
        lo_x = lo_x.min(p[0]);
        hi_x = hi_x.max(p[0]);
        lo_z = lo_z.min(p[2]);
        hi_z = hi_z.max(p[2]);
    }
    m.width_m = mm(hi_x - lo_x);
    m.depth_m = mm(hi_z - lo_z);
    let mut perimeter = 0.0;
    for w in points.windows(2) {
        perimeter += dist_xz(w[0], w[1]);
    }
    if points.len() >= 3 {
        perimeter += dist_xz(points[points.len() - 1], points[0]);
        // Shoelace, in the XZ plane. Absolute: the author's winding order is not their problem.
        let mut twice = 0.0_f64;
        for i in 0..points.len() {
            let a = points[i];
            let b = points[(i + 1) % points.len()];
            twice += f64::from(a[0]) * f64::from(b[2]) - f64::from(b[0]) * f64::from(a[2]);
        }
        m.area_m2 = mm((twice.abs() * 0.5) as f32);
    }
    m.perimeter_m = mm(perimeter);
    m
}

/// Why an outline cannot become a solid yet — a sentence, or `None` when it can.
#[must_use]
pub fn refusal(points: &[[f32; 3]]) -> Option<String> {
    match points.len() {
        0 => Some("nothing is drawn yet — click on the ground to place the first corner".into()),
        1 => Some("one corner is a point, not a shape — place at least two more".into()),
        2 => Some("two corners are a line, not a shape — place at least one more".into()),
        _ => {
            let m = metrics(points, None);
            (m.area_m2 < 1.0e-4).then(|| {
                "the corners are all on one line, so the outline encloses nothing".to_string()
            })
        }
    }
}

/// The outline as the shape baker wants it: a `[x, z]` profile in the polygon's **own** frame, plus
/// the world position that frame sits at.
///
/// The origin is the bounding-box centre rather than the area centroid, deliberately: it is the point
/// the author sees the shape occupying, it is what the transform gizmo will then be attached to, and
/// it is stable under a concave outline (an L-shaped footprint's area centroid can fall outside the
/// L, which puts the move handle in mid-air). `y` comes from the sketch plane, so the solid's base
/// rests exactly on the ground it was drawn on.
///
/// `None` when [`refusal`] would refuse.
#[must_use]
pub fn profile_and_origin(points: &[[f32; 3]]) -> Option<(Vec<[f64; 2]>, [f32; 3])> {
    if refusal(points).is_some() {
        return None;
    }
    let (mut lo_x, mut hi_x) = (f32::INFINITY, f32::NEG_INFINITY);
    let (mut lo_z, mut hi_z) = (f32::INFINITY, f32::NEG_INFINITY);
    for p in points {
        lo_x = lo_x.min(p[0]);
        hi_x = hi_x.max(p[0]);
        lo_z = lo_z.min(p[2]);
        hi_z = hi_z.max(p[2]);
    }
    let origin = [
        mm(f32::midpoint(lo_x, hi_x)),
        points[0][1],
        mm(f32::midpoint(lo_z, hi_z)),
    ];
    let profile = points
        .iter()
        .map(|p| {
            [
                f64::from(mm(p[0] - origin[0])),
                f64::from(mm(p[2] - origin[2])),
            ]
        })
        .collect();
    Some((profile, origin))
}

/// The live read-model the drawing panel renders: everything about the outline, in one reply, so a
/// click closes its own loop without a second round trip (`<ux_quality>` 1).
// FIVE BOOLS, AND THEY ARE FIVE DIFFERENT FACTS. `struct_excessive_bools` is aimed at a parameter
// bag whose flags should have been an enum; this is a READ-MODEL, and armed / will-close / is-closed /
// angle-lock-on / can-build are independent things the panel draws separately. Collapsing any pair
// into a state enum would make the reply describe a state machine the UI does not have.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    /// The tool is armed and the stage is taking clicks.
    pub active: bool,
    /// The corners placed so far, world metres.
    pub points: Vec<[f32; 3]>,
    /// Where the next corner would land. `None` when the cursor is not over the ground.
    pub cursor: Option<[f32; 3]>,
    /// What decided `cursor`, in plain words ([`SnapKind::label`]); empty when there is no cursor.
    pub snap: String,
    /// Taking the cursor's point would close the outline.
    pub closes: bool,
    /// The author has already closed it: the shape is finished and waiting to be raised.
    pub closed: bool,
    pub segment_m: f32,
    pub perimeter_m: f32,
    pub area_m2: f32,
    pub width_m: f32,
    pub depth_m: f32,
    /// The construction plane's height, metres.
    pub plane_y: f32,
    /// Snap pitch, metres; `0` is freehand.
    pub grid_m: f32,
    pub angle_snap: bool,
    /// `true` when the outline could become a solid right now.
    pub can_build: bool,
    /// One sentence: what to do next, or why the outline cannot be built yet. Never empty.
    pub message: String,
}

impl State {
    /// Assemble the read-model. The message is derived here rather than at each call site, so the
    /// panel, a toast and the status bar cannot end up telling three different stories.
    #[must_use]
    pub fn build(
        active: bool,
        points: Vec<[f32; 3]>,
        cursor: Option<[f32; 3]>,
        snap: Option<(SnapKind, bool)>,
        plane_y: f32,
        grid_m: f32,
        angle_snap: bool,
    ) -> Self {
        let m = metrics(&points, cursor);
        let why = refusal(&points);
        let message = match (&why, points.len()) {
            (Some(reason), _) => reason.clone(),
            (None, n) => format!(
                "{n} corners · {:.2} × {:.2} m · {:.1} m² — click the first corner again to finish, \
                 or raise it now",
                m.width_m, m.depth_m, m.area_m2
            ),
        };
        Self {
            active,
            points,
            cursor,
            snap: snap.map(|(k, _)| k.label().to_string()).unwrap_or_default(),
            closes: snap.is_some_and(|(_, c)| c),
            closed: false,
            segment_m: m.segment_m,
            perimeter_m: m.perimeter_m,
            area_m2: m.area_m2,
            width_m: m.width_m,
            depth_m: m.depth_m,
            plane_y,
            grid_m,
            angle_snap,
            can_build: why.is_none(),
            message,
        }
    }
}

// ============================================================================================
// Tests
// ============================================================================================

#[cfg(test)]
// EXACT, ON PURPOSE, AND THAT IS THE ASSERTION. A snap's whole job is to produce a value that is
// EXACTLY on the grid, exactly on an existing corner, or exactly on the locked ray — so an epsilon
// comparison here would pass for a snap that lands merely NEAR the grid, which is the defect. The
// tolerance-shaped assertions in this module use one where the arithmetic really is approximate
// (a trigonometric length, a shoelace area); these do not, because there the number is a decision.
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn p(x: f32, z: f32) -> [f32; 3] {
        [x, 0.0, z]
    }

    #[test]
    fn the_grid_rounds_both_horizontal_axes_and_leaves_the_ground_alone() {
        let s = snap_cursor([3.31, 2.0, -1.19], &[], 0.25, false, 0.3);
        assert_eq!(s.kind, SnapKind::Grid);
        assert_eq!(s.point, [3.25, 2.0, -1.25]);
        assert!(!s.closes);
    }

    #[test]
    fn a_zero_pitch_grid_is_freehand_not_a_divide_by_zero() {
        let s = snap_cursor([3.3141, 0.0, -1.1928], &[], 0.0, false, 0.3);
        assert_eq!(s.kind, SnapKind::Free);
        // Still quantized to the millimetre, so the overlay and the profile agree.
        assert_eq!(s.point, [3.314, 0.0, -1.193]);
    }

    #[test]
    fn every_point_inherits_the_first_points_ground_so_the_outline_stays_planar() {
        // The plane is set by the first click; a later hit at a different height must not tilt it.
        let pts = [[0.0, 4.5, 0.0]];
        let s = snap_cursor([2.0, -11.0, 2.0], &pts, 0.25, false, 0.3);
        assert!(
            (s.point[1] - 4.5).abs() < 1e-6,
            "y came from the first point"
        );
    }

    #[test]
    fn landing_near_the_first_corner_closes_the_loop_even_off_grid() {
        // A first point deliberately OFF the grid: if the grid outranked the close, this outline
        // could never be finished by pointing at where it started.
        let pts = [p(0.07, 0.03), p(4.0, 0.0), p(4.0, 3.0)];
        let s = snap_cursor([0.2, 0.0, 0.1], &pts, 0.25, false, 0.3);
        assert_eq!(s.kind, SnapKind::Close);
        assert!(s.closes);
        assert_eq!(s.point, pts[0], "it takes the first point EXACTLY");
    }

    #[test]
    fn two_corners_cannot_close_a_loop_yet() {
        let pts = [p(0.0, 0.0), p(4.0, 0.0)];
        let s = snap_cursor([0.05, 0.0, 0.05], &pts, 0.25, false, 0.3);
        assert!(!s.closes, "a line is not a shape");
    }

    #[test]
    fn the_angle_lock_squares_the_corner_and_the_grid_still_sets_the_length() {
        // Aiming 3° off the +X axis, 4.31 m out: the lock straightens it, the grid rounds the length.
        let pts = [p(0.0, 0.0)];
        let a: f32 = 3.0_f32.to_radians();
        let raw = [4.31 * a.cos(), 0.0, 4.31 * a.sin()];
        let s = snap_cursor(raw, &pts, 0.25, true, 0.5);
        assert_eq!(s.kind, SnapKind::Axis);
        assert!(
            s.point[2].abs() < 1e-6,
            "the corner is square: {:?}",
            s.point
        );
        assert!(
            (s.point[0] - 4.25).abs() < 1e-6,
            "length on the grid: {:?}",
            s.point
        );
    }

    #[test]
    fn a_deliberately_oblique_wall_is_left_oblique() {
        // 7.5° off axis at 4 m is 0.52 m of offset — well outside a 0.3 m tolerance.
        let pts = [p(0.0, 0.0)];
        let a: f32 = 7.5_f32.to_radians();
        let raw = [4.0 * a.cos(), 0.0, 4.0 * a.sin()];
        let s = snap_cursor(raw, &pts, 0.25, true, 0.3);
        assert_eq!(s.kind, SnapKind::Grid, "the lock must not eat every angle");
    }

    #[test]
    fn the_angle_lock_can_be_turned_off() {
        let pts = [p(0.0, 0.0)];
        let a: f32 = 3.0_f32.to_radians();
        let raw = [4.31 * a.cos(), 0.0, 4.31 * a.sin()];
        let s = snap_cursor(raw, &pts, 0.25, false, 0.5);
        assert_eq!(s.kind, SnapKind::Grid);
    }

    #[test]
    fn the_snap_tolerance_scales_with_what_the_caller_passes() {
        // The same cursor, 0.4 m from an existing corner: caught by a runway-sized tolerance,
        // ignored by a doorway-sized one. This is what keeps the snap the same SIZE ON SCREEN.
        let pts = [p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0)];
        let raw = [10.4, 0.0, 0.0];
        assert_eq!(
            snap_cursor(raw, &pts, 0.0, false, 0.5).kind,
            SnapKind::Vertex
        );
        assert_eq!(snap_cursor(raw, &pts, 0.0, false, 0.1).kind, SnapKind::Free);
    }

    #[test]
    fn a_typed_length_is_exact_along_the_aimed_direction() {
        let pts = [p(2.0, 2.0)];
        let hit = point_at_length(&pts, [9.13, 0.0, 2.4], 7.5, true).expect("aimed and measurable");
        assert!((hit[0] - 9.5).abs() < 1e-3, "{hit:?}");
        assert!((hit[2] - 2.0).abs() < 1e-3, "the lock squared it: {hit:?}");
        let d = ((hit[0] - 2.0).powi(2) + (hit[2] - 2.0).powi(2)).sqrt();
        assert!((d - 7.5).abs() < 1e-3, "exactly 7.5 m: {d}");
    }

    #[test]
    fn a_typed_length_refuses_what_it_cannot_answer() {
        assert!(
            point_at_length(&[], [1.0, 0.0, 0.0], 3.0, true).is_none(),
            "no last point"
        );
        assert!(
            point_at_length(&[p(0.0, 0.0)], [0.0, 0.0, 0.0], 3.0, true).is_none(),
            "no direction"
        );
        assert!(
            point_at_length(&[p(0.0, 0.0)], [1.0, 0.0, 0.0], 0.0, true).is_none(),
            "no length"
        );
        assert!(point_at_length(&[p(0.0, 0.0)], [1.0, 0.0, 0.0], f32::NAN, true).is_none());
    }

    #[test]
    fn metrics_measure_the_rectangle_they_are_given() {
        let pts = [p(0.0, 0.0), p(8.0, 0.0), p(8.0, 5.0), p(0.0, 5.0)];
        let m = metrics(&pts, Some(p(0.0, 2.0)));
        assert!((m.width_m - 8.0).abs() < 1e-4);
        assert!((m.depth_m - 5.0).abs() < 1e-4);
        assert!((m.area_m2 - 40.0).abs() < 1e-3, "{m:?}");
        assert!(
            (m.perimeter_m - 26.0).abs() < 1e-3,
            "closing edge included: {m:?}"
        );
        assert!(
            (m.segment_m - 3.0).abs() < 1e-4,
            "last corner to cursor: {m:?}"
        );
    }

    #[test]
    fn area_does_not_depend_on_which_way_round_the_outline_was_drawn() {
        let cw = [p(0.0, 0.0), p(8.0, 0.0), p(8.0, 5.0), p(0.0, 5.0)];
        let mut ccw = cw;
        ccw.reverse();
        assert!((metrics(&cw, None).area_m2 - metrics(&ccw, None).area_m2).abs() < 1e-4);
    }

    #[test]
    fn a_two_point_outline_claims_no_closing_edge() {
        let m = metrics(&[p(0.0, 0.0), p(4.0, 0.0)], None);
        assert!((m.perimeter_m - 4.0).abs() < 1e-4, "not 8: {m:?}");
        assert_eq!(m.area_m2, 0.0);
    }

    #[test]
    fn every_shape_too_thin_to_be_a_shape_is_refused_in_words() {
        assert!(refusal(&[]).expect("refused").contains("first corner"));
        assert!(refusal(&[p(0.0, 0.0)]).is_some());
        assert!(refusal(&[p(0.0, 0.0), p(1.0, 0.0)]).is_some());
        // Three COLLINEAR points pass the count and enclose nothing — the case ear-clipping would
        // fail on later, named here instead.
        let flat = refusal(&[p(0.0, 0.0), p(1.0, 0.0), p(2.0, 0.0)]).expect("refused");
        assert!(flat.contains("one line"), "{flat}");
        assert!(refusal(&[p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0)]).is_none());
    }

    #[test]
    fn the_profile_is_centred_on_the_origin_the_solid_is_placed_at() {
        // A footprint 20 m from the world origin: the profile must NOT carry that offset, or the
        // solid is built 20 m across and then placed 20 m away again.
        let pts = [
            [20.0, 3.0, 40.0],
            [28.0, 3.0, 40.0],
            [28.0, 3.0, 45.0],
            [20.0, 3.0, 45.0],
        ];
        let (profile, origin) = profile_and_origin(&pts).expect("a rectangle extrudes");
        assert_eq!(
            origin,
            [24.0, 3.0, 42.5],
            "the box centre, at the drawn ground"
        );
        assert_eq!(
            profile,
            vec![[-4.0, -2.5], [4.0, -2.5], [4.0, 2.5], [-4.0, 2.5]]
        );
        // Round-trip: profile + origin reconstructs the world points exactly.
        for (i, q) in profile.iter().enumerate() {
            assert!((q[0] as f32 + origin[0] - pts[i][0]).abs() < 1e-4);
            assert!((q[1] as f32 + origin[2] - pts[i][2]).abs() < 1e-4);
        }
    }

    #[test]
    fn a_concave_outline_still_gets_an_origin_inside_its_own_bounding_box() {
        // An L: its AREA centroid sits in the notch. The box centre is what the gizmo will hang on.
        let pts = [
            p(0.0, 0.0),
            p(6.0, 0.0),
            p(6.0, 2.0),
            p(2.0, 2.0),
            p(2.0, 6.0),
            p(0.0, 6.0),
        ];
        let (_, origin) = profile_and_origin(&pts).expect("the L extrudes");
        assert_eq!(origin, [3.0, 0.0, 3.0]);
    }

    #[test]
    fn an_outline_that_cannot_be_a_solid_yields_no_profile() {
        assert!(profile_and_origin(&[p(0.0, 0.0), p(1.0, 0.0)]).is_none());
        assert!(profile_and_origin(&[p(0.0, 0.0), p(1.0, 0.0), p(2.0, 0.0)]).is_none());
    }

    #[test]
    fn a_drawing_far_larger_than_the_old_sketch_pad_survives_the_round_trip() {
        // The whole point of drawing in the world: the old canvas capped every outline at 10 m × 7 m.
        let pts = [
            p(-60.0, -40.0),
            p(60.0, -40.0),
            p(60.0, 40.0),
            p(-60.0, 40.0),
        ];
        let m = metrics(&pts, None);
        assert!((m.width_m - 120.0).abs() < 1e-3);
        assert!((m.area_m2 - 9600.0).abs() < 1.0, "{m:?}");
        let (profile, origin) = profile_and_origin(&pts).expect("a 120 m footprint extrudes");
        assert_eq!(origin, [0.0, 0.0, 0.0]);
        assert!(profile.iter().any(|q| (q[0] - 60.0).abs() < 1e-6));
    }
}
