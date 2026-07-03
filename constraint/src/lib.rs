//! `metrocalk-constraint` — 2D **sketch constraint solving** (M15.6, ADR-076).
//!
//! The solver math is a 30-year-solved problem — `ezpz` (KittyCAD/Zoo, MIT, pure Rust, wasm-clean) is
//! embedded as the Levenberg-Marquardt engine; **we do not rebuild D-Cubed** (the dossier §4 AVOID line).
//! `ezpz` lives ONLY behind this crate's boundary — the plain-array [`Sketch`]/[`SolveResult`] surface —
//! so no `ezpz` type crosses out (invariant 5, grep-gated); it is confined to [`ezpz_backend`].
//!
//! The value Metrocalk adds is the **layer**, which no existing CAD can build without a replayable
//! op-stream:
//! - **Determinism = witness-config-in-the-op** ([`canonical`]). `ezpz`'s RAW f64 determinism is
//!   UNVERIFIED (a numeric LM solve over faer sparse-LU with runtime SIMD dispatch), so determinism is
//!   OURS: the **witness** (the initial configuration) is stored in the op, so every peer re-solves from
//!   the same start to the same solution **branch** (the "flip" fix); and the **canonical solved state is
//!   quantized to i64** ([`canonical::CANON_QUANTUM_MM`]) — far below any CAD tolerance and far above
//!   `faer`'s cross-ISA ULP noise — so the stored state is **DESIGNED cross-ISA-portable**. Honest boundary:
//!   a *boundary-flip* (a coordinate within ~1 ULP of a grid boundary) can still round differently across
//!   ISAs, but that is ~1e-9/coord — disclosed, not eliminated; a real cross-ISA CI gate verifies the design
//!   holds on the fixture. The raw f64 solve is a derivation (the ADR-020 native/wasm float boundary is ITS).
//! - **Every conflict explained = the minimal conflicting set** ([`conflict`]). An over-constrained sketch
//!   returns the SHORTEST subset of constraints that can't jointly hold (deletion-based reduction), in plain
//!   language — not a bare "over-defined". Honest boundary: `ezpz` is a *local* solver, so the satisfiability
//!   test is a deterministic **multi-start** ([`is_satisfiable`]) that tells a genuine (structural)
//!   over-constraint from a local-minimum trap — reliable for the common contradictory/redundant-dimension
//!   case, best-effort otherwise (a sound proof needs structural DOF analysis, the D-Cubed math we don't
//!   rebuild); multiple independent conflicts return one minimal set + an honest `complete = false`.
//!
//! Determinism was AUDITED before adoption (ADR-029): `ezpz` builds host + wasm32; its solve is
//! bit-reproducible same-machine (measured, in-proc + cross-process); the witness + quantization make the
//! canonical result cross-ISA-portable (verified by a CI gate). If a future `ezpz` bump broke that, the
//! [`Solver`] trait is the escape — a deterministic reformulation implements the same seam (the
//! ARAP/`baby_shark` precedent). **3D assembly mates are a named future** (D-Cubed-class), out of scope.

// Boundary discipline (invariant 5): the public API above re-exports ONLY plain-array/project-owned types —
// NEVER an `ezpz`/`faer` type. A `pub use ezpz::Foo` here would leak a foreign type through a
// `metrocalk_constraint::Foo` path that the `ezpz::` grep-gate cannot see. Keep the surface foreign-free.

pub mod canonical;
pub mod conflict;
pub mod ezpz_backend;
pub mod sketch;

#[cfg(test)]
mod tests;

pub use canonical::{canon_i64, SketchSolve, CANON_QUANTUM_MM};
pub use conflict::{is_satisfiable, minimal_conflicting_set, MinimalConflictingSet};
pub use ezpz_backend::EzpzSolver;
pub use sketch::{Axis, CircleDef, ConstraintDef, Point, Sketch, SketchError};

/// Tunables for a solve. Defaults track `ezpz`'s but tighten the convergence tolerance so the solved
/// coordinates are accurate well below [`CANON_QUANTUM_MM`] (clean quantization).
#[derive(Clone, Copy, Debug)]
pub struct SolverConfig {
    /// Max Levenberg-Marquardt iterations before giving up.
    pub max_iterations: usize,
    /// Residual tolerance for convergence (tighter than the quantum → clean canonical state).
    pub residual_tolerance: f64,
}

impl Default for SolverConfig {
    fn default() -> Self {
        Self {
            max_iterations: 100,
            residual_tolerance: 1e-10,
        }
    }
}

/// The outcome of a solve — plain data, no `ezpz` type.
#[derive(Clone, Debug, PartialEq)]
pub struct SolveResult {
    /// Did the numeric solve converge?
    pub converged: bool,
    /// Were ALL constraints satisfied (within tolerance)? `false` ⇒ over-constrained / inconsistent.
    pub satisfied: bool,
    /// The solved point positions (raw f64 — a derivation; canonicalize via [`canonical`] before storing).
    pub points: Vec<Point>,
    /// The solved circle radii (same index order as [`Sketch::circles`]).
    pub radii: Vec<f64>,
    /// How many iterations the solve took.
    pub iterations: usize,
    /// Constraint indices whose residual is still non-zero (the over-constraint seed for [`conflict`]).
    pub unsatisfied: Vec<usize>,
    /// Point indices whose position is not fully pinned (degrees of freedom remain).
    pub underconstrained_points: Vec<usize>,
    /// Circle indices whose radius is not fully pinned.
    pub underconstrained_radii: Vec<usize>,
    /// Set iff the solve could not run at all (a malformed sketch or a solver error), explained.
    pub error: Option<String>,
}

impl SolveResult {
    /// A well-defined solve: it converged AND every constraint holds (may still be under-constrained,
    /// which is fine — the witness pins the free DoF deterministically).
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.error.is_none() && self.converged && self.satisfied
    }

    /// True iff the sketch is over-constrained / inconsistent (some constraint can't hold).
    #[must_use]
    pub fn is_over_constrained(&self) -> bool {
        self.error.is_none() && !self.satisfied
    }

    /// True iff any point or radius still has a degree of freedom.
    #[must_use]
    pub fn is_under_constrained(&self) -> bool {
        !self.underconstrained_points.is_empty() || !self.underconstrained_radii.is_empty()
    }
}

/// The project-owned solver seam (invariant 5). `ezpz` sits behind this via [`EzpzSolver`]; a future
/// backend (a deterministic reformulation — the ADR-029 wrapper escape) implements the same trait.
pub trait Solver {
    /// Solve `sketch` from its witness (the point/radius initial configuration). **Never panics** — a
    /// malformed sketch returns a [`SolveResult`] with `error` set (explained, not a crash).
    fn solve(&self, sketch: &Sketch) -> SolveResult;

    /// Whether this backend's CANONICAL solve is bit-reproducible from a fixed witness (gates
    /// [`SketchSolve::verify_replay`]). `ezpz` is `true` (measured same-machine; canonical = portable).
    fn deterministic(&self) -> bool;

    /// A human-readable backend name (recorded in the audit / the `SketchSolve`).
    fn name(&self) -> &'static str;
}
