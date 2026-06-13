//! Undo/redo tests including property-based randomized sequences and entity resurrection.

// Test code: long property-test bodies and small loop-counter → i64 casts.
#![allow(clippy::too_many_lines, clippy::cast_possible_wrap)]

use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::rng::Rng;
use metrocalk_ecs::FlecsWorld;

fn engine() -> Engine<FlecsWorld> {
    Engine::new(FlecsWorld::new(), 1)
}

const SEED: u64 = 0x4D45_5452_4F43_4131;

// Fixed (component, field) vocabulary for the per-single-undo property test's state snapshot.
const PROP_COMPS: [&str; 3] = ["Health", "Transform", "Tag"];
const PROP_FIELDS: [&str; 5] = ["hp", "maxHp", "px", "py", "name"];

/// A full field-value snapshot over a fixed entity pool and (component, field) vocabulary. Two
/// snapshots compare equal iff every tracked field reads identically — the semantic scene state
/// (empty vs. absent component both read `None`, so structural tidiness doesn't perturb it).
fn field_snapshot(e: &Engine<FlecsWorld>, entities: &[EntityId]) -> Vec<Option<FieldValue>> {
    let mut v = Vec::with_capacity(entities.len() * PROP_COMPS.len() * PROP_FIELDS.len());
    for &id in entities {
        for c in PROP_COMPS {
            for f in PROP_FIELDS {
                v.push(e.get_field(id, c, f));
            }
        }
    }
    v
}

// ── basic undo/redo ────────────────────────────────────────────────────

#[test]
fn undo_create_entity() {
    let mut e = engine();
    let id = e.alloc_entity_id();
    e.commit("create", vec![Op::CreateEntity { id, parent: None }])
        .unwrap();
    assert!(e.entity_exists(id));

    assert!(e.undo());
    assert!(!e.entity_exists(id));

    assert!(e.redo());
    assert!(e.entity_exists(id));
}

#[test]
fn undo_set_field() {
    let mut e = engine();
    let id = e.alloc_entity_id();
    e.commit("create", vec![Op::CreateEntity { id, parent: None }])
        .unwrap();
    e.commit(
        "set-hp",
        vec![Op::SetField {
            entity: id,
            component: "Health".into(),
            field: "hp".into(),
            value: FieldValue::Integer(100),
        }],
    )
    .unwrap();
    assert_eq!(
        e.get_field(id, "Health", "hp"),
        Some(FieldValue::Integer(100))
    );

    assert!(e.undo());
    // After undoing SetField, the field should be gone (it didn't exist before)
    assert!(e.get_field(id, "Health", "hp").is_none());

    assert!(e.redo());
    assert_eq!(
        e.get_field(id, "Health", "hp"),
        Some(FieldValue::Integer(100))
    );
}

#[test]
fn undo_overwrite_field() {
    let mut e = engine();
    let id = e.alloc_entity_id();
    e.commit(
        "create",
        vec![
            Op::CreateEntity { id, parent: None },
            Op::SetField {
                entity: id,
                component: "Health".into(),
                field: "hp".into(),
                value: FieldValue::Integer(50),
            },
        ],
    )
    .unwrap();

    e.commit(
        "update",
        vec![Op::SetField {
            entity: id,
            component: "Health".into(),
            field: "hp".into(),
            value: FieldValue::Integer(100),
        }],
    )
    .unwrap();
    assert_eq!(
        e.get_field(id, "Health", "hp"),
        Some(FieldValue::Integer(100))
    );

    assert!(e.undo());
    assert_eq!(
        e.get_field(id, "Health", "hp"),
        Some(FieldValue::Integer(50))
    );

    assert!(e.redo());
    assert_eq!(
        e.get_field(id, "Health", "hp"),
        Some(FieldValue::Integer(100))
    );
}

// ── entity resurrection ────────────────────────────────────────────────

#[test]
fn undo_delete_resurrects_entity_with_components() {
    let mut e = engine();
    let id = e.alloc_entity_id();
    e.commit(
        "create-with-data",
        vec![
            Op::CreateEntity { id, parent: None },
            Op::SetField {
                entity: id,
                component: "Health".into(),
                field: "hp".into(),
                value: FieldValue::Integer(100),
            },
            Op::SetField {
                entity: id,
                component: "Health".into(),
                field: "maxHp".into(),
                value: FieldValue::Integer(200),
            },
            Op::SetField {
                entity: id,
                component: "Transform".into(),
                field: "px".into(),
                value: FieldValue::Number(1.5),
            },
        ],
    )
    .unwrap();

    // Delete
    e.commit("delete", vec![Op::DeleteEntity { id }]).unwrap();
    assert!(!e.entity_exists(id));
    assert!(e.get_field(id, "Health", "hp").is_none());

    // Undo delete → entity resurrected with all components
    assert!(e.undo());
    assert!(e.entity_exists(id));
    assert_eq!(
        e.get_field(id, "Health", "hp"),
        Some(FieldValue::Integer(100))
    );
    assert_eq!(
        e.get_field(id, "Health", "maxHp"),
        Some(FieldValue::Integer(200))
    );
    assert_eq!(
        e.get_field(id, "Transform", "px"),
        Some(FieldValue::Number(1.5))
    );
}

#[test]
fn undo_delete_resurrects_subtree() {
    let mut e = engine();
    let parent = e.alloc_entity_id();
    let child = e.alloc_entity_id();
    e.commit(
        "tree",
        vec![
            Op::CreateEntity {
                id: parent,
                parent: None,
            },
            Op::SetField {
                entity: parent,
                component: "Tag".into(),
                field: "name".into(),
                value: FieldValue::Str("parent".into()),
            },
            Op::CreateEntity {
                id: child,
                parent: Some(parent),
            },
            Op::SetField {
                entity: child,
                component: "Tag".into(),
                field: "name".into(),
                value: FieldValue::Str("child".into()),
            },
        ],
    )
    .unwrap();
    assert_eq!(e.entity_count(), 2);

    // Delete parent (cascades to child)
    e.commit("delete-tree", vec![Op::DeleteEntity { id: parent }])
        .unwrap();
    assert_eq!(e.entity_count(), 0);

    // Undo → both parent and child resurrected
    assert!(e.undo());
    assert_eq!(e.entity_count(), 2);
    assert!(e.entity_exists(parent));
    assert!(e.entity_exists(child));
    assert_eq!(
        e.get_field(parent, "Tag", "name"),
        Some(FieldValue::Str("parent".into()))
    );
    assert_eq!(
        e.get_field(child, "Tag", "name"),
        Some(FieldValue::Str("child".into()))
    );
}

#[test]
fn undo_delete_resurrects_bindings() {
    let mut e = engine();
    let a = e.alloc_entity_id();
    let b = e.alloc_entity_id();
    e.commit(
        "setup",
        vec![
            Op::CreateEntity {
                id: a,
                parent: None,
            },
            Op::CreateEntity {
                id: b,
                parent: None,
            },
            Op::AddBinding {
                from: a,
                kind: "bindsTo".into(),
                to: b,
            },
        ],
    )
    .unwrap();

    // Delete a (binding from a should be captured)
    e.commit("delete-a", vec![Op::DeleteEntity { id: a }])
        .unwrap();
    assert!(!e.entity_exists(a));

    // Undo → a resurrected, binding restored
    assert!(e.undo());
    assert!(e.entity_exists(a));
    assert!(e.entity_exists(b));
}

// ── multiple undo/redo ─────────────────────────────────────────────────

#[test]
fn multiple_undo_redo_cycle() {
    let mut e = engine();
    let id = e.alloc_entity_id();
    e.commit("create", vec![Op::CreateEntity { id, parent: None }])
        .unwrap();

    for i in 0..10 {
        e.commit(
            "set",
            vec![Op::SetField {
                entity: id,
                component: "Health".into(),
                field: "hp".into(),
                value: FieldValue::Integer(i),
            }],
        )
        .unwrap();
    }
    assert_eq!(
        e.get_field(id, "Health", "hp"),
        Some(FieldValue::Integer(9))
    );

    // Undo all 10 SetField ops
    for _ in 0..10 {
        assert!(e.undo());
    }
    assert!(e.get_field(id, "Health", "hp").is_none());

    // Redo all 10
    for _ in 0..10 {
        assert!(e.redo());
    }
    assert_eq!(
        e.get_field(id, "Health", "hp"),
        Some(FieldValue::Integer(9))
    );
}

#[test]
fn commit_after_undo_clears_redo() {
    let mut e = engine();
    let id = e.alloc_entity_id();
    e.commit(
        "create",
        vec![
            Op::CreateEntity { id, parent: None },
            Op::SetField {
                entity: id,
                component: "X".into(),
                field: "v".into(),
                value: FieldValue::Integer(1),
            },
        ],
    )
    .unwrap();

    e.commit(
        "set-2",
        vec![Op::SetField {
            entity: id,
            component: "X".into(),
            field: "v".into(),
            value: FieldValue::Integer(2),
        }],
    )
    .unwrap();

    assert!(e.undo()); // back to v=1
    assert!(e.can_redo());

    // New commit clears redo stack
    e.commit(
        "set-3",
        vec![Op::SetField {
            entity: id,
            component: "X".into(),
            field: "v".into(),
            value: FieldValue::Integer(3),
        }],
    )
    .unwrap();
    assert!(!e.can_redo());
    assert_eq!(e.get_field(id, "X", "v"), Some(FieldValue::Integer(3)));
}

// ── empty undo/redo ────────────────────────────────────────────────────

#[test]
fn undo_on_empty_returns_false() {
    let mut e = engine();
    assert!(!e.undo());
    assert!(!e.redo());
}

// ── property test: randomized sequences ────────────────────────────────

#[test]
fn property_undo_redo_random_sequence() {
    let mut rng = Rng::new(SEED);
    let mut e = engine();

    // Create 50 entities
    let mut entities = Vec::new();
    for _ in 0..50 {
        let id = e.alloc_entity_id();
        e.commit("create", vec![Op::CreateEntity { id, parent: None }])
            .unwrap();
        entities.push(id);
    }
    let create_count = 50;

    // Capture state after initial creation
    let initial_entity_count = e.entity_count();
    assert_eq!(initial_entity_count, 50);

    // Execute 200 random operations
    let mut op_count = 0;
    for _ in 0..200 {
        let roll = rng.below(100);
        let eid = entities[rng.below(entities.len())];

        if roll < 60 {
            // SetField
            let val = FieldValue::Integer(rng.below(1000) as i64);
            let comp = ["Health", "Transform", "Script"][rng.below(3)];
            let field = ["hp", "px", "v"][rng.below(3)];
            if e.entity_exists(eid) {
                e.commit(
                    "set",
                    vec![Op::SetField {
                        entity: eid,
                        component: comp.into(),
                        field: field.into(),
                        value: val,
                    }],
                )
                .unwrap();
                op_count += 1;
            }
        } else if roll < 75 {
            // Delete
            if e.entity_exists(eid) {
                e.commit("delete", vec![Op::DeleteEntity { id: eid }])
                    .unwrap();
                op_count += 1;
            }
        } else if roll < 85 {
            // Binding add
            let other = entities[rng.below(entities.len())];
            if e.entity_exists(eid) && e.entity_exists(other) && eid != other {
                e.commit(
                    "bind",
                    vec![Op::AddBinding {
                        from: eid,
                        kind: "bindsTo".into(),
                        to: other,
                    }],
                )
                .unwrap();
                op_count += 1;
            }
        } else {
            // Reparent (skip if entity doesn't exist)
            if e.entity_exists(eid) {
                e.commit(
                    "reparent",
                    vec![Op::Reparent {
                        entity: eid,
                        new_parent: None,
                    }],
                )
                .unwrap();
                op_count += 1;
            }
        }
    }

    // Capture post-op field values for comparison
    let mut post_op_fields = std::collections::HashMap::new();
    for eid in &entities {
        if e.entity_exists(*eid) {
            for comp in &["Health", "Transform", "Script"] {
                for field in &["hp", "px", "v"] {
                    if let Some(v) = e.get_field(*eid, comp, field) {
                        post_op_fields.insert((*eid, comp.to_string(), field.to_string()), v);
                    }
                }
            }
        }
    }
    let post_op_entity_count = e.entity_count();

    // Undo ALL operations (including the initial creates)
    let mut undo_count = 0;
    while e.undo() {
        undo_count += 1;
    }
    assert_eq!(undo_count, op_count + create_count, "should undo all ops");
    assert_eq!(e.entity_count(), 0, "after full undo, no entities remain");

    // Redo ALL operations
    let mut redo_count = 0;
    while e.redo() {
        redo_count += 1;
    }
    assert_eq!(redo_count, undo_count, "redo count matches undo count");
    assert_eq!(
        e.entity_count(),
        post_op_entity_count,
        "entity count restored after redo-all"
    );

    // Verify field values match
    for ((eid, comp, field), expected) in &post_op_fields {
        let actual = e.get_field(*eid, comp, field);
        assert_eq!(
            actual.as_ref(),
            Some(expected),
            "field {eid}.{comp}.{field} mismatch after undo-all → redo-all"
        );
    }
}

#[test]
fn property_delete_undo_resurrection_random() {
    let mut rng = Rng::new(SEED ^ 0xDE);
    let mut e = engine();

    // Create entities with data
    let mut entities = Vec::new();
    for _ in 0..30 {
        let id = e.alloc_entity_id();
        let hp = rng.below(1000) as i64;
        let name = format!("entity_{}", rng.below(9999));
        e.commit(
            "create",
            vec![
                Op::CreateEntity { id, parent: None },
                Op::SetField {
                    entity: id,
                    component: "Health".into(),
                    field: "hp".into(),
                    value: FieldValue::Integer(hp),
                },
                Op::SetField {
                    entity: id,
                    component: "Tag".into(),
                    field: "name".into(),
                    value: FieldValue::Str(name),
                },
            ],
        )
        .unwrap();
        entities.push(id);
    }

    // Delete and undo 20 times — each time, verify resurrection restores data
    for _ in 0..20 {
        let idx = rng.below(entities.len());
        let eid = entities[idx];
        if !e.entity_exists(eid) {
            continue;
        }

        let hp_before = e.get_field(eid, "Health", "hp");
        let name_before = e.get_field(eid, "Tag", "name");

        e.commit("delete", vec![Op::DeleteEntity { id: eid }])
            .unwrap();
        assert!(!e.entity_exists(eid));

        e.undo();
        assert!(e.entity_exists(eid), "entity should be resurrected");
        assert_eq!(e.get_field(eid, "Health", "hp"), hp_before);
        assert_eq!(e.get_field(eid, "Tag", "name"), name_before);
    }
}

// ── M1.6 additive-undo regression ──────────────────────────────────────

/// The M1 audit bug: undoing an additive `SetField` removed the WHOLE component, destroying a
/// sibling field written by an *earlier* transaction. The precise inverse must remove only the
/// field. (Pre-fix, this test fails — `Health.hp` is wiped along with `Health.maxHp`.)
#[test]
fn undo_additive_set_field_preserves_sibling_field() {
    let mut e = engine();
    let id = e.alloc_entity_id();
    e.commit("create", vec![Op::CreateEntity { id, parent: None }])
        .unwrap();

    // tx1: Health.hp
    e.commit(
        "set-hp",
        vec![Op::SetField {
            entity: id,
            component: "Health".into(),
            field: "hp".into(),
            value: FieldValue::Integer(100),
        }],
    )
    .unwrap();

    // tx2: Health.maxHp — additive (same component, new field, SEPARATE transaction)
    e.commit(
        "set-maxhp",
        vec![Op::SetField {
            entity: id,
            component: "Health".into(),
            field: "maxHp".into(),
            value: FieldValue::Integer(200),
        }],
    )
    .unwrap();

    assert_eq!(
        e.get_field(id, "Health", "hp"),
        Some(FieldValue::Integer(100))
    );
    assert_eq!(
        e.get_field(id, "Health", "maxHp"),
        Some(FieldValue::Integer(200))
    );

    // Undo tx2 ONLY: maxHp gone, hp SURVIVES.
    assert!(e.undo());
    assert_eq!(
        e.get_field(id, "Health", "hp"),
        Some(FieldValue::Integer(100)),
        "sibling field hp from an earlier transaction must survive undo of an additive set"
    );
    assert!(e.get_field(id, "Health", "maxHp").is_none());

    // Redo restores maxHp without disturbing hp.
    assert!(e.redo());
    assert_eq!(
        e.get_field(id, "Health", "hp"),
        Some(FieldValue::Integer(100))
    );
    assert_eq!(
        e.get_field(id, "Health", "maxHp"),
        Some(FieldValue::Integer(200))
    );
}

/// `Op::RemoveField` removes one field, leaves siblings intact, and undoes by restoring the
/// removed value.
#[test]
fn remove_field_op_and_undo() {
    let mut e = engine();
    let id = e.alloc_entity_id();
    e.commit(
        "create",
        vec![
            Op::CreateEntity { id, parent: None },
            Op::SetField {
                entity: id,
                component: "Health".into(),
                field: "hp".into(),
                value: FieldValue::Integer(100),
            },
            Op::SetField {
                entity: id,
                component: "Health".into(),
                field: "maxHp".into(),
                value: FieldValue::Integer(200),
            },
        ],
    )
    .unwrap();

    e.commit(
        "rm-hp",
        vec![Op::RemoveField {
            entity: id,
            component: "Health".into(),
            field: "hp".into(),
        }],
    )
    .unwrap();
    assert!(e.get_field(id, "Health", "hp").is_none());
    assert_eq!(
        e.get_field(id, "Health", "maxHp"),
        Some(FieldValue::Integer(200))
    );

    // Undo restores hp without touching maxHp.
    assert!(e.undo());
    assert_eq!(
        e.get_field(id, "Health", "hp"),
        Some(FieldValue::Integer(100))
    );
    assert_eq!(
        e.get_field(id, "Health", "maxHp"),
        Some(FieldValue::Integer(200))
    );
}

// ── M1.6 per-single-undo property invariant ────────────────────────────

/// After EACH single undo, the field state must equal the snapshot taken immediately before the
/// undone transaction — not merely after a full undo-all→redo-all round-trip (which masks the
/// additive over-removal bug: wiping a whole component on undo destroys sibling fields written by
/// earlier transactions, yet still reverts cleanly once ALL transactions are undone).
#[test]
fn property_per_single_undo_restores_prior_state() {
    let mut rng = Rng::new(SEED ^ 0x5117);
    let mut e = engine();

    // Flat entity pool (parent: None keeps resurrection parent-free).
    let mut entities = Vec::new();
    for _ in 0..16 {
        let id = e.alloc_entity_id();
        e.commit("create", vec![Op::CreateEntity { id, parent: None }])
            .unwrap();
        entities.push(id);
    }

    // Build a sequence of single-op transactions, snapshotting the field state before each.
    let mut pre: Vec<Vec<Option<FieldValue>>> = Vec::new();
    for _ in 0..150 {
        let alive: Vec<EntityId> = entities
            .iter()
            .copied()
            .filter(|&id| e.entity_exists(id))
            .collect();
        if alive.is_empty() {
            break;
        }
        let target = alive[rng.below(alive.len())];
        let comp = PROP_COMPS[rng.below(PROP_COMPS.len())];
        let field = PROP_FIELDS[rng.below(PROP_FIELDS.len())];
        let roll = rng.below(100);
        let op = if roll < 60 {
            // Across transactions this builds up multi-field components, exercising the additive
            // path: undoing a later field-add must not wipe an earlier sibling field.
            Op::SetField {
                entity: target,
                component: comp.into(),
                field: field.into(),
                value: FieldValue::Integer(rng.below(1000) as i64),
            }
        } else if roll < 75 {
            Op::RemoveField {
                entity: target,
                component: comp.into(),
                field: field.into(),
            }
        } else if roll < 85 {
            Op::RemoveComponent {
                entity: target,
                component: comp.into(),
            }
        } else if roll < 92 && alive.len() > 5 {
            Op::DeleteEntity { id: target }
        } else {
            Op::SetField {
                entity: target,
                component: comp.into(),
                field: field.into(),
                value: FieldValue::Integer(rng.below(1000) as i64),
            }
        };
        pre.push(field_snapshot(&e, &entities));
        e.commit("tx", vec![op]).unwrap();
    }

    assert!(pre.len() > 50, "should have built a substantial sequence");

    // Undo every transaction, asserting the per-single-undo invariant after each.
    for i in (0..pre.len()).rev() {
        assert!(e.undo(), "undo #{i} should succeed");
        assert_eq!(
            field_snapshot(&e, &entities),
            pre[i],
            "after undoing transaction {i}, field state must equal the pre-transaction snapshot"
        );
    }
}
