//! M11.3 (ADR-042) — lights are ENTITIES. `capscene::add_light` is ONE undoable commit that writes a
//! `Light` component (authored Loro doc state, like any component), it's removable by undo, and it survives
//! close→reopen via the `AddLight` replay record. The per-frame LIT RESULT (the lights buffer the shader
//! loops over) is a render PROJECTION (SceneState, regenerated each rebuild) — never doc state. So the
//! engine document carries ONLY the light ENTITY + its component, which is exactly what these assert.

use std::path::PathBuf;

use metrocalk_core::{Engine, EntityId, FieldValue};
use metrocalk_ecs::FlecsWorld;

use metrocalk_editor_shell::capscene::{self, CapScene};
use metrocalk_editor_shell::persist::{Log, Record};
use metrocalk_editor_shell::MeshCatalog;

const N: usize = 50;

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("mtk-test-{name}.jsonl"))
}

fn seeded() -> (Engine<FlecsWorld>, CapScene) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    capscene::seed(&mut engine, &scene, N).expect("seed");
    engine.clear_history();
    (engine, scene)
}

fn light_field<'a>(engine: &'a Engine<FlecsWorld>, id: EntityId, f: &str) -> Option<FieldValue> {
    engine
        .components_of(id)
        .get("Light")
        .and_then(|m| m.get(f).cloned())
}

fn light_count(engine: &Engine<FlecsWorld>) -> usize {
    engine
        .entity_ids()
        .iter()
        .filter(|id| engine.components_of(**id).contains_key("Light"))
        .count()
}

#[test]
fn add_light_is_one_undoable_commit_writing_a_light_component() {
    let (mut e, scene) = seeded();
    let before = e.entity_count();
    let id = capscene::add_light(
        &mut e,
        &scene,
        "point",
        [0.0, 4.0, 0.0],
        [1.0, 0.9, 0.8],
        60.0,
    )
    .expect("add a light");

    assert_eq!(e.entity_count(), before + 1, "exactly one new light entity");
    assert_eq!(
        light_field(&e, id, "kind"),
        Some(FieldValue::Str("point".into()))
    );
    assert_eq!(
        light_field(&e, id, "intensity"),
        Some(FieldValue::Number(60.0))
    );
    // The colour is authored doc state on the component (the render reads it as a projection).
    assert_eq!(light_field(&e, id, "r"), Some(FieldValue::Number(1.0)));

    // One undoable transaction — Ctrl-Z removes the whole light.
    e.undo();
    assert_eq!(e.entity_count(), before, "undo removed the light entity");
    assert_eq!(light_count(&e), 0, "no Light components linger after undo");
}

#[test]
fn a_light_survives_close_then_reopen_via_replay() {
    let log = Log::open(tmp("lighting"), capscene::fingerprint(N));
    log.clear();

    // run A: author a directional light, persist its record.
    let (mut a, scene_a) = seeded();
    capscene::add_light(
        &mut a,
        &scene_a,
        "directional",
        [0.0, 10.0, 0.0],
        [1.0, 1.0, 1.0],
        3.0,
    )
    .expect("add A");
    log.append(&Record::AddLight {
        light_kind: "directional".into(),
        pos: [0.0, 10.0, 0.0],
        color: [1.0, 1.0, 1.0],
        intensity: 3.0,
    });
    assert_eq!(light_count(&a), 1);
    drop(a); // close

    // run B: fresh deterministic seed + replay (a true close→reopen).
    let (mut b, scene) = seeded();
    let (applied, _skipped) = log.replay(&mut b, &scene, &MeshCatalog::new());
    assert_eq!(applied, 1, "the AddLight record replayed");
    assert_eq!(
        light_count(&b),
        1,
        "the authored light is restored after reopen"
    );
    log.clear();
}
