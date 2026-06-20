//! M6 generation tier — the headless e2e with the deterministic **fake** provider (no network), through
//! the **real** `/core` engine. Proves the last tier of `local → marketplace → generate`: a description
//! that matches nothing offers Generate → a grey placeholder commits instantly (undoable) → the fake
//! generator returns bytes → they import through the prompt-23 pipeline → the real mesh streams in as a
//! **validated AI patch** → reveal offers the attach → undo peels the swap then the placeholder →
//! export→replay survives reload. Plus the tier-order + offline-degradation guards.

#![allow(clippy::cast_precision_loss)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use metrocalk_assets::{demo, AssetStore, GltfImporter};
use metrocalk_core::marketplace::LocalCatalog;
use metrocalk_core::{resolve, stdlib, Engine, EntityId, FieldValue, Resolved};
use metrocalk_ecs::{Entity, FlecsWorld};

use metrocalk_editor_shell::ai::{apply_ai_patch, AiPatch, PatchOp};
use metrocalk_editor_shell::capscene::{self, CapScene};
use metrocalk_editor_shell::generate::{
    FakeGenerator, GenError, GenRequest, MeshGenerator, MeterAction, StubMeter, TokenMeter,
};
use metrocalk_editor_shell::persist::{Log, Record};
use metrocalk_editor_shell::reveal::{reveal, Context};

const N: usize = 200;

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

fn mesh_handle(engine: &Engine<FlecsWorld>, id: EntityId) -> Option<FieldValue> {
    engine
        .components_of(id)
        .get("MeshRenderer")
        .and_then(|m| m.get("mesh").cloned())
}

fn swap_patch(id: EntityId, handle: &str) -> AiPatch {
    AiPatch {
        client_op_id: "gen-stream-in".into(),
        ops: vec![PatchOp::SetField {
            id: id.to_loro_key(),
            component: "MeshRenderer".into(),
            field: "mesh".into(),
            value: serde_json::Value::String(handle.to_string()),
        }],
    }
}

#[test]
fn generate_is_tier_3_reached_only_when_nothing_local_or_marketplace_matches() {
    let lib = stdlib::standard_components();
    let index = LocalCatalog::builtin();
    assert!(
        matches!(resolve(&lib, &index, "health bar"), Resolved::Local(_)),
        "local hit never generates"
    );
    assert!(
        matches!(
            resolve(&lib, &index, "rusty medieval sword"),
            Resolved::Marketplace(_)
        ),
        "marketplace hit never generates"
    );
    assert!(
        matches!(
            resolve(&lib, &index, "quux blorbo widget nonsense"),
            Resolved::Generate
        ),
        "only a no-anywhere-match offers generate"
    );
}

#[test]
fn placeholder_first_stream_in_reveal_undo() {
    let (mut engine, scene) = seeded();
    let lib = stdlib::standard_components();

    // Nothing matches → the generate tier.
    assert!(matches!(
        resolve(&lib, &LocalCatalog::builtin(), "quux blorbo widget"),
        Resolved::Generate
    ));

    // Meter the action (seam — records ≈10 tokens, no money moves).
    assert_eq!(
        StubMeter.charge(MeterAction::Generate, "gen 'quux'"),
        Ok(10)
    );

    // Placeholder drops in instantly as ONE undoable transaction — a grey cube (empty mesh handle),
    // bindable at once (requires Spatial).
    let ph =
        capscene::place_generation_placeholder(&mut engine, &scene, [0.0; 3]).expect("placeholder");
    assert_eq!(
        mesh_handle(&engine, ph),
        Some(FieldValue::Str(String::new())),
        "grey placeholder = empty mesh"
    );

    // Fake generator returns bytes → they IMPORT through the prompt-23 pipeline → a content-addressed handle.
    let gen = FakeGenerator::new(demo::prop_glb(), Duration::ZERO, true);
    let bytes = gen
        .generate(&GenRequest::new("quux blorbo widget"))
        .expect("fake generates");
    let mut store = AssetStore::new();
    let handle = store
        .import(&GltfImporter::new(), &bytes)
        .expect("import the generated mesh");

    // The real mesh streams in as a VALIDATED AI patch (inv. 3) — same entity, same id, swapped handle.
    let delta = apply_ai_patch(
        &mut engine,
        &lib,
        "generate-stream-in",
        &swap_patch(ph, handle.as_str()),
    );
    assert!(
        delta.rejects.is_empty(),
        "the stream-in patch is schema-valid"
    );
    assert_eq!(
        mesh_handle(&engine, ph),
        Some(FieldValue::Str(handle.as_str().to_string())),
        "the generated mesh handle replaced the placeholder"
    );

    // It's a working object — the reveal offers a compatible attach (requires Spatial; the seed has them).
    let pos = capscene::positions(&engine);
    let rec: HashMap<Entity, u64> = HashMap::new();
    let ctx = Context {
        cap_name: &scene.cap_name,
        position: &pos,
        recency: &rec,
    };
    let r = reveal(
        engine.world(),
        engine.ecs_entity(ph).unwrap(),
        scene.rels,
        &ctx,
    );
    assert!(!r.compatible.is_empty(), "the generated object is bindable");

    // Undo peels the stream-in (back to the grey placeholder), then the placeholder (entity gone).
    assert!(engine.undo());
    assert_eq!(
        mesh_handle(&engine, ph),
        Some(FieldValue::Str(String::new())),
        "undo peels the stream-in to the placeholder"
    );
    assert!(engine.undo());
    assert!(
        !engine.entity_exists(ph),
        "another undo removes the placeholder — the whole generation peels"
    );
}

#[test]
fn generation_survives_export_then_replay() {
    let log = Log::open(tmp("generate"), capscene::fingerprint(N));
    log.clear();
    let handle = AssetStore::new()
        .import(&GltfImporter::new(), &demo::prop_glb())
        .expect("import")
        .as_str()
        .to_string();

    // run A: placeholder + stream-in, persisted as one Generate record (the completed generation).
    let (mut a, scene_a) = seeded();
    let lib = stdlib::standard_components();
    let ph = capscene::place_generation_placeholder(&mut a, &scene_a, [1.0, 0.0, 0.0]).unwrap();
    apply_ai_patch(&mut a, &lib, "stream-in", &swap_patch(ph, &handle));
    log.append(&Record::Generate {
        prompt: "quux blorbo widget".into(),
        pos: [1.0, 0.0, 0.0],
        mesh: Some(handle.clone()),
    });
    drop(a); // close

    // run B: fresh deterministic seed + replay (a true close→reopen).
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut b = Engine::new(world, 1);
    capscene::seed(&mut b, &scene, N).expect("re-seed");
    b.clear_history();
    let (applied, skipped) = log.replay(&mut b, &scene, &HashMap::new());
    b.clear_history();

    assert_eq!((applied, skipped), (1, 0), "the generation replayed");
    assert!(
        b.entity_exists(ph),
        "the generated entity survived reload at the same deterministic id"
    );
    assert_eq!(
        mesh_handle(&b, ph),
        Some(FieldValue::Str(handle)),
        "with its generated mesh handle (re-applied as a validated patch on replay)"
    );
    log.clear();
}

#[test]
fn offline_generation_degrades_honestly_without_touching_the_offline_path() {
    // Provider switched off (offline) → an honest Unavailable seam, never a fake asset.
    let gen = FakeGenerator::new(demo::prop_glb(), Duration::ZERO, false);
    assert!(!gen.available());
    assert!(matches!(
        gen.generate(&GenRequest::new("x")),
        Err(GenError::Unavailable(_))
    ));

    // The offline happy path is unaffected — local resolution still works with generation off.
    let lib = stdlib::standard_components();
    let index = LocalCatalog::builtin();
    assert!(matches!(
        resolve(&lib, &index, "health bar"),
        Resolved::Local(_)
    ));

    // And the placeholder is still a real, usable object even if generation never returns.
    let (mut engine, scene) = seeded();
    let ph = capscene::place_generation_placeholder(&mut engine, &scene, [0.0; 3]).unwrap();
    assert!(
        engine.entity_exists(ph),
        "the grey placeholder stands on its own"
    );
}
