//! M4 asset pipeline — the headless acceptance: a real **checked-in glTF** imports through the trait,
//! an entity carrying its `Mesh` handle is **placed through the commit pipeline**, **undo** reverses
//! it, and **export→replay restores** it (mirroring `north_star_*` + `persistence.rs`). This is the
//! mechanism the success criteria name, verified without a GPU: the *render* of the mesh is proven
//! live in the shell, but the asset → ECS-handle → undoable → reload-persistent path is proven here.
//!
//! The handle that lands in the ECS / Loro doc is the asset's content address (a lightweight string) —
//! geometry never enters the doc (invariants 1 & 2). The store reloads (re-import) to the same handle
//! (content-addressed), so a placed mesh survives close→reopen (ADR-013 id determinism).

#![allow(clippy::cast_precision_loss)]

use std::path::PathBuf;

use metrocalk_assets::{AssetStore, Bounds, GltfImporter, MeshGpu, MeshSource};
use metrocalk_core::{Engine, FieldValue};
use metrocalk_ecs::FlecsWorld;

use metrocalk_editor_shell::capscene::{self, CapScene};
use metrocalk_editor_shell::persist::{Log, Record};
use metrocalk_editor_shell::{MeshCatalog, MESH_FIELD};

const N: usize = 200;

/// The same checked-in glTF the shell imports at startup — proving "import a checked-in glTF".
const HEALTHBAR_GLB: &[u8] = include_bytes!("../assets/healthbar.glb");

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("metrocalk-{name}.jsonl"))
}

fn seeded() -> (Engine<FlecsWorld>, CapScene) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    capscene::seed(&mut engine, &scene, N).expect("seed");
    engine.clear_history();
    (engine, scene)
}

/// Read an entity's `MeshRenderer.mesh` handle, if any.
fn mesh_handle(engine: &Engine<FlecsWorld>, id: metrocalk_core::EntityId) -> Option<String> {
    match engine
        .components_of(id)
        .get("MeshRenderer")?
        .get(MESH_FIELD)?
    {
        FieldValue::Str(s) => Some(s.clone()),
        _ => None,
    }
}

#[test]
fn import_asserts_internal_mesh_geometry_and_bounds() {
    let asset = GltfImporter::new()
        .import(HEALTHBAR_GLB)
        .expect("import the checked-in healthbar.glb");
    // The internal mesh: a two-box framed bar → 48 verts / 72 indices (24 triangles).
    assert_eq!(asset.vertex_count(), 48);
    assert_eq!(asset.index_count(), 72);
    assert_eq!(asset.triangle_count(), 24);
    // Bounds are bar-shaped (wide + short), not a unit cube — i.e. the geometry is really the asset's.
    let Bounds { min, max } = asset.bounds();
    assert!((max[0] - min[0]) > 2.0, "bar is wide");
    assert!((max[1] - min[1]) < 0.6, "bar is short");
    // It packs to GPU-ready geometry with in-range indices.
    let gpu = MeshGpu::from_asset(&asset);
    assert_eq!(gpu.vertex_count(), 48);
    assert!(gpu
        .indices
        .iter()
        .all(|&i| (i as usize) < gpu.vertices.len()));
}

#[test]
fn place_mesh_is_one_undoable_transaction_carrying_only_the_handle() {
    let (mut engine, scene) = seeded();
    let mut store = AssetStore::new();
    let handle = store
        .import(&GltfImporter::new(), HEALTHBAR_GLB)
        .expect("store import");
    let before = engine.entity_count();

    let id = capscene::place_mesh(&mut engine, &scene, handle.as_str(), [2.0, 0.0, 0.0])
        .expect("place commits");
    assert_eq!(engine.entity_count(), before + 1, "one entity created");
    // The ECS carries ONLY the lightweight handle — never geometry (invariant 2).
    assert_eq!(
        mesh_handle(&engine, id).as_deref(),
        Some(handle.as_str()),
        "the entity references the asset by handle"
    );
    // The store resolves the handle back to real geometry (held beside the doc, not in it).
    assert!(store.get_str(handle.as_str()).is_some());

    // ONE undoable transaction: a single Ctrl-Z removes the whole placement.
    assert!(engine.undo());
    assert!(
        !engine.entity_exists(id),
        "undo reverses the placement atomically"
    );
    assert_eq!(engine.entity_count(), before);
}

#[test]
fn placed_mesh_survives_export_then_replay() {
    let log = Log::open(tmp("mesh-assets"), capscene::fingerprint(N));
    log.clear();

    // The handle is content-addressed, so it is identical across runs (the store re-imports the same
    // checked-in bytes) — which is exactly why a persisted handle re-resolves after reload.
    let handle = AssetStore::new()
        .import(&GltfImporter::new(), HEALTHBAR_GLB)
        .expect("import")
        .as_str()
        .to_string();

    // run A: place a mesh, persist the placement record.
    let (mut a, scene_a) = seeded();
    let placed = capscene::place_mesh(&mut a, &scene_a, &handle, [3.0, 1.0, 0.0]).expect("place A");
    log.append(&Record::PlaceMesh {
        asset: handle.clone(),
        pos: [3.0, 1.0, 0.0],
    });
    assert_eq!(mesh_handle(&a, placed).as_deref(), Some(handle.as_str()));
    drop(a); // close

    // run B: fresh deterministic seed + replay (a true close→reopen). The catalog is empty here — the
    // PlaceMesh record carries its own handle, so it doesn't need one.
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut b = Engine::new(world, 1);
    capscene::seed(&mut b, &scene, N).expect("re-seed");
    b.clear_history();
    let (applied, skipped) = log.replay(&mut b, &scene, &MeshCatalog::new());
    b.clear_history();

    assert_eq!((applied, skipped), (1, 0), "the placement replayed");
    assert!(b.entity_exists(placed), "the placed mesh survived reload");
    assert_eq!(
        mesh_handle(&b, placed).as_deref(),
        Some(handle.as_str()),
        "and it still references the same asset handle (re-resolves against the reloaded store)"
    );

    log.clear();
}

#[test]
fn describe_create_attaches_the_catalog_mesh_else_falls_back() {
    // Deliverable 4: a resolved kind WITH an associated mesh asset instantiates carrying its handle
    // (so the pre-componentized object now also *looks* right); a kind with no asset → no handle (the
    // renderer's honest cube fallback).
    let (mut engine, scene) = seeded();
    let handle = AssetStore::new()
        .import(&GltfImporter::new(), HEALTHBAR_GLB)
        .expect("import")
        .as_str()
        .to_string();
    let catalog: MeshCatalog = [("HealthBar".to_string(), handle.clone())]
        .into_iter()
        .collect();

    // "health bar" resolves to HealthBar, which the catalog maps to the bar mesh.
    let (with_mesh, kind) =
        capscene::describe_create(&mut engine, &scene, "health bar", [0.0; 3], &catalog)
            .expect("resolves");
    assert_eq!(kind, "HealthBar");
    assert_eq!(
        mesh_handle(&engine, with_mesh).as_deref(),
        Some(handle.as_str()),
        "the described object carries its mesh handle → renders as the mesh, not a cube"
    );

    // Same resolve, but an empty catalog → no mesh handle → the honest placeholder-cube fallback.
    let (no_mesh, _) = capscene::describe_create(
        &mut engine,
        &scene,
        "health bar",
        [5.0, 0.0, 0.0],
        &MeshCatalog::new(),
    )
    .expect("resolves");
    assert!(
        mesh_handle(&engine, no_mesh).is_none(),
        "a kind with no asset honestly has no mesh handle"
    );
}

#[test]
fn unknown_handle_is_an_honest_placement_the_renderer_falls_back_for() {
    // Placing a handle the store doesn't know still commits (the ECS is handle-authoritative); the
    // renderer's cube fallback (slot lookup misses) keeps it visible + honest, never a hard error.
    let (mut engine, scene) = seeded();
    let id = capscene::place_mesh(&mut engine, &scene, "mtkasset:deadbeef", [0.0; 3])
        .expect("placement still commits");
    assert_eq!(
        mesh_handle(&engine, id).as_deref(),
        Some("mtkasset:deadbeef")
    );
}
