//! The M2.6 edit round-trip through the **real `/core`** (no MockCore): an editor `EditTx` →
//! `Engine::commit` → `ProjectionDelta`, with undo via the engine-side stack and rejections carrying
//! the pipeline's real reason. Also pins the wire JSON to the M2.5 editor's camelCase shape.

use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::FlecsWorld;
use metrocalk_editor_shell::{
    apply_edit, project_full, EditIntent, EditTx, ProjectionDelta, ProjectionOp,
};
use serde_json::json;

fn engine_with_entity() -> (Engine<FlecsWorld>, EntityId) {
    let mut e = Engine::new(FlecsWorld::new(), 1);
    let id = e.alloc_entity_id();
    e.commit(
        "create",
        vec![
            Op::CreateEntity { id, parent: None },
            Op::SetField {
                entity: id,
                component: "Transform".into(),
                field: "x".into(),
                value: FieldValue::Integer(0),
            },
        ],
    )
    .unwrap();
    (e, id)
}

#[test]
fn setfield_round_trips_through_real_core_and_undoes() {
    let (mut e, id) = engine_with_entity();
    let tx = EditTx {
        client_op_id: "op1".into(),
        label: "set x".into(),
        patches: vec![],
        intent: EditIntent::SetField {
            id: id.to_loro_key(),
            component: "Transform".into(),
            field: "x".into(),
            value: json!(5),
        },
    };
    let delta = apply_edit(&mut e, &tx);

    assert_eq!(delta.confirms, vec!["op1"]);
    assert!(delta.rejects.is_empty());
    assert_eq!(
        e.get_field(id, "Transform", "x"),
        Some(FieldValue::Integer(5))
    ); // authoritative core updated

    // Ctrl-Z via the engine-side inverse-op stack (M1.6 / ADR-002 F2)
    assert!(e.undo());
    assert_eq!(
        e.get_field(id, "Transform", "x"),
        Some(FieldValue::Integer(0))
    );
}

#[test]
fn bind_to_unknown_entity_is_rejected_with_a_reason() {
    let (mut e, id) = engine_with_entity();
    let bogus = EntityId {
        peer: 9,
        counter: 9,
    }
    .to_loro_key();
    let tx = EditTx {
        client_op_id: "op2".into(),
        label: "bind".into(),
        patches: vec![],
        intent: EditIntent::Bind {
            from: id.to_loro_key(),
            rel: "BindsTo".into(),
            to: bogus,
        },
    };
    let delta = apply_edit(&mut e, &tx);

    assert!(delta.confirms.is_empty());
    assert_eq!(delta.rejects.len(), 1);
    assert_eq!(delta.rejects[0].client_op_id, "op2");
    assert!(
        !delta.rejects[0].reason.is_empty(),
        "every 'no' is explained"
    );
}

#[test]
fn valid_bind_confirms() {
    let mut e = Engine::new(FlecsWorld::new(), 1);
    let a = e.alloc_entity_id();
    let b = e.alloc_entity_id();
    e.commit(
        "seed",
        vec![
            Op::CreateEntity {
                id: a,
                parent: None,
            },
            Op::CreateEntity {
                id: b,
                parent: None,
            },
        ],
    )
    .unwrap();
    let tx = EditTx {
        client_op_id: "op3".into(),
        label: "bind".into(),
        patches: vec![],
        intent: EditIntent::Bind {
            from: a.to_loro_key(),
            rel: "BindsTo".into(),
            to: b.to_loro_key(),
        },
    };
    let delta = apply_edit(&mut e, &tx);
    assert_eq!(delta.confirms, vec!["op3"]);
    assert!(delta.rejects.is_empty());
}

#[test]
fn project_full_emits_the_whole_scene_for_initial_load() {
    let mut e = Engine::new(FlecsWorld::new(), 1);
    let a = e.alloc_entity_id();
    let b = e.alloc_entity_id();
    e.commit(
        "seed",
        vec![
            Op::CreateEntity {
                id: a,
                parent: None,
            },
            Op::CreateEntity {
                id: b,
                parent: Some(a),
            },
            Op::SetField {
                entity: a,
                component: "Transform".into(),
                field: "x".into(),
                value: FieldValue::Integer(3),
            },
            Op::AddBinding {
                from: a,
                kind: "BindsTo".into(),
                to: b,
            },
        ],
    )
    .unwrap();

    let delta = project_full(&e);
    let upserts = delta
        .ops
        .iter()
        .filter(|o| matches!(o, ProjectionOp::Upsert { .. }))
        .count();
    let setfields = delta
        .ops
        .iter()
        .filter(|o| matches!(o, ProjectionOp::SetField { .. }))
        .count();
    let edges = delta
        .ops
        .iter()
        .filter(|o| matches!(o, ProjectionOp::AddEdge { .. }))
        .count();
    assert_eq!(upserts, 2, "one upsert per entity");
    assert!(setfields >= 1, "the Transform.x field is projected");
    assert_eq!(edges, 1, "the binding is projected as an edge");
    assert!(
        delta.confirms.is_empty() && delta.rejects.is_empty(),
        "initial load is server-initiated"
    );
}

#[test]
fn wire_json_matches_the_editor_camelcase_shape() {
    // EditTx deserializes from the editor's exact JSON…
    let tx_json = r#"{"clientOpId":"x","label":"l","patches":[],"intent":{"kind":"setField","id":"1_0","component":"Transform","field":"x","value":7}}"#;
    let tx: EditTx = serde_json::from_str(tx_json).unwrap();
    assert!(matches!(tx.intent, EditIntent::SetField { .. }));
    // …and ProjectionDelta serializes back to the shape the editor consumes.
    let d = ProjectionDelta {
        ops: vec![ProjectionOp::AddEdge {
            from: "a".into(),
            rel: "r".into(),
            to: "b".into(),
        }],
        confirms: vec!["x".into()],
        rejects: vec![],
        full: false,
    };
    let s = serde_json::to_string(&d).unwrap();
    assert!(s.contains(r#""op":"addEdge""#), "{s}");
    assert!(s.contains(r#""confirms":["x"]"#), "{s}");
}
