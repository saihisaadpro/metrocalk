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

use metrocalk_editor_shell::actions::{actions_for, actions_for_selection, Action, ActionItem};
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

    let answer = actions_for_selection(&engine, &scene, &bars, 0);
    assert_eq!((answer.count, answer.missing), (3, 0));

    // The verbs that take the whole set say so with the count, not with a bare `true`.
    // `Duplicate` joined them in ADR-196 — it was the last one reporting `1` over a set it was
    // offered from, and the number is what a row's scope note is drawn from, so it was also the
    // thing telling the user the truth about a verb that then did something else.
    for a in [
        Action::Remove,
        Action::Focus,
        Action::Inspect,
        Action::Duplicate,
    ] {
        assert_eq!(
            scope(&answer.items, a),
            3,
            "{a:?} acts on the whole selection"
        );
    }
    // The one that honestly cannot: a reveal asks its question about ONE requirer.
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
    let ending_on_provider = actions_for_selection(&engine, &scene, &[bars[0], provider], 0);
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
    let answer = actions_for_selection(&engine, &scene, &[bar, mesh], 0);
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
    let all_meshes = actions_for_selection(&engine, &scene, &[mesh], 0);
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
    let mixed = actions_for_selection(&engine, &scene, &[bar, ghost], 0);
    assert_eq!((mixed.count, mixed.missing), (1, 1));
    assert_eq!(scope(&mixed.items, Action::Remove), 1);

    // A repeated id would otherwise inflate every count the menu prints.
    let repeated = actions_for_selection(&engine, &scene, &[bar, bar, bar], 0);
    assert_eq!((repeated.count, repeated.missing), (1, 0));
}

#[test]
fn nothing_live_refuses_every_verb_and_the_two_reasons_are_different_facts() {
    let (engine, scene, _index) = seeded();
    let ghost = EntityId::from_loro_key("1_ffff").unwrap();

    // Every verb EXCEPT `Paste`, which is not about the selection: see the assertions below.
    let selection_verbs = |a: &metrocalk_editor_shell::SelectionActions| -> Vec<ActionItem> {
        a.items
            .iter()
            .filter(|i| i.action != Action::Paste)
            .cloned()
            .collect()
    };

    let empty = actions_for_selection(&engine, &scene, &[], 0);
    assert_eq!((empty.count, empty.missing), (0, 0));
    assert!(selection_verbs(&empty)
        .iter()
        .all(|i| !i.available && i.applies_to == 0));
    assert!(selection_verbs(&empty)
        .iter()
        .all(|i| i.reason.as_deref() == Some("nothing is selected")));

    // Not the same sentence: a person can act on "nothing is selected" and cannot act on "they are
    // gone", so joining them with an `or` would answer neither (the `import cancelled or unsupported`
    // lesson, ADR-178).
    let gone = actions_for_selection(&engine, &scene, &[ghost], 0);
    assert_eq!((gone.count, gone.missing), (0, 1));
    assert!(selection_verbs(&gone)
        .iter()
        .all(|i| i.reason.as_deref() == Some("the selected objects no longer exist")));

    // Every verb the model knows is answered in the refusals too — a new verb cannot be added to the
    // live answer and forgotten in the dead one.
    let live = actions_for_selection(&engine, &scene, &[index_first_bar(&engine, &scene)], 0);
    assert_eq!(empty.items.len(), live.items.len());
    assert_eq!(gone.items.len(), live.items.len());
}

/// **`Paste` is the one row an empty selection does not refuse** (ADR-198).
///
/// Every other verb in this model is a sentence about the selected objects, so "nothing is selected"
/// answers all of them at once. Paste is a sentence about the clipboard, and *nothing selected* is
/// the state a person is most often in when they reach for it — a right-click on empty space, having
/// just copied something. The model refusing it there would be its own shape answering a question it
/// was never asked.
#[test]
fn paste_is_offered_with_nothing_selected_and_refused_with_an_empty_clipboard() {
    let (engine, scene, _index) = seeded();
    let paste_row = |ids: &[EntityId], clipboard: usize| -> ActionItem {
        actions_for_selection(&engine, &scene, ids, clipboard)
            .items
            .into_iter()
            .find(|i| i.action == Action::Paste)
            .expect("the model answers about Paste")
    };

    let held = paste_row(&[], 3);
    assert!(held.available, "nothing selected does not refuse a paste");
    assert_eq!(held.applies_to, 3, "its scope is what the CLIPBOARD holds");

    let nothing_held = paste_row(&[], 0);
    assert!(!nothing_held.available);
    assert_eq!(
        nothing_held.reason.as_deref(),
        Some("nothing has been copied or cut yet"),
        "the refusal names the clipboard, not the selection (every no explained)"
    );

    // And the answer does not change when there IS a selection: the clipboard is the whole scope.
    let with_selection = paste_row(&[index_first_bar(&engine, &scene)], 2);
    assert!(with_selection.available);
    assert_eq!(with_selection.applies_to, 2);
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
        let set = actions_for_selection(&engine, &scene, &[id], 0).items;
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

// ═════════════════════════════════════════════════════════════════════════════════════════════════
// DUPLICATE, ASKED ABOUT WHAT WAS ACTUALLY SELECTED. Three properties, each written before the fix
// that makes it pass, because each one is a thing the shipped verb quietly did NOT do.
// ═════════════════════════════════════════════════════════════════════════════════════════════════

/// A duplicated group carries its contents.
///
/// `duplicate_entity` cloned one entity's components and pairs and stopped there, so duplicating any
/// node with children — a `capscene::group`, an imported CAD assembly's folder (ADR-163), a role's
/// rig — produced an EMPTY node under the same parent, with the same name, and nothing in the
/// viewport. Nothing failed: the transaction committed, a fresh id came back, the outliner grew a
/// row. The clone was just hollow.
#[test]
fn duplicating_a_group_clones_what_is_inside_it() {
    let (mut engine, scene, _) = seeded();
    let a = capscene::create_entity(&mut engine, [0.0, 0.0, 0.0], "Bolt").unwrap();
    let b = capscene::create_entity(&mut engine, [1.0, 0.0, 0.0], "Nut").unwrap();
    let group = capscene::group(&mut engine, &[a, b], "Fixing").unwrap();
    assert_eq!(engine.children_of(group).len(), 2, "the group holds two");

    let clone = capscene::duplicate_entity(&mut engine, &scene, group).expect("duplicate commits");
    assert_eq!(
        engine.children_of(clone).len(),
        2,
        "a duplicated group is not an empty node"
    );
    // And the copies are COPIES — the originals are still under the original group.
    assert_eq!(engine.children_of(group).len(), 2, "the source is untouched");
}

/// A clone can be told apart from its source.
///
/// Every field was cloned including `__meta__.name`, so duplicating `Weld Gun` twice left three rows
/// reading `Weld Gun` in the outliner, three identical hits in `Find objects…` (ADR-185) and three
/// identical toasts. `<ux_quality>` 4 is information scent; three rows that cannot be told apart have
/// none.
#[test]
fn a_clone_is_named_so_the_outliner_can_tell_it_from_its_source() {
    let (mut engine, scene, _) = seeded();
    let src = capscene::create_entity(&mut engine, [0.0, 0.0, 0.0], "Weld Gun").unwrap();

    let first = capscene::duplicate_entity(&mut engine, &scene, src).unwrap();
    let second = capscene::duplicate_entity(&mut engine, &scene, src).unwrap();

    let name_of = |e: &Engine<FlecsWorld>, id| capscene::entity_name(e, id).unwrap_or_default();
    let (a, b, c) = (
        name_of(&engine, src),
        name_of(&engine, first),
        name_of(&engine, second),
    );
    assert_eq!(a, "Weld Gun", "the source keeps its name");
    assert_ne!(b, a, "the first clone is distinguishable");
    assert_ne!(c, a, "the second clone is distinguishable");
    assert_ne!(b, c, "and the two clones are distinguishable from each other");
}

/// Duplicate over a SELECTION duplicates the selection.
///
/// The last ADR-183 verb still acting on one of what it names: the right-click menu, the authoring
/// toolbar row and the action model all reported `applies_to: 1` over a selection of any size, and
/// the toolbar's own description said so — *"the other 13 are left alone"*. One transaction, one
/// undo, N clones.
#[test]
fn duplicating_a_selection_clones_every_member_in_one_transaction() {
    let (mut engine, scene, _) = seeded();
    let ids: Vec<EntityId> = (0..3)
        .map(|i| {
            capscene::create_entity(&mut engine, [i as f32, 0.0, 0.0], &format!("Part {i}")).unwrap()
        })
        .collect();
    let before = engine.entity_count();

    let made = capscene::duplicate_selection(&mut engine, &scene, &ids).expect("commits");
    assert_eq!(made.roots.len(), 3, "one clone per selected object");
    assert_eq!(engine.entity_count(), before + 3);

    // ONE transaction: a single undo takes all three back (inv. 3).
    assert!(engine.undo());
    assert_eq!(engine.entity_count(), before, "one undo, all three gone");
}

/// A parent AND its child selected duplicates the parent once, not the child twice.
///
/// The everyday way to arrive here is Select all over an imported assembly, or a marquee across a
/// group: every part is selected along with the folder that holds it. Cloning each selected id in
/// turn would copy every part twice — once inside the parent's copy, once beside it — and the second
/// copy would sit at the root of the scene looking like a stray.
#[test]
fn a_selection_holding_a_parent_and_its_child_duplicates_the_parent_once() {
    let (mut engine, scene, _) = seeded();
    let a = capscene::create_entity(&mut engine, [0.0, 0.0, 0.0], "Bolt").unwrap();
    let b = capscene::create_entity(&mut engine, [1.0, 0.0, 0.0], "Nut").unwrap();
    let group = capscene::group(&mut engine, &[a, b], "Fixing").unwrap();
    let before = engine.entity_count();

    // The whole subtree selected, folder included — three ids, one thing.
    let made = capscene::duplicate_selection(&mut engine, &scene, &[group, a, b]).unwrap();

    assert_eq!(made.roots.len(), 1, "one copy, of the top-most node");
    assert_eq!(made.nested, 2, "and the two inside it are counted, not silently dropped");
    assert_eq!(
        engine.entity_count(),
        before + 3,
        "three entities: the group's copy and its two children"
    );
    assert_eq!(engine.children_of(made.roots[0]).len(), 2);
    // ONE transaction still, over a subtree.
    assert!(engine.undo());
    assert_eq!(engine.entity_count(), before);
}

/// The copies keep their arrangement, and land BESIDE what was copied rather than inside it.
///
/// Two properties in one, because they are the same number. A fixed 1.5 m offset per object would
/// have put a copied row of parts spanning 8 m straight through the original — and applying it per
/// object rather than per set would have collapsed the arrangement the user built.
#[test]
fn copies_keep_their_arrangement_and_clear_what_they_were_copied_from() {
    let (mut engine, scene, _) = seeded();
    let xs = [0.0f32, 3.0, 8.0];
    let ids: Vec<EntityId> = xs
        .iter()
        .enumerate()
        .map(|(i, &x)| capscene::create_entity(&mut engine, [x, 0.0, 0.0], &format!("Part {i}")).unwrap())
        .collect();

    let made = capscene::duplicate_selection(&mut engine, &scene, &ids).unwrap();

    let x_of = |e: &Engine<FlecsWorld>, id: EntityId| -> f64 {
        match e.components_of(id).get("Transform").and_then(|t| t.get("x")) {
            Some(FieldValue::Number(n)) => *n,
            Some(FieldValue::Integer(i)) => *i as f64,
            _ => panic!("a duplicated entity has no Transform.x"),
        }
    };
    let copies: Vec<f64> = made.roots.iter().map(|&id| x_of(&engine, id)).collect();
    let gap = |a: f64, b: f64, expected: f64| {
        assert!(
            (a - b - expected).abs() < 1e-5,
            "the copies are {} apart where the sources are {expected}",
            a - b
        );
    };
    // ARRANGEMENT: the gaps between the copies are the gaps between the sources.
    gap(copies[1], copies[0], f64::from(xs[1] - xs[0]));
    gap(copies[2], copies[1], f64::from(xs[2] - xs[1]));
    // CLEAR: the nearest copy starts beyond the far end of what was copied. A per-object 1.5 m would
    // have put the first copy at 1.5 — inside a set that runs to 8.
    assert!(
        copies[0] > f64::from(xs[2]),
        "the copies start at {} but the sources run to {}",
        copies[0],
        xs[2]
    );
}

/// A duplicated selection survives export → replay as ONE transaction (ADR-013).
///
/// The reason there is a `Record::DuplicateMany` at all rather than N `Record::Duplicate`s: N records
/// replay as N commits, so the `Record::Undo` that took the whole set back live takes exactly one of
/// them back on reopen — the divergence the live `.exe` run found for `DeleteDeactivateMany`, which
/// would have been reintroduced here by the cheaper spelling.
#[test]
fn a_duplicated_selection_replays_as_one_transaction() {
    let log = Log::open(tmp("duplicate-many"), capscene::fingerprint(N));
    log.clear();

    let (mut a, scene_a, index_a) = seeded();
    let sources = [index_a.health_bars[0], index_a.health_bars[1]];
    let made = capscene::duplicate_selection(&mut a, &scene_a, &sources).unwrap();
    log.append(&Record::DuplicateMany {
        sources: sources.iter().map(EntityId::to_loro_key).collect(),
    });
    // …and the live Ctrl-Z that follows takes BOTH copies back.
    assert!(a.undo());
    log.append(&Record::Undo);
    let clones = made.roots.clone();
    drop(a);

    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut b = Engine::new(world, 1);
    capscene::seed(&mut b, &scene, N).expect("re-seed");
    b.clear_history();
    let (applied, skipped) = b_replay(&log, &mut b, &scene);
    b.clear_history();

    assert_eq!((applied, skipped), (2, 0), "the duplicate and the undo both replayed");
    for clone in clones {
        assert!(
            !b.entity_exists(clone),
            "a copy the user undid came back after a reopen — the undo reverted one record, the \
             duplicate made two objects"
        );
    }
}

/// An entity's `Transform.x`, the way every surface that places something reads it.
fn transform_x(engine: &Engine<FlecsWorld>, id: EntityId) -> f64 {
    match engine.components_of(id).get("Transform").and_then(|t| t.get("x")) {
        Some(FieldValue::Number(n)) => *n,
        Some(FieldValue::Integer(i)) => *i as f64,
        _ => panic!("an entity that should have a Transform.x has none"),
    }
}

// ═════════════════════════════════════════════════════════════════════════════════════════════════
// THE CLIPBOARD, ASKED ABOUT WHAT WAS ACTUALLY SELECTED. The first three were written before the fix
// that makes them pass, because each one is a thing the shipped verb quietly did NOT do.
// `Copy`/`Cut`/`Paste` were the last three verbs in the authoring toolbar still reading "one subtree
// at a time" (ADR-196's owed item 1) - and the round trip lost more than the rest of the selection.
// =================================================================================================

/// A pasted copy is still called something.
///
/// `Engine::save_composition` skipped the whole `__meta__` component so a *re-snapshot* would not
/// compound the `composition` provenance link. But `__meta__` holds the user-facing **name** (M10.6
/// rename) and the **primitive kind** the renderer draws from, in the same component - so copy then
/// paste dropped the object's name and its shape along with the provenance nobody wanted.
#[test]
fn a_pasted_copy_keeps_the_name_of_what_was_copied() {
    let (mut engine, scene, _) = seeded();
    let src = capscene::create_entity(&mut engine, [0.0, 0.0, 0.0], "Weld Gun").unwrap();

    let held = capscene::copy_selection(&engine, &scene, &[src], false).clipboard;
    let made = capscene::paste_clipboard(&mut engine, &scene, &held, 0).expect("paste commits");

    let name = capscene::entity_name(&engine, made.roots[0]);
    assert!(name.is_some(), "a pasted copy came back with no name at all");
    // And it is distinguishable from the object it was copied from, the ADR-196 rule: the source is
    // still there holding `Weld Gun`.
    assert_eq!(name.as_deref(), Some("Weld Gun copy"));
}

/// A pasted copy lands where a person can see it.
///
/// The composition bakes the source's resolved `Transform`, so the paste landed at exactly the
/// source's coordinates - one object hidden inside another, with a toast saying "pasted". The
/// round-trip test asserted that coincidence as a feature ("geometry round-trips").
#[test]
fn a_paste_lands_beside_what_it_was_copied_from_rather_than_inside_it() {
    let (mut engine, scene, _) = seeded();
    let src = capscene::create_entity(&mut engine, [4.0, 0.0, 0.0], "Bolt").unwrap();

    let held = capscene::copy_selection(&engine, &scene, &[src], false).clipboard;
    let made = capscene::paste_clipboard(&mut engine, &scene, &held, 0).expect("paste commits");

    let (a, b) = (transform_x(&engine, made.roots[0]), transform_x(&engine, src));
    assert!(
        (a - b).abs() > 0.5,
        "the copy landed at {a} and the source is at {b} - the same place"
    );
}

/// A pasted copy can still be bound.
///
/// A `Composition` carries components, not capability pairs, so the pasted entity arrived with no
/// `Requires`/`Provides` at all: the M3.1 reveal - the product's headline gesture - had nothing to
/// offer for it. `duplicate_selection` copies the pairs; paste did not, and the two verbs are the
/// same verb with a clipboard in between.
#[test]
fn a_pasted_copy_keeps_the_capabilities_of_what_was_copied() {
    let (mut engine, scene, index) = seeded();
    let bar = index.health_bars[0];

    let held = capscene::copy_selection(&engine, &scene, &[bar], false).clipboard;
    let made = capscene::paste_clipboard(&mut engine, &scene, &held, 0).expect("paste commits");

    let pos = capscene::positions(&engine);
    let rec: HashMap<Entity, u64> = HashMap::new();
    let ctx = Context {
        cap_name: &scene.cap_name,
        position: &pos,
        recency: &rec,
    };
    let r = reveal(
        engine.world(),
        engine.ecs_entity(made.roots[0]).unwrap(),
        scene.rels,
        &ctx,
    );
    assert_eq!(
        r.required,
        vec!["Health".to_string()],
        "the pasted copy requires Health like the object it was copied from"
    );
    assert!(
        !r.compatible.is_empty(),
        "and it is independently bindable, like a duplicate"
    );
}

/// **Copy over a SELECTION copies the selection**, with everything inside each member.
///
/// The clipboard held one `Composition`, so `Copy` took the primary out of however many objects were
/// selected - and said so in the toolbar's own description, *"Copy Weld Gun - one subtree at a
/// time"*. The reduction to top-most is `Duplicate`'s (ADR-196) and is here for the same reason: a
/// marquee over a group selects the folder and its parts, and copying each in turn pastes the parts
/// twice.
#[test]
fn copying_a_selection_puts_every_object_on_the_clipboard_with_what_is_inside_them() {
    let (mut engine, scene, _) = seeded();
    let a = capscene::create_entity(&mut engine, [0.0, 0.0, 0.0], "Bolt").unwrap();
    let b = capscene::create_entity(&mut engine, [1.0, 0.0, 0.0], "Nut").unwrap();
    let group = capscene::group(&mut engine, &[a, b], "Fixing").unwrap();
    let loose = capscene::create_entity(&mut engine, [5.0, 0.0, 0.0], "Washer").unwrap();
    let before = engine.entity_count();

    // The whole subtree selected, folder included - four ids, two things.
    let copied = capscene::copy_selection(&engine, &scene, &[group, a, b, loose], false);
    assert_eq!(copied.clipboard.len(), 2, "two roots, not four and not one");
    assert_eq!(
        copied.nested, 2,
        "the two inside the group are counted, not silently dropped"
    );
    assert_eq!(
        copied.clipboard.entries[0].composition.nodes.len(),
        3,
        "the group went on the clipboard WITH its contents"
    );

    let made =
        capscene::paste_clipboard(&mut engine, &scene, &copied.clipboard, 0).expect("paste commits");
    assert_eq!(made.roots.len(), 2);
    assert_eq!(engine.entity_count(), before + 4, "3 + 1 entities pasted");
    // ONE transaction: a single undo takes the whole paste back (inv. 3).
    assert!(engine.undo());
    assert_eq!(
        engine.entity_count(),
        before,
        "one undo, the whole paste gone"
    );
}

/// A part copied out of an assembly pastes back **into the assembly**.
///
/// `instantiate_composition` created every root with `parent: None`, so a bolt copied from inside a
/// 15,711-part import reappeared at the top of the scene - a row a person then has to find and drag
/// back. The clipboard carries the source's parent as a loro KEY, so a clipboard that has crossed
/// into another project is stale rather than silently aliasing that project's object at the same id.
#[test]
fn a_paste_lands_back_under_the_parent_it_was_copied_out_of() {
    let (mut engine, scene, _) = seeded();
    let bolt = capscene::create_entity(&mut engine, [0.0, 0.0, 0.0], "Bolt").unwrap();
    let assembly = capscene::group(&mut engine, &[bolt], "Assembly").unwrap();

    let held = capscene::copy_selection(&engine, &scene, &[bolt], false).clipboard;
    let made = capscene::paste_clipboard(&mut engine, &scene, &held, 0).expect("paste commits");

    assert_eq!(
        engine.parent_of(made.roots[0]),
        Some(assembly),
        "the copy landed at the root of the scene instead of beside the part it came from"
    );
    assert_eq!(engine.children_of(assembly).len(), 2);

    // A clipboard whose parent is not in THIS document lands at the top level rather than under
    // whatever happens to hold that id here.
    let (mut other, other_scene, _) = seeded();
    let elsewhere =
        capscene::paste_clipboard(&mut other, &other_scene, &held, 0).expect("cross-project paste");
    assert_eq!(other.parent_of(elsewhere.roots[0]), None);
}

/// **A cut and a paste is a MOVE** - so the first paste puts the object back where it was, under the
/// name it already had.
///
/// Two properties that are the same decision. The placement offset exists because a copy has to clear
/// the object it was copied from; a cut has no such object left on screen, and putting it down 1.5 m
/// from where it was picked up is not a move. The name is free for exactly the same reason, which is
/// why the sibling scan skips deactivated rows rather than special-casing a cut.
#[test]
fn a_cut_and_a_paste_puts_the_object_back_where_it_was_under_its_own_name() {
    let (mut engine, scene, _) = seeded();
    let src = capscene::create_entity(&mut engine, [4.0, 0.0, 0.0], "Weld Gun").unwrap();

    let (copied, gone) = capscene::cut_selection(&mut engine, &scene, &[src]).expect("cut commits");
    assert_eq!(gone, vec![src]);
    assert!(!engine.is_active(src), "cut deactivates (non-destructive)");
    assert!(engine.ecs_entity(src).is_some(), "and does not destroy");

    let made =
        capscene::paste_clipboard(&mut engine, &scene, &copied.clipboard, 0).expect("paste commits");
    let landed = transform_x(&engine, made.roots[0]);
    assert!(
        (landed - 4.0).abs() < 1e-5,
        "a cut pasted back landed at {landed} instead of the 4.0 it was cut from"
    );
    assert_eq!(
        capscene::entity_name(&engine, made.roots[0]).as_deref(),
        Some("Weld Gun"),
        "a moved object was renamed to a copy of itself"
    );
}

/// Pasting twice makes two objects **in two places**.
///
/// The offset is computed from the clipboard's own extent, so without a step every paste of the same
/// clipboard lands in the same spot: three Ctrl-Vs, three objects, one visible. That is the
/// coincident-paste defect one level up, and it is the one a person meets first - repeat-paste is
/// what a clipboard is FOR.
#[test]
fn pasting_the_same_clipboard_twice_makes_two_objects_in_two_places() {
    let (mut engine, scene, _) = seeded();
    let src = capscene::create_entity(&mut engine, [0.0, 0.0, 0.0], "Bolt").unwrap();
    let held = capscene::copy_selection(&engine, &scene, &[src], false).clipboard;

    let first = capscene::paste_clipboard(&mut engine, &scene, &held, 0).unwrap();
    let second = capscene::paste_clipboard(&mut engine, &scene, &held, 1).unwrap();

    let (a, b) = (
        transform_x(&engine, first.roots[0]),
        transform_x(&engine, second.roots[0]),
    );
    assert!((a - b).abs() > 0.5, "both pastes landed at {a}");
    // And they can be told apart in the outliner, which is the other half of "two objects".
    assert_ne!(
        capscene::entity_name(&engine, first.roots[0]),
        capscene::entity_name(&engine, second.roots[0])
    );
}

/// **A paste survives a close and reopen** - as ONE transaction (ADR-013).
///
/// Copy, cut and paste had no `Record` at all: the verbs were commits to the Loro doc, which an
/// explicit `.mtk` save persists, and invisible to the in-session replay log that restores the scene
/// on the next launch. So a pasted object vanished, silently, and a cut object came back.
///
/// The record carries the whole clipboard rather than the source ids - the difference from
/// `DuplicateMany`, which re-derives its copies from objects the document still has. A paste's
/// sources may have been cut away, deleted, or left behind in another project.
#[test]
fn a_paste_survives_a_close_and_reopen_as_one_transaction() {
    let log = Log::open(tmp("paste"), capscene::fingerprint(N));
    log.clear();

    let (mut a, scene_a, index_a) = seeded();
    let sources = [index_a.health_bars[0], index_a.health_bars[1]];
    let copied = capscene::copy_selection(&a, &scene_a, &sources, false);
    let made = capscene::paste_clipboard(&mut a, &scene_a, &copied.clipboard, 0).unwrap();
    log.append(&Record::Paste {
        clipboard: copied.clipboard.clone(),
        step: 0,
    });
    let pasted = made.roots.clone();
    assert_eq!(pasted.len(), 2);
    drop(a); // close

    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut b = Engine::new(world, 1);
    capscene::seed(&mut b, &scene, N).expect("re-seed");
    b.clear_history();
    let (applied, skipped) = b_replay(&log, &mut b, &scene);
    b.clear_history();

    assert_eq!((applied, skipped), (1, 0), "the paste replayed");
    for id in &pasted {
        assert!(
            b.entity_exists(*id),
            "a pasted object was gone after a reopen - the paste was never recorded"
        );
    }

    // ...and one live Ctrl-Z after the paste takes the whole paste back on replay too, which is the
    // reason a paste is one record rather than N.
    let (mut c, scene_c, index_c) = seeded();
    let held = capscene::copy_selection(&c, &scene_c, &[index_c.health_bars[0]], false).clipboard;
    let undone = capscene::paste_clipboard(&mut c, &scene_c, &held, 0).unwrap();
    log.clear();
    log.append(&Record::Paste {
        clipboard: held,
        step: 0,
    });
    assert!(c.undo());
    log.append(&Record::Undo);
    drop(c);

    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut d = Engine::new(world, 1);
    capscene::seed(&mut d, &scene, N).expect("re-seed");
    d.clear_history();
    let (applied, skipped) = b_replay(&log, &mut d, &scene);
    assert_eq!((applied, skipped), (2, 0));
    for id in undone.created {
        assert!(
            !d.entity_exists(id),
            "an undone paste came back after a reopen"
        );
    }
}
