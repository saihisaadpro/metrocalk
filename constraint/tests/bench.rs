//! Release benchmark for the constraint solve — the **in-tree, reproducible defense** of the latency number
//! (adversarial-review finding: the p50/p99 must not be an asserted-once dev-box figure). Run for the real
//! number with `cargo test -p metrocalk-constraint --release --test bench -- --nocapture`. A constraint
//! solve is a **discrete, off-hot-path** action (inv. 4 untouched) — this measures + prints p50/p99 and
//! gates only against a CATASTROPHIC (≥100×) regression, so it never flakes in a debug workspace run.
//! Per `<benchmark_discipline>`: record OS/CPU/RAM/rustc + the exact `ezpz`/`faer` versions from Cargo.lock.

use std::time::Instant;

use metrocalk_constraint::{Axis, ConstraintDef, EzpzSolver, Sketch, Solver};

// The audit fixture: a constrained rectangle (40×25) + a circle tangent to its top edge.
fn fixture() -> Sketch {
    let mut s = Sketch::new();
    let p0 = s.add_point(0.05, -0.05);
    let p1 = s.add_point(39.7, 0.2);
    let p2 = s.add_point(40.3, 24.6);
    let p3 = s.add_point(-0.2, 25.4);
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
    s.add(ConstraintDef::HorizontalDistance {
        a: p0,
        b: p1,
        d: 40.0,
    });
    s.add(ConstraintDef::VerticalDistance {
        a: p1,
        b: p2,
        d: 25.0,
    });
    let cc = s.add_point(20.0, 15.0);
    let circle = s.add_circle(cc, 7.5);
    s.add(ConstraintDef::Fixed {
        point: cc,
        axis: Axis::X,
        value: 20.0,
    });
    s.add(ConstraintDef::CircleRadius { circle, r: 8.0 });
    s.add(ConstraintDef::LineTangentToCircle {
        l0: p3,
        l1: p2,
        circle,
    });
    s
}

#[test]
fn solve_latency() {
    let solver = EzpzSolver::new();
    let s = fixture();
    assert!(solver.solve(&s).is_ok(), "the bench fixture must solve");

    // Warm up (allocator + faer symbolic caches), then time N solves.
    for _ in 0..200 {
        let _ = solver.solve(&s);
    }
    let n = 2000;
    let mut us: Vec<f64> = Vec::with_capacity(n);
    for _ in 0..n {
        let t = Instant::now();
        let _ = solver.solve(&s);
        us.push(t.elapsed().as_secs_f64() * 1e6);
    }
    us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = us[n / 2];
    let p99 = us[(n * 99) / 100];
    println!("CONSTRAINT_SOLVE_LATENCY: p50={p50:.1}us p99={p99:.1}us n={n} (run --release for the real number)");

    // Not a hard perf gate — a loose sanity bound that passes in debug (faer is slow un-optimized) and only
    // catches a catastrophic ≥100× regression that would take the solve off "discrete/off-hot-path".
    assert!(
        p50 < 50_000.0,
        "solve p50 {p50:.1}us regressed catastrophically (must stay off the hot path)"
    );
}
