//! Cross-ISA determinism harness for CI (`constraint-determinism.yml`).
//!
//! Solves a fixed, non-trivial sketch from a fixed witness and prints the canonical identity as
//! `FINAL_CONSTRAINT_HASH = <hex>`. Run across the OS matrix (ubuntu/windows = x86_64, macos = arm64): the
//! identity must be IDENTICAL on every arch — the quantized canonical state is cross-ISA-portable even
//! though the raw f64 (faer/gemm SIMD dispatch) is not (ADR-076). Also asserts self-consistency across two
//! runs + `verify_replay` (so a determinism regression FAILS CI — no dark test). Native-only reasoning; a
//! non-blocking wasm leg mirrors the SDF precedent (the ADR-020 web=server-authoritative boundary).

use metrocalk_constraint::{Axis, ConstraintDef, EzpzSolver, Sketch, SketchSolve};

// A constrained rectangle (40×25) + a circle tangent to its top edge + a two-branch "elbow" whose witness
// picks the branch — exercises dimensions, tangency, and the flip fix in one fixture.
fn fixture() -> Sketch {
    let mut s = Sketch::new();
    // Rectangle corners.
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
    // A circle centred inside, tangent to the top edge p3→p2, radius 8.
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

fn main() {
    let solver = EzpzSolver::new();
    let s = fixture();

    let a = SketchSolve::compute(&s, &solver);
    let b = SketchSolve::compute(&s, &solver);
    assert!(a.satisfied, "the CI fixture must solve (got {a:?})");
    assert_eq!(
        a.identity(),
        b.identity(),
        "same witness ⇒ identical canonical identity (2 runs)"
    );
    assert!(
        a.verify_replay(&solver),
        "replay from the stored witness must reproduce bit-for-bit"
    );

    let hex = a.identity();
    let hex = hex.trim_start_matches("mtksketch:");
    println!("solver: {}", solver_name());
    println!("FINAL_CONSTRAINT_HASH = {hex}");
}

fn solver_name() -> &'static str {
    use metrocalk_constraint::Solver;
    EzpzSolver::new().name()
}
