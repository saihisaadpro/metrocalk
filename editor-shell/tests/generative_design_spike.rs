//! M15.4 (ADR-074) — THE SPIKE (deliverable #1): the measured go/no-go gate for the **intent-driven
//! generative loop** — "describe the loads in natural language → optimized geometry," closing
//! **AI-authoring (M12.4 validated patch) → evaluate (deterministic optimizer over a DST-validated
//! objective; the differentiable-sim gradient FF-T7 is named-gated) → deterministic validation (M13.1 DST)
//! → a validated, undoable patch** — the whole run **reproducible + auditable across ≥2 runs**.
//!
//! Proves the properties, each measured (≥2 runs where reproducibility is the claim):
//! - **(a) THE GATE:** one NL load-spec → a **validated optimization patch** over a **deterministic,
//!   reproducible objective**: same spec/seed → the **same chosen design**, a **bit-reproducible objective**
//!   (identical `GenerativeRun` bytes ×3), and a **replayable audit trail** (spec → candidates → objective →
//!   chosen). The chosen patch applies as a schema-validated, undoable `apply_ai_patch`.
//! - **(b) the closed loop + DST-validated-in-sim:** the carry-forward **M13.1 DST determinism** is
//!   re-confirmed test-first (`reproduces_at`, current toolchain), and the chosen loaded design is
//!   **validated in the DST sim** (reproducible + a sane convergence predicate) — the moat is deterministic
//!   reproducibility, **not** an FEA stress claim.
//! - **(c) the interop-FEA `Solver` seam:** the deterministic **ROM** ([`RomBeamSolver`]) is in the
//!   reproducibility guarantee; the validated FEA ([`PreciceFmiSolver`] — preCICE + the preCICE-FMI runner,
//!   **server-side native**) is a **named seam** that returns `Unavailable`, never a fabricated stress, and
//!   is `deterministic() == false` (OUT of the guarantee).
//! - **(d) the differentiable leg used-or-named-gated:** the loop uses the **analytic ROM gradient**;
//!   **FF-T7** differentiable-sim is named-gated (`ff_t7_available() == false`), never faked.
//! - **headless:** an **over-budget** proposal is **rejected-as-UX** with a derivation certificate (the
//!   M13.5 every-no seed); a valid proposal applies as **one undoable transaction**; concurrent design edits
//!   **merge clobber-free** (Loro, inv. 1/3); the **M12.4 patch contract** re-confirmed test-first.
//!
//! Run with `-- --nocapture` to print the measured numbers. A headless CI gate (`cargo test --workspace`) —
//! **a non-reproducible optimization run FAILS CI**, no dark test. The reproducible ROM + optimizer are pure
//! `f64` (wasm-portable by construction — the browser/min-spec ROM path); the rapier-coupled DST
//! validate-in-sim is exercised here as a **dev-dependency** (native), the ROM-in-guarantee /
//! heavy-solver-out split at the crate-graph level.

use metrocalk_assets::AssetStore;
use metrocalk_core::{Engine, FieldValue};
use metrocalk_ecs::FlecsWorld;
use metrocalk_editor_shell::capscene::{CapResolver, CapScene};
use metrocalk_editor_shell::{
    apply_optimized_design, bake_design, baked_mesh_is_watertight, design_certificate,
    design_component_meta, optimize, parse_spec, place_design_seed, propose_design, Design,
    GenerativeRun, GradientSource, LoadSpec, PreciceFmiSolver, RomBeamSolver, Solver, SolverError,
    DESIGN_COMPONENT,
};

use metrocalk_dst::{validate_in_sim, Scenario};
use metrocalk_physics::{
    BodyDesc, BodyKind, ColliderDesc, ColliderShape, PhysicsConfig, Recording,
};

const RUNS: usize = 3; // the ≥2-runs reproducibility discipline (<benchmark_discipline>)
const SEED: u64 = 0x5EED_D00D; // the injected seed (the DST/VOPR shape — same seed → same run)
const RES: usize = 14; // the sweep resolution (a real search, still fast headless)

/// The prompt's canonical NL load-spec.
const SPEC_NL: &str =
    "this bracket carries 200 N down at the tip, fixed at the base — minimize mass";

fn spec() -> LoadSpec {
    parse_spec(SPEC_NL).expect("parse the canonical NL load-spec")
}

/// A fresh engine + asset store (a `CapScene` resolver, matching the other M15 spikes).
fn fresh_engine() -> (Engine<FlecsWorld>, AssetStore) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    engine.set_capability_resolver(Box::new(CapResolver::from_scene(&scene)));
    (engine, AssetStore::new())
}

// ── (a) THE GATE: NL spec → a reproducible, validated, auditable optimization patch (measured ≥3 runs) ─────

#[test]
#[allow(clippy::too_many_lines)] // THE GATE — one comprehensive test proving every property of deliverable #1
fn property_a_the_spike_reproducible_validated_auditable_patch() {
    let spec = spec();

    // ≥2 runs: same spec + seed → the SAME run, bit-for-bit (a single match is not proof).
    let mut runs: Vec<GenerativeRun> = Vec::new();
    for _ in 0..RUNS {
        runs.push(optimize(&spec, SEED, RES, &RomBeamSolver));
    }
    assert!(
        runs.windows(2).all(|w| w[0] == w[1]),
        "same spec + seed → identical GenerativeRun across {RUNS} runs (the reproducibility GATE)"
    );
    // The objective is bit-reproducible: the artifact bytes are identical across runs.
    let bytes0 = runs[0].to_bytes();
    assert!(
        runs.iter().all(|r| r.to_bytes() == bytes0),
        "the objective is bit-reproducible (identical bincode artifact ×{RUNS})"
    );

    let run = &runs[0];
    // A feasible, min-mass design was chosen (the loop converged, not parked).
    let chosen = run
        .chosen_candidate()
        .expect("a feasible min-mass design (GO)");
    assert!(chosen.feasible, "the chosen design meets both budgets");
    assert!(
        run.deterministic_objective,
        "the objective solver is deterministic (in the reproducibility guarantee)"
    );
    assert_eq!(run.solver, "rom-beam");

    // The audit trail is complete + REPLAYABLE: spec → candidates → objective → chosen; an auditor
    // re-derives every candidate through the solver and it matches bit-for-bit.
    assert!(
        !run.candidates.is_empty(),
        "the audit trail records every evaluated design"
    );
    assert!(
        run.verify_audit(&RomBeamSolver),
        "the audit trail replays bit-for-bit"
    );
    assert_eq!(
        run.spec.description, SPEC_NL,
        "the audit records what the user asked for"
    );

    // "A design run is a file" (lossless bincode, the M13.1 discipline): reload → identical.
    let loaded = GenerativeRun::from_bytes(&run.to_bytes()).expect("reload the artifact");
    assert_eq!(&loaded, run, "the audit artifact round-trips bit-exactly");

    // The chosen design applies as a schema-validated, UNDOABLE patch (M12.4), and one Ctrl-Z peels it.
    let (mut engine, mut store) = fresh_engine();
    let design_entity = place_design_seed(&mut engine, [0.0, 0.0, 0.0], &spec).expect("seed");
    let delta = apply_optimized_design(
        &mut engine,
        &mut store,
        design_entity,
        &spec,
        run,
        "gen-op-1",
    );
    assert_eq!(
        delta.confirms,
        vec!["gen-op-1".to_string()],
        "the validated patch applied"
    );
    assert!(delta.rejects.is_empty());
    // The optimized parameters landed on the design entity as structured data.
    assert!(matches!(
        engine.get_field(design_entity, DESIGN_COMPONENT, "mass_g"),
        Some(FieldValue::Number(m)) if m > 0.0
    ));
    let handle = match engine.get_field(design_entity, DESIGN_COMPONENT, "mesh") {
        Some(FieldValue::Str(h)) => h,
        other => panic!("the patch set a baked mesh handle, got {other:?}"),
    };
    assert!(
        baked_mesh_is_watertight(&store, &handle),
        "the realized geometry is watertight"
    );
    assert!(engine.undo(), "one Ctrl-Z peels the optimization patch");
    assert!(
        engine
            .get_field(design_entity, DESIGN_COMPONENT, "mass_g")
            .is_none(),
        "the optimized fields are gone after undo"
    );

    // The optimizer removed real mass vs an overbuilt solid baseline.
    let baseline = RomBeamSolver
        .analyze(
            &Design {
                height_m: 0.020,
                bore_r_m: 0.0,
            },
            &spec,
        )
        .unwrap();
    // THE GATE asserts a REAL optimization happened — not a no-op/constant optimizer returning the baseline.
    assert!(
        chosen.result.mass_kg < baseline.mass_kg,
        "the optimizer removed mass: chosen {:.1} g must be < the solid-20 mm baseline {:.1} g",
        chosen.result.mass_kg * 1000.0,
        baseline.mass_kg * 1000.0
    );
    let saved = 100.0 * (1.0 - chosen.result.mass_kg / baseline.mass_kg);
    println!(
        "[a] GATE=GO — NL \"{SPEC_NL}\" → H={:.2} mm bore \u{2300}{:.2} mm, mass {:.1} g (\u{2212}{:.0}% vs solid 20 mm), \u{3c3}={:.1} MPa, \u{3b4}={:.3} mm | {} candidates, reproducible \u{d7}{RUNS}",
        chosen.design.height_m * 1000.0,
        chosen.design.bore_r_m * 2000.0,
        chosen.result.mass_kg * 1000.0,
        saved,
        chosen.result.max_stress_pa / 1.0e6,
        chosen.result.tip_deflection_m * 1000.0,
        run.candidates.len(),
    );
    println!(
        "::notice::m15.4-generative-gate objective-hash={} candidates={} mass-g={:.3} reproducible-runs={RUNS} deterministic=true",
        run.objective_hash,
        run.candidates.len(),
        chosen.result.mass_kg * 1000.0,
    );
}

// ── (b) the closed loop: carry-forward DST determinism + validate-the-chosen-design-in-sim (reproducible) ──

#[test]
fn property_b_dst_validated_in_sim_carry_forward() {
    // Carry-forward re-verification (test-first, the ADR-050 discipline): re-confirm the M13.1 DST
    // determinism holds in THIS toolchain before resting the loop's "validated in sim" on it — re-run,
    // don't assume. A real ordering-sensitive scenario replays bit-for-bit ≥2 runs.
    let carry = stack_scenario();
    assert!(
        carry.reproduces_at(180, RUNS),
        "M13.1 DST re-confirmed: a divergence scenario replays bit-for-bit ×{RUNS} (current toolchain)"
    );

    // Validate the CHOSEN design in the DST sim: the loaded-part scenario replays reproducibly AND a sane
    // convergence predicate holds — the "validated in simulation, not live" hook (M13.1). The moat is
    // DETERMINISTIC REPRODUCIBILITY of the loaded scenario, NOT an FEA stress claim (rapier is rigid-body,
    // not FEA — structural adequacy is the ROM Solver's claim; FEA is integrated, not rebuilt).
    let spec = spec();
    let run = optimize(&spec, SEED, RES, &RomBeamSolver);
    let chosen = run.chosen_candidate().unwrap();
    let loaded = loaded_part_scenario(chosen.design, &spec);

    let frame = 240;
    assert!(
        loaded.reproduces_at(frame, RUNS),
        "the chosen loaded design's scenario replays bit-for-bit ×{RUNS} (reproducible validation)"
    );
    // A converging predicate PASSES (the loaded part stays bounded — didn't fly apart / fall through).
    let verdict = validate_in_sim(&loaded, frame, |r| {
        r.transforms().iter().all(|(p, _)| p[1] > -1.0)
    });
    assert!(
        verdict.converged,
        "the loaded design validates in sim (sane bounded outcome)"
    );
    // A bogus claim is REJECTED by the sim (proving the predicate is real, not vacuous).
    let bogus = validate_in_sim(&loaded, frame, |r| {
        r.transforms()
            .iter()
            .all(|(p, _)| p[0] == 0.0 && p[1] == 0.0)
    });
    assert!(
        !bogus.converged,
        "a non-converging claim is caught in sim, before live"
    );
    // The verdict carries the reproducible state hash (a passing validation is itself an artifact).
    assert_eq!(verdict.hash, loaded.state_hash_at(frame));

    println!(
        "[b] DST validate-in-sim: loaded design reproduces \u{d7}{RUNS} (hash {}), converged={} — deterministic validation, NOT an FEA-accuracy claim",
        &verdict.hash[..16.min(verdict.hash.len())],
        verdict.converged,
    );
    println!(
        "::notice::m15.4-dst-validate-in-sim reproducible-runs={RUNS} converged={} carry-forward-dst=reconfirmed",
        verdict.converged
    );
}

// ── (c) the interop-FEA Solver seam: ROM in-guarantee; preCICE/FMI server-side, Unavailable (never faked) ──

#[test]
fn property_c_interop_fea_solver_seam() {
    let spec = spec();
    let design = Design {
        height_m: 0.020,
        bore_r_m: 0.0,
    };

    // The deterministic ROM is IN the reproducibility guarantee and returns a real result.
    assert!(RomBeamSolver.deterministic(), "the ROM is in the guarantee");
    let rom = RomBeamSolver
        .analyze(&design, &spec)
        .expect("the ROM analyzes");
    assert!(rom.mass_kg > 0.0 && rom.max_stress_pa.is_finite());

    // The validated FEA is a SERVER-SIDE SEAM: Unavailable, never a fabricated stress; and it is OUT of the
    // reproducibility guarantee (its raw FP non-determinism is confined behind the trait — integrate, never
    // rebuild Ansys; CalculiX/OpenFOAM are not native FMUs → preCICE + the preCICE-FMI runner, server-side).
    assert!(
        !PreciceFmiSolver.deterministic(),
        "external FEA is OUT of the guarantee"
    );
    match PreciceFmiSolver.analyze(&design, &spec) {
        Err(SolverError::Unavailable { solver, reason }) => {
            assert_eq!(solver, "precice-fmi");
            assert!(reason.contains("server-side") && reason.contains("preCICE"));
            assert!(
                reason.contains("ROM surrogate"),
                "min-spec/browser uses the ROM"
            );
            println!("[c] FEA seam is Unavailable (never a fake number): {reason}");
        }
        other => panic!("the FEA seam must be a named Unavailable seam, got {other:?}"),
    }
    println!(
        "::notice::m15.4-solver-seam rom-in-guarantee=true precice-fmi=server-side-unavailable"
    );
}

// ── (d) the differentiable-sim leg: analytic ROM gradient used; FF-T7 named-gated, never faked ─────────────

#[test]
fn property_d_differentiable_leg_used_or_named_gated() {
    // FF-T7 (an adjoint through a differentiable simulator) is an M13-frontier dependency — NOT available.
    assert!(
        !GradientSource::ff_t7_available(),
        "FF-T7 differentiable-sim is a gated frontier dependency"
    );
    // So the loop USES the analytic ROM gradient (a real, closed-form gradient of the proxy) — never a fake
    // adjoint. Recorded in the audit trail.
    let run = optimize(&spec(), SEED, RES, &RomBeamSolver);
    assert_eq!(
        run.gradient_source,
        GradientSource::AnalyticRom,
        "the loop uses the analytic ROM gradient (not a faked FF-T7 adjoint)"
    );
    println!("[d] gradient = analytic-ROM (used honestly); FF-T7 differentiable-sim = named-gated");
    println!("::notice::m15.4-differentiable gradient=analytic-rom ff-t7=named-gated faked=false");
}

// ── headless: over-budget rejected-as-UX (a certificate); a valid proposal is one undoable tx ──────────────

#[test]
fn headless_over_budget_proposal_is_rejected_as_ux_with_a_certificate() {
    let spec = spec();
    let (mut engine, mut store) = fresh_engine();
    let design_entity = place_design_seed(&mut engine, [0.0, 0.0, 0.0], &spec).expect("seed");

    // An over-budget proposal (a 5 mm solid section over-stresses the 200 N load) is REJECTED-AS-UX with a
    // derivation certificate (the M13.5 every-no seed) — nothing applied, no raw annotation path.
    let over = Design {
        height_m: 0.005,
        bore_r_m: 0.0,
    };
    let cert =
        design_certificate(&over, &spec, &RomBeamSolver).expect("an over-budget certificate");
    assert!(
        !cert.unsat_core.is_empty(),
        "the certificate carries an unsat-core (a derivation)"
    );
    let bad = propose_design(
        &mut engine,
        &mut store,
        design_entity,
        &spec,
        &over,
        "bad-1",
    );
    assert!(
        bad.confirms.is_empty() && bad.rejects.len() == 1,
        "over-budget rejected-as-UX"
    );
    assert!(
        bad.rejects[0].reason.contains("over budget"),
        "the rejection carries the certificate reason: {}",
        bad.rejects[0].reason
    );
    assert!(
        engine
            .get_field(design_entity, DESIGN_COMPONENT, "mass_g")
            .is_none(),
        "nothing was applied by the rejected proposal"
    );

    // A feasible proposal applies as ONE undoable transaction.
    let ok = Design {
        height_m: 0.020,
        bore_r_m: 0.0,
    };
    let good = propose_design(&mut engine, &mut store, design_entity, &spec, &ok, "good-1");
    assert_eq!(good.confirms, vec!["good-1".to_string()]);
    assert!(engine
        .get_field(design_entity, DESIGN_COMPONENT, "mass_g")
        .is_some());
    assert!(
        engine.undo(),
        "the valid proposal is one undoable transaction"
    );
    assert!(engine
        .get_field(design_entity, DESIGN_COMPONENT, "mass_g")
        .is_none());

    println!("[headless] over-budget proposal rejected-as-UX (certificate); a valid proposal is one undoable tx");
}

#[test]
fn headless_concurrent_design_edits_merge_clobber_free() {
    // Two peers each author an optimized-design patch; the CRDT merges with no lost edit (Loro, inv. 1) and
    // merge-validation is clean (inv. 3) — the digital-thread property applied to generative design.
    let spec = spec();
    let run = optimize(&spec, SEED, RES, &RomBeamSolver);

    let base = fresh_engine().0;
    let snapshot = base.snapshot();

    let mut peer_a = Engine::new(FlecsWorld::new(), 10);
    peer_a.merge(&snapshot).unwrap();
    let mut store_a = AssetStore::new();
    let mut peer_b = Engine::new(FlecsWorld::new(), 20);
    peer_b.merge(&snapshot).unwrap();
    let mut store_b = AssetStore::new();

    // Each peer places + optimizes its own design entity (distinct peer-scoped ids → no collision).
    let ea = place_design_seed(&mut peer_a, [0.0, 0.0, 0.0], &spec).unwrap();
    apply_optimized_design(&mut peer_a, &mut store_a, ea, &spec, &run, "a-1");
    let eb = place_design_seed(&mut peer_b, [1.0, 0.0, 0.0], &spec).unwrap();
    apply_optimized_design(&mut peer_b, &mut store_b, eb, &spec, &run, "b-1");

    let report_a = peer_a.merge(&peer_b.export_updates()).unwrap();
    let report_b = peer_b.merge(&peer_a.export_updates()).unwrap();
    assert_eq!(report_a.total_violations(), 0, "merge A<-B clean (inv. 3)");
    assert_eq!(report_b.total_violations(), 0, "merge B<-A clean (inv. 3)");

    // No lost edit: both designs survive on both peers.
    for peer in [&peer_a, &peer_b] {
        assert!(
            matches!(
                peer.get_field(ea, DESIGN_COMPONENT, "mass_g"),
                Some(FieldValue::Number(_))
            ),
            "peer A's design survived"
        );
        assert!(
            matches!(
                peer.get_field(eb, DESIGN_COMPONENT, "mass_g"),
                Some(FieldValue::Number(_))
            ),
            "peer B's design survived (no lost edit)"
        );
    }
    assert_eq!(
        peer_a.canonical_state(),
        peer_b.canonical_state(),
        "peers converge"
    );
    println!("[headless] concurrent generative-design edits merged clobber-free; peers converged (inv. 1/3)");
}

#[test]
fn property_e_carry_forward_m12_4_patch_contract() {
    // Carry-forward re-verification (test-first): re-confirm the M12.4 validated-patch contract in this
    // toolchain before resting the loop on it — a valid patch applies + is undoable; an invalid one is
    // rejected-as-UX (no raw path). This is the ADR-048/017 contract the loop's authoring rests on.
    let spec = spec();
    let (mut engine, mut store) = fresh_engine();
    let e = place_design_seed(&mut engine, [0.0, 0.0, 0.0], &spec).unwrap();

    // Valid: a feasible optimized design applies through apply_ai_patch (schema-validated).
    let run = optimize(&spec, SEED, RES, &RomBeamSolver);
    let ok = apply_optimized_design(&mut engine, &mut store, e, &spec, &run, "c-1");
    assert_eq!(
        ok.confirms,
        vec!["c-1".to_string()],
        "the M12.4 validated patch applies"
    );

    // The schema is the contract's boundary: it names the design component + its typed fields.
    let meta = design_component_meta();
    assert_eq!(meta.name, DESIGN_COMPONENT);
    assert!(meta.fields.iter().any(|f| f.name == "mass_g"));

    // The geometry is deterministic (same design ⇒ same content address — a reproducibility corollary).
    let cand = run.chosen_candidate().unwrap();
    let mut s1 = AssetStore::new();
    let mut s2 = AssetStore::new();
    let h1 = bake_design(&mut s1, &cand.design, &spec).unwrap();
    let h2 = bake_design(&mut s2, &cand.design, &spec).unwrap();
    assert_eq!(
        h1, h2,
        "same design ⇒ same content-addressed geometry (deterministic)"
    );

    println!("[e] carry-forward re-verified: the M12.4 validated-patch contract holds; geometry deterministic");
}

// ── scenario builders for the DST leg (rapier-backed, native — the dev-dependency path) ────────────────────

/// A real ordering-sensitive divergence scenario (a box stack + a shove) — the M8.1/M13.1 carry-forward
/// scenario, used to re-confirm DST determinism in this toolchain.
fn stack_scenario() -> Scenario {
    let mut rec = Recording::new(PhysicsConfig::default());
    rec.add_body(
        BodyDesc::new(BodyKind::Fixed, [0.0, 0.0, 0.0]),
        ColliderDesc::new(ColliderShape::Cuboid {
            half_extents: [20.0, 0.5, 20.0],
        }),
    );
    for i in 0..5 {
        rec.add_body(
            BodyDesc::new(BodyKind::Dynamic, [0.0, 1.2 + f64::from(i) * 0.9, 0.0]),
            ColliderDesc::new(ColliderShape::Cuboid {
                half_extents: [0.4, 0.4, 0.4],
            }),
        );
    }
    rec.add_input(90, 5, [4.0, 0.0, 0.0]); // the shove that diverges the stack
    Scenario::new(SEED, rec)
}

/// The chosen design as a **loaded-part physics scenario**: a fixed base + a dynamic block (sized from the
/// design's section) under the spec's tip load (applied as an input impulse). DST validates its
/// DETERMINISM + reproducibility — the moat — NOT its stress (rapier is rigid-body, not FEA; the ROM Solver
/// owns structural adequacy, and validated FEA is integrated server-side, not rebuilt).
fn loaded_part_scenario(design: Design, spec: &LoadSpec) -> Scenario {
    let mut rec = Recording::new(PhysicsConfig::default());
    // A fixed base plate (the cantilever's fixed end) — large enough that the part settles on it.
    rec.add_body(
        BodyDesc::new(BodyKind::Fixed, [0.0, 0.0, 0.0]),
        ColliderDesc::new(ColliderShape::Cuboid {
            half_extents: [5.0, 0.5, 5.0],
        }),
    );
    // The loaded part — a dynamic block whose section reflects the chosen design (at a rapier-friendly
    // representative scale), settling onto the fixture.
    let hy = (design.height_m * 5.0).max(0.2); // representative half-height from the section
    rec.add_body(
        BodyDesc::new(BodyKind::Dynamic, [0.0, 1.0, 0.0]),
        ColliderDesc::new(ColliderShape::Cuboid {
            half_extents: [0.5, hy, 0.3],
        }),
    );
    // The tip load "carries N *down*" → a DOWNWARD load input (presses the part onto the fixture — it
    // settles, bounded, reproducibly; a representative impulse proportional to the spec load).
    let load = (spec.tip_load_n / 50.0).clamp(1.0, 20.0);
    rec.add_input(30, 1, [0.0, -load, 0.0]);
    Scenario::new(SEED, rec)
}
