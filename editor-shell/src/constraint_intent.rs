//! M15.6 (ADR-076) — the **intent + explainability layer** over the embedded 2D constraint solver.
//!
//! `metrocalk_constraint` is the solver (ezpz behind the `Solver` trait; the moat = the witness-config
//! determinism + the minimal-conflicting-set). This module is the editor-shell surfacing that no incumbent
//! CAD can build without a replayable op-stream:
//!
//! 1. **Intent-inference** ([`propose_constraints`]) — *propose the constraint before the user declares
//!    it*, reusing the SHIPPED M9.4 ranker [`crate::reveal::intent_order`] (proximity·affinity·recency·
//!    stable-id, ADR-011/028) — **the ONE ranker, not a parallel heuristic**. Each proposal is a ghost + a
//!    plain-language "why this".
//! 2. **Every conflict explained** ([`explain_conflict`]) — the minimal-conflicting-set wrapped in the
//!    SHIPPED every-no `metrocalk_authoring::Certificate` (M13.5/ADR-054; the full M13.9 theorem, ADR-061,
//!    stays a named future) — reusing the seed, not a new explainer.
//! 3. **Determinism = the witness-config-in-the-op** ([`solve_and_land`]) — a solve lands as ONE undoable
//!    transaction that writes both the solved coordinates AND the witness (`wx`/`wy`) onto each point
//!    entity, so a reload/peer re-solves from the same start to the same branch (the "flip" fix, literal in
//!    the doc). `/core` is untouched — this is CAD-domain surfacing in the shell, like the rest of M15.

use std::collections::HashMap;

use metrocalk_authoring::Certificate;
use metrocalk_constraint::canonical::dequant;
use metrocalk_constraint::{
    minimal_conflicting_set, ConstraintDef, Point, Sketch, SketchSolve, Solver,
};
use metrocalk_core::registry::{ComponentMeta, FieldType};
use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::World;

use crate::reveal::intent_order;

/// The ECS component a sketch point carries: `x`/`y` = the solved position, `wx`/`wy` = the **witness**
/// (the initial configuration stored IN the op — the branch-fixing seed). One component, typed fields.
pub const SKETCH_POINT: &str = "SketchPoint";

/// The registered schema for a sketch point (the ADR-048/017 validation key; the pipeline accepts the
/// fields as data regardless, like [`crate::pmi::fcf_component_meta`]).
#[must_use]
pub fn sketch_point_meta() -> ComponentMeta {
    ComponentMeta::builder(SKETCH_POINT)
        .category("Geometry")
        .field("x", FieldType::Number, true)
        .field("y", FieldType::Number, true)
        .field("wx", FieldType::Number, true)
        .field("wy", FieldType::Number, true)
        .tag("cad")
        .tag("sketch")
        .ui_hint("x", "solved X (mm)")
        .ui_hint("y", "solved Y (mm)")
        .ui_hint(
            "wx",
            "witness X (mm) — the initial guess stored in the op that fixes the solution branch (no 'flip')",
        )
        .ui_hint("wy", "witness Y (mm) — the branch-fixing initial guess")
        .build()
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────────
// 1. Intent-inference — propose the constraint before it's declared (reuse `reveal::intent_order`).
// ─────────────────────────────────────────────────────────────────────────────────────────────────────

/// A proposed constraint the geometry is already close to satisfying — a ghost + a "why this".
#[derive(Clone, Debug, PartialEq)]
pub struct ConstraintProposal {
    /// The ghost: the constraint that would be added (committed via the existing pipeline, never
    /// auto-applied).
    pub constraint: ConstraintDef,
    /// How close the current geometry already is to satisfying it (mm; 0 = exactly satisfied). This is the
    /// ranker's proximity term.
    pub residual_mm: f64,
    /// The constraint kind's semantic strength — the ranker's affinity term (mirrors `SnapKind::affinity`).
    pub affinity: u32,
    /// The plain-language "why this".
    pub why: String,
}

/// Propose the constraints the sketch geometry is *already close* to, ranked by the SHIPPED M9.4 ranker
/// ([`crate::reveal::intent_order`]) — proximity (`residual_mm`) · affinity (kind) · recency (which points
/// were touched) · stable-id. `segments` are the drawn line entities (pairs of point indices); `recency`
/// is the app-owned last-touched map (point index → tick). Only candidates within `tol_mm` are proposed.
#[must_use]
pub fn propose_constraints<S: std::hash::BuildHasher>(
    sketch: &Sketch,
    segments: &[[usize; 2]],
    recency: &HashMap<usize, u64, S>,
    tol_mm: f64,
) -> Vec<ConstraintProposal> {
    let pts = &sketch.points;
    let np = pts.len();
    let mut out: Vec<ConstraintProposal> = Vec::new();

    // Coincident: any two points within tolerance want to be the same point.
    for i in 0..np {
        for j in (i + 1)..np {
            let d = dist(pts[i], pts[j]);
            if d <= tol_mm {
                out.push(proposal(
                    ConstraintDef::Coincident { a: i, b: j },
                    d,
                    format!("points p{i} and p{j} are only {d:.3} mm apart \u{2014} make them coincident"),
                ));
            }
        }
    }

    // Per-segment: nearly horizontal / vertical.
    for &[a, b] in segments {
        if a >= np || b >= np || a == b {
            continue;
        }
        let (dx, dy, _len) = seg_vec(pts, a, b);
        let (adx, ady) = (dx.abs(), dy.abs());
        if ady <= tol_mm && adx > tol_mm {
            out.push(proposal(
                ConstraintDef::Horizontal { a, b },
                ady,
                format!("segment p{a}\u{2192}p{b} is within {ady:.3} mm of horizontal"),
            ));
        }
        if adx <= tol_mm && ady > tol_mm {
            out.push(proposal(
                ConstraintDef::Vertical { a, b },
                adx,
                format!("segment p{a}\u{2192}p{b} is within {adx:.3} mm of vertical"),
            ));
        }
    }

    // Segment pairs: nearly parallel / perpendicular.
    for si in 0..segments.len() {
        for sj in (si + 1)..segments.len() {
            let [a0, a1] = segments[si];
            let [b0, b1] = segments[sj];
            if [a0, a1, b0, b1].iter().any(|&p| p >= np) || a0 == a1 || b0 == b1 {
                continue;
            }
            let (dx1, dy1, l1) = seg_vec(pts, a0, a1);
            let (dx2, dy2, l2) = seg_vec(pts, b0, b1);
            if l1 < 1e-9 || l2 < 1e-9 {
                continue;
            }
            let minlen = l1.min(l2);
            let sin_ab = (dx1 * dy2 - dy1 * dx2).abs() / (l1 * l2);
            let cos_ab = (dx1 * dx2 + dy1 * dy2).abs() / (l1 * l2);
            let resid_par = sin_ab * minlen;
            let resid_perp = cos_ab * minlen;
            if resid_par <= tol_mm {
                out.push(proposal(
                    ConstraintDef::Parallel { a0, a1, b0, b1 },
                    resid_par,
                    format!("p{a0}\u{2192}p{a1} and p{b0}\u{2192}p{b1} are within {resid_par:.3} mm of parallel"),
                ));
            }
            if resid_perp <= tol_mm {
                out.push(proposal(
                    ConstraintDef::Perpendicular { a0, a1, b0, b1 },
                    resid_perp,
                    format!("p{a0}\u{2192}p{a1} and p{b0}\u{2192}p{b1} are within {resid_perp:.3} mm of perpendicular"),
                ));
            }
        }
    }

    // A segment nearly tangent to a circle.
    for &[l0, l1] in segments {
        if l0 >= np || l1 >= np || l0 == l1 {
            continue;
        }
        for (ci, c) in sketch.circles.iter().enumerate() {
            if c.center >= np {
                continue;
            }
            let d = point_line_dist(pts, c.center, l0, l1);
            let resid = (d - c.radius).abs();
            if resid <= tol_mm {
                out.push(proposal(
                    ConstraintDef::LineTangentToCircle { l0, l1, circle: ci },
                    resid,
                    format!("segment p{l0}\u{2192}p{l1} is within {resid:.3} mm of tangent to circle c{ci}"),
                ));
            }
        }
    }

    // Rank by the ONE shared ranker — never a parallel heuristic (the ADR-028 adversarial guard).
    out.sort_by(|x, y| intent_order(rank_key(x, recency), rank_key(y, recency)));
    out
}

// The 4-tuple `reveal::intent_order` consumes: (proximity, affinity, recency, stable_id).
fn rank_key<S: std::hash::BuildHasher>(
    p: &ConstraintProposal,
    recency: &HashMap<usize, u64, S>,
) -> (f32, u32, u64, u64) {
    let rec = involved_points(&p.constraint)
        .iter()
        .filter_map(|pt| recency.get(pt).copied())
        .max()
        .unwrap_or(0);
    (
        to_f32(p.residual_mm),
        p.affinity,
        rec,
        stable_id(&p.constraint),
    )
}

fn proposal(constraint: ConstraintDef, residual_mm: f64, why: String) -> ConstraintProposal {
    ConstraintProposal {
        affinity: affinity_of(&constraint),
        constraint,
        residual_mm,
        why,
    }
}

// Semantic strength of a constraint kind (mirrors the `SnapKind::affinity` ordering): the more the kind
// implies deliberate intent, the higher — a coincidence is the strongest signal, an axis-alignment weakest.
fn affinity_of(c: &ConstraintDef) -> u32 {
    match c {
        ConstraintDef::Coincident { .. } => 7,
        ConstraintDef::LineTangentToCircle { .. } => 6,
        ConstraintDef::Perpendicular { .. } => 5,
        ConstraintDef::Parallel { .. } => 4,
        ConstraintDef::Vertical { .. } => 3,
        ConstraintDef::Horizontal { .. } => 2,
        _ => 1,
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────────
// 2. Every conflict explained — the minimal-conflicting-set, wrapped in the reused every-no Certificate.
// ─────────────────────────────────────────────────────────────────────────────────────────────────────

/// An over-constraint explanation: the minimal-conflicting-set, carried by the SHIPPED
/// `metrocalk_authoring::Certificate` (the M13.5/ADR-054 seed) — reason + `unsat_core`, plus the concrete
/// conflicting constraint indices.
#[derive(Clone, Debug, PartialEq)]
pub struct ConstraintCertificate {
    /// The reused every-no base (reason sentence + the `unsat_core` of conflicting-constraint phrases).
    pub base: Certificate,
    /// The conflicting constraint indices into the sketch (ascending).
    pub minimal: Vec<usize>,
}

/// Explain an over-constrained sketch as a minimal conflicting set (or `None` if it's satisfiable /
/// malformed). Off the hot path (authoring-mode); reuses [`minimal_conflicting_set`] + the every-no seed.
#[must_use]
pub fn explain_conflict(sketch: &Sketch, solver: &impl Solver) -> Option<ConstraintCertificate> {
    let mcs = minimal_conflicting_set(sketch, solver)?;
    Some(ConstraintCertificate {
        base: Certificate {
            reason: mcs.reason,
            unsat_core: mcs.descriptions,
        },
        minimal: mcs.constraints,
    })
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────────
// 3. Determinism = the witness-config-in-the-op — a solve lands as ONE undoable transaction.
// ─────────────────────────────────────────────────────────────────────────────────────────────────────

/// The outcome of solving + landing a sketch.
#[derive(Clone, Debug)]
pub enum SolveLanding {
    /// The sketch solved: the canonical "a solve is a file" artifact + how many points were committed.
    Solved {
        /// The reproducible canonical artifact (carries the witness + the solved state + the identity).
        solve: Box<SketchSolve>,
        /// How many point entities were written (one undoable transaction).
        committed: usize,
    },
    /// The sketch is over-constrained: the minimal-conflicting-set certificate (nothing was committed).
    OverConstrained(ConstraintCertificate),
    /// The sketch/inputs were malformed: explained (nothing committed).
    Invalid(String),
}

/// **Solve the sketch and land the result as ONE undoable transaction** (inv. 1/3). On success, each point
/// entity gets its solved `x`/`y` AND its witness `wx`/`wy` (the branch-fixing initial config, IN the op) —
/// one `engine.commit`, one Ctrl-Z. An over-constrained sketch is **explained (MCS), never committed**; a
/// malformed one is explained. Deterministic + reproducible: the returned [`SketchSolve`] verifies-replay.
pub fn solve_and_land<W: World>(
    engine: &mut Engine<W>,
    sketch: &Sketch,
    point_entities: &[EntityId],
    solver: &impl Solver,
) -> SolveLanding {
    if point_entities.len() != sketch.points.len() {
        return SolveLanding::Invalid(format!(
            "expected {} point entities to match the sketch, got {}",
            sketch.points.len(),
            point_entities.len()
        ));
    }
    let res = solver.solve(sketch);
    if let Some(e) = res.error {
        return SolveLanding::Invalid(e);
    }
    if !res.satisfied {
        // This witness's solve didn't satisfy every constraint. That is EITHER a genuine (structural)
        // over-constraint — `explain_conflict` (a deterministic multi-start check) returns the minimal
        // conflicting set — OR just a local-minimum trap on a satisfiable sketch (`explain_conflict` returns
        // None, because a better start DOES satisfy it). We must NOT falsely blame a solvable sketch as
        // over-constrained; either way we do NOT commit a silent least-squares fudge.
        return match explain_conflict(sketch, solver) {
            Some(cert) => SolveLanding::OverConstrained(cert),
            None => SolveLanding::Invalid(format!(
                "the sketch is solvable, but this initial configuration converged to a local minimum \
                 ({} constraint(s) still unsatisfied) \u{2014} nudge the geometry or the witness and re-solve",
                res.unsatisfied.len()
            )),
        };
    }

    let solve = SketchSolve::compute(sketch, solver);
    let mut ops: Vec<Op> = Vec::with_capacity(point_entities.len() * 4);
    for (i, &ent) in point_entities.iter().enumerate() {
        let [sx, sy] = solve.solved_points[i];
        let w = solve.sketch.points[i]; // the CANONICAL witness (grid-snapped)
        ops.push(set(ent, "x", dequant(sx)));
        ops.push(set(ent, "y", dequant(sy)));
        ops.push(set(ent, "wx", w.x));
        ops.push(set(ent, "wy", w.y));
    }
    match engine.commit("solve-sketch", ops) {
        Ok(()) => SolveLanding::Solved {
            solve: Box::new(solve),
            committed: point_entities.len(),
        },
        Err(e) => SolveLanding::Invalid(e.to_string()),
    }
}

fn set(entity: EntityId, field: &str, value: f64) -> Op {
    Op::SetField {
        entity,
        component: SKETCH_POINT.to_string(),
        field: field.to_string(),
        value: FieldValue::Number(value),
    }
}

/// Reconstruct the witness (the initial configuration) FROM the op — read `wx`/`wy` per point back out of
/// the doc. A reload or a peer feeds this into a fresh solve to reach the SAME branch: the witness-in-the-op
/// round-trip that fixes the "flip". Returns `None` if any point is missing its witness fields.
#[must_use]
pub fn witness_from_doc<W: World>(
    engine: &Engine<W>,
    point_entities: &[EntityId],
) -> Option<Vec<Point>> {
    let mut pts = Vec::with_capacity(point_entities.len());
    for &e in point_entities {
        let x = as_num(&engine.get_field(e, SKETCH_POINT, "wx")?)?;
        let y = as_num(&engine.get_field(e, SKETCH_POINT, "wy")?)?;
        pts.push(Point::new(x, y));
    }
    Some(pts)
}

// A field read as f64 — handles BOTH the Number and the whole-number-Integer arm (the pipeline may canonicalize
// a whole coordinate to Integer; matching only Number would silently miss it — the fieldvalue gotcha).
fn as_num(v: &FieldValue) -> Option<f64> {
    match *v {
        FieldValue::Number(n) => Some(n),
        #[allow(clippy::cast_precision_loss)]
        // sketch coordinates are far inside f64's exact-integer range
        FieldValue::Integer(i) => Some(i as f64),
        _ => None,
    }
}

// ── geometry + ranker helpers (pure f64, no glam — editor-shell stays glam-free per ADR-028) ─────────────

fn dist(a: Point, b: Point) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

// (dx, dy, length) of the segment a→b.
fn seg_vec(pts: &[Point], a: usize, b: usize) -> (f64, f64, f64) {
    let dx = pts[b].x - pts[a].x;
    let dy = pts[b].y - pts[a].y;
    (dx, dy, dx.hypot(dy))
}

// Perpendicular distance from point `p` to the infinite line through l0→l1.
fn point_line_dist(pts: &[Point], p: usize, l0: usize, l1: usize) -> f64 {
    let (dx, dy, len) = seg_vec(pts, l0, l1);
    if len < 1e-12 {
        return dist(pts[p], pts[l0]);
    }
    let cross = (pts[p].x - pts[l0].x) * dy - (pts[p].y - pts[l0].y) * dx;
    cross.abs() / len
}

fn to_f32(v: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation)]
    // a ranking key; f32 precision is ample for proximity ordering
    {
        v as f32
    }
}

// The point indices a constraint touches (for the recency ranker term).
fn involved_points(c: &ConstraintDef) -> Vec<usize> {
    match *c {
        ConstraintDef::Fixed { point, .. } => vec![point],
        ConstraintDef::Coincident { a, b }
        | ConstraintDef::Distance { a, b, .. }
        | ConstraintDef::HorizontalDistance { a, b, .. }
        | ConstraintDef::VerticalDistance { a, b, .. }
        | ConstraintDef::Horizontal { a, b }
        | ConstraintDef::Vertical { a, b } => vec![a, b],
        ConstraintDef::Parallel { a0, a1, b0, b1 }
        | ConstraintDef::Perpendicular { a0, a1, b0, b1 }
        | ConstraintDef::Angle { a0, a1, b0, b1, .. } => vec![a0, a1, b0, b1],
        ConstraintDef::CircleRadius { .. } => vec![],
        ConstraintDef::LineTangentToCircle { l0, l1, .. } => vec![l0, l1],
        ConstraintDef::PointLineDistance { p, l0, l1, .. } => vec![p, l0, l1],
    }
}

// A deterministic stable id for the ranker's final tiebreak (structural, not a formatted-string hash).
fn stable_id(c: &ConstraintDef) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    let mut mix = |v: u64| {
        h ^= v;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    mix(kind_disc(c));
    for p in involved_points(c) {
        mix(p as u64 + 1);
    }
    h
}

fn kind_disc(c: &ConstraintDef) -> u64 {
    match c {
        ConstraintDef::Fixed { .. } => 1,
        ConstraintDef::Coincident { .. } => 2,
        ConstraintDef::Distance { .. } => 3,
        ConstraintDef::HorizontalDistance { .. } => 4,
        ConstraintDef::VerticalDistance { .. } => 5,
        ConstraintDef::Horizontal { .. } => 6,
        ConstraintDef::Vertical { .. } => 7,
        ConstraintDef::Parallel { .. } => 8,
        ConstraintDef::Perpendicular { .. } => 9,
        ConstraintDef::Angle { .. } => 10,
        ConstraintDef::CircleRadius { .. } => 11,
        ConstraintDef::LineTangentToCircle { .. } => 12,
        ConstraintDef::PointLineDistance { .. } => 13,
    }
}
