//! M12.4 (ADR-048) — the AI **compose** round-trip in the shell: a natural-language sentence becomes a
//! reviewable [`Composition`] (the [`DemoComposer`] seam), which applies through the SAME validated commit
//! pipeline a human / plugin uses ([`apply_composition`]) — propose → validate → committed → **one undo
//! reverts the whole thing** → **survives reload** (the composition is re-applied deterministically on
//! replay). A composition that fails validation is **rejected-as-UX** (an explained reason, nothing applied,
//! all-or-nothing) — the AI is never a raw mutation path. The SA-22 grammar + the composition contract are
//! tested in `/core` + `/mcp`; this guards the editor-shell seam (sentence → proposal → the transaction +
//! persistence wiring).
//!
//! [`apply_composition`]: metrocalk_core::apply_composition

use metrocalk_core::compose::{ComposeOp, Composition};
use metrocalk_core::rules::{Action, CompareOp, Condition, RuleData};
use metrocalk_core::stdlib::{standard_actions, standard_components, standard_events};
use metrocalk_core::{
    apply_composition, composition_grammar, validate_composition, Engine, EntityId, FieldValue,
    Registry,
};
use metrocalk_ecs::FlecsWorld;

use metrocalk_editor_shell::capscene::{self, CapScene};
use metrocalk_editor_shell::compose_ai::{Composer, DemoComposer};
use metrocalk_editor_shell::persist::{Log, Record};
use metrocalk_editor_shell::MeshCatalog;

const N: usize = 50;

fn tmp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("mtk-test-{name}.jsonl"))
}

fn seeded() -> (Engine<FlecsWorld>, CapScene) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    capscene::seed(&mut engine, &scene, N).expect("seed");
    engine.clear_history();
    (engine, scene)
}

fn eid(key: &str) -> EntityId {
    EntityId::from_loro_key(key).expect("key")
}

/// The stdlib registry (components + events + actions) the compose path validates against.
fn reg() -> Registry<FlecsWorld> {
    let mut r = Registry::new(FlecsWorld::new());
    for m in standard_components() {
        r.register(m).expect("component");
    }
    for e in standard_events() {
        r.register_event(e);
    }
    for a in standard_actions() {
        r.register_action(a);
    }
    r
}

#[test]
fn a_sentence_proposes_a_composition_that_applies_as_one_undoable_transaction() {
    let (mut a, _scene) = seeded();
    let registry = reg();
    let target = "1_5";

    // 1) The in-app AI seam turns a sentence into a reviewable composition (offline, deterministic demo).
    let grammar = composition_grammar(&standard_components());
    let comp = DemoComposer::new(true)
        .propose(
            "when an enemy dies and kills reach 4, set it on fire",
            Some(target),
            &grammar,
        )
        .expect("the demo sentence composes");

    // 2) It validates against the live scene (the preview is pre-checked — the ADR-017 gate).
    validate_composition(&registry, &comp, |e| a.entity_exists(e)).expect("the proposal is valid");

    // 3) Apply through the ONE commit pipeline. Before: no rule, no KillCounter on the target.
    assert_eq!(a.rules().len(), 0);
    assert_eq!(a.get_field(eid(target), "KillCounter", "count"), None);

    apply_composition(&mut a, &registry, &comp).expect("the composition applies");
    // The complete self-driving quest (M12.6): the tally + ignite + the offered mirror rules, plus the
    // QuestState machine — every piece an ordinary registry op, all in ONE undoable transaction.
    assert_eq!(
        a.rules().len(),
        3,
        "the AI composed the tally + ignite + cleanup rules"
    );
    assert_eq!(
        a.state_machines().len(),
        1,
        "the AI composed the QuestState machine"
    );
    assert_eq!(
        a.get_field(eid(target), "KillCounter", "count"),
        Some(FieldValue::Integer(0)),
        "the SetField op seeded the counter the rule reads"
    );

    // 4) ONE undo reverts the WHOLE composition (it's a single transaction — the AI is not a privileged path).
    assert!(a.undo());
    assert_eq!(a.rules().len(), 0, "undo removed the composed rules");
    assert_eq!(
        a.state_machines().len(),
        0,
        "undo removed the composed machine"
    );
    assert_eq!(
        a.get_field(eid(target), "KillCounter", "count"),
        None,
        "undo reverted the seeded field too — all-or-nothing"
    );
}

#[test]
fn a_composed_rule_survives_close_then_reopen_via_replay() {
    let log = Log::open(tmp("compose"), capscene::fingerprint(N));
    log.clear();

    // run A: compose + persist the record.
    let (mut a, _scene_a) = seeded();
    let registry = reg();
    let comp = Composition {
        ops: vec![ComposeOp::AuthorRule {
            id: "r_ai_ignite".to_string(),
            rule: ignite_rule("1_7", 4),
        }],
    };
    apply_composition(&mut a, &registry, &comp).expect("apply");
    assert_eq!(a.rules().len(), 1);
    log.append(&Record::Compose {
        composition: comp.clone(),
    });
    drop(a);

    // run B: fresh deterministic seed + replay → the composition re-applies on the same ids.
    let (mut b, scene) = seeded();
    let (applied, _skipped) = log.replay(&mut b, &scene, &MeshCatalog::new());
    assert_eq!(applied, 1, "the Compose record replayed");
    assert_eq!(
        b.rules().len(),
        1,
        "the AI-composed rule survives reload (re-applied deterministically through the same pipeline)"
    );
    log.clear();
}

#[test]
fn a_bad_composition_is_rejected_unchanged_and_explained() {
    let (mut a, _scene) = seeded();
    let registry = reg();

    // A rule on an event the engine doesn't know — invalid. The whole composition is refused.
    let mut bad = ignite_rule("1_3", 4);
    bad.event = "Nonsense".to_string();
    let comp = Composition {
        ops: vec![ComposeOp::AuthorRule {
            id: "r".to_string(),
            rule: bad,
        }],
    };

    // The proposal-time validation surfaces the explained reason (rejected-as-UX, before any apply).
    let err = validate_composition(&registry, &comp, |e| a.entity_exists(e)).unwrap_err();
    assert!(
        err.to_string().contains("isn't an event the engine knows"),
        "explained rejection: {err}"
    );

    // And apply refuses too — nothing is committed (all-or-nothing).
    assert!(apply_composition(&mut a, &registry, &comp).is_err());
    assert_eq!(a.rules().len(), 0, "a rejected composition applied nothing");
}

/// The flagship demo rule (mirrors the `compose_ai` builder) — for the replay + reject cases that don't go
/// through the composer.
fn ignite_rule(target: &str, threshold: i64) -> RuleData {
    RuleData {
        name: "ignite on kills".to_string(),
        enabled: true,
        event: "EnemyDied".to_string(),
        conditions: vec![Condition {
            entity: target.to_string(),
            component: "KillCounter".to_string(),
            field: "count".to_string(),
            op: CompareOp::Ge,
            value: FieldValue::Integer(threshold),
        }],
        actions: vec![Action {
            action: "SetField".to_string(),
            entity: target.to_string(),
            component: "Flammable".to_string(),
            field: "lit".to_string(),
            value: FieldValue::Bool(true),
        }],
    }
}
