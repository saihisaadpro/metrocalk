//! The `ezpz`-backed [`Solver`] — **THE ONLY module that names an `ezpz::` type** (invariant 5, grep-gated
//! `ezpz::`). Everything else in the workspace drives constraints through the plain-array
//! [`Sketch`]/[`SolveResult`] boundary; if a future `ezpz` bump broke determinism, only this file changes
//! (or a sibling backend replaces it — the ADR-029 escape).

use ezpz::datatypes::inputs::{DatumCircle, DatumDistance, DatumLineSegment, DatumPoint};
use ezpz::datatypes::{Angle, AngleKind};
use ezpz::{solve_analysis, Config, Constraint, ConstraintRequest, Id, IdGenerator, LineSide};

use crate::sketch::{Axis, ConstraintDef, Point, Sketch};
use crate::{SolveResult, Solver, SolverConfig};

/// The embedded `ezpz` solver (Levenberg-Marquardt over faer sparse-LU), wrapped so no `ezpz` type leaks.
#[derive(Clone, Copy, Debug, Default)]
pub struct EzpzSolver {
    cfg: SolverConfig,
}

impl EzpzSolver {
    /// An `ezpz` solver with default tolerances.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An `ezpz` solver with custom tolerances.
    #[must_use]
    pub fn with_config(cfg: SolverConfig) -> Self {
        Self { cfg }
    }
}

impl Solver for EzpzSolver {
    fn solve(&self, sketch: &Sketch) -> SolveResult {
        solve_ezpz(sketch, self.cfg)
    }

    fn deterministic(&self) -> bool {
        // MEASURED (ADR-076 audit): the solve is bit-reproducible same-machine (in-proc AND cross-process,
        // identical hash). The CANONICAL (quantized) solve is cross-ISA-portable by construction; the raw
        // f64 is a derivation, to which the ADR-020 native/wasm float boundary applies — not the canonical.
        true
    }

    fn name(&self) -> &'static str {
        "ezpz-0.2.27 (Levenberg-Marquardt / faer sparse-LU)"
    }
}

// A solve that could not run (malformed sketch / solver error): echo the witness, mark it explained.
fn error_result(sketch: &Sketch, error: String) -> SolveResult {
    SolveResult {
        converged: false,
        satisfied: false,
        points: sketch.points.clone(),
        radii: sketch.circles.iter().map(|c| c.radius).collect(),
        iterations: 0,
        unsatisfied: (0..sketch.constraints.len()).collect(),
        underconstrained_points: Vec::new(),
        underconstrained_radii: Vec::new(),
        error: Some(error),
    }
}

#[allow(clippy::too_many_lines)] // one flat mapping + result assembly; splitting would obscure it
#[allow(clippy::similar_names)] // free_points / free_radii are the natural paired names
fn solve_ezpz(sketch: &Sketch, cfg: SolverConfig) -> SolveResult {
    // Malformed input is EXPLAINED, never a panic (no out-of-range indexing below).
    if let Err(e) = sketch.validate() {
        return error_result(sketch, e.to_string());
    }

    // Empty system: nothing to solve; the witness IS the answer, everything is free.
    if sketch.constraints.is_empty() {
        return SolveResult {
            converged: true,
            satisfied: true,
            points: sketch.points.clone(),
            radii: sketch.circles.iter().map(|c| c.radius).collect(),
            iterations: 0,
            unsatisfied: Vec::new(),
            underconstrained_points: (0..sketch.points.len()).collect(),
            underconstrained_radii: (0..sketch.circles.len()).collect(),
            error: None,
        };
    }

    let mut ids = IdGenerator::default();
    // Points → DatumPoints, in order (point index i owns datum_points[i]).
    let datum_points: Vec<DatumPoint> = sketch
        .points
        .iter()
        .map(|_| DatumPoint::new(&mut ids))
        .collect();
    // Circle radii → DatumDistances; circles → DatumCircle referencing the centre point's datum.
    let datum_dists: Vec<DatumDistance> = sketch
        .circles
        .iter()
        .map(|_| DatumDistance::new(ids.next_id()))
        .collect();
    let datum_circles: Vec<DatumCircle> = sketch
        .circles
        .iter()
        .enumerate()
        .map(|(ci, c)| DatumCircle {
            center: datum_points[c.center],
            radius: datum_dists[ci],
        })
        .collect();

    // The WITNESS: one initial guess per variable (x,y per point; radius per circle). Storing this in the
    // op is the "flip"-fixing mechanism — every peer re-solves from the same start.
    let mut guesses: Vec<(Id, f64)> =
        Vec::with_capacity(sketch.points.len() * 2 + sketch.circles.len());
    let mut id_to_point: Vec<(Id, usize)> = Vec::new();
    for (i, (dp, p)) in datum_points.iter().zip(&sketch.points).enumerate() {
        guesses.push((dp.id_x(), p.x));
        guesses.push((dp.id_y(), p.y));
        id_to_point.push((dp.id_x(), i));
        id_to_point.push((dp.id_y(), i));
    }
    let mut id_to_circle: Vec<(Id, usize)> = Vec::new();
    for (ci, (dd, c)) in datum_dists.iter().zip(&sketch.circles).enumerate() {
        guesses.push((dd.id, c.radius));
        id_to_circle.push((dd.id, ci));
    }

    // Constraints → ezpz requests, ALL at uniform priority 0. Uniform priority means a genuine
    // over-constraint surfaces in `unsatisfied` (rather than being silently dropped by priority-peeling)
    // — the detection the minimal-conflicting-set builds on.
    let reqs: Vec<ConstraintRequest> = sketch
        .constraints
        .iter()
        .map(|c| ConstraintRequest::highest_priority(to_ezpz(c, &datum_points, &datum_circles)))
        .collect();

    let config = Config::default()
        .with_max_iterations(cfg.max_iterations)
        .with_convergence_tolerance(cfg.residual_tolerance);

    match solve_analysis(&reqs, guesses, config) {
        Ok(res) => {
            let outcome = &res.outcome;
            let points: Vec<Point> = datum_points
                .iter()
                .map(|dp| {
                    let p = outcome.final_value_point(dp);
                    Point::new(p.x, p.y)
                })
                .collect();
            let radii: Vec<f64> = datum_dists
                .iter()
                .map(|dd| outcome.final_value_distance(dd))
                .collect();

            // Map underconstrained variable ids → distinct point / circle indices, deterministically.
            let free = res.analysis.underconstrained();
            let mut free_points: Vec<usize> = free
                .iter()
                .filter_map(|id| id_to_point.iter().find(|(k, _)| k == id).map(|(_, i)| *i))
                .collect();
            free_points.sort_unstable();
            free_points.dedup();
            let mut free_radii: Vec<usize> = free
                .iter()
                .filter_map(|id| id_to_circle.iter().find(|(k, _)| k == id).map(|(_, i)| *i))
                .collect();
            free_radii.sort_unstable();
            free_radii.dedup();

            SolveResult {
                converged: outcome.converged(),
                satisfied: outcome.is_satisfied(),
                points,
                radii,
                iterations: outcome.iterations(),
                unsatisfied: outcome.unsatisfied().to_vec(),
                underconstrained_points: free_points,
                underconstrained_radii: free_radii,
                error: None,
            }
        }
        // A solver-level failure (a singular system, an empty subset, etc.) — explained, not a panic.
        Err(fail) => error_result(sketch, fail.error().to_string()),
    }
}

// Map one project-owned constraint to its `ezpz` equivalent. Called only on a VALIDATED sketch, so every
// index is in range (no panic).
fn to_ezpz(c: &ConstraintDef, dp: &[DatumPoint], dc: &[DatumCircle]) -> Constraint {
    let axis_id = |p: usize, a: Axis| match a {
        Axis::X => dp[p].id_x(),
        Axis::Y => dp[p].id_y(),
    };
    let seg = |a: usize, b: usize| DatumLineSegment::new(dp[a], dp[b]);
    match *c {
        ConstraintDef::Fixed { point, axis, value } => {
            Constraint::Fixed(axis_id(point, axis), value)
        }
        ConstraintDef::Coincident { a, b } => Constraint::PointsCoincident(dp[a], dp[b]),
        ConstraintDef::Distance { a, b, d } => Constraint::Distance(dp[a], dp[b], d),
        ConstraintDef::HorizontalDistance { a, b, d } => {
            Constraint::HorizontalDistance(dp[a], dp[b], d)
        }
        ConstraintDef::VerticalDistance { a, b, d } => {
            Constraint::VerticalDistance(dp[a], dp[b], d)
        }
        ConstraintDef::Horizontal { a, b } => Constraint::Horizontal(seg(a, b)),
        ConstraintDef::Vertical { a, b } => Constraint::Vertical(seg(a, b)),
        ConstraintDef::Parallel { a0, a1, b0, b1 } => {
            Constraint::LinesAtAngle(seg(a0, a1), seg(b0, b1), AngleKind::Parallel)
        }
        ConstraintDef::Perpendicular { a0, a1, b0, b1 } => {
            Constraint::LinesAtAngle(seg(a0, a1), seg(b0, b1), AngleKind::Perpendicular)
        }
        ConstraintDef::Angle {
            a0,
            a1,
            b0,
            b1,
            degrees,
        } => Constraint::LinesAtAngle(
            seg(a0, a1),
            seg(b0, b1),
            AngleKind::Other(Angle::from_degrees(degrees)),
        ),
        ConstraintDef::CircleRadius { circle, r } => Constraint::CircleRadius(dc[circle], r),
        ConstraintDef::LineTangentToCircle { l0, l1, circle } => {
            Constraint::LineTangentToCircle(seg(l0, l1), dc[circle], LineSide::Undefined)
        }
        ConstraintDef::PointLineDistance { p, l0, l1, d } => {
            Constraint::PointLineDistance(dp[p], seg(l0, l1), d)
        }
    }
}
