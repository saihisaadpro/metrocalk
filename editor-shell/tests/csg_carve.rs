//! M13.2 — the **robust-CSG carve integration backstop** (headless; ADR-051). The `metrocalk-csg` unit
//! tests prove the boolean is watertight + deterministic in isolation; this proves the **engine
//! integration**: a carve reads its inputs **by handle** from the asset store, produces a **content-
//! addressed** `MeshAsset`, and is placed as **ONE undoable transaction** through the real `Engine` (Loro +
//! the commit pipeline) — exactly the path a `carve` command on the `.exe` drives. It also exercises the
//! **destructible-wall** workload (carve the result again) staying watertight. An integrated break (a carve
//! that cracks, a result not content-addressed, a placement Ctrl-Z can't peel) fails it in CI, not only on
//! the `.exe`.

use metrocalk_core::{Engine, FieldValue};
use metrocalk_ecs::FlecsWorld;

use metrocalk_assets::AssetStore;
use metrocalk_csg::{box_mesh, validate, BoolOp};
use metrocalk_editor_shell::capscene::{CapResolver, CapScene};
use metrocalk_editor_shell::csg_intent::{carve, mesh_asset_to_trimesh, parse_op, store_mesh};

fn engine() -> (Engine<FlecsWorld>, CapScene) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    engine.set_capability_resolver(Box::new(CapResolver::from_scene(&scene)));
    (engine, scene)
}

/// Store a box as a `MeshAsset` under its content address; return the handle (the input-by-handle path).
fn store_box(store: &mut AssetStore, center: [f64; 3], half: [f64; 3]) -> String {
    store_mesh(store, &box_mesh(center, half), "box")
}

#[test]
fn a_carve_is_content_addressed_and_placed_as_one_undoable_transaction() {
    let (mut engine, scene) = engine();
    let mut store = AssetStore::new();

    // Inputs referenced BY HANDLE (invariant 2): a wall + a carve box (top face coplanar with the wall).
    let wall = store_box(&mut store, [0.0, 0.0, 0.0], [2.0, 1.0, 0.5]);
    let cut = store_box(&mut store, [0.0, 0.5, 0.0], [0.5, 0.5, 1.0]);
    let before = store.len();

    // The carve: robust boolean -> a content-addressed, validator-clean result handle.
    let handle = carve(&mut store, BoolOp::Difference, &wall, &cut).expect("clean carve");
    assert!(
        store.contains(&handle),
        "the carved mesh is content-addressed in the store"
    );
    assert_eq!(store.len(), before + 1, "exactly one new asset");
    let r = validate(&mesh_asset_to_trimesh(store.get_str(&handle).unwrap()));
    assert!(
        r.watertight && r.oriented,
        "the carved result is watertight: {}",
        r.explain()
    );

    // Place it as ONE undoable transaction (the place_mesh op-set; references the result BY HANDLE).
    assert_eq!(engine.entity_count(), 0);
    let id =
        metrocalk_editor_shell::capscene::place_mesh(&mut engine, &scene, &handle, [0.0, 0.0, 0.0])
            .expect("place the carved mesh");
    assert_eq!(engine.entity_count(), 1, "the carved entity exists");

    // The handle landed on the entity's MeshRenderer.mesh field (geometry stays by handle, not in Loro).
    let mesh_field = engine.get_field(id, "MeshRenderer", "mesh");
    assert_eq!(
        mesh_field,
        Some(FieldValue::Str(handle.clone())),
        "the entity references the carve by handle"
    );

    // Ctrl-Z peels the placement as ONE step.
    assert!(engine.can_undo());
    assert!(engine.undo(), "undo the carve placement");
    assert_eq!(
        engine.entity_count(),
        0,
        "one Ctrl-Z removed the carved entity"
    );
}

#[test]
fn a_carve_reloads_to_the_same_handle_content_addressed_determinism() {
    // Two independent "sessions": the same inputs reproduce the SAME content-addressed handle, so a carved
    // mesh re-resolves after a reload (deterministic id space, ADR-013/014).
    let mut s1 = AssetStore::new();
    let a1 = store_box(&mut s1, [0.0, 0.0, 0.0], [2.0, 1.0, 0.5]);
    let b1 = store_box(&mut s1, [0.3, 0.2, 0.0], [0.4, 0.4, 1.0]);
    let h1 = carve(&mut s1, BoolOp::Difference, &a1, &b1).unwrap();

    let mut s2 = AssetStore::new();
    let a2 = store_box(&mut s2, [0.0, 0.0, 0.0], [2.0, 1.0, 0.5]);
    let b2 = store_box(&mut s2, [0.3, 0.2, 0.0], [0.4, 0.4, 1.0]);
    let h2 = carve(&mut s2, BoolOp::Difference, &a2, &b2).unwrap();

    assert_eq!(
        h1, h2,
        "a carve re-resolves to the same handle after a reload"
    );
}

#[test]
fn the_destructible_wall_stays_watertight_when_the_result_is_carved_again() {
    let mut store = AssetStore::new();
    let wall = store_box(&mut store, [0.0, 0.0, 0.0], [3.0, 1.5, 0.5]);

    // Carve the wall, then carve the RESULT mesh again (the destructible workload), all by handle.
    let cuts = [
        ([-2.2, 0.5, 0.0], [0.4, 0.4, 1.0]),
        ([0.3, 0.2, 0.0], [0.3, 0.3, 2.0]),
        ([1.6, 0.4, 0.0], [0.4, 0.4, 1.0]),
    ];
    let mut current = wall;
    for (c, h) in cuts {
        let cut = store_box(&mut store, c, h);
        current = carve(&mut store, BoolOp::Difference, &current, &cut).expect("carve stays clean");
        let r = validate(&mesh_asset_to_trimesh(store.get_str(&current).unwrap()));
        assert!(
            r.watertight && r.oriented,
            "the destructible result stays watertight: {}",
            r.explain()
        );
    }
}

#[test]
fn the_carve_command_parses_its_verb() {
    assert_eq!(parse_op("carve"), Some(BoolOp::Difference));
    assert_eq!(parse_op("union"), Some(BoolOp::Union));
    assert_eq!(parse_op("nonsense"), None);
}
