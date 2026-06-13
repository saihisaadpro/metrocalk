//! The seeded compatibility scene, built **through the [`World`] trait only** — which both proves
//! the trait is sufficient to construct the product's scene and (because it mirrors `spikes/flecs`
//! exactly) reproduces the spike's structure for a wrapper-vs-raw cross-check.
//!
//! Generic over `W: World`, so the Phase-2 Loro backend can run the identical scene code.

use crate::rng::Rng;
use crate::{Entity, World};

/// Same seed as the M0 spikes ("METROCA1").
pub const SEED: u64 = 0x4D45_5452_4F43_4131;
const N_CAPS: usize = 40;
const HEALTH_CAP: usize = 0;
const P_HEALTH: f64 = 0.06;

/// Handles + ground-truth needed to express and check the compatibility query.
pub struct CompatScene {
    /// The `Provides` relationship.
    pub provides: Entity,
    /// The `BindsTo` relationship.
    pub binds_to: Entity,
    /// The `Health` capability target (`caps[0]`).
    pub health: Entity,
    /// All scene entities in creation order.
    pub entities: Vec<Entity>,
    /// Independently-tracked count of entities that provide Health and have no outgoing
    /// `(BindsTo, *)` — the expected size of the compat query result.
    pub expected_compat: usize,
    /// Number of `(BindsTo, *)` edges actually added (self-loops skipped), for the edge-traversal check.
    pub edge_count: usize,
}

/// Build the seeded scene (`n_entities` entities, `n_edges` attempted binding edges) via `w`.
///
/// The RNG draw order mirrors `spikes/flecs` precisely (chance(Health) → extra caps → role, then
/// edges), so at 5000 / 2000 the structure matches the spike: `expected_compat == 211`,
/// `edge_count == 1999`.
pub fn build_compat_scene<W: World>(w: &mut W, n_entities: usize, n_edges: usize) -> CompatScene {
    let mut rng = Rng::new(SEED);

    // Capabilities + relationship kinds are plain runtime entities (no rng draws — keeps the draw
    // sequence aligned with the spike).
    let caps: Vec<Entity> = (0..N_CAPS).map(|_| w.create_entity()).collect();
    let provides = w.create_entity();
    let binds_to = w.create_entity();
    let player = w.create_entity();
    let enemy = w.create_entity();
    let ui_element = w.create_entity();
    let health = caps[HEALTH_CAP];

    // Built directly (not via `defer`) to mirror the spike's mutation order exactly, so the
    // structure — and thus the 211-match compat result — reproduces. `defer` is exercised separately.
    let mut entities = Vec::with_capacity(n_entities);
    let mut provides_health = vec![false; n_entities];
    for slot in &mut provides_health {
        let e = w.create_entity();
        if rng.chance(P_HEALTH) {
            w.add_pair(e, provides, health);
            *slot = true;
        }
        let extra = 3 + rng.below(6); // 3..=8 capabilities
        for _ in 0..extra {
            let c = 1 + rng.below(N_CAPS - 1); // never index 0 here; Health handled above
            w.add_pair(e, provides, caps[c]);
        }
        match rng.below(6) {
            0 => w.add_tag(e, player),
            1 => w.add_tag(e, enemy),
            2 => w.add_tag(e, ui_element),
            _ => {}
        }
        entities.push(e);
    }

    let mut has_binding = vec![false; n_entities];
    let mut edge_count = 0usize;
    for _ in 0..n_edges {
        let si = rng.below(entities.len());
        let di = rng.below(entities.len());
        if si != di {
            w.add_pair(entities[si], binds_to, entities[di]);
            has_binding[si] = true;
            edge_count += 1;
        }
    }

    let expected_compat = (0..n_entities)
        .filter(|&i| provides_health[i] && !has_binding[i])
        .count();
    CompatScene {
        provides,
        binds_to,
        health,
        entities,
        expected_compat,
        edge_count,
    }
}
