//! **The minimal conflicting set** — every over-constraint explained (ADR-076). In editor-shell this is
//! wrapped in the shipped `metrocalk_authoring::Certificate` (the M13.5/ADR-054 every-no seed; the full
//! M13.9 semiring theorem, ADR-061, stays a named future).
//!
//! `ezpz`'s `unsatisfied` set is *coarse* — it names whichever constraints lost the least-squares fight,
//! not a minimal explanation. A genuine **minimal conflicting set** is the SHORTEST subset of constraints
//! that is *jointly* unsatisfiable — every proper subset solves. We compute it by **deletion-based
//! reduction** (drop-one-until-SAT, a QuickXplain-lite), iterating in ascending constraint order so the
//! result is deterministic.
//!
//! **Honest boundary (the LM-locality caveat).** `ezpz` is a *local* Levenberg-Marquardt solver: a single
//! solve returning `satisfied = false` means "this run's endpoint failed the residuals", NOT "the sketch is
//! unsatisfiable" — a satisfiable sketch fed a poor witness can converge to a local minimum. So the SAT test
//! used here is a **deterministic multi-start** ([`is_satisfiable`]): the given witness plus a fixed set of
//! scale-appropriate perturbations, SAT iff ANY start satisfies. This reliably detects **structural**
//! over-constraint (redundant/contradictory dimensions — the common case + what the gates test) and greatly
//! reduces local-minimum false-positives; it is **not** a satisfiability proof (a sound proof needs
//! structural DOF analysis — the D-Cubed math we do not rebuild). The multi-start is deterministic (a fixed
//! seeded perturbation sequence), so the classification and the MCS are reproducible.

use crate::sketch::{ConstraintDef, Sketch};
use crate::Solver;

/// How many perturbed starts the multi-start satisfiability check tries (beyond the given witness). Fixed →
/// deterministic; each try is a cheap 2D solve, off the hot path.
pub const MULTISTART_TRIES: usize = 16;

/// A minimal conflicting set: the shortest subset of constraints that cannot jointly hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MinimalConflictingSet {
    /// The conflicting constraint indices (into the original sketch), ascending.
    pub constraints: Vec<usize>,
    /// One plain-language phrase per conflicting constraint (the every-no `unsat_core` payload).
    pub descriptions: Vec<String>,
    /// The plain-language "why" sentence — what the user reads.
    pub reason: String,
    /// True iff dropping any one member of this set makes the WHOLE sketch satisfiable (i.e. this is the
    /// only conflict). False ⇒ the sketch has further, independent conflicts beyond this one.
    pub complete: bool,
}

// A sub-sketch with the same geometry/witness but only the kept constraints.
fn with_constraints(sketch: &Sketch, keep: &[usize]) -> Sketch {
    Sketch {
        points: sketch.points.clone(),
        circles: sketch.circles.clone(),
        constraints: keep.iter().map(|&i| sketch.constraints[i]).collect(),
    }
}

// Can this subset of constraints all hold at once? (Empty / under-constrained-but-consistent ⇒ satisfiable.)
fn subset_is_sat(sketch: &Sketch, keep: &[usize], solver: &impl Solver) -> bool {
    if keep.is_empty() {
        return true;
    }
    is_satisfiable(&with_constraints(sketch, keep), solver)
}

/// A **deterministic multi-start** satisfiability check. Tries the sketch's own witness first, then a fixed
/// set of scale-appropriate perturbations; returns `true` iff ANY start reaches a fully-satisfied solve.
/// This distinguishes a genuinely (structurally) over-constrained sketch — NO start satisfies — from a
/// local-minimum trap where the given witness merely converged to a non-solution. Deterministic (the
/// perturbation sequence is a fixed seeded splitmix64), so the verdict is reproducible.
#[must_use]
pub fn is_satisfiable(sketch: &Sketch, solver: &impl Solver) -> bool {
    if solver.solve(sketch).satisfied {
        return true;
    }
    let scale = sketch_scale(sketch);
    let mut seed = 0x5EED_C047_5EED_C047_u64; // fixed → deterministic
    for _ in 0..MULTISTART_TRIES {
        let mut s = sketch.clone();
        for p in &mut s.points {
            seed = splitmix64(seed);
            p.x += unit(seed) * scale;
            seed = splitmix64(seed);
            p.y += unit(seed) * scale;
        }
        for c in &mut s.circles {
            seed = splitmix64(seed);
            c.radius = (c.radius + unit(seed) * scale).abs().max(1e-3);
        }
        if solver.solve(&s).satisfied {
            return true;
        }
    }
    false
}

/// Compute a minimal conflicting set for an over-constrained sketch. Returns `None` if the sketch is
/// satisfiable (via [`is_satisfiable`] — a local-minimum trap is NOT reported as a conflict) or malformed.
///
/// Runs `O(n)` multi-start SAT checks (one per constraint), each a handful of cheap 2D solves.
#[must_use]
pub fn minimal_conflicting_set(
    sketch: &Sketch,
    solver: &impl Solver,
) -> Option<MinimalConflictingSet> {
    if solver.solve(sketch).error.is_some() {
        return None; // malformed — the error is surfaced by the solve itself
    }
    if is_satisfiable(sketch, solver) {
        return None; // satisfiable (possibly only from a better witness) ⇒ NOT over-constrained
    }

    // Deletion-based reduction: start with ALL constraints (jointly unsatisfiable), and permanently drop any
    // constraint whose removal keeps the system unsatisfiable — ascending index order ⇒ deterministic.
    let mut core: Vec<usize> = (0..sketch.constraints.len()).collect();
    let candidates = core.clone();
    for &i in &candidates {
        let trial: Vec<usize> = core.iter().copied().filter(|&j| j != i).collect();
        if !subset_is_sat(sketch, &trial, solver) {
            core = trial; // removing i keeps it UNSAT ⇒ i is not essential; drop it.
        }
    }

    // `core` is now minimal: removing any single member makes it satisfiable. Is it the ONLY conflict?
    let remainder: Vec<usize> = (0..sketch.constraints.len())
        .filter(|i| !core.contains(i))
        .collect();
    let complete = subset_is_sat(sketch, &remainder, solver);

    let descriptions: Vec<String> = core.iter().map(|&i| sketch.describe(i)).collect();
    let tail = if complete {
        String::new()
    } else {
        " (this is ONE of several independent conflicts — the sketch has more)".to_string()
    };
    let reason = format!(
        "these {} constraints can't all hold at once \u{2014} drop any one to resolve this conflict{}: {}",
        core.len(),
        tail,
        descriptions.join("; ")
    );
    Some(MinimalConflictingSet {
        constraints: core,
        descriptions,
        reason,
        complete,
    })
}

// The characteristic scale of a sketch (mm) — the largest coordinate magnitude or dimension value. The
// multi-start jitter is drawn from ±scale so perturbations are large enough to escape a collapsed/collinear
// local minimum, whatever the sketch's size.
fn sketch_scale(sketch: &Sketch) -> f64 {
    let mut scale = 1.0_f64;
    for p in &sketch.points {
        scale = scale.max(p.x.abs()).max(p.y.abs());
    }
    for c in &sketch.constraints {
        if let Some(d) = dimension_value(c) {
            scale = scale.max(d.abs());
        }
    }
    scale.max(1.0)
}

fn dimension_value(c: &ConstraintDef) -> Option<f64> {
    match *c {
        ConstraintDef::Distance { d, .. }
        | ConstraintDef::HorizontalDistance { d, .. }
        | ConstraintDef::VerticalDistance { d, .. }
        | ConstraintDef::PointLineDistance { d, .. } => Some(d),
        ConstraintDef::CircleRadius { r, .. } => Some(r),
        ConstraintDef::Fixed { value, .. } => Some(value),
        _ => None,
    }
}

fn splitmix64(x: u64) -> u64 {
    let x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

// Map a seed to a jitter in [-1, 1). The top 53 bits fit exactly in f64.
fn unit(seed: u64) -> f64 {
    #[allow(clippy::cast_precision_loss)] // 53-bit mantissa, exact
    let m = (seed >> 11) as f64 / (1u64 << 53) as f64;
    m * 2.0 - 1.0
}
