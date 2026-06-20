//! M3.1 reveal engine — deterministic correctness, "every 'no' explained" (per-target, on demand),
//! and the <16 ms latency proof on a capability-bearing 5k scene (the real M1.5 query). Built on a
//! `FlecsWorld` carrying `Provides`/`Requires`/`BindsTo` pairs (the `scene.rs` `build_scene` pattern).

// The 5k latency scene uses a small LCG for deterministic positions — the long constants + the
// u64→f32 reduction are intentional (a PRNG, not precise arithmetic).
#![allow(clippy::unreadable_literal, clippy::cast_precision_loss)]

use std::collections::HashMap;

use metrocalk_ecs::{Entity, FlecsWorld, World};
use metrocalk_editor_shell::reveal::{reveal, why_not, Context, Rels, WhyNot};

struct Scene {
    world: FlecsWorld,
    rels: Rels,
    cap_name: HashMap<Entity, String>,
    position: HashMap<Entity, [f32; 3]>,
}

/// A small scene: selection requires Health; two unbound Health providers (compatible), one bound
/// Health provider, one Mana-only provider, one capability-less entity.
fn new_scene() -> (Scene, Entity, [Entity; 3]) {
    let mut world = FlecsWorld::new();
    let rels = Rels {
        provides: world.create_entity(),
        requires: world.create_entity(),
        binds_to: world.create_entity(),
    };
    let health = world.create_entity();
    let mana = world.create_entity();
    let mut cap_name = HashMap::new();
    cap_name.insert(health, "Health".to_string());
    cap_name.insert(mana, "Mana".to_string());
    let mut position = HashMap::new();

    let selected = world.create_entity();
    world.add_pair(selected, rels.requires, health);
    position.insert(selected, [0.0, 0.0, 0.0]);

    let h_near = world.create_entity();
    world.add_pair(h_near, rels.provides, health);
    position.insert(h_near, [1.0, 0.0, 0.0]);
    let h_far = world.create_entity();
    world.add_pair(h_far, rels.provides, health);
    position.insert(h_far, [50.0, 0.0, 0.0]);

    let bound = world.create_entity();
    world.add_pair(bound, rels.provides, health);
    let sink = world.create_entity();
    world.add_pair(bound, rels.binds_to, sink);
    position.insert(bound, [2.0, 0.0, 0.0]);

    let mana_only = world.create_entity();
    world.add_pair(mana_only, rels.provides, mana);
    position.insert(mana_only, [3.0, 0.0, 0.0]);

    let empty = world.create_entity();
    position.insert(empty, [4.0, 0.0, 0.0]);

    (
        Scene {
            world,
            rels,
            cap_name,
            position,
        },
        selected,
        [bound, mana_only, empty],
    )
}

#[test]
fn reveal_ranks_compatible_nearest_first() {
    let (scene, selected, _) = new_scene();
    let recency = HashMap::new();
    let ctx = Context {
        cap_name: &scene.cap_name,
        position: &scene.position,
        recency: &recency,
    };
    let r = reveal(&scene.world, selected, scene.rels, &ctx);

    assert_eq!(r.required, vec!["Health".to_string()]);
    assert_eq!(r.compatible.len(), 2, "two unbound Health providers");
    assert!(
        r.compatible[0].distance < r.compatible[1].distance,
        "ranked by proximity (nearest first)"
    );
    assert!(r.compatible.iter().all(|c| c.affinity == 1));
}

#[test]
fn why_not_explains_every_incompatible_specifically() {
    let (scene, selected, [bound, mana_only, empty]) = new_scene();
    let wn = |c: Entity| why_not(&scene.world, selected, scene.rels, c, &scene.cap_name);

    assert_eq!(wn(bound), Some(WhyNot::AlreadyBound));
    assert_eq!(
        wn(mana_only),
        Some(WhyNot::MissingCapability("Health".to_string()))
    );
    assert_eq!(wn(empty), Some(WhyNot::NoCapability));
    // a compatible target yields no "no"
    let ctx_recency = HashMap::new();
    let ctx = Context {
        cap_name: &scene.cap_name,
        position: &scene.position,
        recency: &ctx_recency,
    };
    let first = reveal(&scene.world, selected, scene.rels, &ctx).compatible[0].entity;
    assert_eq!(wn(first), None, "a compatible target has no 'why not'");
    // helpfulness: specific strings, not generic
    assert_eq!(
        WhyNot::MissingCapability("Health".into()).explain(),
        "doesn't provide Health"
    );
}

#[test]
fn reveal_is_deterministic() {
    let (scene, selected, _) = new_scene();
    let recency = HashMap::new();
    let ctx = Context {
        cap_name: &scene.cap_name,
        position: &scene.position,
        recency: &recency,
    };
    let a: Vec<u64> = reveal(&scene.world, selected, scene.rels, &ctx)
        .compatible
        .iter()
        .map(|c| c.entity.0)
        .collect();
    let b: Vec<u64> = reveal(&scene.world, selected, scene.rels, &ctx)
        .compatible
        .iter()
        .map(|c| c.entity.0)
        .collect();
    assert_eq!(a, b, "same scene → same ranked order");
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release-only timing measurement (discipline: --release for timing)"
)]
fn reveal_latency_under_budget_on_5k_capability_scene() {
    // 5k entities carrying Provides(Health|Mana) + a fraction already bound; one selection requires Health.
    let mut world = FlecsWorld::new();
    let rels = Rels {
        provides: world.create_entity(),
        requires: world.create_entity(),
        binds_to: world.create_entity(),
    };
    let health = world.create_entity();
    let mana = world.create_entity();
    let sink = world.create_entity();
    let mut cap_name = HashMap::new();
    cap_name.insert(health, "Health".to_string());
    cap_name.insert(mana, "Mana".to_string());
    let mut position = HashMap::new();

    let selected = world.create_entity();
    world.add_pair(selected, rels.requires, health);
    position.insert(selected, [0.0, 0.0, 0.0]);

    let mut s: u64 = 0x4D45_5452_4F43_4131;
    let mut rnd = || {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (s >> 33) as f32 / (1u64 << 31) as f32
    };
    for i in 0..5000u32 {
        let e = world.create_entity();
        world.add_pair(e, rels.provides, if i % 2 == 0 { health } else { mana });
        if i % 5 == 0 {
            world.add_pair(e, rels.binds_to, sink);
        }
        position.insert(
            e,
            [
                (rnd() * 100.0) - 50.0,
                (rnd() * 100.0) - 50.0,
                (rnd() * 100.0) - 50.0,
            ],
        );
    }
    let recency = HashMap::new();
    let ctx = Context {
        cap_name: &cap_name,
        position: &position,
        recency: &recency,
    };

    for _ in 0..5 {
        let _ = reveal(&world, selected, rels, &ctx);
    }
    let mut times = Vec::new();
    for _ in 0..100 {
        let t0 = std::time::Instant::now();
        let r = reveal(&world, selected, rels, &ctx);
        times.push(t0.elapsed().as_secs_f64() * 1000.0);
        assert!(!r.compatible.is_empty()); // ~2000 unbound Health providers
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = times[times.len() / 2];
    let p99 = times[times.len() * 99 / 100];
    eprintln!(
        "[M3.1] reveal (compat-query + rank) on 5k capability scene: p50={p50:.3}ms p99={p99:.3}ms"
    );
    assert!(
        p99 < 16.0,
        "reveal must hold the frame budget: p99={p99:.3}ms"
    );
}
