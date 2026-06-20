//! M3.3 viewport action model + transactional Remove/Duplicate — the durable, UI-agnostic core, through
//! the **real** `/core` engine (the part that survives the React `/editor` port). Mirrors
//! `north_star_1.rs` / `persistence.rs`: the action-model query (valid actions + every-"no"-explained),
//! Remove → undo restores the entity + its edges + frees the dependent, Duplicate → a fresh-id clone
//! with the same caps → undo removes it, and both **survive export→replay** (ADR-013).

#![allow(clippy::cast_precision_loss)]

use std::collections::HashMap;
use std::path::PathBuf;

use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::{Entity, FlecsWorld};

use metrocalk_editor_shell::actions::{actions_for, Action};
use metrocalk_editor_shell::capscene::{self, CapScene};
use metrocalk_editor_shell::persist::{Log, Record};
use metrocalk_editor_shell::reveal::{reveal, Context};
use metrocalk_editor_shell::TRACKS;

const N: usize = 200;

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("metrocalk-{name}.jsonl"))
}

fn seeded() -> (
    Engine<FlecsWorld>,
    CapScene,
    metrocalk_editor_shell::SeedIndex,
) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    let index = capscene::seed(&mut engine, &scene, N).expect("seed");
    engine.clear_history();
    (engine, scene, index)
}

fn has_binding(engine: &Engine<FlecsWorld>, from: EntityId, to: EntityId) -> bool {
    engine
        .bindings()
        .iter()
        .any(|(f, k, t)| *f == from && k == TRACKS && *t == to)
}

/// The nearest compatible provider a HealthBar can bind (so the tests have a real (bar, provider) pair).
fn nearest_provider(engine: &Engine<FlecsWorld>, scene: &CapScene, bar: EntityId) -> EntityId {
    let pos = capscene::positions(engine);
    let rec: HashMap<Entity, u64> = HashMap::new();
    let ctx = Context {
        cap_name: &scene.cap_name,
        position: &pos,
        recency: &rec,
    };
    let bar_ecs = engine.ecs_entity(bar).unwrap();
    let r = reveal(engine.world(), bar_ecs, scene.rels, &ctx);
    engine.entity_id_of(r.compatible[0].entity).unwrap()
}

fn action(
    engine: &Engine<FlecsWorld>,
    scene: &CapScene,
    id: EntityId,
    a: Action,
) -> (bool, Option<String>) {
    let items = actions_for(engine, scene, id);
    let item = items
        .iter()
        .find(|i| i.action == a)
        .expect("action present");
    (item.available, item.reason.clone())
}

#[test]
fn action_model_offers_valid_actions_and_explains_every_no() {
    let (mut engine, scene, index) = seeded();
    let bar = index.health_bars[0]; // a HealthBar: requires Health, unbound

    // A requirer with an unmet requirement → Bind… available; the always-on actions are all available.
    assert_eq!(action(&engine, &scene, bar, Action::Bind), (true, None));
    for a in [
        Action::Remove,
        Action::Duplicate,
        Action::Focus,
        Action::Inspect,
    ] {
        assert!(
            action(&engine, &scene, bar, a).0,
            "{a:?} is always available"
        );
    }

    // A bare provider (no required caps) → Bind… greyed with the specific reason.
    let provider = nearest_provider(&engine, &scene, bar);
    let (ok, reason) = action(&engine, &scene, provider, Action::Bind);
    assert!(!ok);
    assert!(
        reason.unwrap().contains("no capabilities"),
        "explains why bind is unavailable"
    );

    // After the HealthBar binds, Bind… greys "already bound to a provider".
    capscene::bind(&mut engine, &scene, bar, provider).unwrap();
    let (ok, reason) = action(&engine, &scene, bar, Action::Bind);
    assert!(!ok);
    assert!(reason.unwrap().contains("already bound"));

    // A non-existent id greys everything with the universal reason.
    let ghost = EntityId::from_loro_key("1_ffff").unwrap();
    assert!(actions_for(&engine, &scene, ghost)
        .iter()
        .all(|i| !i.available));
}

#[test]
fn bind_stays_available_for_a_multi_cap_requirer_bound_for_only_one() {
    // A requirer of TWO caps, bound for one, must still offer Bind for the other (the adversarial-review
    // finding: "has any binding" must not be read as "fully satisfied").
    let (mut engine, scene, index) = seeded();
    let r = engine.alloc_entity_id();
    engine
        .commit(
            "multi-req",
            vec![
                Op::CreateEntity {
                    id: r,
                    parent: None,
                },
                Op::SetField {
                    entity: r,
                    component: "Transform".into(),
                    field: "x".into(),
                    value: FieldValue::Number(0.0),
                },
                Op::AddPair {
                    entity: r,
                    rel: scene.rels.requires,
                    target: scene.cap("Health"),
                },
                Op::AddPair {
                    entity: r,
                    rel: scene.rels.requires,
                    target: scene.cap("Spatial"),
                },
            ],
        )
        .unwrap();
    assert!(
        action(&engine, &scene, r, Action::Bind).0,
        "two unmet caps → Bind available"
    );

    // Bind it to a Health provider → Health satisfied, Spatial still unmet.
    let provider = nearest_provider(&engine, &scene, index.health_bars[0]);
    capscene::bind(&mut engine, &scene, r, provider).unwrap();
    assert!(
        action(&engine, &scene, r, Action::Bind).0,
        "multi-cap requirer can still bind its remaining (Spatial) cap"
    );
}

#[test]
fn remove_frees_the_dependent_and_undo_restores_the_edge() {
    let (mut engine, scene, index) = seeded();
    let bar = index.health_bars[0];
    let provider = nearest_provider(&engine, &scene, bar);
    capscene::bind(&mut engine, &scene, bar, provider).unwrap();
    assert!(has_binding(&engine, bar, provider));
    let before = engine.entity_count();

    // Remove the PROVIDER → the binding edge is freed; the dependent HealthBar can re-bind.
    capscene::remove_entity(&mut engine, &scene, provider).expect("remove commits");
    assert!(!engine.entity_exists(provider), "provider gone");
    assert!(
        !has_binding(&engine, bar, provider),
        "dangling edge cleaned"
    );
    assert_eq!(engine.entity_count(), before - 1);
    // The freed HealthBar re-opens — the reveal offers replacements again.
    let pos = capscene::positions(&engine);
    let rec: HashMap<Entity, u64> = HashMap::new();
    let ctx = Context {
        cap_name: &scene.cap_name,
        position: &pos,
        recency: &rec,
    };
    let r = reveal(
        engine.world(),
        engine.ecs_entity(bar).unwrap(),
        scene.rels,
        &ctx,
    );
    assert!(
        !r.compatible.is_empty(),
        "the freed requirer re-offers compatible targets"
    );

    // ONE undoable transaction: Ctrl-Z restores the provider AND the binding edge atomically.
    assert!(engine.undo());
    assert!(
        engine.entity_exists(provider),
        "provider resurrected (M1.6)"
    );
    assert!(
        has_binding(&engine, bar, provider),
        "the binding edge is restored"
    );
    assert_eq!(engine.entity_count(), before);
}

#[test]
fn remove_requirer_frees_the_provider_marker() {
    // Removing the REQUIRER must clear the provider's consumed-marker (BindsTo) pair so the provider
    // re-enters the candidate set — else it's stranded "already bound" to a deleted entity.
    let (mut engine, scene, index) = seeded();
    let bar = index.health_bars[0];
    let provider = nearest_provider(&engine, &scene, bar);
    capscene::bind(&mut engine, &scene, bar, provider).unwrap();

    capscene::remove_entity(&mut engine, &scene, bar).expect("remove the requirer");
    assert!(!engine.entity_exists(bar));

    // Another HealthBar's reveal now includes the freed provider again.
    let other_bar = index.health_bars[1];
    let pos = capscene::positions(&engine);
    let rec: HashMap<Entity, u64> = HashMap::new();
    let ctx = Context {
        cap_name: &scene.cap_name,
        position: &pos,
        recency: &rec,
    };
    let r = reveal(
        engine.world(),
        engine.ecs_entity(other_bar).unwrap(),
        scene.rels,
        &ctx,
    );
    let provider_ecs = engine.ecs_entity(provider).unwrap();
    assert!(
        r.compatible.iter().any(|c| c.entity == provider_ecs),
        "the freed provider re-enters the candidate set (consumed-marker cleared)"
    );
}

#[test]
fn duplicate_clones_caps_under_a_fresh_id_and_is_independently_bindable() {
    let (mut engine, scene, index) = seeded();
    let bar = index.health_bars[0];
    let before = engine.entity_count();

    let clone = capscene::duplicate_entity(&mut engine, &scene, bar).expect("duplicate commits");
    assert_ne!(clone, bar, "fresh id, not an alias of the original");
    assert_eq!(engine.entity_count(), before + 1);
    // Same components (it's a HealthBar too).
    assert!(engine.components_of(clone).contains_key("HealthBar"));
    // Same required caps → independently bindable (its own reveal offers targets).
    let pos = capscene::positions(&engine);
    let rec: HashMap<Entity, u64> = HashMap::new();
    let ctx = Context {
        cap_name: &scene.cap_name,
        position: &pos,
        recency: &rec,
    };
    let r = reveal(
        engine.world(),
        engine.ecs_entity(clone).unwrap(),
        scene.rels,
        &ctx,
    );
    assert_eq!(
        r.required,
        vec!["Health".to_string()],
        "the clone requires Health like its source"
    );
    assert!(
        !r.compatible.is_empty(),
        "the clone is independently bindable"
    );
    // The clone carries NO binding of its own (fresh, unbound).
    assert!(!engine.bindings().iter().any(|(f, _, _)| *f == clone));

    // undo removes the clone.
    assert!(engine.undo());
    assert!(!engine.entity_exists(clone));
    assert_eq!(engine.entity_count(), before);
}

#[test]
fn remove_and_duplicate_survive_export_then_replay() {
    let log = Log::open(tmp("actions"), capscene::fingerprint(N));
    log.clear();

    // run A: duplicate a HealthBar, then remove a provider — persist both records.
    let (mut a, scene_a, index_a) = seeded();
    let bar = index_a.health_bars[0];
    let clone = capscene::duplicate_entity(&mut a, &scene_a, bar).unwrap();
    log.append(&Record::Duplicate {
        source: bar.to_loro_key(),
    });
    let provider = nearest_provider(&a, &scene_a, bar);
    capscene::remove_entity(&mut a, &scene_a, provider).unwrap();
    log.append(&Record::Remove {
        id: provider.to_loro_key(),
    });
    drop(a); // close

    // run B: fresh deterministic seed + replay (a true close→reopen).
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut b = Engine::new(world, 1);
    capscene::seed(&mut b, &scene, N).expect("re-seed");
    b.clear_history();
    let (applied, skipped) = b_replay(&log, &mut b, &scene);
    b.clear_history();

    assert_eq!(
        (applied, skipped),
        (2, 0),
        "duplicate + remove both replayed"
    );
    assert!(
        b.entity_exists(clone),
        "the duplicated clone survived reload at the same deterministic id"
    );
    assert!(
        !b.entity_exists(provider),
        "the removed provider stayed removed across reload"
    );
}

fn b_replay(log: &Log, engine: &mut Engine<FlecsWorld>, scene: &CapScene) -> (usize, usize) {
    log.replay(engine, scene, &HashMap::new())
}
