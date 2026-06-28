//! M12.4 (ADR-048) — the AI code/Rules tier, headless. The AI composes Rules/components/state-machines as
//! **schema-validated patches** through the reused ADR-017 contract: a composition validates + applies as
//! **one undoable transaction** (undo reverts ALL of it) or is **rejected-as-UX** with an explained reason
//! (test-first); the applied patch is **deterministic** (M8.1 spirit). **SA-22/R1:** the registry schema
//! compiles to a constrained-decoding grammar whose coverage is measured (our flat-scalar schema is inside
//! the reliable subset). **AI is a guest:** every function here is pure data + the deterministic pipeline —
//! no LLM (the no-LLM path is the free-engine/offline property, asserted by the core crate-graph itself).

use metrocalk_core::compose::{
    apply_composition, composition_grammar, grammar_coverage, validate_composition, ComposeError,
    ComposeOp, Composition,
};
use metrocalk_core::rules::{Action, CompareOp, Condition, RuleData};
use metrocalk_core::state_machine::{StateMachine, Transition};
use metrocalk_core::stdlib::{standard_actions, standard_components, standard_events};
use metrocalk_core::{Engine, FieldValue, Op, Registry};
use metrocalk_ecs::FlecsWorld;

fn registry() -> Registry<FlecsWorld> {
    let mut reg = Registry::new(FlecsWorld::new());
    for m in standard_components() {
        reg.register(m).expect("register");
    }
    for e in standard_events() {
        reg.register_event(e);
    }
    for a in standard_actions() {
        reg.register_action(a);
    }
    reg
}

fn engine() -> Engine<FlecsWorld> {
    Engine::new(FlecsWorld::new(), 1)
}

fn spawn(e: &mut Engine<FlecsWorld>) -> String {
    let id = e.alloc_entity_id();
    e.commit("spawn", vec![Op::CreateEntity { id, parent: None }])
        .expect("create");
    id.to_loro_key()
}

/// The canonical test-#5 rule on entity `q`: when an enemy dies, if KillCounter >= 4, ignite the sword.
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

/// A small QuestState machine on `q` (Hunting -> ReadyForBoss).
fn quest_machine(q: &str) -> StateMachine {
    let mut sm = StateMachine {
        name: "quest".into(),
        entity: q.into(),
        component: "QuestState".into(),
        field: "state".into(),
        states: vec!["Hunting".into(), "ReadyForBoss".into()],
        initial: "Hunting".into(),
        transitions: vec![],
    };
    sm.transitions = vec![Transition {
        id: "t1".into(),
        from: "Hunting".into(),
        to: "ReadyForBoss".into(),
        rule: RuleData {
            name: "advance".into(),
            enabled: true,
            event: "EnemyDied".into(),
            conditions: vec![],
            actions: vec![sm.enter_action("ReadyForBoss")],
        },
    }];
    sm
}

/// The test-#5 composition the AI proposes from a sentence: seed a KillCounter, author a QuestState machine,
/// and author the ignite Rule — three schema-validated patches.
fn test5_composition(q: &str) -> Composition {
    Composition {
        ops: vec![
            ComposeOp::SetField {
                entity: q.into(),
                component: "KillCounter".into(),
                field: "count".into(),
                value: FieldValue::Integer(0),
            },
            ComposeOp::AuthorStateMachine {
                id: "sm_ai".into(),
                machine: quest_machine(q),
            },
            ComposeOp::AuthorRule {
                id: "rule_ai".into(),
                rule: ignite_rule(q),
            },
        ],
    }
}

#[test]
fn an_ai_composition_validates_and_applies_as_one_undoable_transaction() {
    let reg = registry();
    let mut e = engine();
    let q = spawn(&mut e);
    let comp = test5_composition(&q);

    apply_composition(&mut e, &reg, &comp).expect("the schema-valid composition applies");
    // All three pieces landed.
    assert_eq!(
        e.get_field(
            metrocalk_core::EntityId::from_loro_key(&q).unwrap(),
            "KillCounter",
            "count"
        ),
        Some(FieldValue::Integer(0))
    );
    assert_eq!(e.rules().len(), 1, "the ignite rule was authored");
    assert_eq!(
        e.state_machines().len(),
        1,
        "the QuestState machine was authored"
    );

    // ONE undo reverts the WHOLE composition (the AI's edit is a single transaction).
    assert!(e.undo());
    assert!(e.rules().is_empty(), "undo reverts the composed rule");
    assert!(
        e.state_machines().is_empty(),
        "undo reverts the composed machine"
    );
    assert!(
        e.get_field(
            metrocalk_core::EntityId::from_loro_key(&q).unwrap(),
            "KillCounter",
            "count"
        )
        .is_none(),
        "undo reverts the composed field"
    );
}

#[test]
fn an_invalid_ai_proposal_is_rejected_as_ux_with_an_explained_reason() {
    // test-first: an out-of-schema AI proposal is REJECTED whole, with a plain reason — nothing applied
    // (a plugin/AI can't reach past validation; the ADR-017 guard, widened to Rules).
    let reg = registry();
    let mut e = engine();
    let q = spawn(&mut e);

    // (a) an AuthorRule whose event isn't in the vocabulary.
    let mut bad_rule = ignite_rule(&q);
    bad_rule.event = "EnemyExploded".into();
    let comp = Composition {
        ops: vec![
            ComposeOp::SetField {
                entity: q.clone(),
                component: "KillCounter".into(),
                field: "count".into(),
                value: FieldValue::Integer(0),
            },
            ComposeOp::AuthorRule {
                id: "r".into(),
                rule: bad_rule,
            },
        ],
    };
    let err = apply_composition(&mut e, &reg, &comp).unwrap_err();
    assert!(matches!(err, ComposeError::Rule { .. }));
    assert!(
        err.to_string().contains("isn't an event the engine knows"),
        "the rejection explains itself faithfully: {err}"
    );
    // ALL-OR-NOTHING: the valid SetField in the same composition was NOT applied either.
    assert!(
        e.get_field(
            metrocalk_core::EntityId::from_loro_key(&q).unwrap(),
            "KillCounter",
            "count"
        )
        .is_none(),
        "a rejected composition applies nothing (all-or-nothing)"
    );

    // (b) a SetField with the wrong scalar type.
    let bad_type = Composition {
        ops: vec![ComposeOp::SetField {
            entity: q.clone(),
            component: "KillCounter".into(),
            field: "count".into(),
            value: FieldValue::Str("four".into()),
        }],
    };
    assert!(matches!(
        apply_composition(&mut e, &reg, &bad_type),
        Err(ComposeError::FieldTypeMismatch { .. })
    ));

    // (c) an empty composition does nothing — refused.
    assert_eq!(
        validate_composition(&reg, &Composition { ops: vec![] }, |id| e.entity_exists(id)),
        Err(ComposeError::Empty)
    );
}

#[test]
fn sa22_the_registry_compiles_to_a_constrained_grammar_within_the_reliable_subset() {
    let components = standard_components();
    // Coverage: every registry field is a flat scalar → the schema sits INSIDE the reliable
    // constrained-decoding subset (the JSONSchemaBench trap doesn't bite). The honest, measured bound.
    let cov = grammar_coverage(&components);
    assert!(
        cov.within_subset,
        "our flat-scalar schema is within the reliable grammar subset"
    );
    assert!(
        cov.flagged.is_empty(),
        "no field exceeds the subset: {:?}",
        cov.flagged
    );
    assert!(cov.field_count > 0);

    // The grammar (the structured-output JSON Schema) constrains the op-set + the component vocabulary, so a
    // model structurally can't emit an out-of-schema op WITHIN it.
    let g = composition_grammar(&components);
    let item = &g["properties"]["ops"]["items"]["oneOf"];
    let ops: Vec<&str> = item
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["properties"]["op"]["const"].as_str().unwrap())
        .collect();
    assert_eq!(
        ops,
        vec!["setField", "authorRule", "authorStateMachine"],
        "the allow-listed op set"
    );
    // The SetField component is an enum of REAL component names (typo-proof by construction).
    let comp_enum = item[0]["properties"]["component"]["enum"]
        .as_array()
        .unwrap();
    let names: Vec<&str> = comp_enum.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(names.contains(&"KillCounter") && names.contains(&"QuestState"));
}

#[test]
fn the_applied_patch_is_deterministic() {
    // The AI proposal is non-deterministic, but the APPLIED patch is deterministic + replayable: the same
    // composition on the same seed yields byte-identical field state (the world edit is in the deterministic
    // pipeline; the generation is the guest).
    let reg = registry();
    let mut a = engine();
    let mut b = engine();
    let qa = spawn(&mut a);
    let qb = spawn(&mut b);
    assert_eq!(qa, qb, "same seed → same entity id");
    apply_composition(&mut a, &reg, &test5_composition(&qa)).unwrap();
    apply_composition(&mut b, &reg, &test5_composition(&qb)).unwrap();
    // Both worlds composed identically (the deterministic, replayable applied patch).
    let id = metrocalk_core::EntityId::from_loro_key(&qa).unwrap();
    assert_eq!(
        a.get_field(id, "KillCounter", "count"),
        b.get_field(id, "KillCounter", "count")
    );
    assert_eq!(a.rules(), b.rules());
    assert_eq!(a.state_machines(), b.state_machines());
}
