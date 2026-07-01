//! M15.3 (ADR-073) — THE SPIKE (deliverable #1): the measured go/no-go gate for **semantic PMI / GD&T as
//! relational-ECS-native**, on imported B-rep, kernel-free.
//!
//! Proves the properties, each measured (≥2 runs where determinism is the claim):
//! - **(a)** a semantic GD&T feature-control-frame **attaches to a real imported STEP B-rep face** (M15.0)
//!   as a **typed ECS relationship** — queryable as `{feature, characteristic, value, datum, standard}`
//!   structured data, **not a parsed label** (machine-readable by construction).
//! - **(b)** a tolerance **stack-up failure renders as a traced derivation certificate** — which features,
//!   which stage, the suggested loosening — reusing the shipped M13.5 `Certificate` (the every-no seed).
//! - **(c)** the Monte-Carlo stack-up is **deterministic + seedable** — the same seed → the same result,
//!   bit-for-bit across ≥2 runs (reproducible analysis — **not** metrology).
//! - **(d)** carry-forward re-verification (test-first): the **M15.0 STEP import** still produces stable,
//!   referenceable B-rep faces, and the seeded determinism holds in the current toolchain.
//! - **structural:** a **graphical-only annotation is not representable** (the type stores only the semantic
//!   tuple — the compile-time guarantee); **AI-GD&T is a validated patch** (overreach rejected-as-UX);
//!   **concurrent PMI edits merge clobber-free** with merge-validation (inv. 1/3).
//!
//! Run with `-- --nocapture` to print the measured numbers. A headless CI gate (`cargo test --workspace`) —
//! no dark test. The PMI relationships are wasm-clean ECS data; the STEP import path follows M15.0
//! (native/server), the standing boundary.

use metrocalk_assets::AssetStore;
use metrocalk_core::{Engine, EntityId, FieldValue};
use metrocalk_ecs::FlecsWorld;
use metrocalk_editor_shell::cad_intent::import_step;
use metrocalk_editor_shell::capscene::{CapResolver, CapScene};
use metrocalk_editor_shell::{
    ai_adjust_tolerance, attach_fcf, fcfs_on, is_cad_feature, read_fcf, Characteristic,
    Contributor, Fcf, Stackup, StackupAnalysis, Standard,
};
use metrocalk_interchange::{CadInterchange, StepInterchange};

const RUNS: usize = 3; // the ≥2-runs reproducibility discipline (<benchmark_discipline>)
const CUBE_STEP: &str = include_str!("../../interchange/tests/fixtures/cube_ap242.step");

/// A fresh engine with a STEP part imported (the M15.0 dependency) → its referenceable B-rep faces.
fn engine_with_step() -> (Engine<FlecsWorld>, Vec<EntityId>) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    engine.set_capability_resolver(Box::new(CapResolver::from_scene(&scene)));
    let mut store = AssetStore::new();
    let cad = StepInterchange
        .import(CUBE_STEP.as_bytes())
        .expect("STEP import");
    let imported = import_step(&mut engine, &scene, &mut store, &cad).expect("map to entities");
    (engine, imported.faces)
}

/// A 4-feature linear stack-up (a shaft-in-bore clearance). Stage 3's ∅0.10 spec is tighter than its
/// process (σ 0.05 → Cpk 0.67) — infeasible; the "loosen to ∅0.15 = 99.7% yield" case the certificate fixes.
fn demo_stackup() -> Stackup {
    Stackup {
        name: "shaft-in-bore clearance".to_string(),
        contributors: vec![
            Contributor {
                feature: "face #12 (bracket seat)".to_string(),
                characteristic: Characteristic::Position,
                nominal_mm: 10.0,
                tolerance_mm: 0.15,
                process_sigma_mm: 0.05,
                direction: -1.0,
            },
            Contributor {
                feature: "face #34 (bolt boss)".to_string(),
                characteristic: Characteristic::Perpendicularity,
                nominal_mm: 3.0,
                tolerance_mm: 0.12,
                process_sigma_mm: 0.04,
                direction: 1.0,
            },
            Contributor {
                feature: "face #56 (spacer)".to_string(),
                characteristic: Characteristic::Position,
                nominal_mm: 5.0,
                tolerance_mm: 0.10,
                process_sigma_mm: 0.05,
                direction: 1.0,
            },
            Contributor {
                feature: "face #78 (cover)".to_string(),
                characteristic: Characteristic::Flatness,
                nominal_mm: 2.5,
                tolerance_mm: 0.09,
                process_sigma_mm: 0.03,
                direction: -1.0,
            },
        ],
        gap_nominal_mm: 0.50,
        gap_min_mm: 0.20,
        gap_max_mm: 0.80,
        target_yield: 0.997,
    }
}

// ── (a) a semantic FCF attaches to a real imported B-rep face as machine-readable structured data ─────────

#[test]
fn property_a_semantic_fcf_on_a_real_brep_face_is_machine_readable() {
    let (mut engine, faces) = engine_with_step();
    assert_eq!(
        faces.len(),
        6,
        "the imported STEP part has 6 referenceable faces"
    );

    // A true-position FCF: ∅0.10 mm relative to a datum face, ASME Y14.5.
    let fcf = Fcf {
        feature: faces[0],
        characteristic: Characteristic::Position,
        tolerance_mm: 0.10,
        datum: Some(faces[1]),
        standard: Standard::AsmeY14_5,
    };
    let id = attach_fcf(&mut engine, &fcf).expect("attach a semantic FCF to a real B-rep face");

    // MACHINE-READABLE BY CONSTRUCTION: read the structured tuple back — the typed enum, not a parsed label.
    let read = read_fcf(&engine, id).expect("read the FCF back as structured data");
    assert_eq!(read.feature, faces[0], "{{feature}} is the toleranced face");
    assert_eq!(
        read.characteristic,
        Characteristic::Position,
        "{{characteristic}} is a typed enum"
    );
    assert!(
        (read.tolerance_mm - 0.10).abs() < 1e-12,
        "{{value}} is the numeric zone"
    );
    assert_eq!(
        read.datum,
        Some(faces[1]),
        "{{datum}} is the datum face entity"
    );
    assert_eq!(
        read.standard,
        Standard::AsmeY14_5,
        "{{standard}} is a typed enum"
    );

    // It is a relationship on the face (the C6-style relational read), and one undoable transaction.
    assert_eq!(fcfs_on(&engine, faces[0]), vec![id]);
    assert!(is_cad_feature(&engine, faces[0]));
    assert!(engine.undo(), "one Ctrl-Z peels the FCF");
    assert!(
        read_fcf(&engine, id).is_none(),
        "the FCF is gone after undo"
    );

    println!(
        "[a] semantic FCF on a real B-rep face: position \u{2300}0.10 mm | datum = face entity | ASME_Y14.5 \u{2014} queried as structured data (not a label)"
    );
    println!(
        "::notice::m15.3-pmi-attach faces={} semantic-fcf-queryable=true",
        faces.len()
    );
}

#[test]
fn property_a_graphical_only_annotation_is_not_representable() {
    // THE STRUCTURAL CLAIM (a compile-time guarantee, exercised through the read path): an FCF is built from
    // a CLOSED `Characteristic` enum + a numeric zone — there is NO `label: String` / free-text field on the
    // type, so a "graphical-only" annotation cannot be constructed. And a stored string that isn't a known
    // characteristic is NOT a valid FCF (so an arbitrary drawn callout is unrepresentable as PMI).
    assert!(
        Characteristic::from_canonical("a hand-drawn callout").is_none(),
        "an arbitrary label is not a valid characteristic — PMI is semantic by construction"
    );
    // Every shipped characteristic round-trips through its typed enum (semantic, never a label).
    for c in Characteristic::ALL {
        assert_eq!(Characteristic::from_canonical(c.canonical()), Some(c));
    }
    println!("[a] graphical-only annotation is not representable (semantic-by-construction type)");
}

// ── (b) a stack-up failure is a traced derivation certificate (reuse the M13.5 every-no seed) ─────────────

#[test]
fn property_b_stackup_failure_is_a_traced_derivation_certificate() {
    let s = demo_stackup();
    match s.analyze(0x00C0_FFEE, 50_000) {
        StackupAnalysis::Fail(cert) => {
            // A CERTIFICATE (an unsat-core / derivation), NOT a copy string — reuses M13.5's `Certificate`.
            assert!(
                !cert.base.unsat_core.is_empty(),
                "the certificate carries an unsat-core"
            );
            assert!(cert.base.reason.contains("not manufacturable"));
            assert!(
                cert.base.reason.contains("stage 3"),
                "it names the failing STAGE"
            );

            // The trace names every contributor + its Cpk (which features contribute).
            assert_eq!(cert.contributions.len(), 4, "all 4 stages traced");
            assert!(
                cert.contributions[2].cpk < 1.0,
                "stage 3 is the infeasible one"
            );

            // The fix: loosen the over-tight ∅0.10 to ∅0.15 (3σ, Cpk 1.0) — the prompt's exact example.
            let fix = cert.fix.as_ref().expect("a suggested fix");
            assert_eq!(fix.feature, "face #56 (spacer)");
            assert!(fix.loosen, "the fix loosens the over-tight tolerance");
            assert!((fix.from_mm - 0.10).abs() < 1e-9 && (fix.to_mm - 0.15).abs() < 1e-9);

            println!("[b] derivation certificate:");
            println!("    reason: {}", cert.base.reason);
            for u in &cert.base.unsat_core {
                println!("    unsat-core: {u}");
            }
            println!("    fix: {}", fix.rationale);
            println!(
                "::notice::m15.3-stackup-certificate stage=3 from=0.10mm to=0.15mm traced=true unsat-core={}",
                cert.base.unsat_core.len()
            );
        }
        StackupAnalysis::Pass { .. } => panic!("stage-3 ∅0.10 (Cpk 0.67) must FAIL the stack-up"),
    }
}

// ── (c) the Monte-Carlo is deterministic + seedable (measured ≥2 runs) ────────────────────────────────────

#[test]
fn property_c_monte_carlo_is_deterministic_and_seedable() {
    let s = demo_stackup();
    let seed = 0x5EED_D00D;
    let samples = 50_000;

    let mut results = Vec::new();
    for run in 0..RUNS {
        let mc = s.monte_carlo(seed, samples);
        println!(
            "[c] run {run}: seed={seed:#x} samples={} pass={} yield={:.4}%",
            mc.samples,
            mc.pass,
            100.0 * mc.yield_fraction()
        );
        results.push(mc);
    }
    // Bit-for-bit reproducible: every run's integer result is identical (the canonical-result discipline).
    assert!(
        results.windows(2).all(|w| w[0] == w[1]),
        "the seeded Monte-Carlo is deterministic across {RUNS} runs: {results:?}"
    );
    // And the analysis verdict (the certificate's mc) is reproducible too.
    let a = s.analyze(seed, samples);
    let b = s.analyze(seed, samples);
    assert_eq!(a, b, "the full seeded analysis is reproducible");

    println!(
        "::notice::m15.3-montecarlo-reproducible seed={seed:#x} samples={samples} pass={} runs={RUNS} identical=true",
        results[0].pass
    );
}

// ── (d) carry-forward re-verification: M15.0 STEP import + the seeded determinism, in THIS toolchain ──────

#[test]
fn property_d_carry_forward_reverify_step_import_and_determinism() {
    // Re-confirm (test-first) that the M15.0 STEP import produces stable, referenceable B-rep faces before
    // we rest the PMI-attach claim on it — re-run, don't assume (the ADR-070 carry-forward).
    for run in 0..RUNS {
        let (engine, faces) = engine_with_step();
        assert_eq!(faces.len(), 6, "run {run}: 6 referenceable faces");
        for &f in &faces {
            assert!(
                is_cad_feature(&engine, f),
                "each face is a real B-rep feature"
            );
            assert!(
                matches!(
                    engine.get_field(f, "CadFace", "step_id"),
                    Some(FieldValue::Integer(n)) if n > 0
                ),
                "each face carries a stable STEP #id"
            );
        }
    }
    // Re-confirm the seeded determinism (the M13.1/ADR-050 property the Monte-Carlo rests on).
    let s = demo_stackup();
    assert_eq!(
        s.monte_carlo(42, 10_000),
        s.monte_carlo(42, 10_000),
        "seeded determinism holds in the current toolchain"
    );
    println!(
        "[d] carry-forward re-verified: M15.0 STEP faces stable ×{RUNS}; seeded determinism holds"
    );
}

// ── headless: one undoable op with provenance; AI-GD&T validated; concurrent PMI merges clobber-free ──────

#[test]
fn headless_ai_gdt_is_a_validated_patch() {
    let (mut engine, faces) = engine_with_step();
    let id = attach_fcf(
        &mut engine,
        &Fcf {
            feature: faces[0],
            characteristic: Characteristic::Position,
            tolerance_mm: 0.10,
            datum: Some(faces[1]),
            standard: Standard::AsmeY14_5,
        },
    )
    .unwrap();

    // A valid AI tolerance loosening → applied through apply_ai_patch as one undoable, schema-validated tx.
    let ok = ai_adjust_tolerance(&mut engine, id, 0.15, "ai-1");
    assert_eq!(ok.confirms, vec!["ai-1".to_string()]);
    assert!(ok.rejects.is_empty());
    assert!((read_fcf(&engine, id).unwrap().tolerance_mm - 0.15).abs() < 1e-12);

    // Overreach is rejected-as-UX (a raw annotation path does not exist): a negative tolerance, nothing applied.
    let bad = ai_adjust_tolerance(&mut engine, id, -0.5, "ai-2");
    assert!(bad.confirms.is_empty() && bad.rejects.len() == 1);
    assert!(
        (read_fcf(&engine, id).unwrap().tolerance_mm - 0.15).abs() < 1e-12,
        "nothing applied"
    );

    // One undoable transaction with provenance (the op-log entry): Ctrl-Z reverts the AI edit.
    assert!(engine.undo(), "undo the AI tolerance edit");
    assert!((read_fcf(&engine, id).unwrap().tolerance_mm - 0.10).abs() < 1e-12);
    println!("[headless] AI-GD&T is a validated, undoable patch; overreach rejected-as-UX");
}

#[test]
fn headless_concurrent_pmi_edits_merge_clobber_free() {
    // Two peers fork a shared imported part; each attaches a distinct FCF; the CRDT merges with no lost edit
    // (Loro, inv. 1) and merge-validation is clean (inv. 3) — the digital-thread property applied to PMI.
    let (base, faces) = engine_with_step();
    let (f0, f1, f2) = (faces[0], faces[1], faces[2]);
    let snapshot = base.snapshot();

    let mut peer_a = Engine::new(FlecsWorld::new(), 10);
    peer_a.merge(&snapshot).unwrap();
    let mut peer_b = Engine::new(FlecsWorld::new(), 20);
    peer_b.merge(&snapshot).unwrap();

    // Peer A tolerances face 0; peer B tolerances face 2 (a flatness) — concurrent, different features.
    let a_fcf = attach_fcf(
        &mut peer_a,
        &Fcf {
            feature: f0,
            characteristic: Characteristic::Position,
            tolerance_mm: 0.10,
            datum: Some(f1),
            standard: Standard::AsmeY14_5,
        },
    )
    .unwrap();
    let b_fcf = attach_fcf(
        &mut peer_b,
        &Fcf {
            feature: f2,
            characteristic: Characteristic::Flatness,
            tolerance_mm: 0.02,
            datum: None,
            standard: Standard::IsoGps,
        },
    )
    .unwrap();

    let report_a = peer_a.merge(&peer_b.export_updates()).unwrap();
    let report_b = peer_b.merge(&peer_a.export_updates()).unwrap();
    assert_eq!(report_a.total_violations(), 0, "merge A<-B clean (inv. 3)");
    assert_eq!(report_b.total_violations(), 0, "merge B<-A clean (inv. 3)");

    // No lost edit: both FCFs survive on both peers.
    for peer in [&peer_a, &peer_b] {
        assert_eq!(
            read_fcf(peer, a_fcf).map(|f| f.characteristic),
            Some(Characteristic::Position),
            "A's position tolerance survived"
        );
        assert_eq!(
            read_fcf(peer, b_fcf).map(|f| f.characteristic),
            Some(Characteristic::Flatness),
            "B's flatness tolerance survived (no lost edit)"
        );
    }
    // Convergence: both peers reach the same canonical logical state (the M15.1 digital-thread property).
    assert_eq!(
        peer_a.canonical_state(),
        peer_b.canonical_state(),
        "peers converge"
    );
    println!("[headless] concurrent PMI edits merged clobber-free; peers converged (inv. 1/3)");
}
