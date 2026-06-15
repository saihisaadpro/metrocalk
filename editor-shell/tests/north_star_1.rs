//! North-star test #1 — "bind by intent" — driven end-to-end through the **real** `/core` engine
//! (the same `Engine<FlecsWorld>` + commit pipeline + Loro the live shell runs, no MockCore). This is
//! the literal acceptance script from `north-star-tests.md §1`, minus the human eyes:
//!
//!   click a HealthBar → ranked compatible targets · every greyed "no" explains itself ·
//!   one-transaction bind (≤2 interactions) · single-step undo · survives reload.
//!
//! What it cannot assert (and does not fake): that it *feels* like the categorical win — the dogfood
//! verdict — and that it works *in the live window*. Those are the human gate (see `progress/M3.md`).

use std::collections::HashMap;

use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::{Entity, FlecsWorld};

use metrocalk_editor_shell::capscene::{self, CapScene, TRACKS};
use metrocalk_editor_shell::reveal::{reveal, why_not, Context, WhyNot};

/// "Why can't I bind to this provider?" — a fresh, short immutable borrow per call (so it composes
/// with the mutable `bind` between calls).
fn wn(
    engine: &Engine<FlecsWorld>,
    scene: &CapScene,
    bar_ecs: Entity,
    provider: EntityId,
) -> Option<WhyNot> {
    let ecs = engine.ecs_entity(provider).unwrap();
    why_not(engine.world(), bar_ecs, scene.rels, ecs, &scene.cap_name)
}

/// Commit one entity with a `Transform.x` and the given capability ops; return its id.
fn spawn(engine: &mut Engine<FlecsWorld>, x: f64, caps: &[Op]) -> EntityId {
    let id = engine.alloc_entity_id();
    let mut ops = vec![
        Op::CreateEntity { id, parent: None },
        Op::SetField {
            entity: id,
            component: "Transform".into(),
            field: "x".into(),
            value: FieldValue::Number(x),
        },
    ];
    // re-target the placeholder ops at this freshly-allocated id
    for op in caps {
        ops.push(retarget(op, id));
    }
    engine.commit("spawn", ops).expect("spawn commits");
    id
}

/// The cap ops are written against a sentinel id; stamp them with the real one.
fn retarget(op: &Op, id: EntityId) -> Op {
    match op {
        Op::SetField {
            component,
            field,
            value,
            ..
        } => Op::SetField {
            entity: id,
            component: component.clone(),
            field: field.clone(),
            value: value.clone(),
        },
        Op::AddPair { rel, target, .. } => Op::AddPair {
            entity: id,
            rel: *rel,
            target: *target,
        },
        other => other.clone(),
    }
}

fn has_binding(engine: &Engine<FlecsWorld>, from: EntityId, to: EntityId) -> bool {
    engine
        .bindings()
        .iter()
        .any(|(f, k, t)| *f == from && k == TRACKS && *t == to)
}

struct World1 {
    engine: Engine<FlecsWorld>,
    scene: CapScene,
    bar: EntityId,
    near: EntityId,
    far: EntityId,
    pre_bound: EntityId,
    renderable_only: EntityId,
    empty: EntityId,
}

/// The explicit test-1 scene: a HealthBar (requires Health) + two unbound Health providers (near/far)
/// + one already-bound Health provider + one Renderable-only entity + one capability-less entity.
fn scene1() -> World1 {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    let health = scene.cap("Health");
    let renderable = scene.cap("Renderable");
    let sentinel = EntityId {
        peer: 0,
        counter: 0,
    };

    let bar = spawn(
        &mut engine,
        0.0,
        &[
            Op::SetField {
                entity: sentinel,
                component: "HealthBar".into(),
                field: "width".into(),
                value: FieldValue::Number(1.0),
            },
            Op::AddPair {
                entity: sentinel,
                rel: scene.rels.requires,
                target: health,
            },
        ],
    );
    let provides_health = |id: EntityId| Op::AddPair {
        entity: id,
        rel: scene.rels.provides,
        target: health,
    };
    let near = spawn(
        &mut engine,
        1.0,
        &[provides_health(EntityId {
            peer: 0,
            counter: 0,
        })],
    );
    let far = spawn(
        &mut engine,
        50.0,
        &[provides_health(EntityId {
            peer: 0,
            counter: 0,
        })],
    );
    let pre_bound = spawn(
        &mut engine,
        2.0,
        &[
            provides_health(EntityId {
                peer: 0,
                counter: 0,
            }),
            Op::AddPair {
                entity: sentinel,
                rel: scene.rels.binds_to,
                target: scene.rels.binds_to, // any target ⇒ "has an outgoing binding"
            },
        ],
    );
    let renderable_only = spawn(
        &mut engine,
        3.0,
        &[Op::AddPair {
            entity: sentinel,
            rel: scene.rels.provides,
            target: renderable,
        }],
    );
    let empty = spawn(&mut engine, 4.0, &[]);

    World1 {
        engine,
        scene,
        bar,
        near,
        far,
        pre_bound,
        renderable_only,
        empty,
    }
}

fn ctx<'a>(
    scene: &'a CapScene,
    position: &'a HashMap<metrocalk_ecs::Entity, [f32; 3]>,
    recency: &'a HashMap<metrocalk_ecs::Entity, u64>,
) -> Context<'a> {
    Context {
        cap_name: &scene.cap_name,
        position,
        recency,
    }
}

// The acceptance script is one linear sequence (reveal → explain → bind → undo → reload); splitting
// it across functions would fragment the very flow it asserts.
#[allow(clippy::too_many_lines)]
#[test]
fn north_star_1_full_acceptance_through_the_real_engine() {
    let mut w = scene1();
    let recency = HashMap::new();

    // ── step 1–2: click the HealthBar → ranked compatible targets ──────────────────────────────
    let pos = capscene::positions(&w.engine);
    let bar_ecs = w.engine.ecs_entity(w.bar).expect("bar has an ecs handle");
    let r = reveal(
        w.engine.world(),
        bar_ecs,
        w.scene.rels,
        &ctx(&w.scene, &pos, &recency),
    );

    assert_eq!(
        r.required,
        vec!["Health".to_string()],
        "the bar requires Health"
    );
    let compat_ids: Vec<EntityId> = r
        .compatible
        .iter()
        .filter_map(|c| w.engine.entity_id_of(c.entity))
        .collect();
    assert_eq!(
        compat_ids,
        vec![w.near, w.far],
        "two unbound Health providers, nearest first"
    );
    assert!(r.compatible.iter().all(|c| c.affinity == 1));
    assert!(
        r.compatible[0].distance < r.compatible[1].distance,
        "ranked by proximity"
    );

    // ── every greyed "no" explains itself, specifically ────────────────────────────────────────
    assert_eq!(
        wn(&w.engine, &w.scene, bar_ecs, w.pre_bound),
        Some(WhyNot::AlreadyBound)
    );
    assert_eq!(
        wn(&w.engine, &w.scene, bar_ecs, w.renderable_only),
        Some(WhyNot::MissingCapability("Health".into()))
    );
    assert_eq!(
        wn(&w.engine, &w.scene, bar_ecs, w.empty),
        Some(WhyNot::NoCapability)
    );
    assert_eq!(
        wn(&w.engine, &w.scene, bar_ecs, w.near),
        None,
        "a compatible target has no 'why not'"
    );
    // the reasons are specific, not generic
    assert_eq!(
        WhyNot::MissingCapability("Health".into()).explain(),
        "doesn't provide Health"
    );

    // ── step 3: bind in one transaction (interaction #2 — click the target) ────────────────────
    capscene::bind(&mut w.engine, &w.scene, w.bar, w.near).expect("bind commits");
    assert!(
        has_binding(&w.engine, w.bar, w.near),
        "the bar now tracks the near provider"
    );

    // the bound provider is consumed — it leaves the compatible set and now greys "already bound"
    let pos2 = capscene::positions(&w.engine);
    let r2 = reveal(
        w.engine.world(),
        bar_ecs,
        w.scene.rels,
        &ctx(&w.scene, &pos2, &recency),
    );
    let compat2: Vec<EntityId> = r2
        .compatible
        .iter()
        .filter_map(|c| w.engine.entity_id_of(c.entity))
        .collect();
    assert_eq!(
        compat2,
        vec![w.far],
        "the just-bound provider is no longer offered"
    );
    assert_eq!(
        wn(&w.engine, &w.scene, bar_ecs, w.near),
        Some(WhyNot::AlreadyBound),
        "re-reveal greys it with the real reason"
    );

    // ── step 4a: single-step undo reverses the entire bind ─────────────────────────────────────
    assert!(w.engine.undo(), "there is something to undo");
    assert!(
        !has_binding(&w.engine, w.bar, w.near),
        "undo removed the binding"
    );
    let pos3 = capscene::positions(&w.engine);
    let r3 = reveal(
        w.engine.world(),
        bar_ecs,
        w.scene.rels,
        &ctx(&w.scene, &pos3, &recency),
    );
    let compat3: Vec<EntityId> = r3
        .compatible
        .iter()
        .filter_map(|c| w.engine.entity_id_of(c.entity))
        .collect();
    assert_eq!(
        compat3,
        vec![w.near, w.far],
        "after undo the provider is offered again"
    );

    // ── step 4b: re-bind, then prove it survives reload (export → fresh engine → merge) ─────────
    capscene::bind(&mut w.engine, &w.scene, w.bar, w.near).expect("re-bind commits");
    let updates = w.engine.export_updates();
    let mut reloaded = Engine::new(FlecsWorld::new(), 1);
    reloaded
        .merge(&updates)
        .expect("reload merges the persisted doc");
    assert!(
        has_binding(&reloaded, w.bar, w.near),
        "the bind persists across a reload (Loro-backed)"
    );
}

#[test]
fn seed_generator_reveals_exactly_its_unbound_providers() {
    // The live shell's generator (not the explicit scene): seed a mid-size capability scene and prove
    // a fresh HealthBar's compatible set equals the ground-truth count of unbound Health providers,
    // each with affinity 1 — i.e. the reveal is correct over the real seeded relational web.
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    let index = capscene::seed(&mut engine, &scene, 2000).expect("seed commits");

    assert!(
        !index.health_bars.is_empty(),
        "the seed always includes a HealthBar"
    );
    assert!(
        index.unbound_health_providers > 0,
        "and unbound Health providers to rank"
    );

    let pos = capscene::positions(&engine);
    let recency = HashMap::new();
    let bar_ecs = engine.ecs_entity(index.health_bars[0]).unwrap();
    let r = reveal(
        engine.world(),
        bar_ecs,
        scene.rels,
        &ctx(&scene, &pos, &recency),
    );

    assert_eq!(r.required, vec!["Health".to_string()]);
    assert_eq!(
        r.compatible.len(),
        index.unbound_health_providers,
        "reveal offers exactly the unbound Health providers"
    );
    assert!(r.compatible.iter().all(|c| c.affinity == 1));
    // ranked by proximity (non-decreasing distance)
    assert!(r
        .compatible
        .windows(2)
        .all(|w| w[0].distance <= w[1].distance));
}

#[test]
fn clear_history_makes_the_seed_non_undoable_but_later_edits_still_undo() {
    // Regression for the live Ctrl-Z bug: the seed is one big transaction, so without clearing the
    // history the user could Ctrl-Z past their binds and undo scene construction itself — deleting
    // every entity. `clear_history` (called by the shell right after seeding) must drop the seed from
    // the undo stack without touching the scene, while leaving later user edits undoable.
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    let index = capscene::seed(&mut engine, &scene, 500).expect("seed commits");
    let n = engine.entity_count();

    engine.clear_history();
    assert!(
        !engine.undo(),
        "after clear_history the seed is NOT undoable"
    );
    assert_eq!(engine.entity_count(), n, "the scene is fully intact");

    // a subsequent user edit is still a normal undoable transaction
    let bar = index.health_bars[0];
    engine
        .commit(
            "edit width",
            vec![Op::SetField {
                entity: bar,
                component: "HealthBar".into(),
                field: "width".into(),
                value: FieldValue::Number(2.0),
            }],
        )
        .expect("edit commits");
    assert!(
        engine.undo(),
        "a real edit after clear_history undoes normally"
    );
    assert_eq!(
        engine.entity_count(),
        n,
        "still intact after the edit's undo"
    );
}
