//! M15.6 (ADR-076) — **THE SPIKE** (deliverable #1): the measured go/no-go gate for intent + explainability
//! constraint solving. `ezpz` is the embedded solver math (behind the `/constraint` `Solver` trait, no
//! `ezpz::` leak); the moat is the LAYER, proven here on an embedded 2D sketch:
//!
//! - **Gate (a) — deterministic across peers via the witness-config-in-the-op** (the "flip" fix): a
//!   two-branch sketch (an "elbow up/down") is bit-reproducible from its stored witness across ≥2 runs AND
//!   across peers, and re-solving from the witness READ BACK OUT OF THE OP reaches the same branch. No other
//!   CAD can do this — none has a replayable op-stream to store the witness in.
//! - **Gate (b) — every over-constraint returns the MINIMAL conflicting set** in plain language (which
//!   constraints conflict, not a bare "over-defined"), reusing the shipped `metrocalk_authoring::Certificate`
//!   every-no seed (M13.5/ADR-054; the full M13.9 theorem, ADR-061, stays a named future).
//!
//! Plus the supporting properties: intent-inference proposes the constraint via the SHIPPED M9.4 ranker
//! (`reveal::intent_order`); a solve lands as ONE undoable op carrying its witness; concurrent constraint
//! edits merge clobber-free (Loro, inv. 1/3); a malformed sketch is explained, never a panic; and the M9.4
//! ranker + M13.5 Certificate are re-confirmed test-first in the current toolchain.
//!
//! A **headless CI gate — no dark test** (run under `cargo test --workspace`; `-- --nocapture` for the
//! numbers). If a gate failed, it would be reported and the milestone parked — not a flipping/unexplained
//! solver shipped.

use std::collections::HashMap;

use metrocalk_authoring::Certificate;
use metrocalk_constraint::{Axis, ConstraintDef, EzpzSolver, Sketch, SketchSolve, Solver};
use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::FlecsWorld;
use metrocalk_editor_shell::constraint_intent::{
    explain_conflict, propose_constraints, solve_and_land, witness_from_doc, SolveLanding,
    SKETCH_POINT,
};
use metrocalk_editor_shell::reveal::intent_order;

const RUNS: usize = 3; // the ≥2-runs reproducibility discipline

fn engine(peer: u64) -> Engine<FlecsWorld> {
    Engine::new(FlecsWorld::new(), peer)
}

// Create `n` bare point entities (one commit); the caller then solves+lands onto them.
fn make_points(e: &mut Engine<FlecsWorld>, n: usize) -> Vec<EntityId> {
    let ids: Vec<EntityId> = (0..n).map(|_| e.alloc_entity_id()).collect();
    let ops: Vec<Op> = ids
        .iter()
        .map(|&id| Op::CreateEntity { id, parent: None })
        .collect();
    e.commit("make-points", ops).unwrap();
    ids
}

// Read a SketchPoint field as f64 (handles the whole-number Integer arm — the fieldvalue gotcha).
#[allow(clippy::cast_precision_loss)]
fn field_num(e: &Engine<FlecsWorld>, id: EntityId, field: &str) -> Option<f64> {
    match e.get_field(id, SKETCH_POINT, field)? {
        FieldValue::Number(n) => Some(n),
        FieldValue::Integer(i) => Some(i as f64),
        _ => None,
    }
}

// The canonical "flip": p2 must be distance 6 from both p0(0,0) and p1(10,0) — two solutions (elbow
// up/down). The witness picks the branch.
fn elbow(up: bool) -> Sketch {
    let mut s = Sketch::new();
    let p0 = s.add_point(0.0, 0.0);
    let p1 = s.add_point(10.0, 0.0);
    let p2 = s.add_point(5.0, if up { 4.0 } else { -4.0 });
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

// An over-constrained sketch: two contradictory horizontal dimensions on the same pair. Returns the sketch
// and the indices of the two conflicting dimensions (the expected minimal conflicting set).
fn over_constrained() -> (Sketch, usize, usize) {
    let mut s = Sketch::new();
    let p0 = s.add_point(0.0, 0.0);
    let p1 = s.add_point(12.0, 0.0);
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
        axis: Axis::Y,
        value: 0.0,
    });
    let a = s.add(ConstraintDef::HorizontalDistance {
        a: p0,
        b: p1,
        d: 10.0,
    });
    let b = s.add(ConstraintDef::HorizontalDistance {
        a: p0,
        b: p1,
        d: 15.0,
    });
    (s, a, b)
}

// A fully-constrained 2-point segment (offset in x), for the concurrent-merge test.
fn segment_sketch(x0: f64) -> Sketch {
    let mut s = Sketch::new();
    let a = s.add_point(x0, 0.0);
    let b = s.add_point(x0 + 4.5, 0.1);
    s.add(ConstraintDef::Fixed {
        point: a,
        axis: Axis::X,
        value: x0,
    });
    s.add(ConstraintDef::Fixed {
        point: a,
        axis: Axis::Y,
        value: 0.0,
    });
    s.add(ConstraintDef::Fixed {
        point: b,
        axis: Axis::Y,
        value: 0.0,
    });
    s.add(ConstraintDef::HorizontalDistance { a, b, d: 5.0 });
    s
}

// ── GATE (a): deterministic across peers via the witness-config-in-the-op (no "flip") ────────────────────

#[test]
fn gate_a_deterministic_across_peers_via_witness_no_flip() {
    let solver = EzpzSolver::new();
    let sketch = elbow(true);

    // (1) The FLIP is real: same constraints, two witnesses → two DIFFERENT valid branches.
    let up = SketchSolve::compute(&elbow(true), &solver);
    let down = SketchSolve::compute(&elbow(false), &solver);
    assert!(
        up.satisfied && down.satisfied,
        "both branches are valid solves"
    );
    assert_ne!(
        up.solved_points, down.solved_points,
        "the flip exists — different witnesses reach different branches"
    );
    assert!(
        up.solved_point(2).y > 0.0 && down.solved_point(2).y < 0.0,
        "the witness picks the side of the line"
    );
    eprintln!(
        "gate(a) flip is real: up p2.y={:.4}  down p2.y={:.4}",
        up.solved_point(2).y,
        down.solved_point(2).y
    );

    // (2) Bit-reproducible across ≥RUNS runs from the SAME stored witness (no flip).
    let ids: Vec<String> = (0..RUNS)
        .map(|_| SketchSolve::compute(&sketch, &solver).identity())
        .collect();
    assert!(
        ids.iter().all(|h| *h == ids[0]),
        "same witness ⇒ identical canonical identity ×{RUNS}: {ids:?}"
    );
    eprintln!("gate(a) reproducible ×{RUNS}: {}", ids[0]);

    // (3) Two PEERS carrying the same witness converge to the same branch (no flip across peers).
    let peer_a = SketchSolve::compute(&sketch, &solver);
    let peer_b = SketchSolve::compute(&sketch, &solver);
    assert_eq!(
        peer_a.identity(),
        peer_b.identity(),
        "two peers, same witness ⇒ the same branch, bit-identical"
    );

    // (4) Re-solve from the witness STORED IN THE OP: land it, read wx/wy back out of the doc, re-solve →
    // the SAME branch. This is the mechanism no other CAD has (a replayable op-stream carrying the witness).
    let mut e = engine(1);
    let pts = make_points(&mut e, sketch.points.len());
    let stored = match solve_and_land(&mut e, &sketch, &pts, &solver) {
        SolveLanding::Solved { solve, .. } => solve,
        other => panic!("expected Solved, got {other:?}"),
    };
    let witness = witness_from_doc(&e, &pts).expect("the witness is stored in the op");
    let recovered = Sketch {
        points: witness,
        circles: sketch.circles.clone(),
        constraints: sketch.constraints.clone(),
    };
    let resolved = SketchSolve::compute(&recovered, &solver);
    assert_eq!(
        resolved.identity(),
        stored.identity(),
        "re-solving from the witness IN THE OP reaches the same branch"
    );
    assert!(
        stored.verify_replay(&solver),
        "the stored solve verifies-replay bit-for-bit"
    );
    eprintln!(
        "gate(a) witness-in-op re-solve identity: {}",
        resolved.identity()
    );
}

// ── GATE (b): every over-constraint returns the minimal conflicting set (reuse the every-no Certificate) ──

#[test]
fn gate_b_over_constraint_returns_minimal_conflicting_set() {
    let solver = EzpzSolver::new();
    let (sketch, ca, cb) = over_constrained();

    let res = solver.solve(&sketch);
    assert!(
        res.is_over_constrained(),
        "two conflicting dims ⇒ over-constrained: {res:?}"
    );

    let cert =
        explain_conflict(&sketch, &solver).expect("an over-constrained sketch has a minimal set");
    // The minimal set is EXACTLY the two conflicting dimensions — not the whole sketch.
    assert_eq!(
        cert.minimal,
        vec![ca, cb],
        "the minimal conflicting set is the two conflicting dims, not the whole sketch"
    );
    // Reuses the SHIPPED every-no `Certificate` (M13.5/ADR-054): a plain reason + one fact per conflict.
    let base: &Certificate = &cert.base;
    assert_eq!(
        base.unsat_core.len(),
        2,
        "one plain-language fact per conflicting constraint"
    );
    assert!(
        base.reason.contains("can't all hold"),
        "plain-language why: {}",
        base.reason
    );
    assert!(
        base.unsat_core
            .iter()
            .all(|d| d.contains("horizontal distance")),
        "the core names the actual conflicting dimensions: {:?}",
        base.unsat_core
    );
    eprintln!("gate(b) minimal conflicting set: {}", base.reason);
}

// ── Supporting properties ────────────────────────────────────────────────────────────────────────────────

#[test]
fn constraint_solve_lands_as_one_undoable_op_carrying_its_witness() {
    let solver = EzpzSolver::new();
    let sketch = elbow(true);
    let mut e = engine(1);
    let pts = make_points(&mut e, sketch.points.len());
    assert!(
        field_num(&e, pts[2], "x").is_none(),
        "no solved coords before the solve"
    );

    let committed = match solve_and_land(&mut e, &sketch, &pts, &solver) {
        SolveLanding::Solved { committed, .. } => committed,
        other => panic!("expected Solved, got {other:?}"),
    };
    assert_eq!(committed, sketch.points.len());
    // The solved coordinates AND the witness (wx/wy — the branch-fixing initial config) are in the doc.
    assert!(field_num(&e, pts[2], "x").is_some(), "solved x landed");
    assert!(
        field_num(&e, pts[2], "wx").is_some(),
        "the witness is IN the op"
    );

    // ONE undoable transaction — a single Ctrl-Z peels the whole solve.
    assert!(e.undo(), "undo peels the solve");
    assert!(
        field_num(&e, pts[2], "x").is_none(),
        "one undo removed ALL the solved coords (one transaction)"
    );
}

#[test]
fn intent_inference_proposes_the_constraint_via_the_m94_ranker() {
    // Geometry already close to two relationships: p2,p3 nearly coincident (0.004 mm); the segment p0→p1
    // nearly horizontal (0.02 mm). The SHIPPED M9.4 ranker (`reveal::intent_order`) surfaces the closest
    // strongest signal first — the ONE ranker, never a parallel heuristic.
    let mut s = Sketch::new();
    let p0 = s.add_point(0.0, 0.0);
    let p1 = s.add_point(10.0, 0.02);
    s.add_point(5.0, 5.0); // p2, p3 are nearly coincident — detected by scanning all point pairs
    s.add_point(5.004, 5.0);
    let segments = [[p0, p1]];
    let recency = HashMap::new();

    let proposals = propose_constraints(&s, &segments, &recency, 0.5);
    assert!(
        proposals.len() >= 2,
        "geometry near two relationships yields proposals: {proposals:?}"
    );
    // Coincident (residual 0.004) outranks Horizontal (residual 0.02) — proximity primary in intent_order.
    assert!(
        matches!(proposals[0].constraint, ConstraintDef::Coincident { .. }),
        "the closest/strongest proposal leads: {:?}",
        proposals[0].constraint
    );
    assert!(
        proposals[0].why.contains("coincident"),
        "a plain-language why: {}",
        proposals[0].why
    );
    assert!(
        proposals[0].residual_mm < proposals[1].residual_mm,
        "ranked nearer-first (the ranker's proximity term)"
    );
    eprintln!(
        "intent proposals (ranked): {:?}",
        proposals
            .iter()
            .map(|p| (p.affinity, p.residual_mm))
            .collect::<Vec<_>>()
    );
}

#[test]
fn over_constraint_is_explained_not_committed() {
    let solver = EzpzSolver::new();
    let (sketch, _, _) = over_constrained();
    let mut e = engine(1);
    let pts = make_points(&mut e, sketch.points.len());
    match solve_and_land(&mut e, &sketch, &pts, &solver) {
        SolveLanding::OverConstrained(cert) => assert!(!cert.minimal.is_empty()),
        other => panic!("expected OverConstrained, got {other:?}"),
    }
    assert!(
        field_num(&e, pts[0], "x").is_none(),
        "an over-constrained sketch is EXPLAINED, never committed as a silent least-squares fudge"
    );
}

#[test]
fn malformed_sketch_is_explained_not_a_panic() {
    let solver = EzpzSolver::new();
    let mut e = engine(1);
    let pts = make_points(&mut e, 1);
    let mut s = Sketch::new();
    s.add_point(0.0, 0.0);
    s.add(ConstraintDef::Distance { a: 0, b: 9, d: 5.0 }); // out-of-range point
    match solve_and_land(&mut e, &s, &pts, &solver) {
        SolveLanding::Invalid(msg) => assert!(msg.contains("point 9"), "explained: {msg}"),
        other => panic!("expected Invalid, got {other:?}"),
    }
    assert!(
        field_num(&e, pts[0], "x").is_none(),
        "nothing committed for a malformed sketch"
    );
}

#[test]
fn concurrent_constraint_edits_merge_clobber_free() {
    let solver = EzpzSolver::new();
    for _ in 0..RUNS {
        // Shared base: 4 points; two peers fork it.
        let mut base = engine(1);
        let pts = make_points(&mut base, 4);
        let snapshot = base.snapshot();
        let mut peer_a = engine(10);
        peer_a.merge(&snapshot).unwrap();
        let mut peer_b = engine(20);
        peer_b.merge(&snapshot).unwrap();

        // Peer A solves a segment touching points {0,1}; Peer B one touching {2,3} — disjoint entities.
        let land_a = solve_and_land(
            &mut peer_a,
            &segment_sketch(0.0),
            &[pts[0], pts[1]],
            &solver,
        );
        let land_b = solve_and_land(
            &mut peer_b,
            &segment_sketch(100.0),
            &[pts[2], pts[3]],
            &solver,
        );
        assert!(
            matches!(land_a, SolveLanding::Solved { .. }),
            "A solved: {land_a:?}"
        );
        assert!(
            matches!(land_b, SolveLanding::Solved { .. }),
            "B solved: {land_b:?}"
        );

        // Cross-merge (the CRDT merge + inv-3 validation, both directions).
        let report_a = peer_a.merge(&peer_b.export_updates()).unwrap();
        let report_b = peer_b.merge(&peer_a.export_updates()).unwrap();
        assert_eq!(
            report_a.total_violations(),
            0,
            "merge A<-B is clean (inv-3)"
        );
        assert_eq!(report_b.total_violations(), 0, "merge B<-A is clean");

        // No lost edit: A's points AND B's points are solved on BOTH peers.
        for pe in [&peer_a, &peer_b] {
            assert!(
                field_num(pe, pts[0], "x").is_some(),
                "A's solve survived the merge"
            );
            assert!(
                field_num(pe, pts[3], "x").is_some(),
                "B's solve survived the merge"
            );
        }
    }
}

#[test]
fn carry_forward_m94_ranker_and_m135_certificate_reconfirmed() {
    use std::cmp::Ordering;
    // (M9.4/ADR-028) re-confirm `reveal::intent_order` in the CURRENT toolchain: nearer first, then higher
    // affinity, then more-recent, then lower stable-id — the ONE ranker the intent-inference rests on.
    assert_eq!(
        intent_order((0.1, 0, 0, 0), (0.2, 9, 9, 9)),
        Ordering::Less,
        "nearer wins"
    );
    assert_eq!(
        intent_order((0.5, 7, 0, 0), (0.5, 2, 0, 0)),
        Ordering::Less,
        "then higher affinity"
    );
    assert_eq!(
        intent_order((0.5, 3, 100, 0), (0.5, 3, 50, 0)),
        Ordering::Less,
        "then more-recent"
    );
    assert_eq!(
        intent_order((0.5, 3, 9, 1), (0.5, 3, 9, 2)),
        Ordering::Less,
        "then lower stable-id"
    );

    // (M13.5/ADR-054) re-confirm the every-no `Certificate` type is present + shaped as M15.3/M15.6 rely on.
    let cert = Certificate {
        reason: "over-constrained".to_string(),
        unsat_core: vec![
            "#3 horizontal distance".into(),
            "#4 horizontal distance".into(),
        ],
    };
    assert_eq!(
        cert.unsat_core.len(),
        2,
        "the seed carries a non-empty unsat-core"
    );
    assert!(
        !cert.reason.is_empty(),
        "the seed carries a plain-language reason"
    );
}

#[test]
fn spike_gate_summary_go() {
    let solver = EzpzSolver::new();
    // Gate (a): deterministic-across-peers via the witness (bit-reproducible + verifies-replay).
    let a1 = SketchSolve::compute(&elbow(true), &solver);
    let a2 = SketchSolve::compute(&elbow(true), &solver);
    let gate_a = a1.satisfied && a1.identity() == a2.identity() && a1.verify_replay(&solver);
    // Gate (b): over-constraint → the exact minimal conflicting set.
    let (sketch, ca, cb) = over_constrained();
    let gate_b = explain_conflict(&sketch, &solver).is_some_and(|c| c.minimal == vec![ca, cb]);

    assert!(gate_a, "GATE (a) deterministic-across-peers must hold");
    assert!(gate_b, "GATE (b) minimal-conflicting-set must hold");
    eprintln!(
        "M15.6 SPIKE = GO — gate(a) deterministic-across-peers via witness \u{2713} · gate(b) minimal-conflicting-set \u{2713}"
    );
}
