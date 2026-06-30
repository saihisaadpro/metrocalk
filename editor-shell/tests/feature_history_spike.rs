//! M15.2 (ADR-072) — THE SPIKE (deliverable #1): the measured go/no-go gate for **feature-history as the
//! op-stream + a deterministic, bisectable rebuild**, kernel-free.
//!
//! Proves, on a real parametric chain (create -> parametrize -> CSG-carve -> pattern, using the shipped
//! M13.2 exact-CSG + transform ops):
//! - **(a)** the feature tree **rebuilds bit-identically** from the op-log (the rebuilt-state hash is equal
//!   across >=2 runs) — the M13.1/ADR-050 deterministic-replay property applied to feature ops.
//! - **(b)** suppressing an **upstream** op surfaces an **explained, bisectable** downstream failure (which
//!   feature lost which dependency, and why) — and a failed rebuild ships as a **lossless bincode artifact**
//!   that re-runs the exact failure ("a broken feature is a file").
//! - **(c)** a **parameter change cascades** through the dependent carve + pattern deterministically.
//! - **(d)** **concurrent multi-user history** merges (reusing the M15.1 branch/merge) — two designers add
//!   different features to the same part and the CRDT converges, never clobbers (impossible in file-CAD).
//!
//! The bit-identical rebuild is the **carry-forward re-verification** (test-first) that the M13.2 exact-CSG
//! `content_hash` (ADR-051) and the `canonical_state` identity (ADR-071) are deterministic in the current
//! toolchain. Native-deterministic; the wasm32 boundary (ADR-020) → server-authoritative on the web.
//! Run with `-- --nocapture` to print the measured numbers. A headless CI gate — no dark test.

use metrocalk_core::{Engine, FieldValue, Op};
use metrocalk_ecs::FlecsWorld;
use metrocalk_editor_shell::{
    branch_from, merge_eco, rebuild, rebuild_reproduces, Configuration, Dim, Expr, FeatureError,
    FeatureHistory, FeatureOp,
};
use std::collections::{BTreeMap, BTreeSet};

const RUNS: usize = 3; // the >=2-runs reproducibility discipline (<benchmark_discipline>)

/// The canonical parametric chain: a `width`-driven base box, a tool box, a carve (base minus tool), and a
/// 3-up linear pattern of the carved body. Deterministic inputs (no RNG / no clock).
fn parametric_chain() -> FeatureHistory {
    let mut variables = BTreeMap::new();
    variables.insert("width".to_string(), Expr::Const(2.0));
    FeatureHistory {
        variables,
        features: vec![
            FeatureOp::Box {
                id: 1,
                pos: [0.0, 0.0, 0.0],
                half: [Dim::Ref("width".into()), Dim::Lit(1.0), Dim::Lit(1.0)],
            },
            FeatureOp::Box {
                id: 2,
                pos: [0.0, 1.0, 0.0],
                half: [Dim::Lit(0.5), Dim::Lit(0.5), Dim::Lit(0.5)],
            },
            FeatureOp::Carve {
                id: 3,
                target: 1,
                tool: 2,
            },
            FeatureOp::Pattern {
                id: 4,
                source: 3,
                count: 3,
                spacing: [5.0, 0.0, 0.0],
            },
        ],
        suppressed: BTreeSet::new(),
    }
}

// ── Property (a): the feature tree rebuilds bit-identically ───────────────────────────────────────────────

#[test]
fn property_a_rebuild_is_bit_identical() {
    let history = parametric_chain();
    let first = rebuild(&history, None).expect("the parametric chain rebuilds");
    println!(
        "[a] rebuild identity = {} ({} features, carve geom = {})",
        first.state_hash, first.built_features, first.geometry[&3]
    );
    assert!(
        rebuild_reproduces(&history, None, RUNS).expect("rebuild"),
        "the feature tree rebuilds bit-identically across {RUNS} runs (CSG + canonical-state determinism)"
    );
    // The whole chain built (box, tool, carve-in-place, pattern) — 4 feature outputs.
    assert_eq!(first.built_features, 4);
}

// ── Property (b): suppressing an upstream op is an explained, bisectable failure ──────────────────────────

#[test]
fn property_b_suppressing_upstream_is_explained_and_bisectable() {
    let mut history = parametric_chain();
    // Suppress the tool box (feature 2) — the carve (feature 3) depends on it.
    history.suppressed.insert(2);

    let err = rebuild(&history, None).expect_err("the carve lost its tool dependency");
    match &err {
        FeatureError::BrokenDependency {
            feature,
            missing,
            why,
        } => {
            assert_eq!(*feature, 3, "the CARVE is the broken downstream feature");
            assert_eq!(*missing, 2, "it lost feature 2 (the suppressed tool)");
            assert!(
                why.contains("suppressed"),
                "the reason names the suppression"
            );
            println!("[b] bisect: feature {feature} broke -> depends on {missing}, {why}");
        }
        other => panic!("expected an explained BrokenDependency, got {other:?}"),
    }
    assert!(
        err.to_string().is_ascii(),
        "the explanation is ASCII-legible through IPC"
    );

    // "A broken feature is a file": the suppressed history serializes losslessly and re-runs the EXACT
    // failure headlessly (the M13.1/ADR-050 reproduction-artifact discipline).
    let artifact = history
        .to_bytes()
        .expect("the broken history is a bincode file");
    let reloaded = FeatureHistory::from_bytes(&artifact).expect("reload the artifact");
    assert_eq!(
        rebuild(&reloaded, None).unwrap_err(),
        err,
        "the artifact reproduces the exact same explained failure"
    );
    println!(
        "[b] the broken feature is a {}-byte reproducible file",
        artifact.len()
    );
}

// ── Property (c): a parameter change cascades deterministically ──────────────────────────────────────────

#[test]
fn property_c_parameter_change_cascades_deterministically() {
    let history = parametric_chain();
    let base = rebuild(&history, None).expect("base rebuild");

    // Change the `width` parameter (which drives the base box) via a configuration override — the cascade
    // re-runs the dependent carve + pattern.
    let wider = Configuration {
        name: "wider".into(),
        overrides: BTreeMap::from([("width".to_string(), 3.5)]),
    };
    let c1 = rebuild(&history, Some(&wider)).expect("cascaded rebuild");
    let c2 = rebuild(&history, Some(&wider)).expect("cascaded rebuild again");

    // The change propagated through to the carved body's geometry (not just the base box).
    assert_ne!(
        base.geometry[&3], c1.geometry[&3],
        "the param cascaded through the carve"
    );
    assert_ne!(
        base.state_hash, c1.state_hash,
        "the variant differs from the base"
    );
    // ...and the cascade is deterministic (same param -> same result, >=2 runs).
    assert_eq!(
        c1.state_hash, c2.state_hash,
        "the cascade replays deterministically"
    );
    println!(
        "[c] base={} -> width=3.5 cascade={} (carve geom changed)",
        base.state_hash, c1.state_hash
    );
}

// ── Property (d): concurrent multi-user history merges (reuse M15.1) ──────────────────────────────────────

#[test]
fn property_d_concurrent_feature_history_merges() {
    // Two designers branch the same base part and each adds a DIFFERENT feature-body (a boss on the left vs
    // the right). The feature history IS the op-log, so the M15.1 branch/merge converges them — file-CAD
    // cannot merge two designers' concurrent feature edits.
    fn add_boss(e: &mut Engine<FlecsWorld>, handle: &str, x: f64) {
        let id = e.alloc_entity_id();
        e.commit(
            "feature:boss",
            vec![
                Op::CreateEntity { id, parent: None },
                Op::SetField {
                    entity: id,
                    component: "Transform".into(),
                    field: "px".into(),
                    value: FieldValue::Number(x),
                },
                Op::SetField {
                    entity: id,
                    component: "MeshRenderer".into(),
                    field: "mesh".into(),
                    value: FieldValue::Str(handle.into()),
                },
            ],
        )
        .unwrap();
    }

    for run in 0..RUNS {
        // A shared released base part.
        let mut main = engine_with_base();
        let base_vv = main.version_vector();
        let snapshot = main.snapshot();

        // Designer A works on a branch; designer B works on the main line — concurrent feature edits.
        let mut branch = branch_from(&snapshot, 2).expect("open an ECO branch");
        add_boss(&mut branch, "mtkasset:bossL", -3.0);
        add_boss(&mut main, "mtkasset:bossR", 3.0);

        // Merge the branch's feature edit back (the M15.1 ECO machinery).
        let delta = branch.export_updates_since(&base_vv);
        let outcome = merge_eco(&mut main, &delta).expect("merge the concurrent feature edit");
        assert!(
            outcome.approved,
            "the concurrent feature-history merge is clean (run {run})"
        );

        // No lost feature: the merged part carries BOTH designers' bosses (base + 2 = 3 bodies).
        assert_eq!(
            main.entity_count(),
            3,
            "both concurrent features survived the merge"
        );
        println!(
            "[d] run {run}: concurrent feature edits merged, approved={}",
            outcome.approved
        );
    }
}

fn engine_with_base() -> Engine<FlecsWorld> {
    let mut e = Engine::new(FlecsWorld::new(), 1);
    let id = e.alloc_entity_id();
    e.commit(
        "feature:base",
        vec![
            Op::CreateEntity { id, parent: None },
            Op::SetField {
                entity: id,
                component: "MeshRenderer".into(),
                field: "mesh".into(),
                value: FieldValue::Str("mtkasset:base".into()),
            },
        ],
    )
    .unwrap();
    e
}
