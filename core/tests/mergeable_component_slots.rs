//! Regression coverage for concurrently first-created component records.

use metrocalk_core::{Engine, FieldValue, Op};
use metrocalk_ecs::FlecsWorld;

fn engine(peer: u64) -> Engine<FlecsWorld> {
    Engine::new(FlecsWorld::new(), peer)
}

#[test]
fn concurrent_first_fields_share_one_mergeable_component_slot() {
    let mut seed = engine(1);
    let entity = seed.alloc_entity_id();
    seed.commit(
        "shared-base-entity",
        vec![Op::CreateEntity {
            id: entity,
            parent: None,
        }],
    )
    .unwrap();

    let shared = seed.fork_doc();
    let mut peer_a = engine(2);
    peer_a.merge(&shared).unwrap();
    let mut peer_b = engine(3);
    peer_b.merge(&shared).unwrap();

    assert!(!peer_a.components_of(entity).contains_key("AnimationTrack"));
    assert!(!peer_b.components_of(entity).contains_key("AnimationTrack"));

    peer_a
        .commit(
            "peer-a-first-field",
            vec![Op::SetField {
                entity,
                component: "AnimationTrack".into(),
                field: "track::translation".into(),
                value: FieldValue::Str("peer-a".into()),
            }],
        )
        .unwrap();
    peer_b
        .commit(
            "peer-b-first-field",
            vec![Op::SetField {
                entity,
                component: "AnimationTrack".into(),
                field: "key::translation::one".into(),
                value: FieldValue::Str("peer-b".into()),
            }],
        )
        .unwrap();

    let peer_b_updates = peer_b.export_updates();
    peer_b.merge(&peer_a.export_updates()).unwrap();
    peer_a.merge(&peer_b_updates).unwrap();

    for peer in [&peer_a, &peer_b] {
        assert_eq!(
            peer.get_field(entity, "AnimationTrack", "track::translation"),
            Some(FieldValue::Str("peer-a".into()))
        );
        assert_eq!(
            peer.get_field(entity, "AnimationTrack", "key::translation::one"),
            Some(FieldValue::Str("peer-b".into()))
        );
    }
    assert_eq!(peer_a.canonical_state(), peer_b.canonical_state());
}

#[test]
fn first_field_commit_undo_redo_round_trips() {
    let mut engine = engine(4);
    let entity = engine.alloc_entity_id();
    engine
        .commit(
            "base-entity",
            vec![Op::CreateEntity {
                id: entity,
                parent: None,
            }],
        )
        .unwrap();
    engine.clear_history();
    let pristine = engine.canonical_state();

    engine
        .commit(
            "first-component-field",
            vec![Op::SetField {
                entity,
                component: "AnimationTrack".into(),
                field: "track::translation".into(),
                value: FieldValue::Str("authored".into()),
            }],
        )
        .unwrap();
    let committed = engine.canonical_state();

    assert!(engine.undo());
    assert!(!engine.components_of(entity).contains_key("AnimationTrack"));
    assert_eq!(
        engine.canonical_state(),
        pristine,
        "an empty physical mergeable slot must not change logical revision identity"
    );

    let snapshot = engine.snapshot();
    let mut reopened = self::engine(40);
    reopened.merge(&snapshot).expect("reload undone document");
    assert_eq!(reopened.canonical_state(), pristine);
    assert!(!reopened
        .components_of(entity)
        .contains_key("AnimationTrack"));

    assert!(engine.redo());
    assert_eq!(
        engine.get_field(entity, "AnimationTrack", "track::translation"),
        Some(FieldValue::Str("authored".into()))
    );
    assert_eq!(engine.canonical_state(), committed);
}

#[test]
fn removed_component_fields_do_not_resurrect_when_its_mergeable_slot_is_reused() {
    let mut engine = engine(5);
    let entity = engine.alloc_entity_id();
    engine
        .commit(
            "component-with-stale-candidate",
            vec![
                Op::CreateEntity {
                    id: entity,
                    parent: None,
                },
                Op::SetField {
                    entity,
                    component: "AnimationTrack".into(),
                    field: "old-field".into(),
                    value: FieldValue::Integer(948),
                },
            ],
        )
        .unwrap();
    engine
        .commit(
            "remove-component",
            vec![Op::RemoveComponent {
                entity,
                component: "AnimationTrack".into(),
            }],
        )
        .unwrap();

    engine
        .commit(
            "reuse-mergeable-slot",
            vec![Op::SetField {
                entity,
                component: "AnimationTrack".into(),
                field: "new-field".into(),
                value: FieldValue::Integer(169),
            }],
        )
        .unwrap();

    assert_eq!(
        engine.get_field(entity, "AnimationTrack", "old-field"),
        None
    );
    assert_eq!(
        engine.get_field(entity, "AnimationTrack", "new-field"),
        Some(FieldValue::Integer(169))
    );

    assert!(engine.undo());
    assert!(!engine.components_of(entity).contains_key("AnimationTrack"));
}

#[test]
fn removing_last_field_does_not_hide_a_concurrent_sibling_field() {
    let mut seed = engine(6);
    let entity = seed.alloc_entity_id();
    seed.commit(
        "shared-component",
        vec![
            Op::CreateEntity {
                id: entity,
                parent: None,
            },
            Op::SetField {
                entity,
                component: "AnimationTrack".into(),
                field: "old-field".into(),
                value: FieldValue::Integer(1),
            },
        ],
    )
    .unwrap();

    let shared = seed.fork_doc();
    let mut removing_peer = engine(7);
    removing_peer.merge(&shared).unwrap();
    let mut adding_peer = engine(8);
    adding_peer.merge(&shared).unwrap();

    removing_peer
        .commit(
            "remove-last-visible-field",
            vec![Op::RemoveField {
                entity,
                component: "AnimationTrack".into(),
                field: "old-field".into(),
            }],
        )
        .unwrap();
    adding_peer
        .commit(
            "add-concurrent-sibling-field",
            vec![Op::SetField {
                entity,
                component: "AnimationTrack".into(),
                field: "peer-field".into(),
                value: FieldValue::Integer(2),
            }],
        )
        .unwrap();

    let remove_updates = removing_peer.export_updates();
    let add_updates = adding_peer.export_updates();
    removing_peer.merge(&add_updates).unwrap();
    adding_peer.merge(&remove_updates).unwrap();

    for peer in [&removing_peer, &adding_peer] {
        assert_eq!(peer.get_field(entity, "AnimationTrack", "old-field"), None);
        assert_eq!(
            peer.get_field(entity, "AnimationTrack", "peer-field"),
            Some(FieldValue::Integer(2))
        );
    }
    assert_eq!(
        removing_peer.canonical_state(),
        adding_peer.canonical_state()
    );
}
