//! Release-only grounding/solve budget for the neuro-symbolic PCG solver (M13.5). ASP-style solving over
//! a BOUNDED design space (the FF-T11 bounded-grounding discipline — no unbounded grounding on the hot
//! path). `#[cfg_attr(debug_assertions, ignore)]` → SKIPPED in the debug `cargo test`, RUN by
//! `cargo test --workspace --release` (release-budgets.yml). NOT a dark test.
//!
//! Min-spec is OWED (dev box, not a true low-core rig); the solver is native/offline (browser =
//! server-side seam). Never fabricate a min-spec number.

use metrocalk_authoring::{BoundedSolver, ContentSpec, Pcg, Scene, SolveOutcome};
use std::time::Instant;

fn percentiles(mut us: Vec<u128>) -> (u128, u128) {
    us.sort_unstable();
    (
        us[us.len() / 2],
        us[(us.len() * 99 / 100).min(us.len() - 1)],
    )
}

/// A path graph of `slots` cells (each adjacent to the next) — a bounded, satisfiable design space.
fn path_scene(slots: usize) -> Scene {
    Scene {
        slots,
        fixed: vec![None; slots],
        adjacency: (0..slots.saturating_sub(1)).map(|i| (i, i + 1)).collect(),
    }
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release-only grounding budget (run by release-budgets.yml)"
)]
fn pcg_solve_holds_an_interactive_budget_on_a_bounded_space() {
    const RUNS: usize = 200;
    let solver = BoundedSolver::new(1_000_000);
    // A 16-slot path, domain 4, "at least 8 slots equal to 1" — satisfiable, non-trivial search.
    let scene = path_scene(16);
    let spec = ContentSpec {
        domain: 4,
        target: 1,
        min_target: 8,
    };

    // Warm up + confirm it solves.
    match solver.generate(&scene, &spec) {
        SolveOutcome::Solved(d) => assert!(d.assignment.iter().filter(|&&v| v == 1).count() >= 8),
        SolveOutcome::Rejected(c) => panic!("expected a solve, got: {}", c.reason),
    }

    let mut us = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let t = Instant::now();
        let _ = solver.generate(&scene, &spec);
        us.push(t.elapsed().as_micros());
    }
    let (p50, p99) = percentiles(us);
    println!("::notice::authoring-pcg-solve-p50-us={p50}");
    println!("::notice::authoring-pcg-solve-p99-us={p99}");

    // A generous interactive budget on the bounded space (a discrete authoring op, off the hot path).
    assert!(
        p99 < 50_000,
        "PCG solve p99 {p99}us exceeded 50ms on the bounded space"
    );
}
