//! M10.4 (ADR-034) Play-mode runtime — the **headless substrate** of the non-destructive guarantee.
//!
//! The live Play/Stop loop runs the deterministic M8 sim on the engine thread (GUI-accepted; the sim
//! itself + its run-to-run determinism are `/physics`'s domain, proven in M8.1/M8.4). These tests lock
//! the two **engine-level** invariants Play depends on:
//!   1. **Stop restores the pre-Play edit state bit-exactly** — restoring from the Play snapshot (a fresh
//!      engine + `merge`, exactly what the Stop command does) reproduces the edit state AND **wipes any
//!      change made during Play** (non-destructive — the adversarial "Play edits leak into the authored
//!      scene" / "Stop doesn't fully restore" guard).
//!   2. **The render-projection reads the sim feeds the viewport with NEVER mutate the engine** — so a
//!      running scene (render-only per ADR-021) can't corrupt the authored document.

use std::collections::HashMap;

use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::{Entity, FlecsWorld};

use metrocalk_editor_shell::capscene::{self, CapResolver, CapScene, TRACKS};
use metrocalk_editor_shell::reveal::{reveal, Context};

fn engine_with_resolver() -> (Engine<FlecsWorld>, CapScene) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    engine.set_capability_resolver(Box::new(CapResolver::from_scene(&scene)));
    (engine, scene)
}

fn ctx<'a>(
    scene: &'a CapScene,
    pos: &'a HashMap<Entity, [f32; 3]>,
    recency: &'a HashMap<Entity, u64>,
) -> Context<'a> {
    Context {
        cap_name: &scene.cap_name,
        position: pos,
        recency,
    }
}

fn spawn_bar(engine: &mut Engine<FlecsWorld>, scene: &CapScene) -> EntityId {
    let id = engine.alloc_entity_id();
    engine
        .commit(
            "bar",
            vec![
                Op::CreateEntity { id, parent: None },
                Op::SetField {
                    entity: id,
                    component: "Transform".into(),
                    field: "x".into(),
                    value: FieldValue::Number(0.0),
                },
                Op::AddPair {
                    entity: id,
                    rel: scene.rels.requires,
                    target: scene.cap("Health"),
                },
            ],
        )
        .expect("bar commits");
    id
}

fn spawn_provider(engine: &mut Engine<FlecsWorld>, scene: &CapScene, x: f64) -> EntityId {
    let id = engine.alloc_entity_id();
    engine
        .commit(
            "provider",
            vec![
                Op::CreateEntity { id, parent: None },
                Op::SetField {
                    entity: id,
                    component: "Transform".into(),
                    field: "x".into(),
                    value: FieldValue::Number(x),
                },
                Op::AddPair {
                    entity: id,
                    rel: scene.rels.provides,
                    target: scene.cap("Health"),
                },
            ],
        )
        .expect("provider commits");
    id
}

#[test]
fn stop_restores_pre_play_edit_state_and_wipes_play_changes() {
    // Author a scene + bind (the edit state).
    let (mut a, scene_a) = engine_with_resolver();
    let bar = spawn_bar(&mut a, &scene_a);
    let p1 = spawn_provider(&mut a, &scene_a, 1.0);
    let p2 = spawn_provider(&mut a, &scene_a, 2.0);
    capscene::bind(&mut a, &scene_a, bar, p1).expect("bind");
    let pre_count = a.entity_count();

    // PLAY: snapshot the edit state (what the Play command captures).
    let snapshot = a.snapshot();

    // A change that LEAKS during Play (an edit, a stray commit). In the live shell edits are disabled in
    // Play AND the sim is render-only, so this can't happen — but Stop restoring from the snapshot makes
    // the non-destructive guarantee airtight even if it did.
    let leaked = spawn_provider(&mut a, &scene_a, 9.0);
    assert_eq!(
        a.entity_count(),
        pre_count + 1,
        "a change was applied during 'Play'"
    );

    // STOP: restore from the snapshot — a fresh engine + scene + resolver + merge (the Stop command).
    let (mut b, scene_b) = engine_with_resolver();
    b.merge(&snapshot).expect("Stop restores the snapshot");

    // The restored engine is the PRE-Play state: the leak is gone, the scene + bind + caps are back.
    assert_eq!(
        b.entity_count(),
        pre_count,
        "Stop restored the exact pre-Play entity count (the Play-time change is wiped)"
    );
    assert!(
        !b.entity_exists(leaked),
        "the change made during Play does NOT leak into the authored scene (non-destructive)"
    );
    assert!(
        b.entity_exists(bar) && b.entity_exists(p1) && b.entity_exists(p2),
        "the authored scene is restored"
    );
    assert!(
        b.bindings()
            .iter()
            .any(|(f, k, t)| *f == bar && k == TRACKS && *t == p1),
        "the binding is restored"
    );
    // Caps restored → the reveal works, with the bound provider still excluded — the exact pre-Play state.
    let bar_ecs = b.ecs_entity(bar).unwrap();
    let pos = capscene::positions(&b);
    let recency = HashMap::new();
    let r = reveal(
        b.world(),
        bar_ecs,
        scene_b.rels,
        &ctx(&scene_b, &pos, &recency),
    );
    let compat: Vec<EntityId> = r
        .compatible
        .iter()
        .filter_map(|c| b.entity_id_of(c.entity))
        .collect();
    assert_eq!(
        compat,
        vec![p2],
        "after Stop the reveal is exactly the pre-Play state (p1 bound, p2 free)"
    );
}

#[test]
fn render_projection_reads_never_mutate_the_engine() {
    // A running scene projects per-tick transforms to the render only (ADR-021); the reads it feeds the
    // viewport with are all `&self`. Driving them like N Play frames must leave the document untouched.
    let (mut a, scene_a) = engine_with_resolver();
    let bar = spawn_bar(&mut a, &scene_a);
    let p1 = spawn_provider(&mut a, &scene_a, 1.0);
    capscene::bind(&mut a, &scene_a, bar, p1).expect("bind");

    let before = a.snapshot();
    for _ in 0..200 {
        let _ = metrocalk_editor_shell::project_full(&a);
        let _ = capscene::positions(&a);
        let _ = capscene::tracking_segments(&a);
        for id in a.entity_ids() {
            let _ = a.components_of(id);
        }
    }
    assert_eq!(
        a.snapshot(),
        before,
        "the render-projection reads left the authored document bit-identical (Play is render-only)"
    );
}
