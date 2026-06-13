//! The shared, seeded, deterministic stress-scene generator — the single source for tests AND
//! benches (`ecs` bench, `core` tests, `tools/scene-bench`), built **through the [`World`] trait
//! only**, so it runs on any backend and proves the trait suffices to construct the product's scene.
//!
//! Determinism: a fixed SplitMix64 seed (same as the M0 spikes) + a fixed draw order ⇒ byte-identical
//! structure across runs and OS, verified by [`Scene::digest`] (a backend-independent FNV-1a over the
//! generation decisions — *not* over backend entity ids, so `sparse` and dense variants share a digest).

use crate::rng::Rng;
use crate::{Entity, World};

/// Same seed as the M0 spikes ("METROCA1").
pub const SEED: u64 = 0x4D45_5452_4F43_4131;

/// Parameters for a generated scene.
#[derive(Clone, Copy, Debug)]
pub struct SceneParams {
    /// Number of scene entities.
    pub entities: usize,
    /// Number of capability kinds (`caps[0]` is Health).
    pub caps: usize,
    /// Probability an entity provides Health.
    pub p_health: f64,
    /// Inclusive range of *extra* capabilities each entity provides.
    pub extra_caps: (usize, usize),
    /// Attempted `(BindsTo, target)` edges (self-loops are skipped).
    pub edges: usize,
}

impl SceneParams {
    /// The 5k preset (the <16 ms compatibility-query gate scene; matches spike ②).
    #[must_use]
    pub fn preset_5k() -> Self {
        Self {
            entities: 5_000,
            caps: 40,
            p_health: 0.06,
            extra_caps: (3, 8),
            edges: 2_000,
        }
    }
    /// The 20k preset (the F1 memory-scaling scene; matches spike ②).
    #[must_use]
    pub fn preset_20k() -> Self {
        Self {
            entities: 20_000,
            caps: 40,
            p_health: 0.06,
            extra_caps: (3, 8),
            edges: 8_000,
        }
    }
}

/// A generated scene: the relationship/capability handles, the entities, ground-truth for the
/// compatibility query, and a backend-independent structural digest.
pub struct Scene {
    /// The `Provides` relationship.
    pub provides: Entity,
    /// The `BindsTo` relationship.
    pub binds_to: Entity,
    /// The Health capability (`caps[0]`).
    pub health: Entity,
    /// Scene entities in creation order.
    pub entities: Vec<Entity>,
    /// Independently-tracked count of Health-providers with no outgoing `(BindsTo, *)`.
    pub expected_compat: usize,
    /// `(BindsTo, *)` edges actually added.
    pub edge_count: usize,
    /// FNV-1a digest of the logical structure — identical across runs/OS for a given seed+params.
    pub digest: u64,
}

/// FNV-1a (64-bit) — a *stable* hash (unlike `DefaultHasher`) for cross-run/OS digest comparison.
struct Fnv(u64);
impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn write(&mut self, v: u64) {
        for b in v.to_le_bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// Build a scene via `w`. If `sparse`, the `Provides`/`BindsTo` relationships are marked sparse
/// (`World::set_sparse`) before any pairs are added — the F1 candidate storage model.
///
/// The RNG draw order mirrors spike ②, so `preset_5k` yields `expected_compat == 211` /
/// `edge_count == 1999` and `preset_20k` yields `expected_compat == 830`.
pub fn build_scene<W: World>(w: &mut W, params: &SceneParams, sparse: bool) -> Scene {
    let mut rng = Rng::new(SEED);
    let mut fnv = Fnv::new();

    let caps: Vec<Entity> = (0..params.caps).map(|_| w.create_entity()).collect();
    let provides = w.create_entity();
    let binds_to = w.create_entity();
    let player = w.create_entity();
    let enemy = w.create_entity();
    let ui_element = w.create_entity();
    let health = caps[0];

    if sparse {
        w.set_sparse(provides);
        w.set_sparse(binds_to);
    }

    let n = params.entities;
    let (extra_min, extra_max) = params.extra_caps;
    let mut entities = Vec::with_capacity(n);
    let mut provides_health = vec![false; n];
    for slot in &mut provides_health {
        let e = w.create_entity();
        let has_health = rng.chance(params.p_health);
        if has_health {
            w.add_pair(e, provides, health);
            *slot = true;
        }
        let extra = extra_min + rng.below(extra_max - extra_min + 1);
        fnv.write(u64::from(has_health));
        fnv.write(extra as u64);
        for _ in 0..extra {
            let c = 1 + rng.below(params.caps - 1);
            w.add_pair(e, provides, caps[c]);
            fnv.write(c as u64);
        }
        let role = rng.below(6);
        match role {
            0 => w.add_tag(e, player),
            1 => w.add_tag(e, enemy),
            2 => w.add_tag(e, ui_element),
            _ => {}
        }
        fnv.write(role as u64);
        entities.push(e);
    }

    let mut has_binding = vec![false; n];
    let mut edge_count = 0usize;
    for _ in 0..params.edges {
        let si = rng.below(entities.len());
        let di = rng.below(entities.len());
        if si != di {
            w.add_pair(entities[si], binds_to, entities[di]);
            has_binding[si] = true;
            edge_count += 1;
            fnv.write(si as u64);
            fnv.write(di as u64);
        }
    }

    let expected_compat = (0..n)
        .filter(|&i| provides_health[i] && !has_binding[i])
        .count();
    Scene {
        provides,
        binds_to,
        health,
        entities,
        expected_compat,
        edge_count,
        digest: fnv.0,
    }
}

/// Clauses for the compatibility query: provides Health and has no outgoing `(BindsTo, *)`.
#[must_use]
pub fn compat_clauses(scene: &Scene) -> Vec<crate::Clause> {
    use crate::{Clause, Target, Term};
    vec![
        Clause::with(Term::Pair {
            rel: scene.provides,
            target: Target::Exact(scene.health),
        }),
        Clause::without(Term::Pair {
            rel: scene.binds_to,
            target: Target::Any,
        }),
    ]
}
