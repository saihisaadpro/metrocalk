//! The override / variant model (M9.2 / ADR-026) — headless spine.
//!
//! Covers every success-criterion + every adversarial trap from the prompt:
//! - a part edit writes ONLY the changed `(component, field)` key (sparsity — not a whole-object copy);
//! - base ⊕ override resolves **override-wins by structure** (a stale base-restore can't beat it);
//! - per-field ops never clobber a sibling field;
//! - `save_composition` → `instantiate_composition` yields a reusable, pre-componentized subtree that
//!   keeps the source link; a named `Variant` re-applies its overrides to a fresh instance;
//! - deactivate-not-delete round-trips through undo;
//! - **two peers editing different fields of the same part converge with no loss**, and
//! - **concurrent first-creation of the same override slot loses neither edit**, surviving the
//!   engine-side undo→redo stack — the F1 re-verification (the data-loss-trap test).

use metrocalk_core::{Composition, Engine, EntityId, FieldValue, Op, Variant, VariantOp};
use metrocalk_ecs::FlecsWorld;

fn engine(peer: u64) -> Engine<FlecsWorld> {
    Engine::new(FlecsWorld::new(), peer)
}

/// Build a tiny "character": a body root with two rigid child parts (a pauldron, a wheel), each
/// carrying a base `Transform`. Returns `(root, [part0, part1])`. One undoable transaction.
fn compose_character(e: &mut Engine<FlecsWorld>) -> (EntityId, [EntityId; 2]) {
    let root = e.alloc_entity_id();
    let p0 = e.alloc_entity_id();
    let p1 = e.alloc_entity_id();
    let set = |entity, field: &str, v: f64| Op::SetField {
        entity,
        component: "Transform".into(),
        field: field.into(),
        value: FieldValue::Number(v),
    };
    e.commit(
        "compose-character",
        vec![
            Op::CreateEntity {
                id: root,
                parent: None,
            },
            set(root, "x", 0.0),
            Op::CreateEntity {
                id: p0,
                parent: Some(root),
            },
            set(p0, "x", 1.0),
            set(p0, "y", 0.0),
            Op::CreateEntity {
                id: p1,
                parent: Some(root),
            },
            set(p1, "x", -1.0),
        ],
    )
    .unwrap();
    (root, [p0, p1])
}

// Takes the owned `Option<FieldValue>` the engine reads return, by value — convenient at the many
// call sites; this is a test helper, not an API.
#[allow(clippy::needless_pass_by_value)]
fn num(v: Option<FieldValue>) -> Option<f64> {
    match v {
        Some(FieldValue::Number(n)) => Some(n),
        _ => None,
    }
}

// ── sparsity: a part edit writes ONLY the changed key ────────────────────────

#[test]
fn override_writes_only_the_changed_key_not_a_whole_object_copy() {
    let mut e = engine(1);
    let (_root, [part, _]) = compose_character(&mut e);

    // Edit one field of the part (the gizmo-drag result, as a sparse override).
    e.commit(
        "edit-part",
        vec![Op::SetOverride {
            entity: part,
            component: "Transform".into(),
            field: "x".into(),
            value: FieldValue::Number(9.0),
        }],
    )
    .unwrap();

    // The override layer holds EXACTLY one entry — the changed key — not a copy of the base record.
    let ov = e.overrides_of(part);
    assert_eq!(
        ov.len(),
        1,
        "exactly one sparse override key written, got {ov:?}"
    );
    assert_eq!(num(ov.get("Transform\u{1f}x").cloned()), Some(9.0));

    // The base component record is untouched (still the original x=1, y=0 — the source layer).
    let base = e.components_of(part);
    assert_eq!(
        num(base["Transform"].get("x").cloned()),
        Some(1.0),
        "base x untouched"
    );
    assert_eq!(
        num(base["Transform"].get("y").cloned()),
        Some(0.0),
        "base y untouched"
    );
}

// ── override-as-stronger-read-layer (the LIVRPS caveat) ──────────────────────

#[test]
fn override_wins_over_base_by_structure_not_timestamp() {
    let mut e = engine(1);
    let (_root, [part, _]) = compose_character(&mut e);

    e.commit(
        "override-x",
        vec![Op::SetOverride {
            entity: part,
            component: "Transform".into(),
            field: "x".into(),
            value: FieldValue::Number(5.0),
        }],
    )
    .unwrap();
    assert_eq!(
        num(e.resolved_components(part)["Transform"].get("x").cloned()),
        Some(5.0),
        "override wins over base in the resolved view"
    );

    // ADVERSARIAL: a LATER write to the BASE layer (a stale base-restore / asset-default re-push)
    // must NOT beat the override. The override is a structurally-stronger read layer, not a
    // timestamp-resolved entry in one map.
    e.commit(
        "stale-base-restore",
        vec![Op::SetField {
            entity: part,
            component: "Transform".into(),
            field: "x".into(),
            value: FieldValue::Number(99.0),
        }],
    )
    .unwrap();
    assert_eq!(
        num(e.resolved_components(part)["Transform"].get("x").cloned()),
        Some(5.0),
        "override still wins after a newer base write — precedence is structural, not by timestamp"
    );
    // The base did change underneath (proves the base write actually landed).
    assert_eq!(num(e.get_field(part, "Transform", "x")), Some(99.0));
}

#[test]
fn per_field_overrides_never_clobber_a_sibling_field() {
    let mut e = engine(1);
    let (_root, [part, _]) = compose_character(&mut e);

    // "rotate the leg" then "scale the leg" — two distinct keys that must coexist.
    e.commit(
        "override-x",
        vec![Op::SetOverride {
            entity: part,
            component: "Transform".into(),
            field: "x".into(),
            value: FieldValue::Number(2.0),
        }],
    )
    .unwrap();
    e.commit(
        "override-scale",
        vec![Op::SetOverride {
            entity: part,
            component: "Transform".into(),
            field: "scale".into(),
            value: FieldValue::Number(3.0),
        }],
    )
    .unwrap();

    assert_eq!(num(e.get_override(part, "Transform", "x")), Some(2.0));
    assert_eq!(
        num(e.get_override(part, "Transform", "scale")),
        Some(3.0),
        "the second override did not clobber the first (per-field, not whole-object)"
    );
}

// ── deactivate-not-delete round-trips through undo ───────────────────────────

#[test]
fn deactivate_not_delete_round_trips_through_undo() {
    let mut e = engine(1);
    let (_root, [part, _]) = compose_character(&mut e);
    assert!(e.is_active(part), "a fresh part is active");

    // "Remove" the part = deactivate it (USD deactivate ≡ reversible hide).
    e.commit(
        "deactivate-part",
        vec![Op::SetActive {
            entity: part,
            active: false,
        }],
    )
    .unwrap();
    assert!(!e.is_active(part), "deactivated");
    // The entity + its data are PRESERVED, not destroyed (the anti-data-loss point).
    assert!(
        e.entity_exists(part),
        "deactivate does not delete the entity"
    );
    assert_eq!(
        num(e.get_field(part, "Transform", "x")),
        Some(1.0),
        "data intact"
    );

    // Undo brings it back.
    assert!(e.undo());
    assert!(e.is_active(part), "undo restored the part to active");
}

// ── save & reuse: composition snapshot + fresh instance ──────────────────────

#[test]
fn save_composition_bakes_the_edit_into_a_reusable_instance() {
    let mut e = engine(1);
    let (root, [part, _]) = compose_character(&mut e);

    // Edit a part (override), then SAVE the edited character for reuse.
    e.commit(
        "edit-before-save",
        vec![Op::SetOverride {
            entity: part,
            component: "Transform".into(),
            field: "x".into(),
            value: FieldValue::Number(7.0),
        }],
    )
    .unwrap();
    let comp: Composition = e.save_composition(root, "char:hero-v1");
    assert_eq!(comp.nodes.len(), 3, "root + two parts captured");
    // The edited (resolved) state is baked into the saved asset's base.
    let saved_part = comp.nodes.iter().find(|n| n.path == "0").unwrap();
    assert_eq!(
        saved_part.components["Transform"].get("x"),
        Some(&FieldValue::Number(7.0)),
        "the edit is baked into the saved composition's resolved base"
    );

    // Drop a FRESH instance — independently-id'd, pre-componentized, source link preserved.
    let inst = e.instantiate_composition(&comp).unwrap();
    assert_ne!(inst, root, "the instance has its own id");
    assert_eq!(
        e.composition_of(inst),
        Some("char:hero-v1".into()),
        "source link kept"
    );
    let inst_part = e.entity_at_path(inst, "0").unwrap();
    assert_eq!(
        num(e.resolved_components(inst_part)["Transform"]
            .get("x")
            .cloned()),
        Some(7.0),
        "the fresh instance arrives with the saved edit present"
    );
}

#[test]
fn named_variant_reapplies_overrides_to_a_fresh_instance() {
    let mut e = engine(1);
    let (root, [part, _]) = compose_character(&mut e);

    // Edit instance A, capture the edit as a named variant.
    e.commit(
        "edit-A",
        vec![
            Op::SetOverride {
                entity: part,
                component: "Transform".into(),
                field: "x".into(),
                value: FieldValue::Number(42.0),
            },
            Op::SetActive {
                entity: part,
                active: false,
            },
        ],
    )
    .unwrap();
    let variant: Variant = e.capture_variant(root, "pose:salute");
    assert!(variant
        .ops
        .iter()
        .any(|o| matches!(o, VariantOp::Deactivate { .. })));
    assert!(variant
        .ops
        .iter()
        .any(|o| matches!(o, VariantOp::SetField { .. })));

    // A structurally-identical CLEAN instance (no overrides yet) to prove rel-path re-application.
    let (root_b, _) = compose_character(&mut e);

    // Apply the variant to instance B by structural rel-path.
    e.apply_variant(root_b, &variant).unwrap();
    let part_b = e.entity_at_path(root_b, "0").unwrap();
    assert_eq!(
        num(e.resolved_components(part_b)["Transform"].get("x").cloned()),
        Some(42.0),
        "the variant's override re-applied to a fresh instance (override-wins by structure)"
    );
    assert!(
        !e.is_active(part_b),
        "the variant's deactivation re-applied too"
    );
    // B's base is untouched — the variant rode the override layer, not a whole-object rewrite.
    assert_eq!(num(e.get_field(part_b, "Transform", "x")), Some(1.0));
}

// ── the data-loss-trap: two peers, same slot, concurrent first-creation ───────

/// Set up two peers sharing the SAME composition (B merges A's snapshot), so the part entity id is
/// identical on both — the precondition for a concurrent first-creation of its override slot.
fn two_peers_sharing_a_part() -> (Engine<FlecsWorld>, Engine<FlecsWorld>, EntityId) {
    let mut a = engine(1);
    let (_root, [part, _]) = compose_character(&mut a);
    let snapshot = a.fork_doc();
    let mut b = engine(2);
    b.merge(&snapshot).unwrap();
    assert!(b.entity_exists(part), "peer B shares the part after merge");
    (a, b, part)
}

#[test]
fn concurrent_different_field_overrides_on_the_same_part_both_survive() {
    let (mut a, mut b, part) = two_peers_sharing_a_part();

    // Concurrent FIRST-creation of the part's override slot on both peers (different fields).
    a.commit(
        "A-edits-x",
        vec![Op::SetOverride {
            entity: part,
            component: "Transform".into(),
            field: "x".into(),
            value: FieldValue::Number(1.0),
        }],
    )
    .unwrap();
    b.commit(
        "B-edits-y",
        vec![Op::SetOverride {
            entity: part,
            component: "Transform".into(),
            field: "y".into(),
            value: FieldValue::Number(2.0),
        }],
    )
    .unwrap();

    // Cross-merge.
    let ua = a.export_updates();
    let ub = b.export_updates();
    a.merge(&ub).unwrap();
    b.merge(&ua).unwrap();

    // NEITHER edit is lost — the mergeable override slot converged to one shared child.
    for (name, e) in [("A", &a), ("B", &b)] {
        assert_eq!(
            num(e.get_override(part, "Transform", "x")),
            Some(1.0),
            "{name}: x survived"
        );
        assert_eq!(
            num(e.get_override(part, "Transform", "y")),
            Some(2.0),
            "{name}: y survived"
        );
    }
}

#[test]
fn deactivate_does_not_lose_a_concurrent_editors_field_edit() {
    // ADVERSARIAL: "a 'delete part' hard-removes and breaks a concurrent editor." Deactivate is a
    // mergeable override entry, not a destructive tree delete, so a concurrent field edit survives.
    let (mut a, mut b, part) = two_peers_sharing_a_part();

    a.commit(
        "A-deactivates",
        vec![Op::SetActive {
            entity: part,
            active: false,
        }],
    )
    .unwrap();
    b.commit(
        "B-edits-x",
        vec![Op::SetOverride {
            entity: part,
            component: "Transform".into(),
            field: "x".into(),
            value: FieldValue::Number(8.0),
        }],
    )
    .unwrap();

    let ua = a.export_updates();
    let ub = b.export_updates();
    a.merge(&ub).unwrap();
    b.merge(&ua).unwrap();

    for (name, e) in [("A", &a), ("B", &b)] {
        assert!(!e.is_active(part), "{name}: deactivation converged");
        assert_eq!(
            num(e.get_override(part, "Transform", "x")),
            Some(8.0),
            "{name}: the concurrent editor's field edit was NOT lost to the deactivate"
        );
        assert!(
            e.entity_exists(part),
            "{name}: the part still exists (not destroyed)"
        );
    }
}

// ── F1 RE-VERIFICATION: mergeable slot survives the engine-side undo→redo stack ──

#[test]
fn f1_mergeable_override_slot_survives_engine_side_undo_redo_then_still_merges() {
    // ADR-002 F1 (M0 gate, 2026-06-13) found Loro's mergeable helper "did not survive undo/redo" on
    // the pre-rework container. This re-verifies on the reworked Mergeable Containers AGAINST OUR
    // ACTUAL undo model: the M1.6 engine-side inverse-op stack (operational undo as new forward
    // commits — never Loro `UndoManager`/checkout, ADR-002 F2). Verdict recorded in ADR-026.

    let mut a = engine(1);
    let (_root, [part, _]) = compose_character(&mut a);

    // Create the part's mergeable override slot (first creation), then undo, then redo.
    a.commit(
        "override",
        vec![Op::SetOverride {
            entity: part,
            component: "Transform".into(),
            field: "x".into(),
            value: FieldValue::Number(1.0),
        }],
    )
    .unwrap();
    assert_eq!(num(a.get_override(part, "Transform", "x")), Some(1.0));

    assert!(a.undo(), "undo the override");
    assert_eq!(
        a.get_override(part, "Transform", "x"),
        None,
        "override cleared by undo"
    );

    assert!(a.redo(), "redo the override");
    assert_eq!(
        num(a.get_override(part, "Transform", "x")),
        Some(1.0),
        "redo resurfaced the mergeable slot's value — F1 does NOT hold under the engine-side undo"
    );

    // And the mergeable IDENTITY survived undo→redo: with two peers sharing the part, A's slot goes
    // through an undo→redo cycle, then a concurrent peer's first-creation of the SAME slot still
    // converges (no silent loss) — proving the container id stays the deterministic mergeable one.
    let (mut a2, mut b2, part2) = two_peers_sharing_a_part();
    a2.commit(
        "A-x",
        vec![Op::SetOverride {
            entity: part2,
            component: "Transform".into(),
            field: "x".into(),
            value: FieldValue::Number(1.0),
        }],
    )
    .unwrap();
    assert!(a2.undo());
    assert!(a2.redo()); // A's slot went through undo→redo
    b2.commit(
        "B-y",
        vec![Op::SetOverride {
            entity: part2,
            component: "Transform".into(),
            field: "y".into(),
            value: FieldValue::Number(2.0),
        }],
    )
    .unwrap();
    let ua = a2.export_updates();
    let ub = b2.export_updates();
    a2.merge(&ub).unwrap();
    b2.merge(&ua).unwrap();
    assert_eq!(
        num(a2.get_override(part2, "Transform", "x")),
        Some(1.0),
        "A's redone x survived merge"
    );
    assert_eq!(
        num(a2.get_override(part2, "Transform", "y")),
        Some(2.0),
        "B's y merged into A's slot"
    );
    assert_eq!(num(b2.get_override(part2, "Transform", "x")), Some(1.0));
    assert_eq!(num(b2.get_override(part2, "Transform", "y")), Some(2.0));
}

// ── multiplayer-undo invariant (deliverable 5) ───────────────────────────────

#[test]
fn multiplayer_undo_reverts_only_my_edit_not_a_concurrent_peers() {
    // Figma's multiplayer-undo invariant, the implementable core: an undo reverses MY action as a new
    // forward commit (operational undo, ADR-002 F2) — it never rewinds shared history, so a concurrent
    // peer's edit to a DIFFERENT field of the same part survives my undo + the subsequent merge.
    let (mut a, mut b, part) = two_peers_sharing_a_part();

    a.commit(
        "A-edits-x",
        vec![Op::SetOverride {
            entity: part,
            component: "Transform".into(),
            field: "x".into(),
            value: FieldValue::Number(1.0),
        }],
    )
    .unwrap();
    b.commit(
        "B-edits-y",
        vec![Op::SetOverride {
            entity: part,
            component: "Transform".into(),
            field: "y".into(),
            value: FieldValue::Number(2.0),
        }],
    )
    .unwrap();

    // A undoes ITS OWN edit (operationally — a new inverse commit), before seeing B.
    assert!(a.undo(), "A undoes its own override");
    assert_eq!(
        a.get_override(part, "Transform", "x"),
        None,
        "A's x reverted"
    );

    // Now merge. A's net contribution is "x set then x removed"; B's is "y set".
    let ua = a.export_updates();
    let ub = b.export_updates();
    a.merge(&ub).unwrap();
    b.merge(&ua).unwrap();

    // The shared doc converges: A's undone edit is gone, B's concurrent edit SURVIVES on both peers —
    // A's undo touched only A's action, never corrupting B's.
    for (name, e) in [("A", &a), ("B", &b)] {
        assert_eq!(
            e.get_override(part, "Transform", "x"),
            None,
            "{name}: A's undone x stays gone"
        );
        assert_eq!(
            num(e.get_override(part, "Transform", "y")),
            Some(2.0),
            "{name}: B's concurrent edit survived A's undo (multiplayer-undo invariant)"
        );
    }
}

// ── marketplace-representable edited variant (deliverable 4; money seamed) ────

#[test]
fn an_edited_composition_is_a_serializable_pre_componentized_marketplace_asset() {
    // An edited variant is a first-class pre-componentized asset the marketplace index (ADR-015, behind
    // a trait) can carry: the Composition serializes losslessly + carries the mesh handle (a component
    // field) + the baked edit. NO economy code here — money/provider stays seamed (M5/M7 discipline);
    // this only proves the asset is transportable + re-instantiable.
    let mut e = engine(1);
    let (root, [part, _]) = compose_character(&mut e);
    // Give the part a renderable mesh handle (a normal component field) + edit it.
    e.commit(
        "mesh+edit",
        vec![
            Op::SetField {
                entity: part,
                component: "MeshRenderer".into(),
                field: "mesh".into(),
                value: FieldValue::Str("assets/pauldron.glb".into()),
            },
            Op::SetOverride {
                entity: part,
                component: "Transform".into(),
                field: "x".into(),
                value: FieldValue::Number(4.0),
            },
        ],
    )
    .unwrap();

    let comp: Composition = e.save_composition(root, "market:hero-edited-v1");

    // Serialize → deserialize losslessly (the marketplace index carries exactly this payload).
    let json = serde_json::to_string(&comp).unwrap();
    let back: Composition = serde_json::from_str(&json).unwrap();
    assert_eq!(
        back, comp,
        "the edited-variant asset round-trips through serde"
    );

    // It carries the mesh handle + the baked edit in its pre-componentized nodes.
    let saved_part = back.nodes.iter().find(|n| n.path == "0").unwrap();
    assert_eq!(
        saved_part.components["MeshRenderer"].get("mesh"),
        Some(&FieldValue::Str("assets/pauldron.glb".into())),
        "the mesh handle is carried in the asset"
    );
    assert_eq!(
        saved_part.components["Transform"].get("x"),
        Some(&FieldValue::Number(4.0)),
        "the baked edit is carried in the asset"
    );

    // And it re-instantiates into a working, independently-id'd entity tree (buy + edit < regenerate).
    let inst = e.instantiate_composition(&back).unwrap();
    let inst_part = e.entity_at_path(inst, "0").unwrap();
    assert_eq!(
        e.get_field(inst_part, "MeshRenderer", "mesh"),
        Some(FieldValue::Str("assets/pauldron.glb".into())),
        "the re-instantiated asset renders as its mesh"
    );
}
