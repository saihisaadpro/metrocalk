//! The capability-bearing scene the binding-by-intent reveal operates on — north-star test #1's
//! world, built on the **real** `/core` stdlib relational web (`HealthBar` requires `Health`;
//! `Health` provides `Health` — see `core/src/stdlib.rs`).
//!
//! M2.6's seed is Transform-only, so the reveal has nothing to rank. This seeds instances that carry
//! their capabilities as `(Provides, cap)` / `(Requires, cap)` pairs, **through the commit pipeline**
//! (invariant 3), so every entity is projectable, bindable, and undoable like any other. The reveal
//! query (`with(Provides, C)` + `without(BindsTo, *)`) then runs over the engine's world exactly as
//! the M1.5 spike proved (~12 µs).
//!
//! The relationship/capability handles are interned in the `FlecsWorld` **before** the `Engine` takes
//! ownership — they are metadata (like the registry's own interned rels), not scene entities. Each
//! instance also gets the matching Loro component (`Health` / `HealthBar`) so the inspector names it
//! and the projection carries it; the component (data) and the pair (queryable capability) are kept
//! consistent at the seam.
//!
//! Scope note: the capability *web* here (Health/HealthBar, `provides`/`requires`) mirrors the `/core`
//! stdlib (`stdlib.rs`), but the `Transform` seeded is a minimal `x`/`y`/`z` viewport placeholder, not
//! the full stdlib `Transform` schema (`px`/`py`/`pz` + `provides Spatial`) — the shell's renderer
//! reads `x`/`y`/`z`. Reconciling the two is a later cleanup; it doesn't affect the reveal (which keys
//! off the capability pairs, not the Transform fields).

// Scene positions are visual coordinates drawn from a PRNG, not precise arithmetic: the f64→f32
// truncation (the viewport + reveal both work in f32) and the i64→f32 read are intentional here.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use std::collections::HashMap;

use metrocalk_core::{Engine, EntityId, FieldValue, Op, PipelineError};
use metrocalk_ecs::rng::Rng;
use metrocalk_ecs::{Entity, FlecsWorld, World};

use crate::reveal::Rels;

/// Same deterministic seed family as the M0 spikes / `ecs::scene` ("METROCA1").
const SEED: u64 = 0x4D45_5452_4F43_4131;

/// The binding kind for a HealthBar tracking a Health provider (the test-1 wire), as the Loro edge's
/// `kind` string.
pub const TRACKS: &str = "tracks";

/// The interned capability graph for a seeded scene: the relationships + capability handles and their
/// names — everything the reveal needs beyond the world itself.
pub struct CapScene {
    /// `Provides` / `Requires` / `BindsTo` relationship handles.
    pub rels: Rels,
    /// capability name → interned handle (e.g. `"Health"`).
    pub caps: HashMap<String, Entity>,
    /// the inverse — handle → name, for the reveal's "doesn't provide X" reason.
    pub cap_name: HashMap<Entity, String>,
}

/// What a seed produced, so the live shell (and tests) can find the clickable requirers without
/// scanning: the seeded HealthBars and the count of unbound Health providers (the ground-truth the
/// reveal should rank).
pub struct SeedIndex {
    /// The seeded HealthBar entities (each `requires Health` — click one to reveal candidates).
    pub health_bars: Vec<EntityId>,
    /// Unbound Health providers — the ground-truth size of a fresh HealthBar's compatible set.
    pub unbound_health_providers: usize,
}

impl CapScene {
    /// Intern the three relationships + the stdlib capabilities into `world` **before** it is handed
    /// to the engine. Call once, on a fresh world.
    #[must_use]
    pub fn intern(world: &mut FlecsWorld) -> Self {
        let rels = Rels {
            provides: world.create_entity(),
            requires: world.create_entity(),
            binds_to: world.create_entity(),
        };
        let mut caps = HashMap::new();
        let mut cap_name = HashMap::new();
        // The capabilities the stdlib's relational web is built on (stdlib.rs).
        for name in [
            "Health",
            "Spatial",
            "Renderable",
            "UIElement",
            "Physics",
            "Audio",
        ] {
            let e = world.create_entity();
            caps.insert(name.to_string(), e);
            cap_name.insert(e, name.to_string());
        }
        Self {
            rels,
            caps,
            cap_name,
        }
    }

    /// A capability handle by name (panics on an unseeded name — the seed only uses interned ones).
    #[must_use]
    pub fn cap(&self, name: &str) -> Entity {
        self.caps[name]
    }
}

/// A fingerprint of the deterministic scene this build produces, persisted as the replay log's header.
/// Replay refuses (and discards) a log written by an incompatible build — different seed, scene size,
/// or `seed()` algorithm — rather than replaying saved ids against a divergent id space (which would
/// silently bind the wrong entities). **Bump `mtkscene1` whenever [`seed`]'s draw sequence changes.**
#[must_use]
pub fn fingerprint(n: usize) -> String {
    format!("mtkscene1 seed={SEED:#x} n={n}")
}

/// Seed `n` entities through the commit pipeline: each gets a `Transform` (spread in a volume, the
/// viewport's geometry) and a deterministic role drawn from the stdlib web — most are Health
/// providers or other-capability providers, a small fraction are HealthBars (the requirers), and some
/// providers start already bound (so the "already bound" greying is demonstrable on first reveal).
///
/// # Errors
/// Propagates a [`PipelineError`] if the seeding transaction fails (it shouldn't — the ops are
/// internally consistent).
pub fn seed(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    n: usize,
) -> Result<SeedIndex, PipelineError> {
    let mut rng = Rng::new(SEED);
    let extent = 18.0 * ((n as f32) / 5000.0).cbrt().max(0.3);
    let health = scene.cap("Health");
    let providable = ["Renderable", "Physics", "Audio", "Spatial"];

    let mut ops: Vec<Op> = Vec::with_capacity(n * 5);
    let mut health_bars = Vec::new();
    let mut unbound_health_providers = 0usize;

    for i in 0..n {
        let id = engine.alloc_entity_id();
        ops.push(Op::CreateEntity { id, parent: None });
        for (f, v) in [
            ("x", (rng.f64() as f32 * 2.0 - 1.0) * extent),
            ("y", (rng.f64() as f32 * 2.0 - 1.0) * extent),
            ("z", (rng.f64() as f32 * 2.0 - 1.0) * extent),
        ] {
            ops.push(Op::SetField {
                entity: id,
                component: "Transform".into(),
                field: f.into(),
                value: FieldValue::Number(f64::from(v)),
            });
        }

        // Role mix. Force the first entity to be a HealthBar near the origin so the live demo always
        // has an obvious starting click; otherwise draw a deterministic role.
        let is_bar = i == 0 || rng.chance(0.02);
        if is_bar {
            // A HealthBar: requires Health (the click-to-reveal entity).
            ops.push(Op::SetField {
                entity: id,
                component: "HealthBar".into(),
                field: "width".into(),
                value: FieldValue::Number(1.0),
            });
            ops.push(Op::AddPair {
                entity: id,
                rel: scene.rels.requires,
                target: health,
            });
            health_bars.push(id);
        } else if rng.chance(0.34) {
            // A Health provider — the candidate set. Some start already bound (greyed "already
            // bound" on first reveal); the rest are unbound (the ranked candidates).
            ops.push(Op::SetField {
                entity: id,
                component: "Health".into(),
                field: "hp".into(),
                value: FieldValue::Integer(100),
            });
            ops.push(Op::SetField {
                entity: id,
                component: "Health".into(),
                field: "maxHp".into(),
                value: FieldValue::Integer(100),
            });
            ops.push(Op::AddPair {
                entity: id,
                rel: scene.rels.provides,
                target: health,
            });
            if rng.chance(0.25) {
                // pre-bound: an outgoing BindsTo marks it consumed (excluded by the reveal query).
                ops.push(Op::AddPair {
                    entity: id,
                    rel: scene.rels.binds_to,
                    target: scene.rels.binds_to, // any target ⇒ the negation term matches; self-ref is fine as a marker
                });
            } else {
                unbound_health_providers += 1;
            }
        } else {
            // Some other-capability provider (greyed "doesn't provide Health"), or — occasionally —
            // a capability-less entity (greyed "provides no capabilities").
            if rng.chance(0.8) {
                let cap = providable[rng.below(providable.len())];
                ops.push(Op::AddPair {
                    entity: id,
                    rel: scene.rels.provides,
                    target: scene.cap(cap),
                });
            }
        }
    }

    engine.commit("seed-capability-scene", ops)?;
    Ok(SeedIndex {
        health_bars,
        unbound_health_providers,
    })
}

/// Wire a binding-by-intent: `bar` (the requirer) tracks `provider` (a compatible target). **One
/// transaction**, so it is a single undoable step (test-1's "single-step undo") and survives reload:
/// the persisted Loro binding edge (`bar --tracks--> provider`, which `project_full` re-emits) **and**
/// the ECS `(BindsTo, bar)` pair on the provider, so the reveal correctly treats the provider as
/// consumed — it leaves the compatible set, and a re-reveal greys it "already bound". Undo reverses
/// both atomically.
///
/// **Reload constraint (carry-forward):** the reveal's exclusion depends on the ECS `BindsTo` pair,
/// which `Engine::merge` does NOT rebuild (it restores entities from Loro but not their ECS
/// tags/pairs). So a binding's exclusion survives undo and full re-projection, but a *merge*/reload
/// would drop it (the Loro edge persists, the ECS pair does not), and the reveal would re-offer the
/// bound provider. The live shell never merges (single peer; undo re-projects), so this is latent
/// today; the fix is to re-derive `(BindsTo, *)` from the Loro `bindings` map in
/// `rebuild_ecs_from_loro` (scheduled with collab).
///
/// # Errors
/// [`PipelineError::UnknownEntity`] if either endpoint isn't a live scene entity, propagated from the
/// pipeline's all-or-nothing validation.
pub fn bind(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    bar: EntityId,
    provider: EntityId,
) -> Result<(), PipelineError> {
    let bar_ecs = engine
        .ecs_entity(bar)
        .ok_or(PipelineError::UnknownEntity(bar))?;
    engine.commit(
        "bind by intent",
        vec![
            Op::AddBinding {
                from: bar,
                kind: TRACKS.into(),
                to: provider,
            },
            Op::AddPair {
                entity: provider,
                rel: scene.rels.binds_to,
                target: bar_ecs,
            },
        ],
    )
}

/// Build the reveal's position map (ECS handle → world position) from the engine's `Transform`
/// components — proximity ranking input. Keyed by the raw [`Entity`] the reveal matches on.
#[must_use]
pub fn positions(engine: &Engine<FlecsWorld>) -> HashMap<Entity, [f32; 3]> {
    let mut out = HashMap::new();
    for id in engine.entity_ids() {
        let Some(ecs) = engine.ecs_entity(id) else {
            continue;
        };
        let comps = engine.components_of(id);
        let t = comps.get("Transform");
        let g = |f: &str| -> f32 {
            t.and_then(|m| m.get(f)).map_or(0.0, |v| match v {
                FieldValue::Number(n) => *n as f32,
                FieldValue::Integer(i) => *i as f32,
                _ => 0.0,
            })
        };
        out.insert(ecs, [g("x"), g("y"), g("z")]);
    }
    out
}
