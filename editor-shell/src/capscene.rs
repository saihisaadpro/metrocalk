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

use metrocalk_core::caps::{canonical, display_name, is_standard};
use metrocalk_core::marketplace::MarketplaceEntry;
use metrocalk_core::{ComponentMeta, Engine, EntityId, FieldType, FieldValue, Op, PipelineError};
use metrocalk_ecs::rng::Rng;
use metrocalk_ecs::{Entity, FlecsWorld, World};

use crate::reveal::Rels;

/// Same deterministic seed family as the M0 spikes / `ecs::scene` ("METROCA1").
const SEED: u64 = 0x4D45_5452_4F43_4131;

/// The binding kind for a HealthBar tracking a Health provider (the test-1 wire), as the Loro edge's
/// `kind` string.
pub const TRACKS: &str = "tracks";

/// The `MeshRenderer` component field that carries the **asset handle** — the lightweight string
/// (a `metrocalk_assets::AssetId`) that references geometry held in the asset store *beside* the doc.
/// The handle is all that enters the ECS / Loro (invariant 2); the renderer resolves it to a mesh.
pub const MESH_FIELD: &str = "mesh";

/// Maps a resolved component **kind** name (e.g. `"HealthBar"`) to the asset **handle** an instance of
/// that kind should render as. Owned by the shell (built at startup from the loaded [`AssetStore`]);
/// describe-to-create consults it so a resolved kind with an associated mesh instantiates *looking*
/// like itself, and a kind with no entry honestly falls back to the placeholder cube. A plain string
/// map (no `metrocalk_assets` dependency leaks into the bridge lib — the handles are opaque here).
pub type MeshCatalog = HashMap<String, String>;

/// The interned capability graph for a seeded scene: the relationships + capability handles and their
/// names — everything the reveal needs beyond the world itself. Capabilities are interned by their
/// **canonical namespaced name** (ADR-015): the curated stdlib is the `std:` standard vocabulary, and
/// a marketplace entry's custom caps (`acme:Health`) are distinct entities that opt into the standard
/// relational web via an `(AliasOf, std:Cap)` pair — so two authors' same-local-name caps never collide
/// yet still bind a `std:` requirer.
pub struct CapScene {
    /// `Provides` / `Requires` / `BindsTo` relationship handles.
    pub rels: Rels,
    /// **canonical** capability name (`std:Health`, `acme:Health`) → interned handle.
    pub caps: HashMap<String, Entity>,
    /// handle → **display** name (`Health`, `Health (acme)`) — the reveal's "doesn't provide X" reason.
    pub cap_name: HashMap<Entity, String>,
    /// The `AliasOf` relationship handle: a custom cap `--AliasOf--> std cap` pair records the opt-in.
    pub alias_of: Entity,
    /// custom cap handle → the standard cap it aliases. Resolved into an extra `Provides` pair at apply,
    /// so a `std:X` requirer binds an `author:X (AliasOf std:X)` provider — across authors.
    pub alias: HashMap<Entity, Entity>,
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
        let alias_of = world.create_entity();
        let mut caps = HashMap::new();
        let mut cap_name = HashMap::new();
        let mut alias = HashMap::new();

        // 1. The standard vocabulary: every capability the stdlib uses (provides + requires), interned
        //    by canonical `std:` key, deterministically (sorted). The world is owned by the engine after
        //    this, so every cap an applied entity could need must be interned up front.
        let mut names: Vec<String> = metrocalk_core::stdlib::standard_components()
            .iter()
            .flat_map(|m| m.provides.iter().chain(m.requires.iter()).cloned())
            .collect();
        names.sort();
        names.dedup();
        for name in &names {
            intern_cap(world, &mut caps, &mut cap_name, name);
        }

        // 2. The marketplace catalog's caps + their `(AliasOf, std:*)` opt-ins. A custom cap
        //    (`acme:Health`) is its own entity; aliasing records the pair + the resolution map so an
        //    applied provider also provides the standard cap (reveal/bind works across authors).
        for entry in metrocalk_core::marketplace::builtin_catalog() {
            for cap in entry.provides.iter().chain(entry.requires.iter()) {
                let cap_canon = cap.canonical_name();
                let c = intern_cap(world, &mut caps, &mut cap_name, &cap_canon);
                if let Some(std_name) = cap.canonical_alias() {
                    // **One-directional, toward the standard vocab (the adversarial guard, ADR-015):**
                    // only a CUSTOM cap may alias a STANDARD cap. A `std:* AliasOf author:*` (an author
                    // trying to re-point / hijack a standard cap) is ignored — the std cap entity is
                    // never made to alias an author's cap, so no cap can redefine `std:Health`.
                    if !is_standard(&cap_canon) && is_standard(&std_name) {
                        let s = intern_cap(world, &mut caps, &mut cap_name, &std_name);
                        if c != s {
                            world.add_pair(c, alias_of, s); // the relational `(AliasOf, std)` pair
                            alias.insert(c, s);
                        }
                    }
                }
            }
        }

        Self {
            rels,
            caps,
            cap_name,
            alias_of,
            alias,
        }
    }

    /// A capability handle by name (canonicalized — `Health` and `std:Health` resolve to the same
    /// entity). Panics on an un-interned name (the seed + catalog intern everything used).
    #[must_use]
    pub fn cap(&self, name: &str) -> Entity {
        self.caps[&canonical(name)]
    }
}

/// Intern a capability by its canonical name (dedup), recording its display name. A free fn (not a
/// closure) so it can borrow `world` + the maps disjointly.
fn intern_cap(
    world: &mut FlecsWorld,
    caps: &mut HashMap<String, Entity>,
    cap_name: &mut HashMap<Entity, String>,
    name: &str,
) -> Entity {
    let key = canonical(name);
    if let Some(&e) = caps.get(&key) {
        return e;
    }
    let e = world.create_entity();
    caps.insert(key.clone(), e);
    cap_name.insert(e, display_name(&key));
    e
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

/// The tracking-line segments for the live bindings: a flat list where each consecutive **pair** of
/// points is one segment, between the two bound entities' `Transform` centres. This is what makes a
/// *restored* bind visible in the viewport on reload — the shell's `rebuild` maps each point to a render
/// instance and `vs_line` draws the segments (no click required). Kept pure over the engine so the
/// restored-bind visualization is unit-testable without a live GPU. A binding to a non-live entity is
/// skipped; a live entity missing a `Transform` field contributes `0.0` there (the viewport default).
#[must_use]
pub fn tracking_segments(engine: &Engine<FlecsWorld>) -> Vec<[f32; 3]> {
    let pos_of = |id: EntityId| -> Option<[f32; 3]> {
        engine.ecs_entity(id)?; // skip a binding referencing a non-live entity
        let comps = engine.components_of(id);
        let t = comps.get("Transform");
        let g = |f: &str| -> f32 {
            t.and_then(|m| m.get(f)).map_or(0.0, |v| match v {
                FieldValue::Number(n) => *n as f32,
                FieldValue::Integer(i) => *i as f32,
                _ => 0.0,
            })
        };
        Some([g("x"), g("y"), g("z")])
    };
    let mut out = Vec::new();
    for (from, _kind, to) in engine.bindings() {
        if let (Some(a), Some(b)) = (pos_of(from), pos_of(to)) {
            out.push(a);
            out.push(b);
        }
    }
    out
}

/// Instantiate a resolved component KIND as a new pre-componentized scene entity — a `Transform` (so it
/// renders) + the kind's own component (default fields) + its capability pairs (provides/requires) —
/// all through the commit pipeline as ONE undoable transaction. This is the "working object, not dead
/// geometry" the describe-to-create loop drops in; its `requires` drive the M3.1 reveal for attach.
///
/// # Errors
/// Propagates a [`PipelineError`] if the create transaction fails (it shouldn't — ops are consistent).
pub fn instantiate(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    meta: &ComponentMeta,
    pos: [f32; 3],
    mesh: Option<&str>,
) -> Result<EntityId, PipelineError> {
    let id = engine.alloc_entity_id();
    let mut ops = vec![Op::CreateEntity { id, parent: None }];
    for (f, v) in [("x", pos[0]), ("y", pos[1]), ("z", pos[2])] {
        ops.push(Op::SetField {
            entity: id,
            component: "Transform".into(),
            field: f.into(),
            value: FieldValue::Number(f64::from(v)),
        });
    }
    // the kind's own component, with default field values — a real, inspectable component record
    for field in &meta.fields {
        ops.push(Op::SetField {
            entity: id,
            component: meta.name.clone(),
            field: field.name.clone(),
            value: default_value(field.ty),
        });
    }
    // the working capabilities: provides/requires as ECS pairs the reveal + attach use (canonicalized —
    // a stdlib kind's bare `"Health"` interns at `std:Health`).
    for cap in &meta.provides {
        if let Some(&c) = scene.caps.get(&canonical(cap)) {
            ops.push(Op::AddPair {
                entity: id,
                rel: scene.rels.provides,
                target: c,
            });
        }
    }
    for cap in &meta.requires {
        if let Some(&c) = scene.caps.get(&canonical(cap)) {
            ops.push(Op::AddPair {
                entity: id,
                rel: scene.rels.requires,
                target: c,
            });
        }
    }
    // If the kind has an associated mesh asset, carry its **handle** (only the handle — geometry stays
    // in the store beside the doc, invariant 2) so the entity renders as that mesh, not a cube.
    if let Some(handle) = mesh {
        ops.push(Op::SetField {
            entity: id,
            component: "MeshRenderer".into(),
            field: MESH_FIELD.into(),
            value: FieldValue::Str(handle.to_string()),
        });
    }
    engine.commit("describe-create", ops)?;
    Ok(id)
}

/// Place an imported mesh as a new entity — a `Transform` + a `MeshRenderer` carrying the asset
/// **handle** + the `Renderable` capability pair — as ONE undoable transaction. The direct
/// import→place path (the headless asset test; a future UI "drop this model"); describe-to-create
/// reuses [`instantiate`]'s mesh arm instead. The handle is opaque here; the renderer resolves it
/// against the store, and a reload re-resolves it (content-addressed id determinism, ADR-013).
///
/// # Errors
/// Propagates a [`PipelineError`] if the create transaction fails (it shouldn't — ops are consistent).
pub fn place_mesh(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    handle: &str,
    pos: [f32; 3],
) -> Result<EntityId, PipelineError> {
    let id = engine.alloc_entity_id();
    let mut ops = vec![Op::CreateEntity { id, parent: None }];
    for (f, v) in [("x", pos[0]), ("y", pos[1]), ("z", pos[2])] {
        ops.push(Op::SetField {
            entity: id,
            component: "Transform".into(),
            field: f.into(),
            value: FieldValue::Number(f64::from(v)),
        });
    }
    ops.push(Op::SetField {
        entity: id,
        component: "MeshRenderer".into(),
        field: MESH_FIELD.into(),
        value: FieldValue::Str(handle.to_string()),
    });
    if let Some(&c) = scene.caps.get(&canonical("Renderable")) {
        ops.push(Op::AddPair {
            entity: id,
            rel: scene.rels.provides,
            target: c,
        });
    }
    engine.commit("place-mesh", ops)?;
    Ok(id)
}

/// Apply a **marketplace entry** as a new pre-componentized scene entity — its component (display
/// marker) + its **namespaced** capability pairs (provides/requires, with an aliased custom cap also
/// providing its standard cap) + its mesh **handle** — all as ONE undoable transaction (invariant 3).
/// This is the marketplace tier's "arrives already wired, not a dead file": identical UX to a local
/// describe-create, only the *source* differs. The caps must be interned (the catalog's are, up front
/// in [`CapScene::intern`]); an un-interned cap is skipped. `mesh` is the asset handle the shell
/// resolved from the entry's logical asset name (or `None` → the honest cube fallback).
///
/// # Errors
/// Propagates a [`PipelineError`] if the create transaction fails (it shouldn't — ops are consistent).
pub fn apply_marketplace_entry(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    entry: &MarketplaceEntry,
    pos: [f32; 3],
    mesh: Option<&str>,
) -> Result<EntityId, PipelineError> {
    let id = engine.alloc_entity_id();
    let mut ops = vec![Op::CreateEntity { id, parent: None }];
    for (f, v) in [("x", pos[0]), ("y", pos[1]), ("z", pos[2])] {
        ops.push(Op::SetField {
            entity: id,
            component: "Transform".into(),
            field: f.into(),
            value: FieldValue::Number(f64::from(v)),
        });
    }
    // The entry's component as an inspectable record (named for the inspector), carrying its source id.
    ops.push(Op::SetField {
        entity: id,
        component: entry.component.clone(),
        field: "source".into(),
        value: FieldValue::Str(entry.id.clone()),
    });
    if let Some(handle) = mesh {
        ops.push(Op::SetField {
            entity: id,
            component: "MeshRenderer".into(),
            field: MESH_FIELD.into(),
            value: FieldValue::Str(handle.to_string()),
        });
    }
    // provides: the namespaced cap, plus its standard cap when aliased (so a `std:X` requirer binds it).
    for cap in &entry.provides {
        if let Some(&c) = scene.caps.get(&cap.canonical_name()) {
            ops.push(Op::AddPair {
                entity: id,
                rel: scene.rels.provides,
                target: c,
            });
            if let Some(&std_cap) = scene.alias.get(&c) {
                ops.push(Op::AddPair {
                    entity: id,
                    rel: scene.rels.provides,
                    target: std_cap,
                });
            }
        }
    }
    for cap in &entry.requires {
        if let Some(&c) = scene.caps.get(&cap.canonical_name()) {
            ops.push(Op::AddPair {
                entity: id,
                rel: scene.rels.requires,
                target: c,
            });
        }
    }
    engine.commit("apply-marketplace", ops)?;
    Ok(id)
}

/// Place a **grey placeholder** for a generation in flight (M6) — a working object as ONE undoable
/// transaction: a `Transform` + a `MeshRenderer` with an **empty** mesh handle (so it renders as the
/// M2.2 cube placeholder until the real mesh streams in) + `provides Renderable` + `requires Spatial`,
/// so it's **bindable at once**. The generated mesh later streams in as a validated AI patch
/// (`SetField MeshRenderer.mesh = handle`); undo peels the swap, then the placeholder. The grey cube
/// is real + usable regardless of whether generation ever returns (the adversarial guard).
///
/// # Errors
/// Propagates a [`PipelineError`] if the create transaction fails (it shouldn't — ops are consistent).
pub fn place_generation_placeholder(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    pos: [f32; 3],
) -> Result<EntityId, PipelineError> {
    let id = engine.alloc_entity_id();
    let mut ops = vec![Op::CreateEntity { id, parent: None }];
    for (f, v) in [("x", pos[0]), ("y", pos[1]), ("z", pos[2])] {
        ops.push(Op::SetField {
            entity: id,
            component: "Transform".into(),
            field: f.into(),
            value: FieldValue::Number(f64::from(v)),
        });
    }
    // empty handle → the cube placeholder; the stream-in swaps in the generated mesh handle.
    ops.push(Op::SetField {
        entity: id,
        component: "MeshRenderer".into(),
        field: MESH_FIELD.into(),
        value: FieldValue::Str(String::new()),
    });
    if let Some(&c) = scene.caps.get(&canonical("Renderable")) {
        ops.push(Op::AddPair {
            entity: id,
            rel: scene.rels.provides,
            target: c,
        });
    }
    if let Some(&c) = scene.caps.get(&canonical("Spatial")) {
        ops.push(Op::AddPair {
            entity: id,
            rel: scene.rels.requires,
            target: c,
        });
    }
    engine.commit("generate-placeholder", ops)?;
    Ok(id)
}

/// The deterministic offset a [`duplicate_entity`] clone is placed at, beside its source (so it's
/// visible, not hidden inside the original). Fixed → replay reproduces the clone's position exactly.
const DUPLICATE_OFFSET_X: f32 = 1.5;

/// **Remove** an entity as ONE undoable transaction (invariant 3): delete it *and* clean up every
/// binding it participates in — so a dependent that was tracking a removed provider is **freed** (its
/// requirement re-opens, the reveal re-offers), and no dangling edge survives. For a binding `from
/// --tracks--> to` involving `id`: the edge is removed, and when `id` is the **requirer** (`from`) the
/// provider's consumed-marker `(BindsTo, id)` pair is removed too (so the freed provider re-enters the
/// candidate set); when `id` is the **provider** (`to`) its own pairs go with the delete. Undo
/// restores the entity (M1.6 entity-resurrection) **and** the edges + pairs, atomically.
///
/// # Errors
/// [`PipelineError`] if the transaction fails (e.g. the entity is already gone).
pub fn remove_entity(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    id: EntityId,
) -> Result<(), PipelineError> {
    let id_ecs = engine.ecs_entity(id);
    let mut ops = Vec::new();
    for (from, kind, to) in engine.bindings() {
        if from != id && to != id {
            continue;
        }
        ops.push(Op::RemoveBinding { from, kind, to });
        // `id` is the requirer of this binding → free the provider `to`'s consumed-marker
        // `(BindsTo, id)` pair (the pair lives on the provider; capscene::bind added it there).
        if from == id {
            if let Some(id_ecs) = id_ecs {
                ops.push(Op::RemovePair {
                    entity: to,
                    rel: scene.rels.binds_to,
                    target: id_ecs,
                });
            }
        }
    }
    ops.push(Op::DeleteEntity { id });
    engine.commit("remove-entity", ops)
}

/// **Duplicate** an entity as ONE undoable transaction (invariant 3): clone its components (fields) +
/// its `Provides`/`Requires` capability pairs under a **fresh deterministic id** ([`Engine::alloc_entity_id`]),
/// placed beside the source. The clone is **independently bindable**: its `BindsTo`/binding edges are
/// **not** cloned (a fresh copy is unbound), so it re-enters the reveal as its own requirer/provider.
/// Deterministic id + offset → a replayed duplicate lands byte-identical (ADR-013). Undo removes it.
///
/// # Errors
/// [`PipelineError`] if the source isn't a live entity, or the create transaction fails.
pub fn duplicate_entity(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    src: EntityId,
) -> Result<EntityId, PipelineError> {
    let src_ecs = engine
        .ecs_entity(src)
        .ok_or(PipelineError::UnknownEntity(src))?;
    let new_id = engine.alloc_entity_id();
    let parent = engine.parent_of(src);
    let mut ops = vec![Op::CreateEntity { id: new_id, parent }];

    // Clone every component field. Then offset the Transform x so the clone sits beside the source
    // (the later SetField wins). Read the source x first.
    let comps = engine.components_of(src);
    let src_x = comps
        .get("Transform")
        .and_then(|t| t.get("x"))
        .map_or(0.0, |v| match v {
            FieldValue::Number(n) => *n as f32,
            FieldValue::Integer(i) => *i as f32,
            _ => 0.0,
        });
    for (component, fields) in comps {
        for (field, value) in fields {
            ops.push(Op::SetField {
                entity: new_id,
                component: component.clone(),
                field,
                value,
            });
        }
    }
    ops.push(Op::SetField {
        entity: new_id,
        component: "Transform".into(),
        field: "x".into(),
        value: FieldValue::Number(f64::from(src_x + DUPLICATE_OFFSET_X)),
    });

    // Clone the capability pairs (provides/requires) — NOT BindsTo, so the clone is fresh + unbound.
    for cap in engine.world().targets(src_ecs, scene.rels.provides) {
        ops.push(Op::AddPair {
            entity: new_id,
            rel: scene.rels.provides,
            target: cap,
        });
    }
    for cap in engine.world().targets(src_ecs, scene.rels.requires) {
        ops.push(Op::AddPair {
            entity: new_id,
            rel: scene.rels.requires,
            target: cap,
        });
    }

    engine.commit("duplicate-entity", ops)?;
    Ok(new_id)
}

/// Describe-to-create, end to end (local tier): resolve `query` over the stdlib and, on a confident
/// match, [`instantiate`] it pre-componentized at `pos` (one undoable transaction). Returns the new
/// entity + the resolved kind name, or `None` for an honest no-match (→ the marketplace seam).
pub fn describe_create(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    query: &str,
    pos: [f32; 3],
    catalog: &MeshCatalog,
) -> Option<(EntityId, String)> {
    let lib = metrocalk_core::stdlib::standard_components();
    let top = metrocalk_core::resolve::resolve_local(&lib, query)
        .matches
        .into_iter()
        .next()?;
    let meta = lib.iter().find(|m| m.name == top.kind)?;
    // A resolved kind WITH an asset in the catalog instantiates *looking* like itself; without one it
    // falls back to the placeholder cube — honest, not hidden (the cube fallback is the renderer's).
    let handle = catalog.get(&top.kind).cloned();
    let id = instantiate(engine, scene, meta, pos, handle.as_deref()).ok()?;
    Some((id, top.kind))
}

fn default_value(ty: FieldType) -> FieldValue {
    match ty {
        FieldType::Integer => FieldValue::Integer(0),
        FieldType::Number => FieldValue::Number(0.0),
        FieldType::Boolean => FieldValue::Bool(false),
        FieldType::String => FieldValue::Str(String::new()),
    }
}
