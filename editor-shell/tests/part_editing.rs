//! M9.2 (G2) rigid part editing — headless spine: a "part" is a child node, edited with G1's gizmo
//! math (parent-space write-back), stored as a per-field OVERRIDE (ADR-026), with descendants following
//! and reparent = one `node.move`. Proves the gradeable core of deliverable 1 without a live GPU:
//! - editing a PARENT's local moves a CHILD's global (descendants follow);
//! - parent-space write-back is correct for a child under a rotated+scaled parent (the #24104 trap);
//! - a part edit is a sparse per-field override, one undoable transaction (override-wins, base intact);
//! - reparent moves the part + undo restores the prior parent;
//! - the EditPart / Reparent persistence records round-trip (reload-persist shape).

use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::FlecsWorld;

use metrocalk_editor_shell::capscene;
use metrocalk_editor_shell::Record;
use metrocalk_gizmo::{axis_angle, Transform as GizmoTransform};

fn engine() -> Engine<FlecsWorld> {
    Engine::new(FlecsWorld::new(), 1)
}

/// A parent at base `(0,0,0)` with one child at base local `(cx,0,0)`. Returns `(parent, child)`.
fn parent_child(e: &mut Engine<FlecsWorld>, cx: f32) -> (EntityId, EntityId) {
    let parent = e.alloc_entity_id();
    let child = e.alloc_entity_id();
    let set = |entity, f: &str, v: f32| Op::SetField {
        entity,
        component: "Transform".into(),
        field: f.into(),
        value: FieldValue::Number(f64::from(v)),
    };
    e.commit(
        "compose",
        vec![
            Op::CreateEntity {
                id: parent,
                parent: None,
            },
            set(parent, "x", 0.0),
            Op::CreateEntity {
                id: child,
                parent: Some(parent),
            },
            set(child, "x", cx),
        ],
    )
    .unwrap();
    (parent, child)
}

#[test]
fn editing_a_parent_local_moves_a_childs_global() {
    let mut e = engine();
    let (parent, child) = parent_child(&mut e, 2.0);

    // Before: child global = parent(0) · local(2) = 2.
    assert!((capscene::global_transform(&e, child).translation[0] - 2.0).abs() < 1e-5);

    // Move the PARENT (as a per-field override) to x=10.
    capscene::set_part_local(&mut e, parent, [10.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], 1.0).unwrap();

    // The child FOLLOWS: its global = parent(10) · local(2) = 12, though its own local is unchanged.
    assert!(
        (capscene::global_transform(&e, parent).translation[0] - 10.0).abs() < 1e-5,
        "parent moved"
    );
    assert!(
        (capscene::global_transform(&e, child).translation[0] - 12.0).abs() < 1e-5,
        "descendant followed the parent edit"
    );
    assert!(
        (capscene::local_transform(&e, child).translation[0] - 2.0).abs() < 1e-5,
        "the child's OWN local is unchanged — it followed, it wasn't moved"
    );
}

#[test]
fn parent_space_write_back_for_a_child_under_a_rotated_scaled_parent() {
    // THE #24104 trap, applied to a child PART: the parent is rotated 90° about Y + scaled 2× + moved.
    // We set the child's desired WORLD transform; the parent-space write-back must store a LOCAL such
    // that the child's recomposed global reproduces the world we asked for.
    let mut e = engine();
    let (parent, child) = parent_child(&mut e, 0.0);

    let rot = axis_angle([0.0, 1.0, 0.0], std::f32::consts::FRAC_PI_2);
    capscene::set_transform(&mut e, parent, [5.0, 0.0, 0.0], rot, 2.0).unwrap();

    let desired_world = GizmoTransform {
        translation: [1.0, 2.0, 3.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0],
    };
    capscene::edit_part_transform(&mut e, child, desired_world).unwrap();

    let got = capscene::global_transform(&e, child);
    for k in 0..3 {
        assert!(
            (got.translation[k] - desired_world.translation[k]).abs() < 1e-3,
            "child world translation[{k}] reproduced: {} vs {}",
            got.translation[k],
            desired_world.translation[k]
        );
    }
}

#[test]
fn part_edit_is_a_sparse_override_one_undoable_tx() {
    let mut e = engine();
    let (_parent, child) = parent_child(&mut e, 2.0);

    capscene::set_part_local(&mut e, child, [9.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], 1.0).unwrap();

    // Stored as a per-field OVERRIDE (the 8 Transform keys), NOT a base rewrite.
    let ov = e.overrides_of(child);
    assert_eq!(ov.len(), 8, "8 sparse Transform override keys, got {ov:?}");
    assert_eq!(
        e.get_override(child, "Transform", "x"),
        Some(FieldValue::Number(9.0))
    );
    // The base is untouched (override-wins by structure): base x still 2.
    assert_eq!(
        e.get_field(child, "Transform", "x"),
        Some(FieldValue::Number(2.0)),
        "the base layer is untouched — the edit rode the override layer"
    );
    // Resolved (what the gizmo/renderer reads) is the override.
    assert!((capscene::local_transform(&e, child).translation[0] - 9.0).abs() < 1e-5);

    // ONE undoable transaction — Ctrl-Z reverts the whole part edit; the part returns to its base.
    assert!(e.undo(), "the part edit is one undoable tx");
    assert!(
        (capscene::local_transform(&e, child).translation[0] - 2.0).abs() < 1e-5,
        "undo reverted the part to its base pose"
    );
}

#[test]
fn reparent_moves_a_part_and_undo_restores() {
    let mut e = engine();
    // Two roots A (at x=0) and B (at x=100), and a child under A at local x=1.
    let a = e.alloc_entity_id();
    let b = e.alloc_entity_id();
    let child = e.alloc_entity_id();
    let set = |entity, f: &str, v: f32| Op::SetField {
        entity,
        component: "Transform".into(),
        field: f.into(),
        value: FieldValue::Number(f64::from(v)),
    };
    e.commit(
        "compose-two-parents",
        vec![
            Op::CreateEntity {
                id: a,
                parent: None,
            },
            set(a, "x", 0.0),
            Op::CreateEntity {
                id: b,
                parent: None,
            },
            set(b, "x", 100.0),
            Op::CreateEntity {
                id: child,
                parent: Some(a),
            },
            set(child, "x", 1.0),
        ],
    )
    .unwrap();
    assert_eq!(e.parent_of(child), Some(a));
    assert!((capscene::global_transform(&e, child).translation[0] - 1.0).abs() < 1e-5);

    // Drag the child into B (one node.move).
    capscene::reparent(&mut e, child, Some(b)).unwrap();
    assert_eq!(e.parent_of(child), Some(b), "reparented to B");
    assert!(
        (capscene::global_transform(&e, child).translation[0] - 101.0).abs() < 1e-5,
        "global recomputed under the new parent (100 + 1)"
    );

    // Undo restores the prior parent.
    assert!(e.undo());
    assert_eq!(
        e.parent_of(child),
        Some(a),
        "undo restored the prior parent"
    );
    assert!((capscene::global_transform(&e, child).translation[0] - 1.0).abs() < 1e-5);
}

#[test]
fn editpart_and_reparent_records_round_trip() {
    // Reload-persist shape: the new persistence records serialize + deserialize losslessly.
    let edit = Record::EditPart {
        id: "1_5".into(),
        x: 1.0,
        y: 2.0,
        z: 3.0,
        qx: 0.0,
        qy: 0.707_106_77,
        qz: 0.0,
        qw: 0.707_106_77,
        scale: 2.0,
    };
    let s = serde_json::to_string(&edit).unwrap();
    let back: Record = serde_json::from_str(&s).unwrap();
    assert!(
        matches!(back, Record::EditPart { ref id, scale, .. } if id == "1_5" && (scale - 2.0).abs() < 1e-9)
    );

    let rep = Record::Reparent {
        id: "1_5".into(),
        parent: Some("1_2".into()),
    };
    let s = serde_json::to_string(&rep).unwrap();
    let back: Record = serde_json::from_str(&s).unwrap();
    assert!(
        matches!(back, Record::Reparent { ref id, parent: Some(ref p) } if id == "1_5" && p == "1_2")
    );

    // Reparent-to-root (None) also round-trips.
    let root = Record::Reparent {
        id: "1_5".into(),
        parent: None,
    };
    let s = serde_json::to_string(&root).unwrap();
    let back: Record = serde_json::from_str(&s).unwrap();
    assert!(matches!(back, Record::Reparent { parent: None, .. }));
}
