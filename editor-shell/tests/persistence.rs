//! Live reload-persistence — deterministic-seed + replay-log (north-star test #1, box 5: "survives
//! reload"). Proven headless through the real engine: a bind committed + logged in one "process"
//! reappears after a fresh seed + replay in another, because seeding is deterministic (same seed →
//! identical `EntityId`s) and replay goes back through the commit pipeline. Plus the determinism
//! foundation, the undo-nets-out case, and the divergence guard (a record that can't apply is skipped,
//! never fatal).

use std::collections::HashMap;
use std::path::PathBuf;

use metrocalk_core::{Engine, EntityId, FieldValue};
use metrocalk_ecs::FlecsWorld;

use metrocalk_editor_shell::capscene::{self, CapScene};
use metrocalk_editor_shell::persist::{Log, Record};
use metrocalk_editor_shell::reveal::{reveal, Context};
use metrocalk_editor_shell::{MeshCatalog, TRACKS};

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("metrocalk-{name}.jsonl"))
}

/// The fingerprint for the 500-entity test scene (matches what `make`/`relaunch` seed).
fn fp() -> String {
    capscene::fingerprint(500)
}

fn has_binding(engine: &Engine<FlecsWorld>, from: EntityId, to: EntityId) -> bool {
    engine
        .bindings()
        .iter()
        .any(|(f, k, t)| *f == from && k == TRACKS && *t == to)
}

/// A fresh seeded engine + scene, plus a HealthBar and its nearest compatible Health provider.
fn make() -> (Engine<FlecsWorld>, CapScene, EntityId, EntityId) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    let index = capscene::seed(&mut engine, &scene, 500).expect("seed");
    let bar = index.health_bars[0];
    let pos = capscene::positions(&engine);
    let recency = HashMap::new();
    let ctx = Context {
        cap_name: &scene.cap_name,
        position: &pos,
        recency: &recency,
    };
    let bar_ecs = engine.ecs_entity(bar).unwrap();
    let r = reveal(engine.world(), bar_ecs, scene.rels, &ctx);
    let provider = engine.entity_id_of(r.compatible[0].entity).unwrap();
    (engine, scene, bar, provider)
}

/// Simulate a fresh process: deterministic re-seed (same ids) + replay the log + clear history.
fn relaunch(log: &Log) -> (Engine<FlecsWorld>, usize, usize) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    capscene::seed(&mut engine, &scene, 500).expect("re-seed");
    engine.clear_history(); // seed not undoable
    let (applied, skipped) = log.replay(&mut engine, &scene, &MeshCatalog::new());
    engine.clear_history(); // restored scene not undoable
    (engine, applied, skipped)
}

#[test]
fn a_bind_survives_a_fresh_process_via_replay_log() {
    let log = Log::open(tmp("persist-bind"), fp());
    log.clear();

    // "Run A": bind a provider and persist it.
    let (mut a, scene_a, bar, provider) = make();
    capscene::bind(&mut a, &scene_a, bar, provider).expect("bind");
    log.append(&Record::Bind {
        from: bar.to_loro_key(),
        to: provider.to_loro_key(),
    });
    assert!(has_binding(&a, bar, provider));
    drop(a); // close

    // "Run B": fresh process — deterministic seed + replay.
    let (b, applied, skipped) = relaunch(&log);
    assert_eq!((applied, skipped), (1, 0), "exactly the one bind replayed");
    assert!(
        has_binding(&b, bar, provider),
        "the bind survived close → reopen (box 5 live mechanism)"
    );

    log.clear();
}

#[test]
fn seed_is_deterministic_across_runs() {
    // The replay-log's whole foundation: same seed → identical entity ids, so saved (from,to) keys
    // still refer to the same entities next launch.
    let ids = || -> Vec<String> {
        let mut world = FlecsWorld::new();
        let scene = CapScene::intern(&mut world);
        let mut e = Engine::new(world, 1);
        capscene::seed(&mut e, &scene, 300).unwrap();
        let mut v: Vec<String> = e.entity_ids().iter().map(EntityId::to_loro_key).collect();
        v.sort();
        v
    };
    assert_eq!(ids(), ids(), "deterministic seed → identical entity-id set");
}

#[test]
fn undo_in_the_log_nets_out_on_replay() {
    let log = Log::open(tmp("persist-undo"), fp());
    log.clear();
    let (_, _, bar, provider) = make();
    log.append(&Record::Bind {
        from: bar.to_loro_key(),
        to: provider.to_loro_key(),
    });
    log.append(&Record::Undo);

    let (e, applied, _) = relaunch(&log);
    assert_eq!(applied, 2, "bind + undo both replay");
    assert!(
        !has_binding(&e, bar, provider),
        "an undone bind does not persist (the log replays the undo too)"
    );
    log.clear();
}

#[test]
fn redo_in_the_log_restores_the_undone_action_on_replay() {
    let log = Log::open(tmp("persist-redo"), fp());
    log.clear();
    let (_, _, bar, provider) = make();
    log.append(&Record::Bind {
        from: bar.to_loro_key(),
        to: provider.to_loro_key(),
    });
    log.append(&Record::Undo);
    log.append(&Record::Redo);

    let (e, applied, skipped) = relaunch(&log);
    assert_eq!(applied, 3, "bind + undo + redo all replay");
    assert_eq!(skipped, 0, "a valid redo record must not be skipped");
    assert!(
        has_binding(&e, bar, provider),
        "a redone bind survives close and reopen"
    );
    log.clear();
}

#[test]
fn asset_lab_variant_replays_with_its_name_and_comparison_transform() {
    let log = Log::open(tmp("persist-asset-lab"), fp());
    log.clear();
    log.append(&Record::AssetLabVariant {
        asset: "mtkasset:derived-fixture".into(),
        pos: [3.0, 2.0, -1.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: 1.25,
        source_entity: Some("1_2".into()),
        root_entity: Some("1_0".into()),
        name: "Valve — Optimized".into(),
    });

    let (engine, applied, skipped) = relaunch(&log);
    assert_eq!((applied, skipped), (1, 0));
    let restored = engine.entity_ids().into_iter().find(|id| {
        engine
            .components_of(*id)
            .get("MeshRenderer")
            .and_then(|fields| fields.get(capscene::MESH_FIELD))
            == Some(&FieldValue::Str("mtkasset:derived-fixture".into()))
    });
    let restored = restored.expect("derived entity replayed");
    let components = engine.components_of(restored);
    assert_eq!(
        components
            .get("__meta__")
            .and_then(|fields| fields.get(capscene::NAME_FIELD)),
        Some(&FieldValue::Str("Valve — Optimized".into()))
    );
    assert_eq!(
        components
            .get("Transform")
            .and_then(|fields| fields.get("scale")),
        Some(&FieldValue::Number(1.25))
    );
    assert_eq!(
        components
            .get(capscene::ASSET_LAB_COMPONENT)
            .and_then(|fields| fields.get(capscene::ASSET_LAB_SOURCE_FIELD)),
        Some(&FieldValue::Str("1_2".into()))
    );
    assert_eq!(
        components
            .get(capscene::ASSET_LAB_COMPONENT)
            .and_then(|fields| fields.get(capscene::ASSET_LAB_ROOT_FIELD)),
        Some(&FieldValue::Str("1_0".into()))
    );
    log.clear();
}

#[test]
fn a_divergent_record_is_skipped_not_fatal() {
    // The adversarial case: a record the fresh seed can't honour (here, ids absent from the scene).
    // It must be skipped, not crash the restore — the valid records still apply.
    let log = Log::open(tmp("persist-diverge"), fp());
    log.clear();
    let (_, _, bar, provider) = make();
    log.append(&Record::Bind {
        from: "9_9999".into(), // no such entity in the deterministic seed
        to: "9_8888".into(),
    });
    log.append(&Record::Bind {
        from: bar.to_loro_key(),
        to: provider.to_loro_key(),
    });

    let (e, applied, skipped) = relaunch(&log);
    assert_eq!(
        (applied, skipped),
        (1, 1),
        "divergent record skipped, valid one applied"
    );
    assert!(has_binding(&e, bar, provider));
    log.clear();
}

#[test]
fn an_incompatible_fingerprint_log_is_discarded() {
    // A log written by one build, then opened by an incompatible build (different scene size → a
    // different deterministic id space). Replay must discard it, not replay saved ids against the
    // wrong entities.
    let path = tmp("persist-fp");
    let log_a = Log::open(path.clone(), capscene::fingerprint(500));
    log_a.clear();
    let (_, _, bar, provider) = make();
    log_a.append(&Record::Bind {
        from: bar.to_loro_key(),
        to: provider.to_loro_key(),
    });

    // "new build": same file, different fingerprint.
    let log_b = Log::open(path, capscene::fingerprint(999));
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut e = Engine::new(world, 1);
    capscene::seed(&mut e, &scene, 500).unwrap();
    e.clear_history();
    let (applied, skipped) = log_b.replay(&mut e, &scene, &MeshCatalog::new());
    assert_eq!(
        (applied, skipped),
        (0, 0),
        "incompatible-build log is discarded"
    );
    assert!(
        !has_binding(&e, bar, provider),
        "no saved bind is restored from an incompatible log"
    );
    log_b.clear();
}

#[test]
fn a_placed_camera_survives_a_fresh_process_via_replay_log() {
    // ADR-192, and the half `pillar_persistence` cannot reach. That test proves a `.mtk` written to
    // disk and reopened; THIS one proves the editor-SESSION restore, which is a different mechanism
    // entirely — a deterministic re-seed plus a replay of the append-only log through the same commit
    // pipeline. The two are the only two ways a placed camera can come back, and until this test the
    // replay arm was a code path nothing executed.
    //
    // THE POSE IS IN THE RECORD ON PURPOSE. "The camera the viewport was at" is render state: the
    // orbit is not in the document, so a record carrying the GESTURE would replay "shoot from this
    // view" against wherever the camera happens to be during a replay and film a different shot every
    // time. This test would pass on that design too, which is why it also asserts the exact numbers.
    let log = Log::open(tmp("persist-placed-camera"), fp());
    log.clear();

    let placed = metrocalk_animation::shot::ShotCamera {
        eye: [7.4, 2.9, -5.1],
        look_at: [0.2, 1.35, 0.4],
        fov_deg: 38.5,
    };

    // "Run A": author a two-shot cut and place a camera on the second one.
    let (mut a, _scene_a, bar, _provider) = make();
    for kind in ["establish", "hero"] {
        let (ops, _) = metrocalk_editor_shell::add_shot_ops(&a, bar, kind, bar).expect("shot");
        a.commit("cinema-shot", ops).expect("shot commits");
        log.append(&Record::CinemaShot {
            id: bar.to_loro_key(),
            shot: kind.to_string(),
            subject: Some(bar.to_loro_key()),
        });
    }
    let (ops, _) = metrocalk_editor_shell::set_shot_camera_ops(&a, bar, 1, placed).expect("places");
    a.commit("cinema-camera", ops).expect("camera commits");
    log.append(&Record::CinemaShotCamera {
        id: bar.to_loro_key(),
        index: 1,
        camera: Some(placed),
    });
    drop(a);

    // "Run B": a fresh process.
    let (b, applied, skipped) = relaunch(&log);
    assert_eq!(
        (applied, skipped),
        (3, 0),
        "two shots and one camera replayed"
    );
    let cut = metrocalk_editor_shell::cutscene_of(&b, bar);
    assert_eq!(cut.shots.len(), 2, "the cut itself came back");
    assert_eq!(
        cut.shots[1].camera,
        Some(placed),
        "the placed camera came back"
    );
    assert_eq!(
        cut.shots[0].camera, None,
        "the shot beside it is still on its card"
    );
    log.clear();

    // ...and the CLEAR replays too, netting the pose back out — the undo of the gesture is as durable
    // as the gesture.
    let log = Log::open(tmp("persist-cleared-camera"), fp());
    log.clear();
    let (mut a, _scene_a, bar, _provider) = make();
    let (ops, _) = metrocalk_editor_shell::add_shot_ops(&a, bar, "hero", bar).expect("shot");
    a.commit("cinema-shot", ops).expect("shot commits");
    log.append(&Record::CinemaShot {
        id: bar.to_loro_key(),
        shot: "hero".to_string(),
        subject: Some(bar.to_loro_key()),
    });
    let (ops, _) = metrocalk_editor_shell::set_shot_camera_ops(&a, bar, 0, placed).expect("places");
    a.commit("cinema-camera", ops).expect("camera commits");
    log.append(&Record::CinemaShotCamera {
        id: bar.to_loro_key(),
        index: 0,
        camera: Some(placed),
    });
    let (ops, _) = metrocalk_editor_shell::clear_shot_camera_ops(&a, bar, 0).expect("clears");
    a.commit("cinema-camera", ops).expect("clear commits");
    log.append(&Record::CinemaShotCamera {
        id: bar.to_loro_key(),
        index: 0,
        camera: None,
    });
    drop(a);

    let (b, applied, skipped) = relaunch(&log);
    assert_eq!(
        (applied, skipped),
        (3, 0),
        "shot, place and clear all replayed"
    );
    let cut = metrocalk_editor_shell::cutscene_of(&b, bar);
    assert_eq!(cut.shots[0].camera, None, "the clear replayed too");
    log.clear();
}
