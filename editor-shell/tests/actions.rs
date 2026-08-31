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

use metrocalk_editor_shell::actions::{actions_for, actions_for_selection, Action};
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

/// Every seeded entity carries a real, role-shaped name — not its raw Loro key.
///
/// The outliner falls back to the entity key when `__meta__.name` is absent, so an unnamed scene renders
/// as a column of `1_5` / `1_d` / `1_a` that tells a user nothing about what they are looking at. This is
/// asserted over EVERY entity rather than a sample, because one unnamed row is one row a person cannot
/// find by name, cannot search for, and cannot identify in the inspector.
#[test]
fn every_seeded_entity_is_named_for_what_it_actually_is() {
    let (engine, _scene, _index) = seeded();
    let mut roles: HashMap<String, usize> = HashMap::new();
    let mut entities = 0usize;
    for id in engine.entity_ids() {
        entities += 1;
        let name =
            metrocalk_editor_shell::capscene::entity_name(&engine, id).unwrap_or_else(|| {
                panic!("entity {id:?} has no name, so the outliner shows its raw key")
            });
        // "Health Bar 3" → the role is everything before the trailing number.
        let (role, number) = name
            .rsplit_once(' ')
            .unwrap_or_else(|| panic!("name {name:?} is not `<role> <number>`"));
        assert!(
            number.parse::<usize>().is_ok(),
            "name {name:?} does not end in a number, so two of the same role are indistinguishable"
        );
        *roles.entry(role.to_owned()).or_default() += 1;
    }
    assert_eq!(entities, N);
    // The vocabulary is closed: an unrecognised role means a seed path added entities this test has never
    // seen, which is exactly when a naming gap would slip back in.
    let known = [
        "Health Bar",
        "Character",
        "Prop",
        "Physics Body",
        "Speaker",
        "Marker",
        "Empty",
    ];
    for role in roles.keys() {
        assert!(known.contains(&role.as_str()), "unexpected role {role:?}");
    }
    // The two roles the first-run scene is built around must both be present, or the demo it exists to
    // support is not there either.
    assert!(roles.get("Health Bar").is_some_and(|&n| n > 0));
    assert!(roles.get("Character").is_some_and(|&n| n > 0));

    // Names are deterministic: the same seed produces the same scene, so a capture or a golden that
    // mentions "Character 4" keeps meaning the same entity. Compared BY ENTITY ID rather than by
    // iteration position — `entity_ids()` is not order-stable, and comparing positionally would report a
    // naming bug that is really just two engines enumerating the same scene in different orders.
    let (again, _s, _i) = seeded();
    let named = |e: &Engine<FlecsWorld>| -> HashMap<EntityId, Option<String>> {
        e.entity_ids()
            .into_iter()
            .map(|id| (id, metrocalk_editor_shell::capscene::entity_name(e, id)))
            .collect()
    };
    assert_eq!(named(&engine), named(&again));
}

// ─── The action model over a SELECTION (ADR-183) ──────────────────────────────────────────────────
//
// The editor has selected sets since M10.6 and this model answered about exactly one object, so the
// most direct surface in the product — right-click — offered the weakest verbs in it. These pin the
// SCOPE each verb reports, because scope is the whole content of the change: a `Delete` row over 378
// selected bolts that acts on one is the same defect the authoring toolbar's `Delete` had while its
// trigger read `Actions · 14`.

/// How many of the selection a verb says it acts on.
fn scope(items: &[metrocalk_editor_shell::ActionItem], a: Action) -> usize {
    items
        .iter()
        .find(|i| i.action == a)
        .expect("action present")
        .applies_to
}

#[test]
fn a_selection_scopes_every_verb_and_the_primary_is_the_last_id() {
    let (engine, scene, index) = seeded();
    let bars = [
        index.health_bars[0],
        index.health_bars[1],
        index.health_bars[2],
    ];

    let answer = actions_for_selection(&engine, &scene, &bars);
    assert_eq!((answer.count, answer.missing), (3, 0));

    // The verbs that take the whole set say so with the count, not with a bare `true`.
    for a in [Action::Remove, Action::Focus, Action::Inspect] {
        assert_eq!(
            scope(&answer.items, a),
            3,
            "{a:?} acts on the whole selection"
        );
    }
    // The two that honestly cannot: one clone beside one source, one reveal about one requirer.
    assert_eq!(scope(&answer.items, Action::Duplicate), 1);
    assert_eq!(scope(&answer.items, Action::Bind), 1);
    // `applies_to` IS the availability — there is no third state.
    assert!(answer
        .items
        .iter()
        .all(|i| i.available == (i.applies_to > 0)));

    // THE PRIMARY IS THE LAST ID, and Bind… is answered about it. Ending the selection on a bare
    // provider — which requires nothing — must grey Bind with the provider's own reason, not the
    // requirer's answer from the head of the list.
    let provider = nearest_provider(&engine, &scene, bars[0]);
    let ending_on_provider = actions_for_selection(&engine, &scene, &[bars[0], provider]);
    let bind = ending_on_provider
        .items
        .iter()
        .find(|i| i.action == Action::Bind)
        .unwrap();
    assert!(!bind.available);
    assert!(bind.reason.as_deref().unwrap().contains("no capabilities"));
}

#[test]
fn make_dynamic_counts_the_members_it_can_actually_act_on() {
    // A MIXED selection is the normal case after a marquee, and a flat refusal there would be wrong in
    // both directions: it would refuse work that is possible, and a flat "yes" would promise work that
    // is not. The count is the honest answer, and the reason still names what the rest are.
    let (mut engine, scene, index) = seeded();
    let mesh = engine.alloc_entity_id();
    engine
        .commit(
            "a dead mesh",
            vec![
                Op::CreateEntity {
                    id: mesh,
                    parent: None,
                },
                Op::SetField {
                    entity: mesh,
                    component: "MeshRenderer".into(),
                    field: "mesh".into(),
                    value: FieldValue::Str("mtkasset:bolt".into()),
                },
            ],
        )
        .unwrap();

    let bar = index.health_bars[0];
    let answer = actions_for_selection(&engine, &scene, &[bar, mesh]);
    assert_eq!(answer.count, 2);
    assert_eq!(
        scope(&answer.items, Action::MakeDynamic),
        1,
        "one of the two is a dead mesh"
    );
    let md = answer
        .items
        .iter()
        .find(|i| i.action == Action::MakeDynamic)
        .unwrap();
    assert!(
        md.available,
        "a partially-applicable verb is offered, not refused"
    );
    assert_eq!(
        md.reason.as_deref(),
        Some("1 of 2 can — the rest are already bodies, or are not meshes")
    );

    // Every member qualifying says nothing extra: there is no partial to explain.
    let all_meshes = actions_for_selection(&engine, &scene, &[mesh]);
    let md = all_meshes
        .items
        .iter()
        .find(|i| i.action == Action::MakeDynamic)
        .unwrap();
    assert_eq!((md.applies_to, md.reason.clone()), (1, None));
}

#[test]
fn a_dead_id_is_counted_missing_rather_than_silently_dropped_and_a_repeat_is_counted_once() {
    let (engine, scene, index) = seeded();
    let bar = index.health_bars[0];
    let ghost = EntityId::from_loro_key("1_ffff").unwrap();

    // A stale right-click after a Remove/Undo race: the live half still acts, and the menu is told how
    // many are gone so it can say so rather than quietly narrowing the scope it printed.
    let mixed = actions_for_selection(&engine, &scene, &[bar, ghost]);
    assert_eq!((mixed.count, mixed.missing), (1, 1));
    assert_eq!(scope(&mixed.items, Action::Remove), 1);

    // A repeated id would otherwise inflate every count the menu prints.
    let repeated = actions_for_selection(&engine, &scene, &[bar, bar, bar]);
    assert_eq!((repeated.count, repeated.missing), (1, 0));
}

#[test]
fn nothing_live_refuses_every_verb_and_the_two_reasons_are_different_facts() {
    let (engine, scene, _index) = seeded();
    let ghost = EntityId::from_loro_key("1_ffff").unwrap();

    let empty = actions_for_selection(&engine, &scene, &[]);
    assert_eq!((empty.count, empty.missing), (0, 0));
    assert!(empty
        .items
        .iter()
        .all(|i| !i.available && i.applies_to == 0));
    assert!(empty
        .items
        .iter()
        .all(|i| i.reason.as_deref() == Some("nothing is selected")));

    // Not the same sentence: a person can act on "nothing is selected" and cannot act on "they are
    // gone", so joining them with an `or` would answer neither (the `import cancelled or unsupported`
    // lesson, ADR-178).
    let gone = actions_for_selection(&engine, &scene, &[ghost]);
    assert_eq!((gone.count, gone.missing), (0, 1));
    assert!(gone
        .items
        .iter()
        .all(|i| i.reason.as_deref() == Some("the selected objects no longer exist")));

    // Every verb the model knows is answered in the refusals too — a new verb cannot be added to the
    // live answer and forgotten in the dead one.
    let live = actions_for_selection(&engine, &scene, &[index_first_bar(&engine, &scene)]);
    assert_eq!(empty.items.len(), live.items.len());
    assert_eq!(gone.items.len(), live.items.len());
}

/// Any live entity — the refusal path must answer about the same verb list the live path does.
fn index_first_bar(engine: &Engine<FlecsWorld>, _scene: &CapScene) -> EntityId {
    engine
        .entity_ids()
        .into_iter()
        .next()
        .expect("the seeded scene has entities")
}

#[test]
fn the_single_entity_form_is_the_selection_form_asked_about_one() {
    // ONE STATEMENT OF THE POLICY. Written side by side, the single form and the set form are two
    // things the compiler checks separately and never against each other — which is exactly how they
    // come to disagree about the same object while both compile (`<test_and_ci_discipline>` 6).
    let (mut engine, scene, index) = seeded();
    let bar = index.health_bars[0];
    let provider = nearest_provider(&engine, &scene, bar);
    capscene::bind(&mut engine, &scene, bar, provider).unwrap();
    let ghost = EntityId::from_loro_key("1_ffff").unwrap();

    for id in [bar, provider, index.health_bars[1], ghost] {
        let single = actions_for(&engine, &scene, id);
        let set = actions_for_selection(&engine, &scene, &[id]).items;
        assert_eq!(single.len(), set.len());
        for (a, b) in single.iter().zip(set.iter()) {
            assert_eq!(
                (a.action, a.available, a.applies_to, a.reason.clone()),
                (b.action, b.available, b.applies_to, b.reason.clone()),
                "the two forms disagree about {id:?}"
            );
        }
    }
}
