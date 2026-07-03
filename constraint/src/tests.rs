//! Crate unit tests — the constraint layer's own gate (run under `cargo test --workspace`, not dark).
//! The full deterministic-across-peers + minimal-conflicting-set spike lives in
//! `editor-shell/tests/constraint_spike.rs` (with the intent-inference + undoable-op + Certificate legs).

use crate::canonical::{canon_i64, dequant, snap_witness, SketchSolve, CANON_QUANTUM_MM};
use crate::conflict::{is_satisfiable, minimal_conflicting_set};
use crate::sketch::{Axis, ConstraintDef, Sketch, SketchError};
use crate::{EzpzSolver, Solver};

// A fully-constrained rectangle W×H with p0 pinned at the origin. Points: p0,p1,p2,p3 (CCW).
fn rectangle(w: f64, h: f64) -> Sketch {
    let mut s = Sketch::new();
    let p0 = s.add_point(0.1, -0.1); // rough witness
    let p1 = s.add_point(w - 0.3, 0.2);
    let p2 = s.add_point(w + 0.2, h - 0.4);
    let p3 = s.add_point(-0.2, h + 0.3);
    s.add(ConstraintDef::Fixed {
        point: p0,
        axis: Axis::X,
        value: 0.0,
    });
    s.add(ConstraintDef::Fixed {
        point: p0,
        axis: Axis::Y,
        value: 0.0,
    });
    s.add(ConstraintDef::Horizontal { a: p0, b: p1 });
    s.add(ConstraintDef::Vertical { a: p1, b: p2 });
    s.add(ConstraintDef::Horizontal { a: p3, b: p2 });
    s.add(ConstraintDef::Vertical { a: p0, b: p3 });
    s.add(ConstraintDef::HorizontalDistance { a: p0, b: p1, d: w });
    s.add(ConstraintDef::VerticalDistance { a: p1, b: p2, d: h });
    s
}

// The canonical "flip": p2 must be distance 6 from both p0(0,0) and p1(10,0) — two solutions (elbow
// up/down). `up` chooses the witness above the p0-p1 line, `down` below.
fn elbow(up: bool) -> Sketch {
    let mut s = Sketch::new();
    let p0 = s.add_point(0.0, 0.0);
    let p1 = s.add_point(10.0, 0.0);
    let y = if up { 4.0 } else { -4.0 };
    let p2 = s.add_point(5.0, y);
    s.add(ConstraintDef::Fixed {
        point: p0,
        axis: Axis::X,
        value: 0.0,
    });
    s.add(ConstraintDef::Fixed {
        point: p0,
        axis: Axis::Y,
        value: 0.0,
    });
    s.add(ConstraintDef::Fixed {
        point: p1,
        axis: Axis::X,
        value: 10.0,
    });
    s.add(ConstraintDef::Fixed {
        point: p1,
        axis: Axis::Y,
        value: 0.0,
    });
    s.add(ConstraintDef::Distance {
        a: p0,
        b: p2,
        d: 6.0,
    });
    s.add(ConstraintDef::Distance {
        a: p1,
        b: p2,
        d: 6.0,
    });
    s
}

#[test]
fn well_constrained_rectangle_solves() {
    let solver = EzpzSolver::new();
    let res = solver.solve(&rectangle(40.0, 25.0));
    assert!(res.is_ok(), "rectangle should solve: {res:?}");
    assert!(
        !res.is_under_constrained(),
        "a rectangle w/ pinned origin is fully constrained"
    );
    // Assert the SHAPE invariants (sign-convention-independent): a 40×25 axis-aligned rectangle.
    let p = &res.points;
    let w = (p[1].x - p[0].x).abs();
    let h = (p[2].y - p[1].y).abs();
    assert!(
        (w - 40.0).abs() < 1e-6,
        "width should be 40, got {w} ({p:?})"
    );
    assert!((h - 25.0).abs() < 1e-6, "height should be 25, got {h}");
    assert!((p[1].y - p[0].y).abs() < 1e-6, "bottom edge horizontal");
    assert!((p[2].x - p[1].x).abs() < 1e-6, "right edge vertical");
    assert!((p[3].y - p[2].y).abs() < 1e-6, "top edge horizontal");
    assert!((p[3].x - p[0].x).abs() < 1e-6, "left edge vertical");
    // p0 is pinned at the origin.
    assert!(
        p[0].x.abs() < 1e-6 && p[0].y.abs() < 1e-6,
        "p0 pinned at origin"
    );
}

#[test]
fn empty_sketch_is_trivially_solved() {
    let solver = EzpzSolver::new();
    let mut s = Sketch::new();
    s.add_point(3.0, 4.0);
    let res = solver.solve(&s);
    assert!(res.is_ok());
    assert!(res.is_under_constrained(), "an unconstrained point is free");
    assert_eq!(
        res.points[0].x.to_bits(),
        3.0_f64.to_bits(),
        "the witness is the answer, unchanged"
    );
}

#[test]
fn determinism_same_witness_is_bit_identical() {
    let solver = EzpzSolver::new();
    let s = rectangle(40.0, 25.0);
    let a = SketchSolve::compute(&s, &solver);
    let b = SketchSolve::compute(&s, &solver);
    assert_eq!(
        a.identity(),
        b.identity(),
        "same witness ⇒ identical canonical identity"
    );
    assert_eq!(a.solved_points, b.solved_points);
    assert!(
        a.verify_replay(&solver),
        "replay from the stored witness reproduces"
    );
}

#[test]
fn the_flip_exists_and_the_witness_fixes_it() {
    let solver = EzpzSolver::new();
    // The flip is real: the SAME constraints, two witnesses, two DIFFERENT branches.
    let up = SketchSolve::compute(&elbow(true), &solver);
    let down = SketchSolve::compute(&elbow(false), &solver);
    assert!(
        up.satisfied && down.satisfied,
        "both branches are valid solutions"
    );
    assert_ne!(
        up.solved_points, down.solved_points,
        "the two witnesses must reach DIFFERENT branches (the flip is real)"
    );
    assert!(
        up.solved_point(2).y > 0.0,
        "the 'up' witness lands above the line"
    );
    assert!(
        down.solved_point(2).y < 0.0,
        "the 'down' witness lands below the line"
    );

    // Two "peers" carry the SAME stored witness ⇒ they re-solve to the SAME branch (no flip).
    let peer_a = SketchSolve::compute(&elbow(true), &solver);
    let peer_b = SketchSolve::compute(&elbow(true), &solver);
    assert_eq!(
        peer_a.identity(),
        peer_b.identity(),
        "two peers from the same witness converge to the same branch, bit-identical"
    );
}

#[test]
fn over_constraint_yields_the_minimal_conflicting_set() {
    let solver = EzpzSolver::new();
    let mut s = Sketch::new();
    let p0 = s.add_point(0.0, 0.0);
    let p1 = s.add_point(12.0, 0.0);
    // Two well-constrained fixes, then TWO contradictory horizontal dimensions on the same pair.
    let _c0 = s.add(ConstraintDef::Fixed {
        point: p0,
        axis: Axis::X,
        value: 0.0,
    });
    let _c1 = s.add(ConstraintDef::Fixed {
        point: p0,
        axis: Axis::Y,
        value: 0.0,
    });
    let _c2 = s.add(ConstraintDef::Fixed {
        point: p1,
        axis: Axis::Y,
        value: 0.0,
    });
    let c3 = s.add(ConstraintDef::HorizontalDistance {
        a: p0,
        b: p1,
        d: 10.0,
    });
    let c4 = s.add(ConstraintDef::HorizontalDistance {
        a: p0,
        b: p1,
        d: 15.0,
    });

    let res = solver.solve(&s);
    assert!(
        res.is_over_constrained(),
        "conflicting dims ⇒ over-constrained: {res:?}"
    );

    let mcs = minimal_conflicting_set(&s, &solver).expect("an over-constrained sketch has an MCS");
    assert_eq!(
        mcs.constraints,
        vec![c3, c4],
        "the minimal set is exactly the two conflicting dimensions, not the whole sketch"
    );
    assert_eq!(mcs.descriptions.len(), 2);
    assert!(mcs.reason.contains("can't all hold"));
    // Dropping either member of the MCS resolves the conflict.
    let mut without = s.clone();
    without.constraints.remove(c4);
    assert!(
        solver.solve(&without).is_ok(),
        "dropping one conflicting dim solves it"
    );
}

#[test]
fn satisfiable_sketch_not_falsely_flagged_over_constrained() {
    // The LM-locality trap (adversarial finding): a SATISFIABLE sketch fed a collapsed witness can converge
    // to a local minimum (a single solve returns satisfied=false). The deterministic multi-start must
    // recognize it as satisfiable and NOT emit a false minimal-conflicting-set. A regular octagon (8 sides =
    // 10, p0 pinned) is satisfiable (under-constrained, many solutions); the witness collapses all points
    // near the origin — a classic local-minimum trap.
    let solver = EzpzSolver::new();
    let mut s = Sketch::new();
    for _ in 0..8 {
        s.add_point(0.01, -0.01);
    }
    s.add(ConstraintDef::Fixed {
        point: 0,
        axis: Axis::X,
        value: 0.0,
    });
    s.add(ConstraintDef::Fixed {
        point: 0,
        axis: Axis::Y,
        value: 0.0,
    });
    for k in 0..8 {
        s.add(ConstraintDef::Distance {
            a: k,
            b: (k + 1) % 8,
            d: 10.0,
        });
    }
    assert!(
        is_satisfiable(&s, &solver),
        "a satisfiable octagon is recognized (multi-start), even from a collapsed witness"
    );
    assert!(
        minimal_conflicting_set(&s, &solver).is_none(),
        "a solvable sketch must NOT be falsely reported as over-constrained (no false MCS)"
    );
}

#[test]
fn multiple_disjoint_conflicts_report_incompleteness() {
    // Two INDEPENDENT conflicts (adversarial finding): the returned MCS is one genuine minimal set, but it
    // must honestly say the sketch has more (dropping one member does NOT resolve everything).
    let solver = EzpzSolver::new();
    let mut s = Sketch::new();
    let p0 = s.add_point(0.0, 0.0);
    let p1 = s.add_point(11.0, 0.0);
    let p2 = s.add_point(0.0, 5.0);
    let p3 = s.add_point(22.0, 5.0);
    for (pt, v) in [(p0, 0.0), (p1, 0.0), (p2, 5.0), (p3, 5.0)] {
        s.add(ConstraintDef::Fixed {
            point: pt,
            axis: Axis::Y,
            value: v,
        });
    }
    s.add(ConstraintDef::Fixed {
        point: p0,
        axis: Axis::X,
        value: 0.0,
    });
    // Conflict A on (p0,p1); conflict B on (p2,p3) — disjoint, independent.
    s.add(ConstraintDef::HorizontalDistance {
        a: p0,
        b: p1,
        d: 10.0,
    });
    s.add(ConstraintDef::HorizontalDistance {
        a: p0,
        b: p1,
        d: 15.0,
    });
    s.add(ConstraintDef::HorizontalDistance {
        a: p2,
        b: p3,
        d: 20.0,
    });
    s.add(ConstraintDef::HorizontalDistance {
        a: p2,
        b: p3,
        d: 25.0,
    });

    let mcs =
        minimal_conflicting_set(&s, &solver).expect("a doubly-over-constrained sketch has an MCS");
    assert_eq!(
        mcs.constraints.len(),
        2,
        "one minimal set = exactly two conflicting dims"
    );
    assert!(
        !mcs.complete,
        "with two independent conflicts, resolving this one does NOT resolve the whole sketch"
    );
    assert!(
        mcs.reason.contains("independent conflicts"),
        "the reason is honest about further conflicts: {}",
        mcs.reason
    );
}

#[test]
fn well_constrained_sketch_has_no_conflict() {
    let solver = EzpzSolver::new();
    assert!(
        minimal_conflicting_set(&rectangle(40.0, 25.0), &solver).is_none(),
        "a satisfiable sketch has no minimal-conflicting-set"
    );
}

#[test]
fn under_constraint_reports_the_free_points() {
    let solver = EzpzSolver::new();
    let mut s = Sketch::new();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(3.9, 0.1);
    s.add(ConstraintDef::Fixed {
        point: a,
        axis: Axis::X,
        value: 0.0,
    });
    s.add(ConstraintDef::Fixed {
        point: a,
        axis: Axis::Y,
        value: 0.0,
    });
    s.add(ConstraintDef::Distance { a, b, d: 4.0 });
    let res = solver.solve(&s);
    assert!(res.satisfied, "distance-from-origin is satisfiable");
    assert!(
        res.is_under_constrained(),
        "b lies on a circle ⇒ it has a DoF"
    );
    assert!(res.underconstrained_points.contains(&b));
}

#[test]
fn malformed_sketch_is_explained_not_a_panic() {
    let solver = EzpzSolver::new();
    let mut s = Sketch::new();
    s.add_point(0.0, 0.0);
    // Reference a point index that doesn't exist.
    s.add(ConstraintDef::Distance { a: 0, b: 7, d: 5.0 });
    let res = solver.solve(&s);
    assert!(
        res.error.is_some(),
        "an out-of-range index is explained, not a panic"
    );
    assert!(res.error.unwrap().contains("point 7"));

    // A degenerate zero-length segment is also explained.
    let mut s2 = Sketch::new();
    s2.add_point(0.0, 0.0);
    s2.add(ConstraintDef::Horizontal { a: 0, b: 0 });
    assert!(matches!(
        s2.validate(),
        Err(SketchError::DegenerateSegment { .. })
    ));

    // A NaN dimension is rejected.
    let mut s3 = Sketch::new();
    s3.add_point(0.0, 0.0);
    s3.add_point(1.0, 0.0);
    s3.add(ConstraintDef::Distance {
        a: 0,
        b: 1,
        d: f64::NAN,
    });
    assert!(matches!(s3.validate(), Err(SketchError::BadValue { .. })));
}

#[test]
fn snap_witness_is_idempotent_and_on_grid() {
    let mut s = Sketch::new();
    s.add_point(1.234_567_891, -2.999_999_9);
    let once = snap_witness(&s);
    let twice = snap_witness(&once);
    assert_eq!(once, twice, "snapping a snapped witness is a no-op");
    // Every snapped coord is exactly an i64 grid multiple (bit-exact idempotence).
    let x = once.points[0].x;
    assert_eq!(dequant(canon_i64(x)).to_bits(), x.to_bits());
}

#[test]
fn quantum_is_far_below_cad_tolerance() {
    // 1 nm: below any real CAD tolerance, above faer's cross-ISA ULP noise.
    const _: () = assert!(CANON_QUANTUM_MM <= 1e-6);
    assert_eq!(canon_i64(1.0), 1_000_000);
    assert_eq!(canon_i64(-0.000_001_4), -1); // round-half handled deterministically
}

#[test]
fn non_deterministic_backend_refuses_to_replay() {
    // A backend that declares itself non-deterministic must not claim a replay (honest false, never faked).
    struct FakeND;
    impl Solver for FakeND {
        fn solve(&self, sketch: &Sketch) -> crate::SolveResult {
            EzpzSolver::new().solve(sketch)
        }
        fn deterministic(&self) -> bool {
            false
        }
        fn name(&self) -> &'static str {
            "fake-nondeterministic"
        }
    }
    let solve = SketchSolve::compute(&rectangle(40.0, 25.0), &FakeND);
    assert!(
        !solve.verify_replay(&FakeND),
        "a non-deterministic backend never claims a replay"
    );
}
