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
