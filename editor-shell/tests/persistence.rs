//! Live reload-persistence — deterministic-seed + replay-log (north-star test #1, box 5: "survives
//! reload"). Proven headless through the real engine: a bind committed + logged in one "process"
//! reappears after a fresh seed + replay in another, because seeding is deterministic (same seed →
//! identical `EntityId`s) and replay goes back through the commit pipeline. Plus the determinism
//! foundation, the undo-nets-out case, and the divergence guard (a record that can't apply is skipped,
//! never fatal).

use std::collections::HashMap;
use std::path::PathBuf;

use metrocalk_core::{Engine, EntityId};
use metrocalk_ecs::FlecsWorld;

use metrocalk_editor_shell::capscene::{self, CapScene};
use metrocalk_editor_shell::persist::{Log, Record};
use metrocalk_editor_shell::reveal::{reveal, Context};
use metrocalk_editor_shell::TRACKS;

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("metrocalk-{name}.jsonl"))
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
    let (applied, skipped) = log.replay(&mut engine, &scene);
    engine.clear_history(); // restored scene not undoable
    (engine, applied, skipped)
}

#[test]
fn a_bind_survives_a_fresh_process_via_replay_log() {
    let log = Log::open(tmp("persist-bind"));
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
    let log = Log::open(tmp("persist-undo"));
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
fn a_divergent_record_is_skipped_not_fatal() {
    // The adversarial case: a record the fresh seed can't honour (here, ids absent from the scene).
    // It must be skipped, not crash the restore — the valid records still apply.
    let log = Log::open(tmp("persist-diverge"));
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
