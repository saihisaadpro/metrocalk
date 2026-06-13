//! Shared scene model for the Flecs spike, used by both the benchmark driver (`main.rs`) and the
//! criterion cross-check (`benches/compat.rs`). Keeping it in a lib lets the criterion harness
//! build the exact same scene/query the hand-rolled timing loop uses.

pub mod rng;

use flecs_ecs::prelude::*;
use rng::Rng;

/// Fixed RNG seed ("METROCA1"), same as the loro spike.
pub const SEED: u64 = 0x4D45_5452_4F43_4131;
pub const N_CAPS: usize = 40;
pub const HEALTH_CAP: usize = 0; // capability index 0 is "Health"
pub const P_HEALTH: f64 = 0.06; // ~300 / 5000 entities provide Health
pub const EXTRA_CAPS: (usize, usize) = (3, 8);

// Relationship tags (idiomatic Flecs: zero-sized #[derive(Component)] structs).
#[derive(Component)]
pub struct Provides;
#[derive(Component)]
pub struct BindsTo;

// Role tags.
#[derive(Component)]
pub struct Player;
#[derive(Component)]
pub struct Enemy;
#[derive(Component)]
pub struct UiElement;

pub struct Scene {
    pub world: World,
    pub caps: Vec<Entity>,
    pub entities: Vec<Entity>,
}

pub fn build_scene(seed: u64, n_entities: usize, n_edges: usize) -> Scene {
    let mut rng = Rng::new(seed);
    let world = World::new();

    let caps: Vec<Entity> = (0..N_CAPS)
        .map(|i| world.entity_named(&format!("cap_{i}")).id())
        .collect();

    let mut entities = Vec::with_capacity(n_entities);
    for _ in 0..n_entities {
        let e = world.entity();
        if rng.chance(P_HEALTH) {
            e.add((Provides, caps[HEALTH_CAP]));
        }
        let extra = EXTRA_CAPS.0 + rng.below(EXTRA_CAPS.1 - EXTRA_CAPS.0 + 1);
        for _ in 0..extra {
            let c = 1 + rng.below(N_CAPS - 1);
            e.add((Provides, caps[c]));
        }
        match rng.below(6) {
            0 => {
                e.add(Player);
            }
            1 => {
                e.add(Enemy);
            }
            2 => {
                e.add(UiElement);
            }
            _ => {}
        }
        entities.push(e.id());
    }

    for _ in 0..n_edges {
        let src = entities[rng.below(entities.len())];
        let dst = entities[rng.below(entities.len())];
        if src != dst {
            world.entity_from_id(src).add((BindsTo, dst));
        }
    }

    Scene { world, caps, entities }
}

/// The compatibility query: entities that **provide Health** and have **no outgoing `(BindsTo,*)`**.
/// Exercises a pair term + a wildcard negation term — the core of click-to-bind / describe ranking.
pub fn compat_query(scene: &Scene, cached: bool) -> Query<()> {
    let health = scene.caps[HEALTH_CAP];
    let mut b = scene.world.query::<()>();
    b.with((Provides, health));
    b.without((BindsTo, id::<flecs::Wildcard>()));
    if cached {
        b.set_cached();
    }
    b.build()
}
