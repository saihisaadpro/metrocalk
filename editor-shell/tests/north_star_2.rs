//! North-star test #2 — "describe to create" — the buildable (local-tier) boxes, end-to-end through
//! the **real** `/core` engine. Mirrors `north_star_1.rs`: a free-text description resolves over the
//! curated stdlib → instantiates a **pre-componentized, working** object (real capabilities, not dead
//! geometry) → the M3.1 reveal offers a one-click attach → single-step undo reverses it →
//! describe + attach **survive reload** via the replay-log.
//!
//! Honest scope: there are no real art assets yet, so the demo resolves to a stdlib *kind* (HealthBar),
//! not a 3D mesh; the "Press Play → pick-up-able" / streamed-mesh boxes are gated on the runtime/asset
//! tiers (flagged in `north-star-tests.md`, not faked here).

#![allow(clippy::cast_precision_loss)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use metrocalk_core::resolve::resolve_local;
use metrocalk_core::stdlib::standard_components;
use metrocalk_core::{Engine, EntityId};
use metrocalk_ecs::{Entity, FlecsWorld};

use metrocalk_editor_shell::capscene::{self, CapScene};
use metrocalk_editor_shell::persist::{Log, Record};
use metrocalk_editor_shell::reveal::{reveal, Context};
use metrocalk_editor_shell::{MeshCatalog, TRACKS};

const N: usize = 200;

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("metrocalk-{name}.jsonl"))
}

fn has_binding(engine: &Engine<FlecsWorld>, from: EntityId, to: EntityId) -> bool {
    engine
        .bindings()
        .iter()
        .any(|(f, k, t)| *f == from && k == TRACKS && *t == to)
}

/// A fresh engine seeded with the capability scene (which includes Health providers to attach to).
fn seeded() -> (Engine<FlecsWorld>, CapScene) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    capscene::seed(&mut engine, &scene, N).expect("seed");
    engine.clear_history();
    (engine, scene)
}

fn ctx<'a>(
    scene: &'a CapScene,
    pos: &'a HashMap<Entity, [f32; 3]>,
    rec: &'a HashMap<Entity, u64>,
) -> Context<'a> {
    Context {
        cap_name: &scene.cap_name,
        position: pos,
        recency: rec,
    }
}

#[test]
fn describe_componentize_attach_undo() {
    let (mut e, scene) = seeded();

    // DESCRIBE → componentize: "health bar" resolves + instantiates a HealthBar.
    let (bar, kind) = capscene::describe_create(
        &mut e,
        &scene,
        "health bar",
        [0.0, 0.0, 0.0],
        &MeshCatalog::new(),
    )
    .expect("resolves");
    assert_eq!(kind, "HealthBar");

    // A WORKING object, not dead geometry: it carries the HealthBar component AND requires Health.
    assert!(
        e.components_of(bar).contains_key("HealthBar"),
        "the described entity has its real component"
    );
    let pos = capscene::positions(&e);
    let rec = HashMap::new();
    let bar_ecs = e.ecs_entity(bar).unwrap();
    let r = reveal(e.world(), bar_ecs, scene.rels, &ctx(&scene, &pos, &rec));
    assert_eq!(
        r.required,
        vec!["Health".to_string()],
        "the described HealthBar requires Health"
    );
    assert!(
        !r.compatible.is_empty(),
        "the scene's Health providers are offered for attach"
    );

    // ATTACH (≤2 interactions total: describe + click) — bind to the nearest compatible provider.
    let provider = e.entity_id_of(r.compatible[0].entity).unwrap();
    capscene::bind(&mut e, &scene, bar, provider).expect("attach binds");
    assert!(has_binding(&e, bar, provider));

    // single-step undo reverses the attach.
    assert!(e.undo());
    assert!(!has_binding(&e, bar, provider));
}

#[test]
fn described_creation_and_attach_survive_reload_via_replay_log() {
    let log = Log::open(tmp("ns2"), capscene::fingerprint(N));
    log.clear();

    // run A: describe + attach, each persisted to the replay-log.
    let (mut a, scene_a) = seeded();
    let (bar, _) = capscene::describe_create(
        &mut a,
        &scene_a,
        "health bar",
        [1.0, 0.0, 0.0],
        &MeshCatalog::new(),
    )
    .unwrap();
    log.append(&Record::Describe {
        query: "health bar".into(),
        pos: [1.0, 0.0, 0.0],
    });
    let pos = capscene::positions(&a);
    let rec = HashMap::new();
    let r = reveal(
        a.world(),
        a.ecs_entity(bar).unwrap(),
        scene_a.rels,
        &ctx(&scene_a, &pos, &rec),
    );
    let provider = a.entity_id_of(r.compatible[0].entity).unwrap();
    capscene::bind(&mut a, &scene_a, bar, provider).unwrap();
    log.append(&Record::Bind {
        from: bar.to_loro_key(),
        to: provider.to_loro_key(),
    });
    drop(a); // close

    // run B: fresh deterministic seed + replay → the described entity is recreated + the attach holds.
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut b = Engine::new(world, 1);
    capscene::seed(&mut b, &scene, N).unwrap();
    b.clear_history();
    let (applied, skipped) = log.replay(&mut b, &scene, &MeshCatalog::new());
    b.clear_history();

    assert_eq!((applied, skipped), (2, 0), "describe + bind both replayed");
    assert!(
        b.entity_exists(bar),
        "the described HealthBar survived reload"
    );
    assert!(b.components_of(bar).contains_key("HealthBar"));
    assert!(has_binding(&b, bar, provider), "the attach survived reload");
    log.clear();
}

#[test]
fn resolve_latency_is_well_under_budget() {
    // D5: resolve latency on the stdlib library (release is the metric).
    let lib = standard_components();
    for _ in 0..200 {
        let _ = resolve_local(&lib, "health bar");
    }
    let mut t = Vec::new();
    for _ in 0..2000 {
        let t0 = Instant::now();
        let _ = resolve_local(&lib, "health bar");
        t.push(t0.elapsed().as_secs_f64() * 1e6); // µs
    }
    t.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let (p50, p99) = (t[t.len() / 2], t[t.len() * 99 / 100]);
    eprintln!(
        "[M3.2] resolve_local p50={p50:.2}us p99={p99:.2}us over {} stdlib kinds",
        lib.len()
    );
    assert!(p99 < 16_000.0, "resolve must be ≪ the 16 ms frame budget");
}
