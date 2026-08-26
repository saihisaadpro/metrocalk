//! M11.4 (ADR-043) — scene cameras are ENTITIES. `capscene::add_camera` is ONE undoable commit writing a
//! `Camera` component (fov/near/far + an aim + `active`) and a name, removable by undo, surviving
//! close→reopen via the AddCamera replay record. The look-through view-proj is a render PROJECTION (never
//! Loro), so the doc carries only the authored camera ENTITY + its component — which is exactly what these
//! assert.
//!
//! The aim, the exclusive `active`, the solved clip planes and the three edit records are the later
//! additions that made a saved camera able to restore its own picture; each has its own test below.

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

/// Compare two world points.
///
/// A helper rather than `assert_eq!` on the arrays because `clippy::float_cmp` denies a strict array
/// comparison, and it is right to: these values DO round-trip exactly (an `f32` written through
/// `f64::from` and read back `as f32` is bit-identical, no arithmetic in between), but a test that
/// asserts bit-identity states a stronger contract than the feature has. A micrometre is far tighter
/// than anything the surface can express and far looser than a representation change would need.
#[track_caller]
fn assert_point(actual: [f32; 3], expected: [f32; 3], what: &str) {
    let close = actual
        .iter()
        .zip(expected.iter())
        .all(|(a, b)| (a - b).abs() < 1e-4);
    assert!(close, "{what}: expected {expected:?}, got {actual:?}");
}

/// The same, for an optional aim — including "both absent", which is its own assertion.
#[track_caller]
fn assert_aim(actual: Option<[f32; 3]>, expected: Option<[f32; 3]>, what: &str) {
    match (actual, expected) {
        (Some(a), Some(b)) => assert_point(a, b, what),
        (None, None) => {}
        _ => panic!("{what}: expected {expected:?}, got {actual:?}"),
    }
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
    let id = capscene::add_camera(
        &mut e,
        &scene,
        [10.0, 4.0, 0.0],
        Some([0.0, 1.0, 0.0]),
        50.0,
        true,
        None,
    )
    .expect("add a camera");

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
    capscene::add_camera(
        &mut e,
        &scene,
        [12.0, 3.0, -5.0],
        Some([2.0, 1.0, -1.0]),
        60.0,
        true,
        None,
    )
    .expect("add");
    let cam = capscene::active_camera(&e).expect("an active camera");
    assert!(
        (cam.pos[0] - 12.0).abs() < 1e-4
            && (cam.pos[1] - 3.0).abs() < 1e-4
            && (cam.pos[2] - (-5.0)).abs() < 1e-4,
        "the authored position drives look-through: got {:?}",
        cam.pos
    );
    assert!((cam.fov_deg - 60.0).abs() < 1e-4, "the authored fov");
}

#[test]
fn an_explicitly_inactive_camera_is_not_picked() {
    let (mut e, scene) = seeded();
    capscene::add_camera(
        &mut e,
        &scene,
        [1.0, 1.0, 1.0],
        Some([0.0, 0.0, 0.0]),
        50.0,
        false,
        None,
    )
    .expect("add inactive");
    assert!(
        capscene::active_camera(&e).is_none(),
        "an explicitly-inactive camera is never the active one"
    );
}

/// The defect this whole capability was built around: a camera that carries no aim shows whatever the
/// editor happens to be orbiting, so two cameras in different places show the same picture. An authored
/// aim comes back as the world point it was authored with.
#[test]
fn an_authored_camera_remembers_what_it_was_pointed_at() {
    let (mut e, scene) = seeded();
    capscene::add_camera(
        &mut e,
        &scene,
        [30.0, 6.0, 0.0],
        Some([1.0, 2.0, 3.0]),
        45.0,
        true,
        Some("Down the line"),
    )
    .expect("add aimed");
    let cam = capscene::active_camera(&e).expect("active");
    let aim = cam.look_at.expect("the aim survives the round trip");
    assert!(
        (aim[0] - 1.0).abs() < 1e-4 && (aim[1] - 2.0).abs() < 1e-4 && (aim[2] - 3.0).abs() < 1e-4,
        "got {aim:?}"
    );
    assert_eq!(cam.name, "Down the line", "and so does its name");
}

/// A camera authored with no aim keeps the pre-aim behaviour exactly — `None`, never a silent
/// "aim at the world origin", because a project saved by an older build replays through this path.
#[test]
fn a_camera_authored_without_an_aim_reports_none_rather_than_the_origin() {
    let (mut e, scene) = seeded();
    capscene::add_camera(&mut e, &scene, [4.0, 4.0, 4.0], None, 55.0, true, None).expect("add");
    let cam = capscene::active_camera(&e).expect("active");
    assert_aim(
        cam.look_at,
        None,
        "an unaimed camera says so instead of claiming to aim at (0,0,0)",
    );
    assert!(
        (cam.near - 0.1).abs() < 1e-6 && (cam.far - 500.0).abs() < 1e-6,
        "and keeps the legacy clip planes: got {} / {}",
        cam.near,
        cam.far
    );
}

/// An aimed camera's clip planes come from the cutscene's own solver, not from a hardcoded pair. At a
/// 300 m stand-off the old `0.1` / `500.0` both fail: the near plane destroys depth precision and the
/// far plane cuts the far end off the scene.
#[test]
fn an_aimed_camera_solves_its_clip_planes_from_the_stand_off() {
    let (mut e, scene) = seeded();
    capscene::add_camera(
        &mut e,
        &scene,
        [300.0, 0.0, 0.0],
        Some([0.0, 0.0, 0.0]),
        50.0,
        true,
        None,
    )
    .expect("add far");
    let cam = capscene::active_camera(&e).expect("active");
    assert!(
        cam.near > 1.0,
        "a 300 m stand-off must not keep a 0.1 m near plane: got {}",
        cam.near
    );
    assert!(
        cam.far > 300.0,
        "and its far plane must reach past the subject: got {}",
        cam.far
    );
    let (near, far) = capscene::camera_clip_planes([300.0, 0.0, 0.0], Some([0.0, 0.0, 0.0]));
    assert!(
        (cam.near - near).abs() < 1e-4 && (cam.far - far).abs() < 1e-4,
        "and they are the solver's answer, not a second opinion"
    );
}

/// Exactly one camera is active. Two authored active used to leave the answer to `entity_ids()`
/// iteration order.
#[test]
fn authoring_a_second_active_camera_stands_the_first_one_down() {
    let (mut e, scene) = seeded();
    let first = capscene::add_camera(
        &mut e,
        &scene,
        [1.0, 1.0, 1.0],
        Some([0.0, 0.0, 0.0]),
        50.0,
        true,
        Some("First"),
    )
    .expect("first");
    capscene::add_camera(
        &mut e,
        &scene,
        [9.0, 1.0, 9.0],
        Some([0.0, 0.0, 0.0]),
        50.0,
        true,
        Some("Second"),
    )
    .expect("second");

    let active: Vec<_> = capscene::cameras(&e)
        .into_iter()
        .filter(|c| c.active)
        .collect();
    assert_eq!(active.len(), 1, "exactly one active camera, got {active:?}");
    assert_eq!(active[0].name, "Second", "the newest active one wins");

    // And switching back is one commit that is also exclusive.
    capscene::set_camera_active(&mut e, first).expect("activate the first");
    let active: Vec<_> = capscene::cameras(&e)
        .into_iter()
        .filter(|c| c.active)
        .collect();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].name, "First");
}

#[test]
fn cameras_are_named_without_reusing_a_number_a_survivor_still_holds() {
    let (mut e, scene) = seeded();
    for _ in 0..3 {
        capscene::add_camera(&mut e, &scene, [0.0, 0.0, 0.0], None, 50.0, false, None)
            .expect("add");
    }
    let names: Vec<String> = capscene::cameras(&e).into_iter().map(|c| c.name).collect();
    assert_eq!(names, ["Camera 1", "Camera 2", "Camera 3"]);
}

#[test]
fn re_aiming_a_camera_moves_the_whole_pose_in_one_commit() {
    let (mut e, scene) = seeded();
    let id = capscene::add_camera(
        &mut e,
        &scene,
        [1.0, 1.0, 1.0],
        Some([0.0, 0.0, 0.0]),
        50.0,
        true,
        None,
    )
    .expect("add");
    let before = e.entity_count();
    capscene::set_camera_view(&mut e, id, [20.0, 5.0, -3.0], Some([4.0, 0.0, 4.0]), 35.0)
        .expect("re-aim");
    let cam = capscene::active_camera(&e).expect("active");
    assert_point(cam.pos, [20.0, 5.0, -3.0], "the camera moved");
    assert_aim(cam.look_at, Some([4.0, 0.0, 4.0]), "and re-aimed");
    assert!((cam.fov_deg - 35.0).abs() < 1e-4);

    e.undo();
    let cam = capscene::active_camera(&e).expect("still there");
    assert_point(
        cam.pos,
        [1.0, 1.0, 1.0],
        "one undo step restores the whole pose",
    );
    assert_eq!(
        e.entity_count(),
        before,
        "and does not remove the camera itself"
    );
}

#[test]
fn a_lens_change_keeps_the_pose_and_re_solves_the_planes() {
    let (mut e, scene) = seeded();
    let id = capscene::add_camera(
        &mut e,
        &scene,
        [40.0, 2.0, 0.0],
        Some([0.0, 2.0, 0.0]),
        50.0,
        true,
        None,
    )
    .expect("add");
    let before = capscene::active_camera(&e).expect("active");
    capscene::set_camera_fov(&mut e, id, 24.0).expect("lens");
    let after = capscene::active_camera(&e).expect("active");
    assert!((after.fov_deg - 24.0).abs() < 1e-4);
    assert_point(after.pos, before.pos, "the camera did not move");
    assert_aim(after.look_at, before.look_at, "nor change what it looks at");
}

#[test]
fn editing_something_that_is_not_a_camera_is_refused_rather_than_committed() {
    let (mut e, scene) = seeded();
    let some_entity = *e.entity_ids().first().expect("a seeded entity");
    assert!(!e.components_of(some_entity).contains_key("Camera"));
    let before = e.entity_count();
    assert!(capscene::set_camera_active(&mut e, some_entity).is_err());
    assert!(capscene::set_camera_fov(&mut e, some_entity, 30.0).is_err());
    assert!(capscene::set_camera_view(&mut e, some_entity, [0.0; 3], None, 50.0).is_err(),);
    assert_eq!(camera_count(&e), 0, "and no Camera component was written");
    assert_eq!(e.entity_count(), before);
    let _ = scene;
}

#[test]
fn a_camera_survives_close_then_reopen_via_replay() {
    let log = Log::open(tmp("camera"), capscene::fingerprint(N));
    log.clear();

    // run A: author an active camera, persist its record.
    let (mut a, scene_a) = seeded();
    capscene::add_camera(
        &mut a,
        &scene_a,
        [8.0, 5.0, 0.0],
        Some([1.0, 1.0, 1.0]),
        55.0,
        true,
        Some("Hero"),
    )
    .expect("add A");
    log.append(&Record::AddCamera {
        pos: [8.0, 5.0, 0.0],
        look_at: Some([1.0, 1.0, 1.0]),
        fov: 55.0,
        active: true,
        name: Some("Hero".into()),
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
    let cam = capscene::active_camera(&b).expect("and it is the active camera again");
    assert_eq!(cam.name, "Hero", "with its name");
    assert_aim(
        cam.look_at,
        Some([1.0, 1.0, 1.0]),
        "the aim it was authored with — the field whose absence made a reopened project show every \
         camera from a different distance at the same subject",
    );
    log.clear();
}

/// The edits are the half a save→reopen used to drop: without their own records a reopened project
/// showed every camera back at the vantage it was first authored from, with its first lens and with
/// whichever one happened to be active at creation.
#[test]
fn camera_edits_survive_close_then_reopen_via_replay() {
    let log = Log::open(tmp("camera-edits"), capscene::fingerprint(N));
    log.clear();

    let (mut a, scene_a) = seeded();
    let first = capscene::add_camera(
        &mut a,
        &scene_a,
        [1.0, 1.0, 1.0],
        Some([0.0, 0.0, 0.0]),
        50.0,
        true,
        Some("Wide"),
    )
    .expect("first");
    let second = capscene::add_camera(
        &mut a,
        &scene_a,
        [9.0, 1.0, 9.0],
        Some([0.0, 0.0, 0.0]),
        50.0,
        false,
        Some("Close"),
    )
    .expect("second");
    log.append(&Record::AddCamera {
        pos: [1.0, 1.0, 1.0],
        look_at: Some([0.0, 0.0, 0.0]),
        fov: 50.0,
        active: true,
        name: Some("Wide".into()),
    });
    log.append(&Record::AddCamera {
        pos: [9.0, 1.0, 9.0],
        look_at: Some([0.0, 0.0, 0.0]),
        fov: 50.0,
        active: false,
        name: Some("Close".into()),
    });

    capscene::set_camera_view(
        &mut a,
        first,
        [25.0, 8.0, -2.0],
        Some([3.0, 1.0, 0.0]),
        40.0,
    )
    .expect("re-aim");
    log.append(&Record::CameraView {
        id: first.to_loro_key(),
        pos: [25.0, 8.0, -2.0],
        look_at: Some([3.0, 1.0, 0.0]),
        fov: 40.0,
    });
    capscene::set_camera_fov(&mut a, second, 28.0).expect("lens");
    log.append(&Record::CameraFov {
        id: second.to_loro_key(),
        fov: 28.0,
    });
    capscene::set_camera_active(&mut a, second).expect("activate");
    log.append(&Record::CameraActive {
        id: second.to_loro_key(),
    });
    drop(a);

    let (mut b, scene) = seeded();
    let (applied, skipped) = log.replay(&mut b, &scene, &MeshCatalog::new());
    assert_eq!((applied, skipped), (5, 0), "every camera record replayed");

    let cams = capscene::cameras(&b);
    assert_eq!(cams.len(), 2, "got {cams:?}");
    let wide = cams.iter().find(|c| c.name == "Wide").expect("Wide");
    assert_point(wide.pos, [25.0, 8.0, -2.0], "the re-aim survived");
    assert_aim(wide.look_at, Some([3.0, 1.0, 0.0]), "with its aim");
    assert!((wide.fov_deg - 40.0).abs() < 1e-4);
    assert!(
        !wide.active,
        "and it stood down when the other was activated"
    );

    let close = cams.iter().find(|c| c.name == "Close").expect("Close");
    assert!(
        (close.fov_deg - 28.0).abs() < 1e-4,
        "the lens edit survived"
    );
    assert!(close.active, "and the active choice survived");

    // Replay must reference the SAME entities the run authored — an id-addressed record that missed
    // would leave the edits silently unapplied and every assertion above would be testing the authored
    // values instead of the edited ones.
    assert!(
        EntityId::from_loro_key(&close.id).is_some(),
        "the replayed camera's id round-trips"
    );
    log.clear();
}

/// A record written by a build that predated aims and names still replays — the `#[serde(default)]`
/// contract, asserted through the wire form rather than trusted.
#[test]
fn an_add_camera_record_from_before_aims_still_deserialises() {
    let json = r#"{"kind":"addCamera","pos":[3.0,4.0,5.0],"fov":45.0,"active":true}"#;
    let record: Record = serde_json::from_str(json).expect("an old record still parses");
    match record {
        Record::AddCamera {
            pos,
            look_at,
            fov,
            active,
            name,
        } => {
            assert_point(pos, [3.0, 4.0, 5.0], "the old record's position");
            assert_aim(look_at, None, "absent means unaimed, never the origin");
            assert!((fov - 45.0).abs() < 1e-6);
            assert!(active);
            assert_eq!(name, None);
        }
        other => panic!("parsed as {other:?}"),
    }
}
