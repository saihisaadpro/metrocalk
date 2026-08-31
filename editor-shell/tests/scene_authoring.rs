//! M10.6 (ADR-036) — the scene-authoring verbs, headless. Proves the gradeable core of "build a scene"
//! without a live GPU: each verb is ONE undoable pipeline transaction riding the **Loro Movable Tree**
//! (reparent = `node.move`, cycle-safe) + the **override model** (delete = deactivate-not-destroy, M9.2),
//! extending M3.3's surface (Remove/Duplicate). The adversarial guards are asserted head-on:
//! - a drag-reparent moves the tree edge + undo reverts it; a cycle-reparent is **rejected**;
//! - multi-edit on N entities is **ONE** undoable tx (one undo restores all N, never N un-grouped ops);
//! - copy→paste round-trips a sub-tree under **new ids** (no id-alias); cross-project paste works;
//! - delete = **deactivate** (undo restores; concurrent edits never lost to a destructive delete) and
//!   **frees dependents** (the M3.3 rule);
//! - every verb **survives reload** (the verbs are commits → the Loro doc → the M10.3 `.mtk` save/open).

// Test numeric conversions (index → coord, Integer field → f64) are deliberate, bounded.
#![allow(clippy::cast_precision_loss)]

use std::path::PathBuf;

use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::FlecsWorld;

use metrocalk_editor_shell::capscene::{self, CapResolver, CapScene};
use metrocalk_editor_shell::project;

/// A fresh engine + its interned cap vocabulary + the resolver (so caps mirror into the durable doc and a
/// reload re-derives them — the same setup as the project/cap-rebuild tests).
fn engine_with_resolver() -> (Engine<FlecsWorld>, CapScene) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    engine.set_capability_resolver(Box::new(CapResolver::from_scene(&scene)));
    (engine, scene)
}

/// A flat entity at `x` with a Transform — the unit of a hand-built scene.
fn spawn_at(engine: &mut Engine<FlecsWorld>, x: f32) -> EntityId {
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
                    value: FieldValue::Number(f64::from(x)),
                },
            ],
        )
        .expect("spawn commits");
    id
}

fn x_of(engine: &Engine<FlecsWorld>, id: EntityId) -> f64 {
    engine
        .components_of(id)
        .get("Transform")
        .and_then(|t| t.get("x"))
        .and_then(|v| match v {
            FieldValue::Number(n) => Some(*n),
            FieldValue::Integer(i) => Some(*i as f64),
            _ => None,
        })
        .unwrap_or(f64::NAN)
}

// ── CREATE ───────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn create_entity_is_one_undoable_tx() {
    let (mut e, _scene) = engine_with_resolver();
    let id = capscene::create_entity(&mut e, [1.0, 2.0, 3.0], "My Thing").expect("create");
    assert!(e.ecs_entity(id).is_some(), "the created entity is live");
    assert_eq!(capscene::entity_name(&e, id).as_deref(), Some("My Thing"));
    assert!(
        (x_of(&e, id) - 1.0).abs() < 1e-9,
        "placed at the requested x"
    );
    assert!(e.undo(), "create is one undoable tx");
    assert!(e.ecs_entity(id).is_none(), "undo removed it entirely");
}

#[test]
fn create_primitive_tags_its_kind() {
    let (mut e, scene) = engine_with_resolver();
    let id =
        capscene::create_primitive(&mut e, &scene, "cube", [0.0, 0.0, 0.0]).expect("create cube");
    assert!(e.ecs_entity(id).is_some());
    let prim = e
        .components_of(id)
        .get("__meta__")
        .and_then(|m| m.get("primitive"))
        .cloned();
    assert_eq!(
        prim,
        Some(FieldValue::Str("cube".into())),
        "tagged as a cube primitive (the renderer draws the kind)"
    );
    assert!(e.undo());
    assert!(e.ecs_entity(id).is_none());
}

// ── RENAME ───────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn rename_is_one_undoable_tx_and_undo_restores_the_prior_name() {
    let (mut e, _scene) = engine_with_resolver();
    let id = capscene::create_entity(&mut e, [0.0; 3], "first").expect("create");
    capscene::rename(&mut e, id, "second").expect("rename");
    assert_eq!(capscene::entity_name(&e, id).as_deref(), Some("second"));
    assert!(e.undo(), "rename is one undoable tx");
    assert_eq!(
        capscene::entity_name(&e, id).as_deref(),
        Some("first"),
        "undo restored the prior name"
    );
    // Renaming an unknown entity is an explained error, not a panic.
    let ghost = e.alloc_entity_id();
    assert!(capscene::rename(&mut e, ghost, "x").is_err());
}

// ── REPARENT (Movable Tree, cycle-safe) ───────────────────────────────────────────────────────────────

#[test]
fn reparent_moves_the_tree_edge_and_undo_reverts() {
    let (mut e, _scene) = engine_with_resolver();
    let a = spawn_at(&mut e, 0.0);
    let b = spawn_at(&mut e, 5.0);
    assert_eq!(e.parent_of(b), None, "b starts at the root");

    capscene::reparent_entity(&mut e, b, Some(a)).expect("reparent b under a");
    assert_eq!(
        e.parent_of(b),
        Some(a),
        "b is now a child of a (the node.move edge)"
    );
    assert!(e.children_of(a).contains(&b));

    assert!(e.undo(), "reparent is one undoable tx");
    assert_eq!(e.parent_of(b), None, "undo restored b to the root");
}

#[test]
fn a_reparent_that_would_create_a_cycle_is_rejected() {
    let (mut e, _scene) = engine_with_resolver();
    let parent = spawn_at(&mut e, 0.0);
    let child = spawn_at(&mut e, 1.0);
    capscene::reparent_entity(&mut e, child, Some(parent)).expect("child under parent");

    // Moving the parent UNDER its own child would orphan a cycle — REJECTED (and Loro's MovableTree
    // rejects it too). The tree is unchanged.
    let err = capscene::reparent_entity(&mut e, parent, Some(child));
    assert!(
        err.is_err(),
        "a cycle-creating reparent is rejected: {err:?}"
    );
    assert_eq!(e.parent_of(parent), None, "the parent stayed at the root");
    assert_eq!(
        e.parent_of(child),
        Some(parent),
        "the child kept its parent"
    );
    // Self-parenting is a cycle too.
    assert!(capscene::reparent_entity(&mut e, parent, Some(parent)).is_err());
}

// ── GROUP / UNGROUP ──────────────────────────────────────────────────────────────────────────────────

#[test]
fn group_wraps_a_selection_preserving_world_transforms_and_undo_dissolves_it() {
    let (mut e, _scene) = engine_with_resolver();
    let a = spawn_at(&mut e, 2.0);
    let b = spawn_at(&mut e, -3.0);
    let wa = capscene::global_transform(&e, a).translation;
    let wb = capscene::global_transform(&e, b).translation;

    let g = capscene::group(&mut e, &[a, b], "Group").expect("group");
    assert_eq!(e.parent_of(a), Some(g), "a is under the group");
    assert_eq!(e.parent_of(b), Some(g), "b is under the group");
    // World transforms preserved (the group is an identity node).
    let wa2 = capscene::global_transform(&e, a).translation;
    let wb2 = capscene::global_transform(&e, b).translation;
    assert!(
        (wa2[0] - wa[0]).abs() < 1e-5 && (wb2[0] - wb[0]).abs() < 1e-5,
        "world preserved"
    );

    // One undo dissolves the group + restores both prior parents atomically.
    assert!(e.undo(), "group is one undoable tx");
    assert_eq!(e.parent_of(a), None);
    assert_eq!(e.parent_of(b), None);
    assert!(
        e.ecs_entity(g).is_none(),
        "the group node is gone after undo"
    );
}

#[test]
fn ungroup_dissolves_a_group_and_frees_its_children() {
    let (mut e, _scene) = engine_with_resolver();
    let a = spawn_at(&mut e, 1.0);
    let b = spawn_at(&mut e, 2.0);
    let g = capscene::group(&mut e, &[a, b], "G").expect("group");

    let freed = capscene::ungroup(&mut e, g).expect("ungroup");
    assert_eq!(freed.len(), 2, "both children freed");
    assert_eq!(
        e.parent_of(a),
        None,
        "a is back at the root (the group's parent)"
    );
    assert_eq!(e.parent_of(b), None);
    assert!(e.ecs_entity(g).is_none(), "the empty group node is gone");

    assert!(e.undo(), "ungroup is one undoable tx");
    assert_eq!(e.parent_of(a), Some(g), "undo restored the grouping");
    assert_eq!(e.parent_of(b), Some(g));
}

// ── MULTI-SELECT + MULTI-EDIT (batched atomic tx) ──────────────────────────────────────────────────────

#[test]
fn multi_edit_is_one_batched_undoable_tx_for_all_n() {
    let (mut e, _scene) = engine_with_resolver();
    let ids: Vec<EntityId> = (0..3).map(|i| spawn_at(&mut e, i as f32)).collect();

    capscene::multi_edit(&mut e, &ids, "Transform", "y", &FieldValue::Number(9.0))
        .expect("multi-edit");
    for &id in &ids {
        let y = e
            .components_of(id)
            .get("Transform")
            .and_then(|t| t.get("y"))
            .cloned();
        assert_eq!(
            y,
            Some(FieldValue::Number(9.0)),
            "every selected entity got the edit"
        );
    }

    // ONE undo restores ALL N at once (the adversarial trap: N un-grouped ops would need N undos).
    assert!(e.undo(), "multi-edit is ONE undoable tx");
    for &id in &ids {
        let y = e
            .components_of(id)
            .get("Transform")
            .and_then(|t| t.get("y"))
            .cloned();
        assert_eq!(
            y, None,
            "the single undo reverted ALL N entities, not just one"
        );
    }
}

#[test]
fn multi_edit_is_all_or_nothing_when_an_id_is_unknown() {
    let (mut e, _scene) = engine_with_resolver();
    let real = spawn_at(&mut e, 0.0);
    let ghost = e.alloc_entity_id();
    let r = capscene::multi_edit(
        &mut e,
        &[real, ghost],
        "Transform",
        "y",
        &FieldValue::Number(1.0),
    );
    assert!(
        r.is_err(),
        "an unknown id fails the whole batch (no half-edit)"
    );
    let y = e
        .components_of(real)
        .get("Transform")
        .and_then(|t| t.get("y"))
        .cloned();
    assert_eq!(y, None, "the real entity was NOT edited (atomic)");
}

// ── ADR-169 — THE SELECTION IS THE UNIT OF WORK ────────────────────────────────────────────────────────
// The editor could always SELECT many and could only CHANGE one. These are the engine-side claims the
// Inspector's multi-selection form rests on: any scalar, a refusal that can be read out loud, and one
// transaction — so one Ctrl-Z answers one user action, however many objects it touched.

#[test]
fn multi_edit_carries_any_scalar_not_only_a_number() {
    // `multi_edit` was reachable only through a Tauri command declared `value: f64`, so the only
    // property a selection could be edited through was a number. Nothing about the engine required
    // that: `FieldValue` has been the parameter here since M10.6.
    let (mut e, _scene) = engine_with_resolver();
    let ids: Vec<EntityId> = (0..2).map(|i| spawn_at(&mut e, i as f32)).collect();

    capscene::multi_edit(
        &mut e,
        &ids,
        "Transform",
        "label",
        &FieldValue::Str("bay".into()),
    )
    .expect("a string is a value");
    capscene::multi_edit(&mut e, &ids, "Transform", "frozen", &FieldValue::Bool(true))
        .expect("a boolean is a value");
    for &id in &ids {
        let c = e.components_of(id);
        let t = c.get("Transform").expect("Transform");
        assert_eq!(t.get("label"), Some(&FieldValue::Str("bay".into())));
        assert_eq!(t.get("frozen"), Some(&FieldValue::Bool(true)));
    }
}

#[test]
fn multi_edit_refuses_a_component_only_part_of_the_selection_carries_and_says_how_many() {
    // THE REASON THIS GUARD EXISTS AT ALL: `Op::SetField` validates only that the entity is alive —
    // component and field are CREATED on write — so this batch would otherwise SUCCEED and leave the
    // bare entity carrying a one-field `Light` it never had, inside an undoable transaction that
    // reported itself as a success.
    let (mut e, _scene) = engine_with_resolver();
    let lit = spawn_at(&mut e, 0.0);
    let bare = e.alloc_entity_id();
    e.commit(
        "bare",
        vec![Op::CreateEntity {
            id: bare,
            parent: None,
        }],
    )
    .unwrap();
    e.commit(
        "light",
        vec![Op::SetField {
            entity: lit,
            component: "Light".into(),
            field: "intensity".into(),
            value: FieldValue::Number(60.0),
        }],
    )
    .unwrap();

    let err = capscene::multi_edit(
        &mut e,
        &[lit, bare],
        "Light",
        "intensity",
        &FieldValue::Number(12.0),
    )
    .expect_err("a selection that disagrees about the component is refused");
    let said = err.to_string();
    assert!(
        said.contains("1 of 2") && said.contains("Light"),
        "the refusal names how many and which component, not just 'no': {said}"
    );
    assert!(
        !e.components_of(bare).contains_key("Light"),
        "the bare entity did NOT silently gain a Light component"
    );
    assert_eq!(
        e.components_of(lit)
            .get("Light")
            .and_then(|l| l.get("intensity")),
        Some(&FieldValue::Number(60.0)),
        "and the lit one was not half-edited either (all-or-nothing)"
    );
}

#[test]
fn multi_edit_still_creates_a_component_none_of_them_carry() {
    // The refusal is about the selection DISAGREEING, not about creation: seeding a component across
    // a whole selection is the AI/rule-authoring path's normal use, and the `.exe` play-rules gate
    // does exactly this with one id. A guard that also blocked it would break a shipped workflow to
    // fix a different one.
    let (mut e, _scene) = engine_with_resolver();
    let ids: Vec<EntityId> = (0..3).map(|i| spawn_at(&mut e, i as f32)).collect();
    capscene::multi_edit(
        &mut e,
        &ids,
        "KillCounter",
        "count",
        &FieldValue::Integer(0),
    )
    .expect("uniformly creating a component is allowed");
    for &id in &ids {
        assert_eq!(
            e.components_of(id)
                .get("KillCounter")
                .and_then(|k| k.get("count")),
            Some(&FieldValue::Integer(0))
        );
    }
}

#[test]
fn deleting_a_selection_is_one_undoable_transaction() {
    let (mut e, scene) = engine_with_resolver();
    let ids: Vec<EntityId> = (0..3).map(|i| spawn_at(&mut e, i as f32)).collect();

    capscene::delete_deactivate_many(&mut e, &scene, &ids).expect("delete the selection");
    for &id in &ids {
        assert!(!e.is_active(id), "every selected object is deactivated");
    }

    // The whole point: the toast says "Ctrl-Z to undo", singular, and it has to be true.
    assert!(e.undo(), "one undo");
    for &id in &ids {
        assert!(
            e.is_active(id),
            "ONE undo restored ALL of them, not the last one"
        );
    }
}

#[test]
fn deleting_two_objects_bound_to_each_other_does_not_emit_the_binding_twice() {
    // The selection a user is most likely to make is a thing and the thing it is plugged into. A
    // per-target loop over the binding list emits that binding's `RemoveBinding` once per endpoint.
    let (mut e, scene) = engine_with_resolver();
    let bar = e.alloc_entity_id();
    e.commit(
        "requirer",
        vec![
            Op::CreateEntity {
                id: bar,
                parent: None,
            },
            Op::AddPair {
                entity: bar,
                rel: scene.rels.requires,
                target: scene.cap("Health"),
            },
        ],
    )
    .unwrap();
    let provider = e.alloc_entity_id();
    e.commit(
        "provider",
        vec![
            Op::CreateEntity {
                id: provider,
                parent: None,
            },
            Op::AddPair {
                entity: provider,
                rel: scene.rels.provides,
                target: scene.cap("Health"),
            },
        ],
    )
    .unwrap();
    capscene::bind(&mut e, &scene, bar, provider).expect("bind");
    assert_eq!(e.bindings().len(), 1);

    capscene::delete_deactivate_many(&mut e, &scene, &[bar, provider])
        .expect("both ends of one binding delete together");
    assert!(!e.is_active(bar) && !e.is_active(provider));
    assert_eq!(e.bindings().len(), 0, "the shared binding is freed once");

    assert!(e.undo(), "one undo");
    assert!(e.is_active(bar) && e.is_active(provider));
    assert_eq!(e.bindings().len(), 1, "and the binding came back with them");
}

#[test]
fn duplicating_a_selection_is_one_undoable_transaction() {
    let (mut e, scene) = engine_with_resolver();
    let ids: Vec<EntityId> = (0..3).map(|i| spawn_at(&mut e, i as f32)).collect();
    let before = e.entity_ids().len();

    let clones =
        capscene::duplicate_entities(&mut e, &scene, &ids).expect("duplicate the selection");
    assert_eq!(clones.len(), 3, "one clone per source, in source order");
    assert_eq!(e.entity_ids().len(), before + 3);
    for (&src, &clone) in ids.iter().zip(clones.iter()) {
        assert_ne!(src, clone, "a clone is a NEW id, never an alias");
        assert!(e.components_of(clone).contains_key("Transform"));
    }

    assert!(e.undo(), "one undo");
    assert_eq!(
        e.entity_ids().len(),
        before,
        "ONE undo removed ALL three clones"
    );
}

#[test]
fn duplicating_a_selection_containing_a_dead_id_clones_none_of_it() {
    let (mut e, scene) = engine_with_resolver();
    let real = spawn_at(&mut e, 0.0);
    let ghost = e.alloc_entity_id();
    let before = e.entity_ids().len();

    let r = capscene::duplicate_entities(&mut e, &scene, &[real, ghost]);
    assert!(r.is_err(), "an unknown id fails the whole batch");
    assert_eq!(
        e.entity_ids().len(),
        before,
        "not even the good one was cloned (all-or-nothing)"
    );
}

// ── COPY / CUT / PASTE / DUPLICATE ─────────────────────────────────────────────────────────────────────

#[test]
fn copy_paste_round_trips_a_subtree_with_new_ids() {
    let (mut e, _scene) = engine_with_resolver();
    // A 2-node subtree: a named parent with a child.
    let parent = capscene::create_entity(&mut e, [4.0, 0.0, 0.0], "Root").expect("parent");
    let child = spawn_at(&mut e, 1.0);
    capscene::reparent_entity(&mut e, child, Some(parent)).expect("child under parent");

    let clip = capscene::copy_subtree(&e, parent, "clip");
    let new_root = capscene::paste_composition(&mut e, &clip).expect("paste");

    assert_ne!(new_root, parent, "paste allocated a FRESH id (no alias)");
    assert!(
        e.ecs_entity(parent).is_some(),
        "copy is non-destructive — the original survives"
    );
    assert!(e.ecs_entity(new_root).is_some(), "the pasted root is live");
    // The sub-tree came along under new ids.
    let new_children = e.children_of(new_root);
    assert_eq!(new_children.len(), 1, "the child sub-node was pasted too");
    assert!(
        !new_children.contains(&child),
        "the pasted child is a fresh id, not the original"
    );
    // Resolved state matches (the parent's x).
    assert!(
        (x_of(&e, new_root) - x_of(&e, parent)).abs() < 1e-9,
        "geometry round-trips"
    );

    // Paste is one undoable tx.
    assert!(e.undo(), "paste is one undoable tx");
    assert!(
        e.ecs_entity(new_root).is_none(),
        "undo removed the whole pasted sub-tree"
    );
}

#[test]
fn copy_paste_works_across_projects_via_the_serde_clipboard() {
    let (mut a, _sa) = engine_with_resolver();
    let root = capscene::create_entity(&mut a, [7.0, 0.0, 0.0], "Crossing").expect("create");
    let clip = capscene::copy_subtree(&a, root, "clip");

    // The clipboard crosses as bytes (a different project / process) — never a stale id.
    let json = serde_json::to_string(&clip).expect("Composition serializes");
    let clip_b: metrocalk_core::Composition = serde_json::from_str(&json).expect("deserializes");

    let (mut b, _sb) = engine_with_resolver();
    let pasted = capscene::paste_composition(&mut b, &clip_b).expect("paste into project B");
    assert!(
        b.ecs_entity(pasted).is_some(),
        "the sub-tree pasted into a fresh project"
    );
    assert!(
        (x_of(&b, pasted) - 7.0).abs() < 1e-9,
        "its geometry survived the crossing"
    );
}

#[test]
fn cut_copies_then_non_destructively_deletes_the_source() {
    let (mut e, scene) = engine_with_resolver();
    let root = capscene::create_entity(&mut e, [0.0; 3], "Cut me").expect("create");
    let clip = capscene::cut_subtree(&mut e, &scene, root, "clip").expect("cut");
    assert!(
        !clip.nodes.is_empty(),
        "the clipboard holds the cut sub-tree"
    );
    assert!(
        !e.is_active(root),
        "cut deactivated the source (non-destructive — undo restores)"
    );
    assert!(
        e.ecs_entity(root).is_some(),
        "the source still EXISTS (deactivate, not destroy)"
    );
    // Paste the cut clipboard elsewhere.
    let pasted = capscene::paste_composition(&mut e, &clip).expect("paste the cut");
    assert!(e.ecs_entity(pasted).is_some());
}

// ── DELETE = DEACTIVATE (undo restores + frees dependents) ─────────────────────────────────────────────

#[test]
fn delete_is_deactivate_not_destroy_and_undo_restores() {
    let (mut e, scene) = engine_with_resolver();
    let id = capscene::create_entity(&mut e, [0.0; 3], "Deletable").expect("create");
    capscene::delete_deactivate(&mut e, &scene, id).expect("delete");
    assert!(!e.is_active(id), "deleted = deactivated");
    assert!(
        e.ecs_entity(id).is_some(),
        "the entity + its data SURVIVE (recoverable, merge-safe)"
    );
    assert!(e.undo(), "delete is one undoable tx");
    assert!(e.is_active(id), "undo re-activated it");
}

#[test]
fn deleting_a_provider_frees_its_dependents() {
    let (mut e, scene) = engine_with_resolver();
    // A requirer (bar) tracking a provider (health).
    let bar = e.alloc_entity_id();
    e.commit(
        "bar",
        vec![
            Op::CreateEntity {
                id: bar,
                parent: None,
            },
            Op::AddPair {
                entity: bar,
                rel: scene.rels.requires,
                target: scene.cap("Health"),
            },
        ],
    )
    .unwrap();
    let provider = e.alloc_entity_id();
    e.commit(
        "provider",
        vec![
            Op::CreateEntity {
                id: provider,
                parent: None,
            },
            Op::AddPair {
                entity: provider,
                rel: scene.rels.provides,
                target: scene.cap("Health"),
            },
        ],
    )
    .unwrap();
    capscene::bind(&mut e, &scene, bar, provider).expect("bind");
    assert_eq!(e.bindings().len(), 1, "bar tracks provider");

    capscene::delete_deactivate(&mut e, &scene, provider).expect("delete provider");
    assert!(!e.is_active(provider), "provider deactivated");
    assert_eq!(
        e.bindings().len(),
        0,
        "the binding is freed — bar's requirement re-opens (M3.3 rule)"
    );

    assert!(e.undo(), "one undo");
    assert!(e.is_active(provider), "provider re-activated");
    assert_eq!(e.bindings().len(), 1, "the binding restored atomically");
}

// ── SURVIVES RELOAD (the verbs are commits → the Loro doc → M10.3 save/open) ───────────────────────────

#[test]
fn the_authoring_verbs_survive_an_mtk_save_and_reopen() {
    let dir = std::env::temp_dir();
    let path: PathBuf = dir.join("mtk_scene_authoring_reload.mtk");
    let _ = std::fs::remove_file(&path);

    // Build a small scene with the verbs in project A.
    let (mut a, _sa) = engine_with_resolver();
    let parent = capscene::create_entity(&mut a, [1.0, 0.0, 0.0], "Parent").expect("create");
    let child = spawn_at(&mut a, 2.0);
    capscene::reparent_entity(&mut a, child, Some(parent)).expect("reparent");
    capscene::rename(&mut a, child, "Renamed Child").expect("rename");
    let g = capscene::group(&mut a, &[parent], "Grp").expect("group");

    project::save(&a, &path).expect("save the .mtk");

    // Reopen into a FRESH engine B.
    let (mut b, _sb) = engine_with_resolver();
    project::open_into(&mut b, &path).expect("reopen");

    // The hierarchy + the name survived (the verbs were commits → the durable doc).
    assert_eq!(
        b.parent_of(child),
        Some(parent),
        "the reparent survived reload"
    );
    assert_eq!(b.parent_of(parent), Some(g), "the grouping survived reload");
    assert_eq!(
        capscene::entity_name(&b, child).as_deref(),
        Some("Renamed Child"),
        "the rename survived reload"
    );
    let _ = std::fs::remove_file(&path);
}

// ── SET A ROTATION (ADR-172) — four stored numbers, one property, one transaction ──────────────────────

/// Read a Transform field as an `f64` (the four quaternion components are all `Number`s on the wire).
fn tf(engine: &Engine<FlecsWorld>, id: EntityId, field: &str) -> Option<f64> {
    match engine
        .components_of(id)
        .get("Transform")
        .and_then(|t| t.get(field))
    {
        Some(FieldValue::Number(n)) => Some(*n),
        _ => None,
    }
}

#[test]
fn set_rotation_writes_four_fields_across_n_entities_as_one_undoable_tx() {
    let (mut e, _scene) = engine_with_resolver();
    let ids: Vec<EntityId> = (0..3).map(|i| spawn_at(&mut e, i as f32)).collect();

    // 90° about Y.
    let s = std::f64::consts::FRAC_1_SQRT_2;
    capscene::set_rotation(&mut e, &ids, [0.0, s, 0.0, s]).expect("rotate the selection");
    for &id in &ids {
        assert!((tf(&e, id, "qy").unwrap() - s).abs() < 1e-9);
        assert!((tf(&e, id, "qw").unwrap() - s).abs() < 1e-9);
    }

    // ONE undo takes all twelve field writes back, not the last one.
    assert!(e.undo(), "the rotation is ONE undoable transaction");
    for &id in &ids {
        assert_eq!(tf(&e, id, "qy"), None, "one undo reverted every entity");
        assert_eq!(tf(&e, id, "qw"), None);
    }
}

#[test]
fn set_rotation_normalises_so_the_document_can_never_hold_a_non_rotation() {
    // THE DEFECT THIS CLOSES: the Inspector rendered qx/qy/qz/qw as four independent number boxes, so
    // typing 5 into one of them committed a quaternion of length 5 — which is not a rotation, and
    // which every reader downstream (`local_transform` → the renderer, picking, framing, export)
    // would nevertheless use.
    let (mut e, _scene) = engine_with_resolver();
    let id = spawn_at(&mut e, 0.0);

    capscene::set_rotation(&mut e, &[id], [0.0, 5.0, 0.0, 5.0])
        .expect("a scaled quaternion is a rotation, once normalised");
    let length = ["qx", "qy", "qz", "qw"]
        .iter()
        .map(|f| tf(&e, id, f).unwrap().powi(2))
        .sum::<f64>()
        .sqrt();
    assert!(
        (length - 1.0).abs() < 1e-9,
        "the stored quaternion is a UNIT quaternion, whatever length the caller passed: {length}"
    );
}

#[test]
fn set_rotation_refuses_four_numbers_that_are_not_a_rotation() {
    let (mut e, _scene) = engine_with_resolver();
    let id = spawn_at(&mut e, 0.0);

    for bad in [[0.0, 0.0, 0.0, 0.0], [f64::NAN, 0.0, 0.0, 1.0]] {
        let err = capscene::set_rotation(&mut e, &[id], bad).expect_err("refused");
        assert!(
            err.to_string().contains("not a rotation"),
            "the refusal says what is wrong, in words a panel can print: {err}"
        );
    }
    assert_eq!(tf(&e, id, "qw"), None, "and nothing was written");
}

#[test]
fn set_rotation_refuses_an_object_with_no_place_in_the_world_and_says_how_many() {
    // Stricter than `multi_edit`'s disagreement rule, on purpose: a `Transform` carrying a rotation
    // and no position is not a pose — `local_transform` reads it as "rotated, at the world origin".
    let (mut e, _scene) = engine_with_resolver();
    let placed = spawn_at(&mut e, 0.0);
    let bare = e.alloc_entity_id();
    e.commit(
        "bare",
        vec![Op::CreateEntity {
            id: bare,
            parent: None,
        }],
    )
    .unwrap();

    let err = capscene::set_rotation(&mut e, &[placed, bare], [0.0, 0.0, 0.0, 1.0])
        .expect_err("a selection with a non-spatial object is refused");
    let said = err.to_string();
    assert!(
        said.contains("1 of 2"),
        "the refusal names how many, not just 'no': {said}"
    );
    assert!(
        !e.components_of(bare).contains_key("Transform"),
        "the bare entity did NOT silently gain a Transform"
    );
    assert_eq!(
        tf(&e, placed, "qw"),
        None,
        "and the placed one was not half-rotated"
    );
}
