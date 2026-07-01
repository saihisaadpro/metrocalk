//! M15.5 (ADR-075) — THE SPIKE (deliverable #1): the measured go/no-go gate for **STEP AP242 + semantic-PMI
//! interop hardening**. A STEP AP242 file that carries semantic PMI **survives a round-trip**
//! (import → the M15.3 typed ECS PMI → re-export to STEP AP242 → re-import) with the **PMI still SEMANTIC**
//! (queryable `{feature, characteristic, value, datum, standard}` structured data — **not** downgraded to a
//! graphical callout), the **fidelity MEASURED** (a table, not a badge), and the geometry within the M15.0
//! declared tolerance budget — **measured ≥2 runs**.
//!
//! Properties (each measured; determinism claims ≥2 runs):
//! - **(a) THE GATE** — a STEP AP242 part with semantic PMI round-trips with **every FCF still semantic** and
//!   **fully faithful** (value + datum + standard preserved), the geometry deviation ≤ the M15.0 budget,
//!   **reproducible ×3** (byte-identical STEP + identical fidelity table).
//! - **(b)** a re-imported FCF is **queryable typed structured data** (the closed `Characteristic`/`Standard`
//!   enum, not a parsed label), attached to the **geometrically-correct** re-imported face, one undoable tx.
//! - **(c)** the **fidelity is measured + published** — the per-characteristic table (semantic/value/datum/
//!   standard), computed deterministically.
//! - **(d)** carry-forward re-verification (test-first): the **M15.0 STEP round-trip budget** + the **M15.3
//!   semantic-PMI representation** hold in the current toolchain — re-run, not assumed.
//! - **headless:** a malformed / oversized STEP-with-PMI file is **bounds-checked + explained, never a panic**;
//!   a **graphical-only** callout is **not** promoted to semantic (the honest downgrade, noted).
//!
//! A headless CI gate (`cargo test --workspace`) — a regressed fidelity number or a budget breach fails CI, no
//! dark test. The STEP import path follows M15.0 (native/server), the standing boundary; the semantic PMI is
//! wasm-clean ECS data.

use metrocalk_assets::AssetStore;
use metrocalk_core::{Engine, EntityId};
use metrocalk_ecs::FlecsWorld;
use metrocalk_editor_shell::capscene::{CapResolver, CapScene};
use metrocalk_editor_shell::{
    attach_fcf, collect_semantic_fcfs, export_step_pmi, fcfs_on, import_step_text,
    measure_fidelity, read_fcf, reimport_with_pmi, scene_with_pmi, Characteristic, Fcf,
    RoundTripFidelity, Standard,
};
use metrocalk_interchange::{
    round_trip_deviation, CadInterchange, CadScene, StepInterchange, ROUND_TRIP_BUDGET,
};

const RUNS: usize = 3; // the ≥2-runs reproducibility discipline (<benchmark_discipline>)
const CUBE_STEP: &str = include_str!("../../interchange/tests/fixtures/cube_ap242.step");

/// A fresh engine with the AP242 cube imported → its 6 referenceable B-rep faces + the neutral `CadScene`.
fn engine_with_step() -> (Engine<FlecsWorld>, Vec<EntityId>, CadScene) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    engine.set_capability_resolver(Box::new(CapResolver::from_scene(&scene)));
    let mut store = AssetStore::new();
    let cad = StepInterchange
        .import(CUBE_STEP.as_bytes())
        .expect("STEP import");
    let imported = metrocalk_editor_shell::import_step(&mut engine, &scene, &mut store, &cad)
        .expect("map to entities");
    (engine, imported.faces, cad)
}

/// Attach one FCF per declared characteristic across the faces (form = datumless; orientation/location get a
/// datum). 10 distinct `(face, characteristic)` FCFs — the full declared subset.
#[allow(clippy::cast_precision_loss)] // i is 0..9 — the usize→f64 cast is exact
fn attach_all_characteristics(engine: &mut Engine<FlecsWorld>, faces: &[EntityId]) {
    for (i, characteristic) in Characteristic::ALL.into_iter().enumerate() {
        let feature = faces[i % faces.len()];
        let datum = characteristic
            .needs_datum()
            .then(|| faces[(i + 1) % faces.len()]);
        attach_fcf(
            engine,
            &Fcf {
                feature,
                characteristic,
                tolerance_mm: 0.01 * (i as f64 + 1.0),
                datum,
                standard: if i % 2 == 0 {
                    Standard::AsmeY14_5
                } else {
                    Standard::IsoGps
                },
            },
        )
        .expect("attach a semantic FCF to a real B-rep face");
    }
}

/// Do ONE full round-trip: cube + 10 FCFs → export STEP AP242 → re-import → re-attach → measure fidelity.
/// Returns `(step_text, fidelity)` so a caller can assert reproducibility across runs.
fn round_trip_once() -> (String, RoundTripFidelity) {
    // 1) the source: an imported STEP part with semantic PMI attached (the M15.3 typed ECS relationships).
    let (mut engine_a, faces_a, cad_a) = engine_with_step();
    attach_all_characteristics(&mut engine_a, &faces_a);
    let original = collect_semantic_fcfs(&engine_a, &faces_a);

    // 2) serialize the PMI into the scene and re-export to STEP AP242 (semantic geometric_tolerance entities).
    let scene_with = scene_with_pmi(&engine_a, &faces_a, cad_a.clone());
    let step_text = export_step_pmi(&scene_with).expect("re-export STEP with PMI");

    // 3) re-import the STEP AP242 file (geometry + PMI) into a fresh engine and re-attach the FCFs.
    let cad_b = import_step_text(&step_text).expect("re-import STEP with PMI");
    let mut world_b = FlecsWorld::new();
    let scene_b = CapScene::intern(&mut world_b);
    let mut engine_b = Engine::new(world_b, 2);
    engine_b.set_capability_resolver(Box::new(CapResolver::from_scene(&scene_b)));
    let mut store_b = AssetStore::new();
    let faces_b =
        reimport_with_pmi(&mut engine_b, &scene_b, &mut store_b, &cad_b).expect("re-attach PMI");
    let reimported = collect_semantic_fcfs(&engine_b, &faces_b);

    // 4) MEASURE the fidelity (semantic survival + value/datum/standard) + the geometry deviation.
    let geometry_dev = round_trip_deviation(&cad_a, &cad_b);
    let fidelity = measure_fidelity(&original, &reimported, geometry_dev);
    (step_text, fidelity)
}

// ── (a) THE GATE: semantic PMI survives the round-trip still semantic, measured, within budget, ≥2 runs ─────

#[test]
fn property_a_the_gate_semantic_pmi_round_trips_measured_and_reproducible() {
    let mut runs = Vec::new();
    for run in 0..RUNS {
        let (step_text, fidelity) = round_trip_once();
        let (survived, total) = fidelity.semantic_survival();
        println!(
            "[a] run {run}: {survived}/{total} FCFs semantic | fully-faithful={} | geometry-dev={:e} (budget {:e})",
            fidelity.fully_faithful(),
            fidelity.geometry_deviation,
            ROUND_TRIP_BUDGET
        );

        // THE GATE: every FCF survived as SEMANTIC machine-readable data (not graphical, not dropped) and is
        // fully faithful (value + datum + standard preserved).
        assert!(
            fidelity.all_semantic(),
            "run {run}: every FCF must survive SEMANTIC (not downgraded to graphical): {fidelity:?}"
        );
        assert!(
            fidelity.fully_faithful(),
            "run {run}: every FCF round-trips fully faithfully (value+datum+standard): {fidelity:?}"
        );
        assert_eq!(
            (survived, total),
            (10, 10),
            "all 10 declared characteristics"
        );

        // The geometry is within the M15.0 declared budget (PMI didn't perturb the vertices).
        assert!(
            fidelity.geometry_deviation <= ROUND_TRIP_BUDGET,
            "run {run}: geometry deviation {:e} <= budget {:e}",
            fidelity.geometry_deviation,
            ROUND_TRIP_BUDGET
        );
        runs.push((step_text, fidelity));
    }

    // Reproducible ×RUNS: byte-identical STEP text AND identical fidelity table (a single match is not proof).
    assert!(
        runs.windows(2).all(|w| w[0].0 == w[1].0),
        "the exported STEP AP242 (with PMI) is byte-identical across {RUNS} runs"
    );
    assert!(
        runs.windows(2).all(|w| w[0].1 == w[1].1),
        "the measured fidelity table is identical across {RUNS} runs"
    );

    println!(
        "::notice::m15.5-pmi-roundtrip semantic={}/10 fully-faithful=true geometry-dev-le-budget=true runs={RUNS} reproducible=true",
        runs[0].1.semantic_survival().0
    );
}

// ── (b) a re-imported FCF is queryable typed structured data on the right face, one undoable tx ──────────────

#[test]
fn property_b_reimported_fcf_is_queryable_typed_data_not_a_label() {
    // Round-trip a single, unambiguous FCF and prove the re-imported one reads back as the typed tuple.
    let (mut engine_a, faces_a, cad_a) = engine_with_step();
    attach_fcf(
        &mut engine_a,
        &Fcf {
            feature: faces_a[0],
            characteristic: Characteristic::Position,
            tolerance_mm: 0.10,
            datum: Some(faces_a[1]),
            standard: Standard::AsmeY14_5,
        },
    )
    .unwrap();

    let scene_with = scene_with_pmi(&engine_a, &faces_a, cad_a);
    let step_text = export_step_pmi(&scene_with).unwrap();
    let cad_b = import_step_text(&step_text).unwrap();

    let mut world_b = FlecsWorld::new();
    let scene_b = CapScene::intern(&mut world_b);
    let mut engine_b = Engine::new(world_b, 2);
    engine_b.set_capability_resolver(Box::new(CapResolver::from_scene(&scene_b)));
    let mut store_b = AssetStore::new();
    let faces_b = reimport_with_pmi(&mut engine_b, &scene_b, &mut store_b, &cad_b).unwrap();

    // The FCF is a relationship on face 0 (the C6-style relational read) — SEMANTIC, queryable.
    let on_face0 = fcfs_on(&engine_b, faces_b[0]);
    assert_eq!(on_face0.len(), 1, "the re-imported FCF is on the same face");
    let read =
        read_fcf(&engine_b, on_face0[0]).expect("read the re-imported FCF as structured data");
    assert_eq!(
        read.characteristic,
        Characteristic::Position,
        "typed enum, not a label"
    );
    assert!(
        (read.tolerance_mm - 0.10).abs() < 1e-12,
        "the numeric zone survived"
    );
    assert_eq!(
        read.datum,
        Some(faces_b[1]),
        "the datum reconnected to the right face"
    );
    assert_eq!(read.standard, Standard::AsmeY14_5, "the standard survived");

    // One undoable transaction: Ctrl-Z peels the re-attached FCF.
    assert!(engine_b.undo(), "one Ctrl-Z peels the re-attached FCF");
    assert!(
        read_fcf(&engine_b, on_face0[0]).is_none(),
        "the FCF is gone after undo"
    );
    println!(
        "[b] re-imported FCF is queryable typed data (position ∅0.10 | datum face | ASME_Y14.5)"
    );
}

// ── (c) the fidelity is measured + published (the table) ────────────────────────────────────────────────────

#[test]
fn property_c_fidelity_is_a_measured_published_table() {
    let (_step, fidelity) = round_trip_once();
    assert_eq!(fidelity.rows.len(), 10, "one row per FCF");
    println!("[c] MEASURED semantic-PMI round-trip fidelity (declared subset, pure-Rust Part-21):");
    println!("    characteristic     | semantic | value | datum | standard");
    for r in &fidelity.rows {
        println!(
            "    {:<18} |   {:^5}  | {:^5} | {:^5} | {:^5}",
            r.characteristic, r.semantic, r.value_exact, r.datum_preserved, r.standard_preserved
        );
    }
    let (survived, total) = fidelity.semantic_survival();
    println!(
        "    => {survived}/{total} survive SEMANTIC; geometry-dev {:e}",
        fidelity.geometry_deviation
    );
    // A genuinely-measured number: every declared-subset characteristic is faithful here.
    assert!(fidelity.fully_faithful());
}

// ── (d) carry-forward re-verification (test-first): M15.0 budget + M15.3 PMI in THIS toolchain ──────────────

#[test]
fn property_d_carry_forward_reverify_m150_budget_and_m153_pmi() {
    // M15.0: the STEP geometry round-trip still holds the declared budget in the current toolchain.
    let step = StepInterchange;
    let before = step.import(CUBE_STEP.as_bytes()).expect("import");
    let exported = step.export(&before).expect("export");
    let after = step.import(exported.as_bytes()).expect("re-import");
    let dev = round_trip_deviation(&before, &after);
    assert!(
        dev <= ROUND_TRIP_BUDGET,
        "M15.0 budget re-confirmed: {dev:e} <= {ROUND_TRIP_BUDGET:e}"
    );

    // M15.3: a semantic FCF still attaches to a real imported B-rep face as queryable typed data.
    let (mut engine, faces, _cad) = engine_with_step();
    assert_eq!(faces.len(), 6, "6 referenceable faces (M15.0)");
    let id = attach_fcf(
        &mut engine,
        &Fcf {
            feature: faces[0],
            characteristic: Characteristic::Flatness,
            tolerance_mm: 0.02,
            datum: None,
            standard: Standard::IsoGps,
        },
    )
    .expect("M15.3 attach");
    assert_eq!(
        read_fcf(&engine, id).map(|f| f.characteristic),
        Some(Characteristic::Flatness),
        "M15.3 semantic-PMI representation re-confirmed"
    );
    println!("[d] carry-forward re-verified: M15.0 budget ({dev:e}) + M15.3 semantic PMI hold");
}

// ── headless: malformed/oversized STEP-with-PMI is explained never a panic; graphical is not promoted ───────

#[test]
fn headless_malformed_step_with_pmi_is_explained_never_a_panic() {
    // A truncated geometric_tolerance / broken file → an explained error, never a panic (the M10.2 gate).
    assert!(import_step_text("not a step file at all").is_err());
    assert!(
        import_step_text("ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n#1 = POSITION_TOLERANCE(\nENDSEC;\nEND-ISO-10303-21;\n")
            .is_err(),
        "a truncated tolerance entity is explained, not a panic"
    );
    // Oversized is rejected before parse (the decode-bomb / size cap).
    let big = "x".repeat(metrocalk_interchange::MAX_STEP_BYTES + 1);
    assert!(
        import_step_text(&big).is_err(),
        "oversized STEP is bounds-checked"
    );
    println!("[headless] malformed/oversized STEP-with-PMI is explained, never a panic");
}

#[test]
fn headless_a_graphical_only_callout_is_not_promoted_to_semantic() {
    // A file whose only PMI is a GRAPHICAL annotation (a drawn callout) must NOT be surfaced as semantic PMI
    // — it is an explained downgrade note (the honest boundary; recovering semantics from graphics is the
    // OCCT / full-AP242 seam).
    let (engine_a, faces_a, cad_a) = engine_with_step();
    let scene = scene_with_pmi(&engine_a, &faces_a, cad_a); // no FCFs attached → empty pmi
    let mut step_text = export_step_pmi(&scene).unwrap();
    step_text = step_text.replace(
        "ENDSEC;\nEND-ISO-10303-21;\n",
        "#9001 = ANNOTATION_OCCURRENCE('drawn callout',$,$);\nENDSEC;\nEND-ISO-10303-21;\n",
    );
    let cad_b = import_step_text(&step_text).unwrap();
    assert!(
        cad_b.pmi.is_empty(),
        "a graphical callout is NOT semantic PMI"
    );
    assert!(
        cad_b
            .notes
            .iter()
            .any(|n| n.detail.contains("machine-readable")),
        "the graphical downgrade is explained, not silent"
    );
    println!(
        "[headless] a graphical-only callout is NOT promoted to semantic (explained downgrade)"
    );
}
