//! M11.4 (ADR-043) — scene cameras are ENTITIES. `capscene::add_camera` is ONE undoable commit writing a
//! `Camera` component (fov/near/far + `active`), removable by undo, surviving close→reopen via the AddCamera
//! replay record. The look-through view-proj is a render PROJECTION (never Loro), so the doc carries only the
//! authored camera ENTITY + its component — which is exactly what these assert.

use std::path::PathBuf;

use metrocalk_core::{Engine, FieldValue};
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

fn camera_count(engine: &Engine<FlecsWorld>) -> usize {
    engine
        .entity_ids()
        .iter()
        .filter(|id| engine.components_of(**id).contains_key("Camera"))
        .count()
}

#[test]
fn add_camera_is_one_undoable_commit_writing_a_camera_component() {
    let (mut e, scene) = seeded();
    let before = e.entity_count();
    let id =
        capscene::add_camera(&mut e, &scene, [10.0, 4.0, 0.0], 50.0, true).expect("add a camera");

    assert_eq!(
        e.entity_count(),
        before + 1,
        "exactly one new camera entity"
    );
    let comps = e.components_of(id);
    let cam = comps
        .get("Camera")
        .expect("the entity has a Camera component");
    assert_eq!(cam.get("fov"), Some(&FieldValue::Number(50.0)));
    assert_eq!(cam.get("active"), Some(&FieldValue::Bool(true)));

    // One undoable transaction — Ctrl-Z removes the whole camera.
    e.undo();
    assert_eq!(e.entity_count(), before, "undo removed the camera entity");
    assert_eq!(
        camera_count(&e),
        0,
        "no Camera components linger after undo"
    );
}

#[test]
fn active_camera_returns_the_authored_pose_and_fov() {
    let (mut e, scene) = seeded();
    assert!(
        capscene::active_camera(&e).is_none(),
        "no camera → none active"
    );
    capscene::add_camera(&mut e, &scene, [12.0, 3.0, -5.0], 60.0, true).expect("add");
    let (pos, fov, _near, _far) = capscene::active_camera(&e).expect("an active camera");
    assert!(
        (pos[0] - 12.0).abs() < 1e-4
            && (pos[1] - 3.0).abs() < 1e-4
            && (pos[2] - (-5.0)).abs() < 1e-4,
        "the authored position drives look-through: got {pos:?}"
    );
    assert!((fov - 60.0).abs() < 1e-4, "the authored fov");
}

#[test]
fn an_explicitly_inactive_camera_is_not_picked() {
    let (mut e, scene) = seeded();
    capscene::add_camera(&mut e, &scene, [1.0, 1.0, 1.0], 50.0, false).expect("add inactive");
    assert!(
        capscene::active_camera(&e).is_none(),
        "an explicitly-inactive camera is never the active one"
    );
}

#[test]
fn a_camera_survives_close_then_reopen_via_replay() {
    let log = Log::open(tmp("camera"), capscene::fingerprint(N));
    log.clear();

    // run A: author an active camera, persist its record.
    let (mut a, scene_a) = seeded();
    capscene::add_camera(&mut a, &scene_a, [8.0, 5.0, 0.0], 55.0, true).expect("add A");
    log.append(&Record::AddCamera {
        pos: [8.0, 5.0, 0.0],
        fov: 55.0,
        active: true,
    });
    assert_eq!(camera_count(&a), 1);
    drop(a); // close

    // run B: fresh deterministic seed + replay (a true close→reopen).
    let (mut b, scene) = seeded();
    let (applied, _skipped) = log.replay(&mut b, &scene, &MeshCatalog::new());
    assert_eq!(applied, 1, "the AddCamera record replayed");
    assert_eq!(
        camera_count(&b),
        1,
        "the authored camera is restored after reopen"
    );
    assert!(
        capscene::active_camera(&b).is_some(),
        "and it is the active camera again"
    );
    log.clear();
}
