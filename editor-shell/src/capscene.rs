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
use metrocalk_core::variant::INSTANCE_META;
use metrocalk_core::{
    CapRole, CapabilityResolver, ComponentMeta, Composition, Engine, EntityId, FieldType,
    FieldValue, Op, PipelineError,
};
use metrocalk_ecs::rng::Rng;
use metrocalk_ecs::{Entity, FlecsWorld, World};
// M9.5 / G5 deform: a handle deform is saved as a G2 override; `reconstruct_part_deform` reads it back
// + runs the deterministic ARAP deformer (no foreign math type leaks — plain f64 arrays).
use metrocalk_deform::{ArapConfig, ArapDeformer, DeformMesh, Deformer, Region};
// M9.2 part editing reuses G1's math (no new gizmo math) — the plain-array `Transform` + the
// parent-space write-back; glam stays behind the `Gizmo` trait (the public types are arrays).
use metrocalk_gizmo::{mat_mul, to_local, Transform as GizmoTransform};

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

/// A standalone [`CapabilityResolver`] (ADR-032) built from a [`CapScene`] — set on the [`Engine`]
/// (`Engine::set_capability_resolver`) so the commit pipeline mirrors capability pairs into the durable
/// Loro document by canonical name, and a load/merge re-derives them (the capability-rebuild fix for
/// the M1.6 carry-forward). It **clones** the scene's relationship handles + the cap name↔handle maps
/// (handles are `Copy`), so the engine can own the resolver while the rest of the shell keeps borrowing
/// the `CapScene`. Resolution is by **canonical name**, so it is correct even though a fresh launch's
/// interned cap handles differ run-to-run — the persisted name re-interns to this world's handle.
pub struct CapResolver {
    rels: Rels,
    /// canonical name (`std:Health`) → interned handle (resolve a persisted pair back on load).
    handle_of: HashMap<String, Entity>,
    /// interned handle → canonical name (persist a pair by stable name at commit).
    name_of: HashMap<Entity, String>,
}

impl CapResolver {
    /// Build a resolver mirroring `scene`'s interned capability vocabulary.
    #[must_use]
    pub fn from_scene(scene: &CapScene) -> Self {
        let mut name_of = HashMap::with_capacity(scene.caps.len());
        for (canon, &handle) in &scene.caps {
            name_of.insert(handle, canon.clone());
        }
        Self {
            rels: scene.rels,
            handle_of: scene.caps.clone(),
            name_of,
        }
    }
}

impl CapabilityResolver for CapResolver {
    fn classify_pair(&self, rel: Entity, target: Entity) -> Option<(CapRole, String)> {
        // Only the two cap relationships the scene uses are mirrored; BindsTo (and any other rel) is
        // reconstructed from the bindings map, so it returns `None` and is not persisted here.
        let role = if rel == self.rels.provides {
            CapRole::Provides
        } else if rel == self.rels.requires {
            CapRole::Requires
        } else {
            return None;
        };
        Some((role, self.name_of.get(&target)?.clone()))
    }

    fn resolve_pair(&self, role: CapRole, canonical_name: &str) -> Option<(Entity, Entity)> {
        let rel = match role {
            CapRole::Provides => self.rels.provides,
            CapRole::Requires => self.rels.requires,
            // The shell scene interns no Observes relationship (no scene entity uses one), so a
            // persisted Observes pair — there are none today — is skipped rather than mis-bound.
            CapRole::Observes => return None,
        };
        Some((rel, *self.handle_of.get(canonical_name)?))
    }

    fn binds_to_rel(&self) -> Entity {
        self.rels.binds_to
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
/// silently bind the wrong entities). **Bump the `mtksceneN` tag whenever [`seed`]'s — or the
/// post-seed deterministic construction's — draw sequence changes** (M9.2 added a composed character
/// right after the seed, shifting the post-seed alloc sequence → `mtkscene2`).
#[must_use]
pub fn fingerprint(n: usize) -> String {
    format!("mtkscene2 seed={SEED:#x} n={n}")
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
/// **Reload (RESOLVED, ADR-032):** the reveal's exclusion depends on the ECS `BindsTo` pair, which
/// is ECS-only (not in Loro). Earlier `Engine::merge` rebuilt entities but not their pairs, so a
/// merge/load dropped the exclusion and re-offered the bound provider (the M1.6 carry-forward). Now
/// `rebuild_ecs_from_loro` re-derives `(BindsTo, requirer)` for each persisted binding edge (and the
/// provides/requires pairs from the durable `caps` mirror), so a binding's exclusion survives a load
/// exactly as it survives undo — via the [`CapResolver`] set on the engine.
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

/// M11.3 (ADR-042) — author a `Light` entity as ONE undoable transaction: a `Transform` (so it has a
/// position + rides the gizmo/inspector like any entity) + a `Light` component (`kind`/colour/intensity/
/// range) + the `Provides(Lighting)` capability pair. Replayed by id so the light survives close→reopen.
/// `kind` ∈ {"directional","point","spot"}. The per-frame LIT RESULT is a render projection (never Loro).
pub fn add_light(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    kind: &str,
    pos: [f32; 3],
    color: [f32; 3],
    intensity: f32,
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
        component: "Light".into(),
        field: "kind".into(),
        value: FieldValue::Str(kind.to_string()),
    });
    ops.push(Op::SetField {
        entity: id,
        component: "Light".into(),
        field: "intensity".into(),
        value: FieldValue::Number(f64::from(intensity)),
    });
    for (f, v) in [("r", color[0]), ("g", color[1]), ("b", color[2])] {
        ops.push(Op::SetField {
            entity: id,
            component: "Light".into(),
            field: f.into(),
            value: FieldValue::Number(f64::from(v)),
        });
    }
    // M11.3 — author the shine DIRECTION (a directional light defaults straight down) + whether the light
    // CASTS the shadow map, as REAL component fields so they EXIST on the entity: the data-driven inspector
    // can edit/toggle them (one undoable `SetField`), and `collect_lights` reads the authored values instead
    // of its absent-field fallbacks. (Render-only effect; the lit/shadow RESULT stays a projection — ADR-021.)
    for (f, v) in [("dirX", 0.0_f32), ("dirY", -1.0), ("dirZ", 0.0)] {
        ops.push(Op::SetField {
            entity: id,
            component: "Light".into(),
            field: f.into(),
            value: FieldValue::Number(f64::from(v)),
        });
    }
    ops.push(Op::SetField {
        entity: id,
        component: "Light".into(),
        field: "castShadows".into(),
        value: FieldValue::Bool(true),
    });
    if let Some(&c) = scene.caps.get(&canonical("Lighting")) {
        ops.push(Op::AddPair {
            entity: id,
            rel: scene.rels.provides,
            target: c,
        });
    }
    engine.commit("add-light", ops)?;
    Ok(id)
}

/// Spawn a complete, simulatable physics body as ONE undoable transaction (M8.2): a `Transform` + a
/// dynamic `RigidBody` + a ball `Collider` + (optionally) a `MeshRenderer` handle so it renders as a real
/// mesh, plus the physics capability pairs the reveal/attach use. This is ECS-authoritative **setup** —
/// the live simulation is mirrored into the project-owned `Physics` trait by the engine thread *after*
/// this commits, and undo removes the whole body in one step. Field values are written **explicitly**
/// (`kind="dynamic"`, `shape="ball"`, an explicit `radius`) so the body is valid without relying on the
/// generic [`default_value`] (which would leave `kind`/`shape` empty). The per-tick transform stream is a
/// separate projection, never a commit — so this never floods the undo stack (ADR-021: sim-replay is a
/// distinct channel from Loro time-travel).
///
/// # Errors
/// Propagates a [`PipelineError`] if the create transaction fails (it shouldn't — ops are consistent).
pub fn spawn_physics_body(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    handle: Option<&str>,
    pos: [f32; 3],
    radius: f32,
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
    // A dynamic RigidBody + a ball Collider, fields explicit (no reliance on default_value).
    for (component, field, value) in [
        ("RigidBody", "kind", FieldValue::Str("dynamic".into())),
        ("RigidBody", "mass", FieldValue::Number(1.0)),
        ("Collider", "shape", FieldValue::Str("ball".into())),
        ("Collider", "radius", FieldValue::Number(f64::from(radius))),
    ] {
        ops.push(Op::SetField {
            entity: id,
            component: component.into(),
            field: field.into(),
            value,
        });
    }
    // Capability pairs for the intent system (the body is queryable as a Physics + Collision provider,
    // and — like every spatial thing — a Spatial requirer). Canonicalized; skipped if not interned.
    for (rel, cap) in [
        (scene.rels.provides, "Physics"),
        (scene.rels.provides, "Collision"),
        (scene.rels.requires, "Spatial"),
    ] {
        if let Some(&c) = scene.caps.get(&canonical(cap)) {
            ops.push(Op::AddPair {
                entity: id,
                rel,
                target: c,
            });
        }
    }
    if let Some(h) = handle {
        ops.push(Op::SetField {
            entity: id,
            component: "MeshRenderer".into(),
            field: MESH_FIELD.into(),
            value: FieldValue::Str(h.to_string()),
        });
    }
    engine.commit("spawn-physics-body", ops)?;
    Ok(id)
}

/// Instantiate a parsed [`SceneImport`](metrocalk_interchange::SceneImport) (URDF / USD-Physics, M8.5) as
/// registry-component entities in **ONE undoable transaction** (invariant 3): each imported body → a
/// `Transform` + `RigidBody` + `Collider` (+ the Physics/Collision/Spatial caps so it rides the intent
/// system), each imported joint → a `Joint` component referencing its two body entities. Returns the body
/// entity ids (parallel to `import.bodies`). The import is intent-wired + inspectable + undoable like any
/// scene edit — the foreign format becomes ordinary entities, no privileged objects.
///
/// # Errors
/// Propagates a [`PipelineError`] if the transaction fails (the ops are registry-consistent by construction).
#[allow(clippy::too_many_lines)] // a flat body+joint→ops mapping; splitting it would obscure, not clarify
pub fn import_scene(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    import: &metrocalk_interchange::SceneImport,
) -> Result<Vec<EntityId>, PipelineError> {
    use metrocalk_interchange::{BodyKind, ColliderShape, JointDesc};

    let body_ids: Vec<EntityId> = import
        .bodies
        .iter()
        .map(|_| engine.alloc_entity_id())
        .collect();
    let mut ops = Vec::new();
    let set = |ops: &mut Vec<Op>, id, comp: &str, field: &str, value| {
        ops.push(Op::SetField {
            entity: id,
            component: comp.into(),
            field: field.into(),
            value,
        });
    };

    for (body, &id) in import.bodies.iter().zip(&body_ids) {
        ops.push(Op::CreateEntity { id, parent: None });
        for (f, v) in [
            ("x", body.translation[0]),
            ("y", body.translation[1]),
            ("z", body.translation[2]),
        ] {
            set(&mut ops, id, "Transform", f, FieldValue::Number(v));
        }
        let kind = match body.kind {
            BodyKind::Fixed => "fixed",
            BodyKind::KinematicPosition => "kinematicPosition",
            BodyKind::KinematicVelocity => "kinematicVelocity",
            BodyKind::Dynamic => "dynamic",
        };
        set(
            &mut ops,
            id,
            "RigidBody",
            "kind",
            FieldValue::Str(kind.into()),
        );
        if let Some(m) = body.mass {
            set(&mut ops, id, "RigidBody", "mass", FieldValue::Number(m));
        }
        if let Some(col) = &body.collider {
            set(
                &mut ops,
                id,
                "Collider",
                "density",
                FieldValue::Number(col.density),
            );
            set(
                &mut ops,
                id,
                "Collider",
                "friction",
                FieldValue::Number(col.friction),
            );
            set(
                &mut ops,
                id,
                "Collider",
                "restitution",
                FieldValue::Number(col.restitution),
            );
            match &col.shape {
                ColliderShape::Ball { radius } => {
                    set(
                        &mut ops,
                        id,
                        "Collider",
                        "shape",
                        FieldValue::Str("ball".into()),
                    );
                    set(
                        &mut ops,
                        id,
                        "Collider",
                        "radius",
                        FieldValue::Number(*radius),
                    );
                }
                ColliderShape::Cuboid { half_extents } => {
                    set(
                        &mut ops,
                        id,
                        "Collider",
                        "shape",
                        FieldValue::Str("cuboid".into()),
                    );
                    set(
                        &mut ops,
                        id,
                        "Collider",
                        "halfX",
                        FieldValue::Number(half_extents[0]),
                    );
                    set(
                        &mut ops,
                        id,
                        "Collider",
                        "halfY",
                        FieldValue::Number(half_extents[1]),
                    );
                    set(
                        &mut ops,
                        id,
                        "Collider",
                        "halfZ",
                        FieldValue::Number(half_extents[2]),
                    );
                }
                ColliderShape::Capsule {
                    half_height,
                    radius,
                } => {
                    set(
                        &mut ops,
                        id,
                        "Collider",
                        "shape",
                        FieldValue::Str("capsule".into()),
                    );
                    set(
                        &mut ops,
                        id,
                        "Collider",
                        "halfHeight",
                        FieldValue::Number(*half_height),
                    );
                    set(
                        &mut ops,
                        id,
                        "Collider",
                        "radius",
                        FieldValue::Number(*radius),
                    );
                }
                // Hull/tri-mesh/seam shapes (e.g. a URDF mesh collider) carry no primitive params — a
                // ball stand-in keeps the body simulable; the import note explains the real story.
                _ => {
                    set(
                        &mut ops,
                        id,
                        "Collider",
                        "shape",
                        FieldValue::Str("ball".into()),
                    );
                    set(&mut ops, id, "Collider", "radius", FieldValue::Number(0.25));
                }
            }
        }
        for (rel, cap) in [
            (scene.rels.provides, "Physics"),
            (scene.rels.provides, "Collision"),
            (scene.rels.requires, "Spatial"),
        ] {
            if let Some(&c) = scene.caps.get(&canonical(cap)) {
                ops.push(Op::AddPair {
                    entity: id,
                    rel,
                    target: c,
                });
            }
        }
    }

    // Joints → a `Joint` component referencing the two body entities (inspectable; the editor's live
    // joint-constrained sim is a named follow-up — the mapping itself is proven in /interchange).
    for joint in &import.joints {
        let (Some(&a), Some(&b)) = (body_ids.get(joint.parent), body_ids.get(joint.child)) else {
            continue;
        };
        let id = engine.alloc_entity_id();
        ops.push(Op::CreateEntity { id, parent: None });
        let kind = match joint.joint {
            JointDesc::Revolute { .. } => "revolute",
            JointDesc::Spherical { .. } => "spherical",
            // Fixed + any future variant → the rigid-weld default.
            _ => "fixed",
        };
        set(&mut ops, id, "Joint", "kind", FieldValue::Str(kind.into()));
        set(
            &mut ops,
            id,
            "Joint",
            "bodyA",
            FieldValue::Str(a.to_loro_key()),
        );
        set(
            &mut ops,
            id,
            "Joint",
            "bodyB",
            FieldValue::Str(b.to_loro_key()),
        );
    }

    engine.commit("import-interchange", ops)?;
    Ok(body_ids)
}

/// Commit an entity's `Transform` — position (x/y/z) + rotation quaternion (qx/qy/qz/qw) + uniform display
/// scale — as **ONE undoable transaction** (M9.1 — the coalesced gizmo-drag result, or a replayed transform
/// edit). All `SetField` ops in a single `engine.commit`, so Ctrl-Z reverses the whole move atomically
/// (invariant 3). The shell's `Transform` uses its own `x/y/z` + `qx/qy/qz/qw` + `scale` field convention
/// (the same minimal-placeholder convention as `x/y/z`); the renderer reads them back.
///
/// # Errors
/// Propagates a [`PipelineError`] if the transaction fails (the ops are registry-consistent by construction).
pub fn set_transform(
    engine: &mut Engine<FlecsWorld>,
    id: EntityId,
    pos: [f32; 3],
    rot: [f32; 4],
    scale: f32,
) -> Result<(), PipelineError> {
    let field = |f: &str, v: f32| Op::SetField {
        entity: id,
        component: "Transform".into(),
        field: f.into(),
        value: FieldValue::Number(f64::from(v)),
    };
    let ops = vec![
        field("x", pos[0]),
        field("y", pos[1]),
        field("z", pos[2]),
        field("qx", rot[0]),
        field("qy", rot[1]),
        field("qz", rot[2]),
        field("qw", rot[3]),
        field("scale", scale),
    ];
    engine.commit("gizmo-transform", ops)
}

// ── M9.2 (G2): rigid part editing — G1's gizmo applied to a CHILD node (ADR-026) ───────────────────

/// Read a part's **effective LOCAL** transform — the Transform component RESOLVED through the override
/// layer (base ⊕ override, override-wins; [`Engine::resolved_components`]) into a gizmo [`GizmoTransform`].
/// So a part's per-field override drives its local TRS, and missing fields default to identity
/// (translation 0, quat identity, scale 1 — the renderer/`ReadTransform` convention).
#[must_use]
pub fn local_transform(engine: &Engine<FlecsWorld>, id: EntityId) -> GizmoTransform {
    let comps = engine.resolved_components(id);
    let t = comps.get("Transform");
    let g = |f: &str, default: f32| -> f32 {
        t.and_then(|m| m.get(f)).map_or(default, |v| match v {
            FieldValue::Number(n) => *n as f32,
            FieldValue::Integer(i) => *i as f32,
            _ => default,
        })
    };
    let s = g("scale", 1.0);
    GizmoTransform {
        translation: [g("x", 0.0), g("y", 0.0), g("z", 0.0)],
        rotation: [g("qx", 0.0), g("qy", 0.0), g("qz", 0.0), g("qw", 1.0)],
        scale: [s, s, s],
    }
}

/// A part's **GLOBAL (world)** transform = `parent_global · local`, walking the Movable-Tree hierarchy
/// (`global(child) = global(parent) · local(child)`). This is why **descendants follow** a parent edit:
/// a parent's new local recomputes every descendant's global. Reuses G1's matrix math (no new gizmo math).
#[must_use]
pub fn global_transform(engine: &Engine<FlecsWorld>, id: EntityId) -> GizmoTransform {
    let local = local_transform(engine, id);
    match engine.parent_of(id) {
        Some(parent) => GizmoTransform::from_matrix(mat_mul(
            global_transform(engine, parent).to_matrix(),
            local.to_matrix(),
        )),
        None => local,
    }
}

/// Write a part's **LOCAL** TRS as a sparse **per-field override** (8 `SetOverride` ops in ONE undoable
/// transaction, ADR-026): "rotate the leg" and "scale the leg" are separate keys that never clobber, and
/// they overlay the part's base by structure (override-wins) — never a whole-object rewrite. This is the
/// M9.2 part edit + the replay primitive. Uniform display scale (rigid-part scope; non-uniform = G5).
///
/// # Errors
/// Propagates a [`PipelineError`] if the override transaction fails.
pub fn set_part_local(
    engine: &mut Engine<FlecsWorld>,
    id: EntityId,
    pos: [f32; 3],
    rot: [f32; 4],
    scale: f32,
) -> Result<(), PipelineError> {
    let ov = |f: &str, v: f32| Op::SetOverride {
        entity: id,
        component: "Transform".into(),
        field: f.into(),
        value: FieldValue::Number(f64::from(v)),
    };
    engine.commit(
        "edit-part",
        vec![
            ov("x", pos[0]),
            ov("y", pos[1]),
            ov("z", pos[2]),
            ov("qx", rot[0]),
            ov("qy", rot[1]),
            ov("qz", rot[2]),
            ov("qw", rot[3]),
            ov("scale", scale),
        ],
    )
}

/// **Parent-space write-back for a part** (G1 on a child, ADR-025 deliverable 4 applied to G2): the
/// gizmo acts in WORLD space, but a child stores its LOCAL transform, so `local = inverse(parent_global)
/// · world` ([`to_local`]). Stores the result as a per-field override ([`set_part_local`]); returns the
/// LOCAL TRS written (the caller persists the local, so replay is parent-independent + deterministic).
/// For a root part the parent is identity ⇒ `local == world` (the M9.1 flat-entity behavior preserved).
///
/// # Errors
/// Propagates a [`PipelineError`] if the override transaction fails.
pub fn edit_part_transform(
    engine: &mut Engine<FlecsWorld>,
    id: EntityId,
    world: GizmoTransform,
) -> Result<GizmoTransform, PipelineError> {
    let parent_world = match engine.parent_of(id) {
        Some(parent) => global_transform(engine, parent).to_matrix(),
        None => GizmoTransform::IDENTITY.to_matrix(),
    };
    let local = to_local(&world, parent_world);
    set_part_local(
        engine,
        id,
        local.translation,
        local.rotation,
        local.scale[0],
    )?;
    Ok(local)
}

/// **Reparent a part** ("drag in hierarchy") — one `node.move` op in ONE undoable transaction (the Loro
/// Movable-Tree move: fractional index + PeerID tiebreak). `new_parent = None` moves it to the root.
/// Undo restores the prior parent (the pipeline captures it as the inverse).
///
/// # Errors
/// [`PipelineError::UnknownEntity`] if the part or the new parent isn't a live entity.
pub fn reparent(
    engine: &mut Engine<FlecsWorld>,
    id: EntityId,
    new_parent: Option<EntityId>,
) -> Result<(), PipelineError> {
    engine.commit(
        "reparent-part",
        vec![Op::Reparent {
            entity: id,
            new_parent,
        }],
    )
}

/// **Deactivate-not-delete** a part (or reactivate it) — one undoable `SetActive` (ADR-026). The entity,
/// its data, and a concurrent editor's edits are all preserved; the renderer hides a deactivated child,
/// and undo re-activates it (the part reappears).
///
/// # Errors
/// [`PipelineError::UnknownEntity`] if `id` isn't a live entity.
pub fn set_part_active(
    engine: &mut Engine<FlecsWorld>,
    id: EntityId,
    active: bool,
) -> Result<(), PipelineError> {
    engine.commit(
        "set-part-active",
        vec![Op::SetActive { entity: id, active }],
    )
}

/// **Save a fidelity deformation as a G2 override** (M9.5 / G5, ADR-029): an ARAP/cage handle edit is
/// stored as the **sparse set of moved handle targets** — NOT the baked vertices (geometry never enters
/// the doc — invariant 2; the surface is *reproduced* deterministically from these targets, exactly as a
/// pose stores joint TRS deltas, not baked verts). Each handle is three per-field override ops
/// (`Deform / {idx}:{x,y,z}`) in ONE undoable transaction (ADR-026 per-field LWW: two handles never
/// clobber, and the deform overlays the base by structure). So a deform is automatically **undoable +
/// reload-persistent + merge-aware** — it rides the same override pipeline as a part transform.
///
/// # Errors
/// Propagates a [`PipelineError`] if the override transaction fails.
pub fn set_part_deform(
    engine: &mut Engine<FlecsWorld>,
    id: EntityId,
    handles: &[(usize, [f32; 3])],
) -> Result<(), PipelineError> {
    let mut ops = Vec::with_capacity(handles.len() * 3);
    for &(idx, t) in handles {
        for (axis, v) in [("x", t[0]), ("y", t[1]), ("z", t[2])] {
            ops.push(Op::SetOverride {
                entity: id,
                component: "Deform".into(),
                field: format!("{idx}:{axis}"),
                value: FieldValue::Number(f64::from(v)),
            });
        }
    }
    engine.commit("deform-part", ops)
}

/// Read back a part's stored deform — the `(handle_index, target)` pairs saved by [`set_part_deform`],
/// sorted by handle index. Empty ⇒ the part carries no deformation (its surface is the rest mesh).
#[must_use]
pub fn part_deform_handles(engine: &Engine<FlecsWorld>, id: EntityId) -> Vec<(usize, [f32; 3])> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<usize, [f32; 3]> = BTreeMap::new();
    for (key, value) in engine.overrides_of(id) {
        let Some((comp, field)) = key.split_once('\u{1f}') else {
            continue;
        };
        if comp != "Deform" {
            continue;
        }
        let (Some((idx_s, axis)), FieldValue::Number(n)) = (field.split_once(':'), value) else {
            continue;
        };
        let Ok(idx) = idx_s.parse::<usize>() else {
            continue;
        };
        let slot = map.entry(idx).or_insert([0.0; 3]);
        #[allow(clippy::cast_possible_truncation)]
        // override stores f64; the deform surface is f32-ish
        let n32 = n as f32;
        match axis {
            "x" => slot[0] = n32,
            "y" => slot[1] = n32,
            "z" => slot[2] = n32,
            _ => {}
        }
    }
    map.into_iter().collect()
}

/// **Reproduce a part's deformed surface** from its stored G2 deform override (M9.5 / G5): read the saved
/// handle targets, place them onto `region`'s handles (un-moved handles stay at rest), and run the
/// deterministic ARAP deformer over `mesh`. Returns the deformed `f64` vertex positions, or `None` if the
/// region's solve is degenerate. Deterministic — the override is the *only* persisted state; the surface
/// is recomputed (so it's reload-stable and never stored as full geometry).
#[must_use]
pub fn reconstruct_part_deform(
    mesh: &DeformMesh,
    region: &Region,
    engine: &Engine<FlecsWorld>,
    id: EntityId,
) -> Option<Vec<[f64; 3]>> {
    // Targets default to each handle's rest position; the stored overrides move the edited ones.
    let mut targets: Vec<[f64; 3]> = region
        .handles
        .iter()
        .map(|&h| mesh.positions[h as usize])
        .collect();
    for (idx, t) in part_deform_handles(engine, id) {
        if let Some(slot) = targets.get_mut(idx) {
            *slot = [f64::from(t[0]), f64::from(t[1]), f64::from(t[2])];
        }
    }
    let arap = ArapDeformer::prepare(mesh, region, ArapConfig::default())?;
    Some(arap.deform(&targets))
}

/// Seed a small **composed character** for rigid part editing (M9.2 / G2): a root "body" with two child
/// parts, each a child Movable-Tree node with its own LOCAL `Transform` (+ a `MeshRenderer` handle so it
/// renders, + a `Part` marker so the inspector names it) — ONE undoable transaction. Returns `(root,
/// [part0, part1])`. The parts are selected + gizmo-edited like any entity; editing a child writes a
/// per-field override ([`edit_part_transform`]), reparenting is [`reparent`], and the whole character is
/// saved for reuse via `Engine::save_composition`. Deterministic ids (seeded as scene construction) so
/// the override-edit persistence (`Record::EditPart`) re-binds across launches.
///
/// # Errors
/// Propagates a [`PipelineError`] if the create transaction fails (it shouldn't — ops are consistent).
pub fn compose_character(
    engine: &mut Engine<FlecsWorld>,
    pos: [f32; 3],
    mesh: Option<&str>,
) -> Result<(EntityId, Vec<EntityId>), PipelineError> {
    let root = engine.alloc_entity_id();
    let parts = [engine.alloc_entity_id(), engine.alloc_entity_id()];
    let tf = |entity, f: &str, v: f32| Op::SetField {
        entity,
        component: "Transform".into(),
        field: f.into(),
        value: FieldValue::Number(f64::from(v)),
    };
    let mesh_op = |entity| {
        mesh.map(|h| Op::SetField {
            entity,
            component: "MeshRenderer".into(),
            field: MESH_FIELD.into(),
            value: FieldValue::Str(h.to_string()),
        })
    };
    let mut ops = vec![
        Op::CreateEntity {
            id: root,
            parent: None,
        },
        tf(root, "x", pos[0]),
        tf(root, "y", pos[1]),
        tf(root, "z", pos[2]),
    ];
    ops.extend(mesh_op(root));
    // Two rigid child parts at local offsets either side of the body (local-space, so a body move/rotate
    // carries them — the "descendants follow" demo).
    for (part, dx) in [(parts[0], 1.2_f32), (parts[1], -1.2_f32)] {
        ops.push(Op::CreateEntity {
            id: part,
            parent: Some(root),
        });
        ops.push(tf(part, "x", dx));
        ops.push(tf(part, "y", 0.8));
        ops.extend(mesh_op(part));
        ops.push(Op::SetField {
            entity: part,
            component: "Part".into(),
            field: "kind".into(),
            value: FieldValue::Str("rigid".into()),
        });
    }
    engine.commit("compose-character", ops)?;
    Ok((root, parts.to_vec()))
}

/// The root of the Movable-Tree subtree `id` belongs to (walk parents to the top) — so the **whole
/// character** can be saved from any selected part (`Engine::save_composition` takes the root).
#[must_use]
pub fn root_of(engine: &Engine<FlecsWorld>, id: EntityId) -> EntityId {
    let mut cur = id;
    while let Some(p) = engine.parent_of(cur) {
        cur = p;
    }
    cur
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

// ═════════════════════════════════════════════════════════════════════════════════════════════════════
// M10.6 — SCENE-AUTHORING VERBS (ADR-036). The editor's compose-a-scene verbs — create · rename · reparent ·
// group/ungroup · multi-edit · copy/cut/paste · delete — each ONE undoable pipeline transaction (inv. 3)
// riding the **Loro Movable Tree** (reparent = `node.move`, cycle-safe) + the **override model** (delete =
// deactivate-not-destroy, M9.2). They EXTEND M3.3's surface (Remove/Duplicate/Focus/Inspect), reusing the
// shipped primitives: reparent reuses M9.2's `node.move`, delete reuses M9.2's `SetActive`, copy/paste
// reuses the variant `Composition` machinery — no new model, no fork. Verbs are commits → they mutate the
// Loro document, so they **survive reload** via the M10.3 `.mtk` save/open (snapshot+oplog) by construction.
// ═════════════════════════════════════════════════════════════════════════════════════════════════════

/// The `__meta__` field carrying an entity's user-facing **name** (M10.6 rename) — distinct from the
/// `composition` provenance field (variant.rs). Stored in the durable doc, so a name survives reload.
pub const NAME_FIELD: &str = "name";
/// The `__meta__` field tagging a created **primitive** kind (`cube`/`plane`/`empty`) for the renderer.
pub const PRIMITIVE_FIELD: &str = "primitive";

/// Read an entity's user-facing name (set by [`rename`]), or `None` if it was never named.
#[must_use]
pub fn entity_name(engine: &Engine<FlecsWorld>, id: EntityId) -> Option<String> {
    engine
        .components_of(id)
        .get(INSTANCE_META)
        .and_then(|m| m.get(NAME_FIELD))
        .and_then(|v| match v {
            FieldValue::Str(s) => Some(s.clone()),
            _ => None,
        })
}

/// Build the three `Transform` x/y/z `SetField` ops for `pos`.
fn transform_ops(id: EntityId, pos: [f32; 3]) -> Vec<Op> {
    ["x", "y", "z"]
        .into_iter()
        .zip(pos)
        .map(|(f, v)| Op::SetField {
            entity: id,
            component: "Transform".into(),
            field: f.into(),
            value: FieldValue::Number(f64::from(v)),
        })
        .collect()
}

/// **CREATE** an empty named entity at `pos` — ONE undoable transaction; the caller selects it. A
/// transform-only node (no mesh); a visible primitive is [`create_primitive`]. Undo removes it.
///
/// # Errors
/// [`PipelineError`] if the create transaction fails.
pub fn create_entity(
    engine: &mut Engine<FlecsWorld>,
    pos: [f32; 3],
    name: &str,
) -> Result<EntityId, PipelineError> {
    let id = engine.alloc_entity_id();
    let mut ops = vec![Op::CreateEntity { id, parent: None }];
    ops.extend(transform_ops(id, pos));
    ops.push(Op::SetField {
        entity: id,
        component: INSTANCE_META.into(),
        field: NAME_FIELD.into(),
        value: FieldValue::Str(name.into()),
    });
    engine.commit("create-entity", ops)?;
    Ok(id)
}

/// **CREATE a primitive** (cube/plane/…) at `pos` — an entity carrying a `Transform`, a name, a primitive-
/// kind tag, and the `Renderable` capability so it shows; ONE undoable transaction. The renderer draws the
/// primitive/placeholder mesh for the kind (a dedicated primitive mesh asset is a render concern).
///
/// # Errors
/// [`PipelineError`] if the create transaction fails.
pub fn create_primitive(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    kind: &str,
    pos: [f32; 3],
) -> Result<EntityId, PipelineError> {
    let id = engine.alloc_entity_id();
    let mut ops = vec![Op::CreateEntity { id, parent: None }];
    ops.extend(transform_ops(id, pos));
    ops.push(Op::SetField {
        entity: id,
        component: INSTANCE_META.into(),
        field: NAME_FIELD.into(),
        value: FieldValue::Str(kind.into()),
    });
    ops.push(Op::SetField {
        entity: id,
        component: INSTANCE_META.into(),
        field: PRIMITIVE_FIELD.into(),
        value: FieldValue::Str(kind.into()),
    });
    if let Some(&c) = scene.caps.get(&canonical("Renderable")) {
        ops.push(Op::AddPair {
            entity: id,
            rel: scene.rels.provides,
            target: c,
        });
    }
    engine.commit("create-primitive", ops)?;
    Ok(id)
}

/// **RENAME** an entity — ONE undoable transaction (a single `SetField` on `__meta__.name`); the
/// projection re-reads it (inv. 1). Undo restores the prior name. Reload-persistent (it's in the doc).
///
/// # Errors
/// [`PipelineError::UnknownEntity`] if `id` isn't a live entity, or the commit fails.
pub fn rename(
    engine: &mut Engine<FlecsWorld>,
    id: EntityId,
    name: &str,
) -> Result<(), PipelineError> {
    if engine.ecs_entity(id).is_none() {
        return Err(PipelineError::UnknownEntity(id));
    }
    engine.commit(
        "rename",
        vec![Op::SetField {
            entity: id,
            component: INSTANCE_META.into(),
            field: NAME_FIELD.into(),
            value: FieldValue::Str(name.into()),
        }],
    )
}

/// Whether `maybe_descendant` is `ancestor` itself or anywhere below it in the Movable Tree — the cycle
/// test for a reparent (moving `ancestor` under one of its own descendants would orphan a cycle).
fn is_descendant(
    engine: &Engine<FlecsWorld>,
    ancestor: EntityId,
    maybe_descendant: EntityId,
) -> bool {
    if ancestor == maybe_descendant {
        return true;
    }
    let mut stack = engine.children_of(ancestor);
    while let Some(c) = stack.pop() {
        if c == maybe_descendant {
            return true;
        }
        stack.extend(engine.children_of(c));
    }
    false
}

/// **REPARENT** `id` under `new_parent` via the Movable Tree (`node.move`) — ONE undoable transaction;
/// `new_parent = None` moves it to the root. **Cycle-safe:** a move that would make `id` its own ancestor
/// is **rejected** before the commit (and Loro's `MovableTree` rejects it too — `CyclicMoveError`). Undo
/// restores the prior parent (the pipeline captures it as the inverse). Child world-transform is preserved
/// for the flat/identity-parent case; a moved parent re-localizes per the gizmo rule (ADR-025).
///
/// # Errors
/// [`PipelineError`] on a cycle (`Loro`), an unknown entity, or a failed commit.
pub fn reparent_entity(
    engine: &mut Engine<FlecsWorld>,
    id: EntityId,
    new_parent: Option<EntityId>,
) -> Result<(), PipelineError> {
    if let Some(p) = new_parent {
        if is_descendant(engine, id, p) {
            return Err(PipelineError::Loro(format!(
                "reparent rejected — would create a cycle ({p} is {id} or its descendant)"
            )));
        }
    }
    engine.commit(
        "reparent",
        vec![Op::Reparent {
            entity: id,
            new_parent,
        }],
    )
}

/// **GROUP** a selection under a NEW parent node — ONE undoable transaction. The group node is created at
/// the origin (identity), so each member's **world transform is preserved** (`global = identity · local =
/// world` for the flat members; a later group move re-localizes per ADR-025). Reparenting every member in
/// the SAME commit means one undo dissolves the group + restores every prior parent atomically. Returns
/// the group id (the caller selects it).
///
/// # Errors
/// [`PipelineError`] if any member isn't live, or the commit fails.
pub fn group(
    engine: &mut Engine<FlecsWorld>,
    members: &[EntityId],
    name: &str,
) -> Result<EntityId, PipelineError> {
    let group_id = engine.alloc_entity_id();
    let mut ops = vec![Op::CreateEntity {
        id: group_id,
        parent: None,
    }];
    ops.extend(transform_ops(group_id, [0.0, 0.0, 0.0]));
    ops.push(Op::SetField {
        entity: group_id,
        component: INSTANCE_META.into(),
        field: NAME_FIELD.into(),
        value: FieldValue::Str(name.into()),
    });
    for &m in members {
        ops.push(Op::Reparent {
            entity: m,
            new_parent: Some(group_id),
        });
    }
    engine.commit("group", ops)?;
    Ok(group_id)
}

/// **UNGROUP** — dissolve a group: reparent every child to the group's parent (its grandparent), then
/// delete the now-empty group node; ONE undoable transaction. World transforms are preserved for an
/// identity group. Undo reverses it atomically (the group resurrects + the children move back). Returns
/// the freed children.
///
/// # Errors
/// [`PipelineError`] if the commit fails.
pub fn ungroup(
    engine: &mut Engine<FlecsWorld>,
    group_id: EntityId,
) -> Result<Vec<EntityId>, PipelineError> {
    let children = engine.children_of(group_id);
    let grandparent = engine.parent_of(group_id);
    let mut ops = Vec::with_capacity(children.len() + 1);
    for &c in &children {
        ops.push(Op::Reparent {
            entity: c,
            new_parent: grandparent,
        });
    }
    // Children are reparented OUT first (ops apply in order), so the group is empty when it's deleted —
    // the cascade catches nothing extra. Undo resurrects the empty group, then moves the children back.
    ops.push(Op::DeleteEntity { id: group_id });
    engine.commit("ungroup", ops)?;
    Ok(children)
}

/// **MULTI-EDIT** — apply one field edit to N entities as ONE **batched, atomic, undoable** transaction
/// (NOT N silent ops). A single undo restores **all N** at once (inv. 3). Unknown ids in the batch fail the
/// whole transaction (all-or-nothing), so a multi-selection is never half-edited.
///
/// # Errors
/// [`PipelineError`] if any entity is unknown, or the commit fails.
pub fn multi_edit(
    engine: &mut Engine<FlecsWorld>,
    ids: &[EntityId],
    component: &str,
    field: &str,
    value: &FieldValue,
) -> Result<(), PipelineError> {
    let ops = ids
        .iter()
        .map(|&id| Op::SetField {
            entity: id,
            component: component.into(),
            field: field.into(),
            value: value.clone(),
        })
        .collect();
    engine.commit("multi-edit", ops)
}

/// **DELETE** (M10.6) — **deactivate, not destroy** (ADR-026): set `id` inactive (so undo restores it and a
/// concurrent editor's field edits are never lost to a destructive delete) AND **free any dependents**
/// tracking it (the M3.3 Remove rule — the requirement re-opens), ONE undoable transaction. Distinct from
/// M3.3 [`remove_entity`] (a destructive `DeleteEntity`); this is the non-destructive scene-authoring
/// delete the compose-a-scene loop uses.
///
/// # Errors
/// [`PipelineError`] if the commit fails.
pub fn delete_deactivate(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    id: EntityId,
) -> Result<(), PipelineError> {
    let id_ecs = engine.ecs_entity(id);
    let mut ops = vec![Op::SetActive {
        entity: id,
        active: false,
    }];
    // Free dependents (the M3.3 rule): drop every binding `id` participates in, and — when `id` is the
    // requirer — the provider's consumed-marker `(BindsTo, id)` pair, so the freed provider re-enters the
    // reveal. Undo restores them with the re-activation.
    for (from, kind, to) in engine.bindings() {
        if from != id && to != id {
            continue;
        }
        ops.push(Op::RemoveBinding { from, kind, to });
        if from == id {
            if let Some(e) = id_ecs {
                ops.push(Op::RemovePair {
                    entity: to,
                    rel: scene.rels.binds_to,
                    target: e,
                });
            }
        }
    }
    engine.commit("delete-deactivate", ops)
}

/// **COPY** — snapshot a sub-tree's **resolved** state (components + overrides + active flags) to a
/// portable clipboard value, reusing the variant [`Composition`] machinery (no new model — pure read).
/// The [`Composition`] is `serde`, so a clipboard captured in one project pastes into another
/// (**cross-project**), same-process or serialized.
#[must_use]
pub fn copy_subtree(engine: &Engine<FlecsWorld>, root: EntityId, clip_id: &str) -> Composition {
    engine.save_composition(root, clip_id)
}

/// **PASTE** a copied [`Composition`] under **fresh deterministic ids** ([`Engine::alloc_entity_id`]) — ONE
/// undoable transaction (undo removes the whole pasted sub-tree). Deterministic ids ⇒ a replayed paste
/// lands byte-identical (ADR-013). Works across projects (the [`Composition`] is the only thing that
/// crosses — never a stale id). Returns the new root.
///
/// # Errors
/// [`PipelineError`] if the instantiate transaction fails.
pub fn paste_composition(
    engine: &mut Engine<FlecsWorld>,
    comp: &Composition,
) -> Result<EntityId, PipelineError> {
    engine.instantiate_composition(comp)
}

/// **CUT** — copy a sub-tree to the clipboard, then **delete (deactivate)** the source (non-destructive, so
/// the data is preserved on the clipboard AND recoverable via undo). Returns the clipboard [`Composition`].
///
/// # Errors
/// [`PipelineError`] if the delete transaction fails.
pub fn cut_subtree(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    root: EntityId,
    clip_id: &str,
) -> Result<Composition, PipelineError> {
    let clip = engine.save_composition(root, clip_id);
    delete_deactivate(engine, scene, root)?;
    Ok(clip)
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

/// Add a stdlib component **kind** directly (the "+ Add" palette, M3.4) — look up the kind's metadata
/// and instantiate it through the **same** [`instantiate`] path as [`describe_create`], so Add and
/// describe-to-create converge on one pre-componentized instantiate (not two code paths). A kind WITH a
/// catalog asset carries its mesh handle (renders as that mesh); else the honest cube. `None` if unknown.
pub fn add_kind(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    kind: &str,
    pos: [f32; 3],
    catalog: &MeshCatalog,
) -> Option<EntityId> {
    let lib = metrocalk_core::stdlib::standard_components();
    let meta = lib.iter().find(|m| m.name == kind)?;
    let handle = catalog.get(kind).cloned();
    instantiate(engine, scene, meta, pos, handle.as_deref()).ok()
}

fn default_value(ty: FieldType) -> FieldValue {
    match ty {
        FieldType::Integer => FieldValue::Integer(0),
        FieldType::Number => FieldValue::Number(0.0),
        FieldType::Boolean => FieldValue::Bool(false),
        FieldType::String => FieldValue::Str(String::new()),
    }
}
