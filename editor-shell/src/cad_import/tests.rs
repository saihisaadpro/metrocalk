//! Substrate-native wiring tests: a [`CadImport`] lands as ONE undoable transaction of queryable,
//! units-normalized, deduped renderable entities — never-empty + never-silent on the real op-stream.

use super::*;
use crate::capscene::{CapResolver, CapScene};
use metrocalk_assets::AssetStore;
use metrocalk_core::{Engine, FieldValue};
use metrocalk_ecs::FlecsWorld;
use metrocalk_interchange::{
    build_import, CadInterchange, GroupNode, PartSource, RawPart, StepInterchange, Units,
    IDENTITY_4X4,
};

const CUBE_STEP: &str = include_str!("../../../interchange/tests/fixtures/cube_ap242.step");

fn engine() -> (Engine<FlecsWorld>, CapScene) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    engine.set_capability_resolver(Box::new(CapResolver::from_scene(&scene)));
    (engine, scene)
}

/// A mixed synthetic import: two identical exact bolts (from the cube's faces) at different mm positions +
/// one proprietary CATIA part — exercising exact geometry, dedup, real placement, and the diagnosed proxy.
fn mixed_import() -> CadImport {
    let faces = StepInterchange.import(CUBE_STEP.as_bytes()).unwrap().solids[0]
        .faces
        .clone();
    let mut t_bolt_b = IDENTITY_4X4;
    t_bolt_b[12] = 1000.0; // 1000 mm along x
    let mut t_native = IDENTITY_4X4;
    t_native[13] = 2000.0; // 2000 mm along y
    let parts = vec![
        RawPart {
            id: 1,
            name: "Bolt A".into(),
            reference: "bolt".into(),
            transform: IDENTITY_4X4,
            source: PartSource::ExactBrep(faces.clone()),
            color: None,
            parent: None,
        },
        RawPart {
            id: 2,
            name: "Bolt B".into(),
            reference: "bolt".into(),
            transform: t_bolt_b,
            source: PartSource::ExactBrep(faces),
            color: None,
            parent: None,
        },
        RawPart {
            id: 3,
            name: "Weld Boom Base".into(),
            reference: "native".into(),
            transform: t_native,
            source: PartSource::ProprietaryRep {
                encoding: "CATIA V5_CFV3/CB0001".into(),
            },
            color: None,
            parent: None,
        },
    ];
    // Millimetre units (STEP/3DXML convention) → land_import normalizes to metres.
    build_import(
        "Skid Assembly".into(),
        "TEST".into(),
        Units {
            meters_per_unit: 0.001,
            kilograms_per_unit: 1.0,
        },
        42,
        parts,
        vec![],
        vec![],
    )
}

#[test]
fn a_cad_import_lands_as_one_undoable_transaction_of_queryable_deduped_entities() {
    let (mut engine, scene) = engine();
    let mut store = AssetStore::new();
    assert_eq!(engine.entity_count(), 0);

    let report = mixed_import();
    assert!(report.never_empty() && report.never_silent());
    let landing = land_import(&mut engine, &scene, &mut store, report).expect("land");

    // One entity per part.
    assert_eq!(landing.entities.len(), 3);
    assert_eq!(engine.entity_count(), 3, "3 part entities");

    // DEDUP: two identical bolts share one stored mesh; the proxy is a second → 2 unique meshes for 3 parts.
    assert_eq!(landing.unique_meshes, 2, "bolt mesh + proxy box = 2 unique");

    // QUERYABLE per-part report, ECS-native: fidelity + name + reference are readable off each entity.
    let fid = |e| engine.get_field(e, CAD_PART, "fidelity");
    assert_eq!(
        fid(landing.entities[0]),
        Some(FieldValue::Str("exact-brep".into())),
        "bolt A is exact B-rep"
    );
    assert_eq!(
        fid(landing.entities[2]),
        Some(FieldValue::Str("proxy".into())),
        "the proprietary part is a diagnosed proxy"
    );
    assert_eq!(
        engine.get_field(landing.entities[2], CAD_PART, "name"),
        Some(FieldValue::Str("Weld Boom Base".into()))
    );

    // UNITS-NORMALIZED real placement (mm → m): bolt B at 1000 mm → x = 1.0 m (never the origin-collapse).
    assert_eq!(
        engine.get_field(landing.entities[1], "Transform", "x"),
        Some(FieldValue::Number(1.0)),
        "1000 mm → 1.0 m (units backstop, real placement)"
    );
    assert_eq!(
        engine.get_field(landing.entities[2], "Transform", "y"),
        Some(FieldValue::Number(2.0)),
        "2000 mm → 2.0 m"
    );
    // Geometry shares that metric frame: the mm mesh is scaled by m_per_unit (0.001), so a real mesh renders
    // at true size, not 1000× oversized (the adversarial-review units fix).
    assert_eq!(
        engine.get_field(landing.entities[0], "Transform", "scale"),
        Some(FieldValue::Number(0.001)),
        "mesh scaled mm→m (geometry & placement in one metric frame)"
    );
    // Every part renders a mesh by handle (never-empty — even the proxy).
    assert!(engine
        .get_field(landing.entities[2], "MeshRenderer", MESH_FIELD)
        .is_some());

    // ONE Ctrl-Z peels the whole import.
    assert!(engine.undo(), "undo the import");
    assert_eq!(engine.entity_count(), 0, "one undo removed every part");
}

#[test]
fn read_cad_routes_step_and_lands_exact_geometry() {
    let (mut engine, scene) = engine();
    let mut store = AssetStore::new();
    let landing =
        import_cad(&mut engine, &scene, &mut store, CUBE_STEP.as_bytes()).expect("import");
    assert_eq!(
        landing.entities.len(),
        1,
        "the cube is one solid → one part"
    );
    assert_eq!(
        engine.get_field(landing.entities[0], CAD_PART, "fidelity"),
        Some(FieldValue::Str("exact-brep".into())),
        "STEP planar B-rep lands exact"
    );
    // The mesh is real (content-addressed), stored.
    assert_eq!(landing.unique_meshes, 1);
    assert!(landing.report.never_empty() && landing.report.never_silent());
}

#[test]
fn a_cad_import_preserves_the_source_hierarchy_as_named_group_folders() {
    // (adversarial review) land_import must land the SAME named assembly tree the live app path does —
    // not silently flatten the groups + parents the report now carries.
    let (mut engine, scene) = engine();
    let mut store = AssetStore::new();
    let faces = StepInterchange.import(CUBE_STEP.as_bytes()).unwrap().solids[0]
        .faces
        .clone();
    let groups = vec![
        GroupNode {
            id: 100,
            name: "Skid Line".into(),
            parent: None,
        },
        GroupNode {
            id: 101,
            name: "Robot Cell".into(),
            parent: Some(100),
        },
    ];
    let parts = vec![RawPart {
        id: 1,
        name: "Weld Gun".into(),
        reference: "gun".into(),
        transform: IDENTITY_4X4,
        source: PartSource::ExactBrep(faces),
        color: None,
        parent: Some(101),
    }];
    let report = build_import(
        "nested".into(),
        "TEST".into(),
        Units {
            meters_per_unit: 0.001,
            kilograms_per_unit: 1.0,
        },
        7,
        parts,
        groups,
        vec![],
    );
    let landing = land_import(&mut engine, &scene, &mut store, report).expect("land");
    assert_eq!(landing.group_entities.len(), 2);
    assert_eq!(engine.entity_count(), 3, "2 group folders + 1 part");
    let (line, cell) = (landing.group_entities[0], landing.group_entities[1]);
    let meta = metrocalk_core::variant::INSTANCE_META;
    assert_eq!(
        engine.get_field(line, meta, "name"),
        Some(FieldValue::Str("Skid Line".into()))
    );
    assert_eq!(
        engine.get_field(cell, meta, "kind"),
        Some(FieldValue::Str("group".into())),
        "folders are marked group (the outliner's folder icon)"
    );
    // The source nesting survives: gun › cell › line.
    assert_eq!(engine.parent_of(landing.entities[0]), Some(cell));
    assert_eq!(engine.parent_of(cell), Some(line));
    assert_eq!(engine.parent_of(line), None);
    // Still ONE undoable transaction — the tree peels with the parts.
    assert!(engine.undo());
    assert_eq!(engine.entity_count(), 0);
}

#[test]
fn persisted_cad_meshes_reload_under_the_same_handle() {
    // (adversarial review) the derived-mesh sidecar: what the live import persists must reload under the
    // SAME handle with the same geometry — the guarantee that a saved doc's `mtkcad:` field re-resolves
    // after restart instead of silently degrading to a placeholder cube.
    let dir = std::env::temp_dir().join(format!("mtk-cad-mesh-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mesh = metrocalk_csg::TriMesh::new(
        vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]],
        vec![[0, 1, 2]],
    );
    persist_cad_mesh(
        &dir,
        "mtkcad:00000000deadbeef:ff0000",
        &mesh,
        Some([1.0, 0.0, 0.0]),
    )
    .expect("persist");
    let loaded = load_persisted_cad_meshes(&dir);
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].0, "mtkcad:00000000deadbeef:ff0000");
    let extent = loaded[0].1.bounds().max_extent();
    assert!(
        (extent - 10.0).abs() < 1e-6,
        "the restored asset carries the same geometry (max extent {extent}, expected 10.0)"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_cad_rejects_an_unrecognized_container_explained() {
    assert!(read_cad(b"not a cad file at all").is_err());
}

#[test]
fn reimport_of_the_same_file_is_all_unchanged_not_a_retessellation() {
    let a = read_cad(CUBE_STEP.as_bytes()).unwrap();
    let b = read_cad(CUBE_STEP.as_bytes()).unwrap();
    let d = reimport_diff(&a, &b);
    assert_eq!(
        changed_count(&d),
        0,
        "re-importing the same file → 0 parts changed"
    );
}
