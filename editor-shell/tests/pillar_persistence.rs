//! **Does the user's work survive a reload?** — a `.mtk` round-trip over everything the three pillars
//! write (bar item B7 from the production audit).
//!
//! The audit's finding was blunt: `project_save_open.rs` covers exactly one M10.3 HealthBar scene, so
//! not one field of any pillar had ever been proven to come back. That matters most for the newest
//! plumbing: `RuleData.any_of` and `RuleData.subject` ride their own Loro slots written this session,
//! and a rule field with no read-back is precisely the kind of thing that vanishes silently on open —
//! the game still loads, it just quietly stops being the game you authored.
//!
//! Each test below fails if a single field is dropped.

use std::path::PathBuf;

use metrocalk_core::rules::{Action, CompareOp, Condition, RuleData, RuleId, SUBJECT_ENTITY};
use metrocalk_core::{Engine, EntityId, FieldValue, Op, Registry};
use metrocalk_ecs::FlecsWorld;

use metrocalk_editor_shell::capscene::{CapResolver, CapScene};
use metrocalk_editor_shell::condition_intent::{
    add_clause_ops, build_clause, clauses_of, ClauseRequest,
};
use metrocalk_editor_shell::project;
use metrocalk_editor_shell::role_intent::{assign_role, role_of, ROLE_COMPONENT};

fn engine_with_resolver() -> (Engine<FlecsWorld>, CapScene) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    engine.set_capability_resolver(Box::new(CapResolver::from_scene(&scene)));
    (engine, scene)
}

fn stdlib() -> Registry<FlecsWorld> {
    let mut registry = Registry::new(FlecsWorld::new());
    for meta in metrocalk_core::stdlib::standard_components() {
        registry.register(meta).expect("stdlib registers");
    }
    for event in metrocalk_core::stdlib::standard_events() {
        registry.register_event(event);
    }
    for action in metrocalk_core::stdlib::standard_actions() {
        registry.register_action(action);
    }
    registry
}

fn spawn(engine: &mut Engine<FlecsWorld>, x: f64) -> EntityId {
    let id = engine.alloc_entity_id();
    engine
        .commit(
            "spawn",
            vec![
                Op::CreateEntity { id, parent: None },
                Op::SetField {
                    entity: id,
                    component: "Transform".into(),
                    field: "x".into(),
                    value: FieldValue::Number(x),
                },
            ],
        )
        .expect("spawns");
    id
}

fn temp_mtk(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("metrocalk-pillar-{}-{tag}.mtk", std::process::id()))
}

#[test]
fn a_role_its_tuned_values_and_its_only_if_clauses_all_survive_save_and_open() {
    let path = temp_mtk("roles");
    let _ = std::fs::remove_file(&path);

    let (mut a, scene) = engine_with_resolver();
    let registry = stdlib();
    let coin = spawn(&mut a, 1.0);
    let key = spawn(&mut a, 3.0);
    assign_role(&mut a, &scene, &registry, coin, "collectible").expect("coin role");
    assign_role(&mut a, &scene, &registry, key, "collectible").expect("key role");

    // A tuned value the user typed (Round 6) — this is the field that was inert until Round 7.
    a.commit(
        "tune",
        vec![Op::SetField {
            entity: coin,
            component: ROLE_COMPONENT.into(),
            field: "points".into(),
            value: FieldValue::Integer(10),
        }],
    )
    .expect("tune commits");

    // An authored "only if" clause (Round 7).
    let clause = build_clause(
        &a,
        &ClauseRequest {
            kind: "other_gone".into(),
            object: Some(key.to_loro_key()),
            ..ClauseRequest::default()
        },
    )
    .expect("clause builds");
    let ops = add_clause_ops(&a, coin, clause, false).expect("clause ops");
    a.commit("only-if", ops).expect("clause commits");

    project::save(&a, &path).expect("save");

    // Reopen into a FRESH engine — the real "close the app and come back" path.
    let (mut b, _scene_b) = engine_with_resolver();
    project::open_into(&mut b, &path).expect("open");

    assert_eq!(
        role_of(&b, coin).as_deref(),
        Some("collectible"),
        "the role came back"
    );
    assert_eq!(
        b.get_field(coin, ROLE_COMPONENT, "points"),
        Some(FieldValue::Integer(10)),
        "the tuned Points value came back — not reset to the default"
    );
    let restored = clauses_of(&b, coin);
    assert_eq!(restored.all.len(), 1, "the only-if clause came back");
    assert_eq!(
        restored.all[0].entity,
        key.to_loro_key(),
        "and it still points at the same object"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_rules_or_group_and_subject_pin_survive_save_and_open() {
    let path = temp_mtk("ruleslots");
    let _ = std::fs::remove_file(&path);

    let (mut a, _scene) = engine_with_resolver();
    let target = spawn(&mut a, 0.0);
    let key = target.to_loro_key();

    // Both fields are new Loro slots written this session. If either write or read is missing, the
    // rule still loads — just quietly without its OR group or its pin, which changes the game.
    let rule = RuleData {
        name: "gated".into(),
        enabled: true,
        event: "Touched".into(),
        conditions: vec![Condition {
            entity: key.clone(),
            component: ROLE_COMPONENT.into(),
            field: "active".into(),
            op: CompareOp::Eq,
            value: FieldValue::Bool(true),
        }],
        any_of: vec![
            Condition {
                entity: key.clone(),
                component: "KillCounter".into(),
                field: "count".into(),
                op: CompareOp::Ge,
                value: FieldValue::Integer(3),
            },
            Condition {
                entity: key.clone(),
                component: ROLE_COMPONENT.into(),
                field: "role".into(),
                op: CompareOp::Eq,
                value: FieldValue::Str("player".into()),
            },
        ],
        subject: Some(key.clone()),
        actions: vec![Action {
            action: "SetField".into(),
            entity: SUBJECT_ENTITY.into(),
            component: ROLE_COMPONENT.into(),
            field: "active".into(),
            value: FieldValue::Bool(false),
        }],
    };
    a.commit(
        "author rule",
        vec![Op::SetRule {
            id: RuleId::new("r_gated"),
            rule: rule.clone(),
        }],
    )
    .expect("rule commits");

    project::save(&a, &path).expect("save");
    let (mut b, _scene_b) = engine_with_resolver();
    project::open_into(&mut b, &path).expect("open");

    let back = b.rule(&RuleId::new("r_gated")).expect("the rule came back");
    assert_eq!(back.conditions.len(), 1, "the AND clause survived");
    assert_eq!(back.any_of.len(), 2, "the OR group survived the reload");
    assert_eq!(
        back.subject.as_deref(),
        Some(key.as_str()),
        "the subject pin survived the reload"
    );
    assert_eq!(back, rule, "the whole rule round-trips byte-for-byte");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn clearing_an_or_group_persists_as_cleared_rather_than_leaving_a_stale_slot() {
    let path = temp_mtk("cleared");
    let _ = std::fs::remove_file(&path);

    let (mut a, _scene) = engine_with_resolver();
    let target = spawn(&mut a, 0.0);
    let id = RuleId::new("r_clear");
    let base = RuleData {
        name: "gated".into(),
        enabled: true,
        event: "Touched".into(),
        conditions: vec![],
        any_of: vec![Condition {
            entity: target.to_loro_key(),
            component: "KillCounter".into(),
            field: "count".into(),
            op: CompareOp::Ge,
            value: FieldValue::Integer(3),
        }],
        subject: Some(target.to_loro_key()),
        actions: vec![Action {
            action: "AdjustCounter".into(),
            entity: target.to_loro_key(),
            component: "KillCounter".into(),
            field: "count".into(),
            value: FieldValue::Integer(1),
        }],
    };
    a.commit(
        "author",
        vec![Op::SetRule {
            id: id.clone(),
            rule: base.clone(),
        }],
    )
    .expect("commits");

    // Now the user removes the alternative and the pin — overwriting the SAME rule id.
    let cleared = RuleData {
        any_of: vec![],
        subject: None,
        ..base
    };
    a.commit(
        "clear",
        vec![Op::SetRule {
            id: id.clone(),
            rule: cleared.clone(),
        }],
    )
    .expect("commits");

    project::save(&a, &path).expect("save");
    let (mut b, _scene_b) = engine_with_resolver();
    project::open_into(&mut b, &path).expect("open");

    let back = b.rule(&id).expect("rule");
    assert!(
        back.any_of.is_empty(),
        "a removed OR group must not resurrect from a stale slot"
    );
    assert!(back.subject.is_none(), "a removed pin must stay removed");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_cutscene_and_its_effects_survive_save_and_open() {
    // The two newest pillars write canonical JSON into `Cinematic.source` / `Vfx.source`. A blob field
    // that does not round-trip fails in the worst possible way: the project opens, the object is there,
    // and the cutscene and the fire are simply gone with no error anywhere.
    let path = temp_mtk("cinema-vfx");
    let _ = std::fs::remove_file(&path);

    let (mut a, _scene) = engine_with_resolver();
    let statue = spawn(&mut a, 2.0);

    let (ops, _) = metrocalk_editor_shell::add_shot_ops(&a, statue, "hero", statue).expect("shot");
    a.commit("shot", ops).expect("shot commits");
    let (ops, _) =
        metrocalk_editor_shell::add_shot_ops(&a, statue, "orbit", statue).expect("shot 2");
    a.commit("shot", ops).expect("shot 2 commits");

    let (ops, _) = metrocalk_editor_shell::add_effect_ops(
        &a,
        statue,
        "fire",
        metrocalk_editor_shell::VfxTrigger::Always,
    )
    .expect("fire");
    a.commit("fx", ops).expect("fire commits");
    let (ops, _) = metrocalk_editor_shell::add_effect_ops(
        &a,
        statue,
        "smoke",
        metrocalk_editor_shell::VfxTrigger::Always,
    )
    .expect("smoke");
    a.commit("fx", ops).expect("smoke commits");

    let before_cut = metrocalk_editor_shell::cutscene_of(&a, statue);
    let before_fx = metrocalk_editor_shell::stack_of(&a, statue);
    assert_eq!(before_cut.shots.len(), 2);
    assert_eq!(before_fx.layers.len(), 2);

    project::save(&a, &path).expect("save");

    let (mut b, _scene_b) = engine_with_resolver();
    project::open_into(&mut b, &path).expect("open");

    let after_cut = metrocalk_editor_shell::cutscene_of(&b, statue);
    let after_fx = metrocalk_editor_shell::stack_of(&b, statue);
    // Not "roughly the same" — the SAME. A shot's framing and an effect's colour ramp are the whole
    // artifact, so anything less than structural equality is a silent loss.
    assert_eq!(
        after_cut, before_cut,
        "the cutscene changed across a reload"
    );
    assert_eq!(after_fx, before_fx, "the effects changed across a reload");
    // and the specifics, spelled out, so a future refactor that guts one field still fails here
    assert_eq!(after_cut.shots[0].subject, statue.to_loro_key());
    assert_eq!(after_fx.layers[0].kind, "fire");
    assert_eq!(after_fx.layers[1].kind, "smoke");
    assert!(
        after_fx.layers[0].color[0] > 1.0,
        "the HDR ramp must survive"
    );

    let _ = std::fs::remove_file(&path);
}
