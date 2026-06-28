//! M12.4 (ADR-048) — the MCP server's tool LOGIC, headless (no rmcp/protocol — that's a thin wiring layer
//! in main.rs). A client's `apply_composition` tool edits a `.mtk` project through the SAME validated commit
//! pipeline as a human: a schema-valid composition validates + applies + round-trips the saved project; an
//! invalid one is **rejected-as-UX with an explained reason, the project byte-unchanged** (nothing applied);
//! a corrupt project is a contained error, never a panic. The read tools (grammar/vocabulary) describe the
//! constrained schema the client composes within.

use metrocalk_core::compose::{ComposeOp, Composition};
use metrocalk_core::rules::{Action, CompareOp, Condition, RuleData};
use metrocalk_core::{project, Engine, FieldValue, Op};
use metrocalk_ecs::FlecsWorld;
use metrocalk_mcp::{apply_to_project, grammar, vocabulary};

/// A real `.mtk` project carrying one entity (the target a composition references).
fn project_with_entity() -> (Vec<u8>, String) {
    let mut e = Engine::new(FlecsWorld::new(), 7);
    let id = e.alloc_entity_id();
    e.commit("spawn", vec![Op::CreateEntity { id, parent: None }])
        .expect("create");
    (project::build(&e.snapshot()), id.to_loro_key())
}

fn ignite_rule(q: &str) -> RuleData {
    RuleData {
        name: "rusty sword ignites".into(),
        enabled: true,
        event: "EnemyDied".into(),
        conditions: vec![Condition {
            entity: q.into(),
            component: "KillCounter".into(),
            field: "count".into(),
            op: CompareOp::Ge,
            value: FieldValue::Integer(4),
        }],
        actions: vec![Action {
            action: "SetField".into(),
            entity: q.into(),
            component: "Flammable".into(),
            field: "lit".into(),
            value: FieldValue::Bool(true),
        }],
    }
}

#[test]
fn the_grammar_and_vocabulary_read_tools_describe_the_schema() {
    let g = grammar();
    assert_eq!(g["title"], "MetrocalkComposition");
    assert!(
        g["properties"]["ops"].is_object(),
        "the SA-22 op-set grammar"
    );
    let v = vocabulary();
    let comps = v["components"].as_array().expect("components");
    assert!(
        comps.iter().any(|c| c["name"] == "KillCounter"),
        "the registry vocabulary lists composable components"
    );
}

#[test]
fn apply_composition_validates_commits_and_round_trips_the_project() {
    let (proj, q) = project_with_entity();
    let comp = Composition {
        ops: vec![
            ComposeOp::SetField {
                entity: q.clone(),
                component: "KillCounter".into(),
                field: "count".into(),
                value: FieldValue::Integer(0),
            },
            ComposeOp::AuthorRule {
                id: "r_ai".into(),
                rule: ignite_rule(&q),
            },
        ],
    };
    let (new_bytes, res) = apply_to_project(Some(&proj), &comp).expect("apply");
    assert!(res.applied, "the schema-valid composition applies: {res:?}");
    assert_eq!(res.rules, 1);
    assert_ne!(new_bytes, proj, "the project changed");

    // Round-trip: re-load the SAVED project → the composed rule is there (the edit persisted through .mtk).
    let snap = project::parse(&new_bytes).expect("parse saved");
    let mut reopened = Engine::new(FlecsWorld::new(), 9);
    reopened.merge(&snap).expect("reload");
    assert_eq!(
        reopened.rules().len(),
        1,
        "the AI-composed rule survives save/reload (same pipeline as a human edit)"
    );
}

#[test]
fn an_invalid_composition_is_rejected_unchanged_and_explained() {
    let (proj, q) = project_with_entity();
    let mut bad = ignite_rule(&q);
    bad.event = "Nonsense".into(); // not in the registry vocabulary
    let comp = Composition {
        ops: vec![ComposeOp::AuthorRule {
            id: "r".into(),
            rule: bad,
        }],
    };
    let (bytes, res) = apply_to_project(Some(&proj), &comp).expect("call");
    assert!(!res.applied);
    assert!(
        res.error
            .as_deref()
            .unwrap()
            .contains("isn't an event the engine knows"),
        "explained rejection: {res:?}"
    );
    assert_eq!(
        bytes, proj,
        "a rejected composition leaves the project byte-unchanged (nothing applied, all-or-nothing)"
    );
}

#[test]
fn a_corrupt_project_is_a_contained_error_not_a_panic() {
    let err = apply_to_project(
        Some(b"definitely not a Metrocalk project"),
        &Composition { ops: vec![] },
    )
    .unwrap_err();
    assert!(
        !err.is_empty(),
        "a bad project surfaces an explained error: {err}"
    );
}
