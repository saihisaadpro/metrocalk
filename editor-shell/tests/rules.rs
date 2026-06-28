//! M12.1 (ADR-045) — rules survive close→reopen via the `AuthorRule`/`RemoveRule` replay records (the shell
//! session-restore path), mirroring the camera/light replay tests. The Rule data model + registry validation
//! + the mirror offer are tested in `core/tests/rules.rs`; this guards the SHELL persistence wiring.

use std::path::PathBuf;

use metrocalk_core::{Action, CompareOp, Condition, Engine, FieldValue, Op, RuleData, RuleId};
use metrocalk_ecs::FlecsWorld;

use metrocalk_editor_shell::capscene::{self, CapScene};
use metrocalk_editor_shell::persist::{Log, Record};
use metrocalk_editor_shell::MeshCatalog;

const N: usize = 50;

fn tmp(name: &str) -> PathBuf {
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

/// The canonical test-#5 conditional as `RuleData` (the shape `author_rule` commits).
fn demo_rule() -> RuleData {
    RuleData {
        name: "rusty sword ignites".into(),
        enabled: true,
        event: "EnemyDied".into(),
        conditions: vec![Condition {
            entity: "1_0".into(),
            component: "KillCounter".into(),
            field: "count".into(),
            op: CompareOp::Ge,
            value: FieldValue::Integer(4),
        }],
        actions: vec![Action {
            action: "SetField".into(),
            entity: "1_0".into(),
            component: "Flammable".into(),
            field: "lit".into(),
            value: FieldValue::Bool(true),
        }],
    }
}

#[test]
fn a_rule_survives_close_then_reopen_via_replay() {
    let log = Log::open(tmp("rules"), capscene::fingerprint(N));
    log.clear();

    // run A: author a rule, persist its record.
    let (mut a, _scene_a) = seeded();
    let id = a.alloc_rule_id();
    let rule = demo_rule();
    a.commit(
        "author rule",
        vec![Op::SetRule {
            id: id.clone(),
            rule: rule.clone(),
        }],
    )
    .expect("author");
    log.append(&Record::AuthorRule {
        id: id.as_str().to_string(),
        rule: rule.clone(),
    });
    assert_eq!(a.rules().len(), 1);
    drop(a); // close

    // run B: fresh deterministic seed + replay (a true close→reopen).
    let (mut b, scene) = seeded();
    let (applied, _skipped) = log.replay(&mut b, &scene, &MeshCatalog::new());
    assert_eq!(applied, 1, "the AuthorRule record replayed");
    let restored = b
        .rule(&RuleId::new(id.as_str()))
        .expect("the rule is restored after reopen");
    assert_eq!(restored, rule, "every part of the rule survives the reload");
    log.clear();
}

#[test]
fn removing_a_rule_persists_across_reopen() {
    let log = Log::open(tmp("rules-remove"), capscene::fingerprint(N));
    log.clear();

    let (mut a, _s) = seeded();
    let id = a.alloc_rule_id();
    a.commit(
        "author",
        vec![Op::SetRule {
            id: id.clone(),
            rule: demo_rule(),
        }],
    )
    .unwrap();
    log.append(&Record::AuthorRule {
        id: id.as_str().to_string(),
        rule: demo_rule(),
    });
    a.commit("remove", vec![Op::RemoveRule { id: id.clone() }])
        .unwrap();
    log.append(&Record::RemoveRule {
        id: id.as_str().to_string(),
    });
    assert!(a.rules().is_empty());
    drop(a);

    // Replay author-then-remove → the rule does NOT come back (the removal is durable).
    let (mut b, scene) = seeded();
    let (applied, _skipped) = log.replay(&mut b, &scene, &MeshCatalog::new());
    assert_eq!(applied, 2, "both the author and the remove replayed");
    assert!(
        b.rules().is_empty(),
        "a removed rule stays removed after reopen"
    );
    log.clear();
}
