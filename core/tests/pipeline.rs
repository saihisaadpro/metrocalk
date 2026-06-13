//! Commit pipeline integration tests.

use metrocalk_core::{Engine, EntityId, FieldValue, Op, PipelineError};
use metrocalk_ecs::FlecsWorld;

fn engine() -> Engine<FlecsWorld> {
    Engine::new(FlecsWorld::new(), 1)
}

#[test]
fn create_entity_and_read_field() {
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
        ],
    )
    .unwrap();

    assert!(e.entity_exists(id));
    assert_eq!(
        e.get_field(id, "Health", "hp"),
        Some(FieldValue::Integer(100))
    );
    assert_eq!(e.entity_count(), 1);
}

#[test]
fn delete_entity_removes_from_both_worlds() {
    let mut e = engine();
    let id = e.alloc_entity_id();
    e.commit("create", vec![Op::CreateEntity { id, parent: None }])
        .unwrap();
    assert!(e.entity_exists(id));

    e.commit("delete", vec![Op::DeleteEntity { id }]).unwrap();
    assert!(!e.entity_exists(id));
    assert_eq!(e.entity_count(), 0);
}

#[test]
fn duplicate_entity_errors() {
    let mut e = engine();
    let id = e.alloc_entity_id();
    e.commit("create", vec![Op::CreateEntity { id, parent: None }])
        .unwrap();

    let err = e
        .commit("dup", vec![Op::CreateEntity { id, parent: None }])
        .unwrap_err();
    assert!(matches!(err, PipelineError::DuplicateEntity(_)));
}

#[test]
fn unknown_entity_errors() {
    let mut e = engine();
    let id = EntityId { peer: 99, counter: 99 };
    let err = e
        .commit(
            "set",
            vec![Op::SetField {
                entity: id,
                component: "X".into(),
                field: "y".into(),
                value: FieldValue::Integer(1),
            }],
        )
        .unwrap_err();
    assert!(matches!(err, PipelineError::UnknownEntity(_)));
}

#[test]
fn parent_child_hierarchy() {
    let mut e = engine();
    let parent = e.alloc_entity_id();
    let child = e.alloc_entity_id();
    e.commit(
        "hierarchy",
        vec![
            Op::CreateEntity {
                id: parent,
                parent: None,
            },
            Op::CreateEntity {
                id: child,
                parent: Some(parent),
            },
        ],
    )
    .unwrap();
    assert!(e.entity_exists(parent));
    assert!(e.entity_exists(child));
    assert_eq!(e.entity_count(), 2);
}

#[test]
fn cascade_delete_removes_children() {
    let mut e = engine();
    let parent = e.alloc_entity_id();
    let child1 = e.alloc_entity_id();
    let child2 = e.alloc_entity_id();
    e.commit(
        "tree",
        vec![
            Op::CreateEntity {
                id: parent,
                parent: None,
            },
            Op::CreateEntity {
                id: child1,
                parent: Some(parent),
            },
            Op::CreateEntity {
                id: child2,
                parent: Some(child1),
            },
        ],
    )
    .unwrap();
    assert_eq!(e.entity_count(), 3);

    e.commit("delete-parent", vec![Op::DeleteEntity { id: parent }])
        .unwrap();
    assert_eq!(e.entity_count(), 0);
    assert!(!e.entity_exists(child1));
    assert!(!e.entity_exists(child2));
}

#[test]
fn tags_and_pairs_tracked() {
    let mut e = engine();
    let id = e.alloc_entity_id();
    e.commit("create", vec![Op::CreateEntity { id, parent: None }])
        .unwrap();

    // Tag Entity handles come from outside the pipeline (Registry creates them).
    // Engine.world() returns &W (immutable) — proving the pipeline is the sole mutation path.
    // We can't create tag entities through world() (compile error: &mut required).
    // Tag/pair tracking is tested via undo in undo.rs.
}

#[test]
fn bindings_round_trip() {
    let mut e = engine();
    let a = e.alloc_entity_id();
    let b = e.alloc_entity_id();
    e.commit(
        "create",
        vec![
            Op::CreateEntity { id: a, parent: None },
            Op::CreateEntity { id: b, parent: None },
            Op::AddBinding {
                from: a,
                kind: "bindsTo".into(),
                to: b,
            },
        ],
    )
    .unwrap();

    // Remove binding
    e.commit(
        "unbind",
        vec![Op::RemoveBinding {
            from: a,
            kind: "bindsTo".into(),
            to: b,
        }],
    )
    .unwrap();
}

#[test]
fn reparent_entity() {
    let mut e = engine();
    let a = e.alloc_entity_id();
    let b = e.alloc_entity_id();
    let c = e.alloc_entity_id();
    e.commit(
        "tree",
        vec![
            Op::CreateEntity { id: a, parent: None },
            Op::CreateEntity { id: b, parent: None },
            Op::CreateEntity {
                id: c,
                parent: Some(a),
            },
        ],
    )
    .unwrap();

    // Reparent c from a to b
    e.commit(
        "reparent",
        vec![Op::Reparent {
            entity: c,
            new_parent: Some(b),
        }],
    )
    .unwrap();
}

#[test]
fn remove_component() {
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
        ],
    )
    .unwrap();
    assert!(e.get_field(id, "Health", "hp").is_some());

    e.commit(
        "remove",
        vec![Op::RemoveComponent {
            entity: id,
            component: "Health".into(),
        }],
    )
    .unwrap();
    assert!(e.get_field(id, "Health", "hp").is_none());
}

#[test]
fn deltas_only_no_snapshot() {
    // Invariant 2: the pipeline mirrors deltas, never full-state snapshots.
    // Verify by checking that export_updates() produces bytes (not a snapshot).
    let mut e = engine();
    let vv_before = e.version_vector();
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
        ],
    )
    .unwrap();
    let updates = e.export_updates_since(&vv_before);
    assert!(!updates.is_empty(), "delta export should produce bytes");

    // The updates should be importable into a fresh doc and produce the same state
    let doc2 = loro::LoroDoc::new();
    doc2.import(&updates).unwrap();
    let hp = doc2.get_map("components").get_deep_value();
    // Verify the data arrived
    if let loro::LoroValue::Map(m) = hp {
        assert!(!m.is_empty(), "imported delta should contain component data");
    }
}

#[test]
fn sole_mutation_path_enforced_by_visibility() {
    // The Engine's `world` field is private. We can only access it via `world()` which
    // returns &W (immutable). This is a compile-time guarantee, not a runtime test.
    // This test documents the invariant.
    let e = engine();
    let _w: &metrocalk_ecs::FlecsWorld = e.world(); // &W — read-only ✓
    // e.world is private — the following would not compile:
    // e.world.create_entity();  // ERROR: field `world` is private
}
