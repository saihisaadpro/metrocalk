//! Regression for the M1.6 **capability-rebuild-on-merge** carry-forward (ADR-032): a project saved as
//! a Loro document (snapshot + oplog) and re-opened in a **fresh** engine must restore not only the
//! entities + Loro binding edges but every entity's ECS **capability pairs** — so binding-by-intent's
//! reveal (`with(Provides,C)` + `without(BindsTo,*)`) is **non-empty after load**, the bound provider
//! stays excluded, and caps that are NOT 1:1 derivable from a single component kind (a physics body's
//! `Physics`/`Collision`) survive too.
//!
//! This is the exact bug ADR-013 documented: `rebuild_ecs_from_loro` rebuilt entities but not their
//! capability pairs, so the compat query was empty after a merge. **RED** before the fix (reveal empty,
//! physics caps gone); **GREEN** after — caps are mirrored into the durable document and re-derived on
//! load. The physics-body assertion is deliberate: it proves caps are *persisted*, not re-derived from
//! a component-kind registry (which would be lossy for multi-cap / non-registry entities).

use std::collections::HashMap;

use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::{Entity, FlecsWorld, World};

use metrocalk_editor_shell::capscene::{self, CapResolver, CapScene, TRACKS};
use metrocalk_editor_shell::reveal::{reveal, why_not, Context, WhyNot};

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

/// A fresh engine with the capability resolver installed (so cap pairs mirror into the durable doc and
/// are re-derived on merge), plus its scene's interned capability vocabulary.
fn engine_with_resolver() -> (Engine<FlecsWorld>, CapScene) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    engine.set_capability_resolver(Box::new(CapResolver::from_scene(&scene)));
    (engine, scene)
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
                Op::SetField {
                    entity: id,
                    component: "HealthBar".into(),
                    field: "width".into(),
                    value: FieldValue::Number(1.0),
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
                Op::SetField {
                    entity: id,
                    component: "Health".into(),
                    field: "hp".into(),
                    value: FieldValue::Integer(100),
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

fn has_binding(engine: &Engine<FlecsWorld>, from: EntityId, to: EntityId) -> bool {
    engine
        .bindings()
        .iter()
        .any(|(f, k, t)| *f == from && k == TRACKS && *t == to)
}

#[test]
fn load_restores_capabilities_so_reveal_and_bind_work() {
    // ── 1. Build + bind in engine A (the "before save" world) ───────────────────────────────────
    let (mut a, scene_a) = engine_with_resolver();
    let bar = spawn_bar(&mut a, &scene_a);
    let p1 = spawn_provider(&mut a, &scene_a, 1.0);
    let p2 = spawn_provider(&mut a, &scene_a, 2.0);
    // A physics body: provides Physics + Collision, requires Spatial — caps that don't map 1:1 to a
    // single component kind, so registry-derivation would not faithfully reconstruct them.
    let body = capscene::spawn_physics_body(&mut a, &scene_a, None, [5.0, 0.0, 0.0], 0.45)
        .expect("body spawns");

    // Sanity: before save, the bar reveals both unbound providers.
    let pos_a = capscene::positions(&a);
    let recency = HashMap::new();
    let bar_ecs_a = a.ecs_entity(bar).unwrap();
    let r_a = reveal(
        a.world(),
        bar_ecs_a,
        scene_a.rels,
        &ctx(&scene_a, &pos_a, &recency),
    );
    assert_eq!(
        r_a.compatible.len(),
        2,
        "both providers offered before the bind"
    );

    // Bind the bar to p1 (the bound provider should be excluded after load).
    capscene::bind(&mut a, &scene_a, bar, p1).expect("bind commits");

    // ── 2. Save = export the Loro document; Open = a fresh engine + merge ────────────────────────
    let doc = a.export_updates();
    let (mut b, scene_b) = engine_with_resolver();
    b.merge(&doc).expect("the saved document re-opens (merge)");

    // ── 3. Caps restored: reveal is NON-EMPTY, the requirement survived, the bound one is excluded ─
    let bar_ecs = b.ecs_entity(bar).expect("the bar entity is restored");
    let pos = capscene::positions(&b);
    let r = reveal(
        b.world(),
        bar_ecs,
        scene_b.rels,
        &ctx(&scene_b, &pos, &recency),
    );

    assert_eq!(
        r.required,
        vec!["Health".to_string()],
        "the bar still REQUIRES Health after load (the requires pair was restored)"
    );
    assert!(
        !r.compatible.is_empty(),
        "reveal is non-empty after load — the capability-rebuild regression (was empty before ADR-032)"
    );
    let compat: Vec<EntityId> = r
        .compatible
        .iter()
        .filter_map(|c| b.entity_id_of(c.entity))
        .collect();
    assert!(
        compat.contains(&p2),
        "the unbound provider is still offered after load"
    );
    assert!(
        !compat.contains(&p1),
        "the BOUND provider is excluded after load (BindsTo reconstructed from the durable edge)"
    );

    // The bound provider explains itself as already-bound (every 'no' explained, post-load).
    let p1_ecs = b.ecs_entity(p1).unwrap();
    assert_eq!(
        why_not(b.world(), bar_ecs, scene_b.rels, p1_ecs, &scene_b.cap_name),
        Some(WhyNot::AlreadyBound),
        "the bound provider is greyed 'already bound' after load"
    );

    // The durable binding edge survived (Loro-backed).
    assert!(
        has_binding(&b, bar, p1),
        "the binding edge survives the load"
    );

    // ── 4. Generality: a physics body's caps round-trip (persisted, not registry-derived) ─────────
    let body_ecs = b.ecs_entity(body).expect("the physics body is restored");
    let provides = b.world().targets(body_ecs, scene_b.rels.provides);
    assert!(
        provides.contains(&scene_b.cap("Physics")),
        "the physics body still PROVIDES Physics after load"
    );
    assert!(
        provides.contains(&scene_b.cap("Collision")),
        "the physics body still PROVIDES Collision after load"
    );
    let requires = b.world().targets(body_ecs, scene_b.rels.requires);
    assert!(
        requires.contains(&scene_b.cap("Spatial")),
        "the physics body still REQUIRES Spatial after load"
    );
}

#[test]
fn save_load_is_idempotent_for_caps() {
    // Round-trip determinism for the cap mirror: re-exporting a loaded document yields a byte-identical
    // capability layer (the "two saves of the same scene differ" adversarial guard, for caps).
    let (mut a, scene_a) = engine_with_resolver();
    let bar = spawn_bar(&mut a, &scene_a);
    let p1 = spawn_provider(&mut a, &scene_a, 1.0);
    capscene::bind(&mut a, &scene_a, bar, p1).expect("bind commits");

    let doc = a.export_updates();
    let (mut b, _scene_b) = engine_with_resolver();
    b.merge(&doc).expect("merge");

    // The bar's caps (Requires Health) and p1's (Provides Health) must be present after load — i.e. the
    // reveal would work — and a re-export carries them identically (a load loses nothing).
    assert!(
        b.entity_exists(bar) && b.entity_exists(p1),
        "entities restored"
    );
    let again = b.export_updates();
    let (mut c, scene_c) = engine_with_resolver();
    c.merge(&again).expect("re-merge of the re-exported doc");
    let bar_ecs = c.ecs_entity(bar).unwrap();
    let pos = capscene::positions(&c);
    let recency = HashMap::new();
    let r = reveal(
        c.world(),
        bar_ecs,
        scene_c.rels,
        &ctx(&scene_c, &pos, &recency),
    );
    assert_eq!(
        r.required,
        vec!["Health".to_string()],
        "the requirement survives a second save→load round-trip"
    );
}
