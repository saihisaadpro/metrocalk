//! M5 marketplace gate — the headless acceptance, through the **real** `/core` engine.
//!
//! Two things this proves: (1) **capability namespacing** — two authors' same-local-name custom caps
//! (`acme:Health` vs `brandx:Health`) do **not** cross-bind, yet both `(AliasOf std:Health)` correctly
//! bind a `std:Health` requirer (the collision the bare-string M1.3 registry would have had is now
//! impossible, and the compat web still works across namespaces). (2) The **marketplace tier** — a
//! description with no local match resolves to a **pre-componentized** entry that applies already wired
//! (namespaced caps + a mesh handle) as **one undoable transaction**, offers the M3.1 attach, and
//! **survives reload** via the replay-log. Mirrors `north_star_2.rs`.

#![allow(clippy::cast_precision_loss)]

use std::collections::HashMap;
use std::path::PathBuf;

use metrocalk_core::marketplace::{LocalCatalog, MarketplaceIndex};
use metrocalk_core::{resolve, stdlib, Engine, EntityId, FieldValue, Op, Resolved};
use metrocalk_ecs::{Entity, FlecsWorld};

use metrocalk_editor_shell::capscene::{self, CapScene};
use metrocalk_editor_shell::persist::{Log, Record};
use metrocalk_editor_shell::reveal::{reveal, Context};
use metrocalk_editor_shell::TRACKS;

const N: usize = 200;

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("metrocalk-{name}.jsonl"))
}

fn seeded() -> (Engine<FlecsWorld>, CapScene) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    capscene::seed(&mut engine, &scene, N).expect("seed");
    engine.clear_history();
    (engine, scene)
}

fn ctx<'a>(
    scene: &'a CapScene,
    pos: &'a HashMap<Entity, [f32; 3]>,
    rec: &'a HashMap<Entity, u64>,
) -> Context<'a> {
    Context {
        cap_name: &scene.cap_name,
        position: pos,
        recency: rec,
    }
}

fn has_binding(engine: &Engine<FlecsWorld>, from: EntityId, to: EntityId) -> bool {
    engine
        .bindings()
        .iter()
        .any(|(f, k, t)| *f == from && k == TRACKS && *t == to)
}

/// Create a bare requirer of a single capability (by canonical name) at `pos` — to probe the compat
/// query directly (e.g. "what binds an `acme:Health` requirer?").
fn make_requirer(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    cap: &str,
    pos: [f32; 3],
) -> EntityId {
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
    ops.push(Op::AddPair {
        entity: id,
        rel: scene.rels.requires,
        target: scene.cap(cap),
    });
    engine
        .commit("test-requirer", ops)
        .expect("requirer commits");
    id
}

/// The compatible target ids a reveal offers `selected`.
fn compatible_ids(
    engine: &Engine<FlecsWorld>,
    scene: &CapScene,
    selected: EntityId,
) -> Vec<EntityId> {
    let pos = capscene::positions(engine);
    let rec = HashMap::new();
    let sel = engine.ecs_entity(selected).unwrap();
    let r = reveal(engine.world(), sel, scene.rels, &ctx(scene, &pos, &rec));
    r.compatible
        .iter()
        .filter_map(|c| engine.entity_id_of(c.entity))
        .collect()
}

#[test]
fn namespaced_custom_caps_do_not_collide_but_aliases_bind_across_authors() {
    let (mut e, scene) = seeded();

    // Two authors' companions: acme provides acme:Health (AliasOf std:Health); brandx provides
    // brandx:Health (AliasOf std:Health). Both also provide their std:Health via the alias resolution.
    let cat = LocalCatalog::builtin();
    let acme = cat.get("acme:companion-drone").unwrap();
    let brandx = cat.get("brandx:spirit-familiar").unwrap();
    let acme_e =
        capscene::apply_marketplace_entry(&mut e, &scene, &acme, [1.0, 0.0, 0.0], None).unwrap();
    let brandx_e =
        capscene::apply_marketplace_entry(&mut e, &scene, &brandx, [1.2, 0.0, 0.0], None).unwrap();

    // A std:Health requirer (the stdlib HealthBar) binds BOTH — across authors — via the std alias.
    let bar = capscene::describe_create(
        &mut e,
        &scene,
        "health bar",
        [1.1, 0.0, 0.0],
        &HashMap::new(),
    )
    .unwrap()
    .0;
    let std_compat = compatible_ids(&e, &scene, bar);
    assert!(
        std_compat.contains(&acme_e),
        "acme companion binds a std:Health requirer"
    );
    assert!(
        std_compat.contains(&brandx_e),
        "brandx familiar binds a std:Health requirer too"
    );

    // An `acme:Health` requirer binds ONLY the acme provider — brandx:Health is a DISTINCT cap (the
    // collision the bare-string registry would have had — `acme:Health` == `brandx:Health` == "Health"
    // — is impossible now).
    let acme_req = make_requirer(&mut e, &scene, "acme:Health", [1.1, 0.0, 0.0]);
    let acme_compat = compatible_ids(&e, &scene, acme_req);
    assert!(
        acme_compat.contains(&acme_e),
        "acme:Health requirer binds the acme provider"
    );
    assert!(
        !acme_compat.contains(&brandx_e),
        "acme:Health requirer must NOT bind a brandx:Health provider (no cross-author collision)"
    );
}

#[test]
fn describe_no_local_match_resolves_marketplace_and_applies_pre_componentized() {
    let (mut e, scene) = seeded();
    let lib = stdlib::standard_components();
    let index = LocalCatalog::builtin();

    // "rusty medieval sword" has no local match → the resolver's SECOND tier returns the entry.
    let entry = match resolve(&lib, &index, "rusty medieval sword") {
        Resolved::Marketplace(m) => m[0].entry.clone(),
        other => panic!("expected the marketplace tier, got {other:?}"),
    };
    assert_eq!(entry.id, "forge:rusty-sword");

    // Apply it pre-componentized — its component + namespaced caps + a mesh handle, one undoable tx.
    let handle = "mtkasset:demo-sword";
    let sword =
        capscene::apply_marketplace_entry(&mut e, &scene, &entry, [0.0, 0.0, 0.0], Some(handle))
            .unwrap();

    // A WORKING object, not a dead file: it carries the entry's component, its mesh handle, and its
    // namespaced caps (provides std:Renderable, requires std:Spatial).
    let comps = e.components_of(sword);
    assert!(
        comps.contains_key("Weapon"),
        "the entry's component is attached"
    );
    assert_eq!(
        comps.get("MeshRenderer").and_then(|m| m.get("mesh")),
        Some(&FieldValue::Str(handle.to_string())),
        "the mesh handle (only the handle) is carried"
    );

    // The reveal offers the compatible attach (the seed has std:Spatial providers the sword requires).
    let compat = compatible_ids(&e, &scene, sword);
    assert!(
        !compat.is_empty(),
        "the marketplace object offers a one-click attach"
    );
    let target = compat[0];
    capscene::bind(&mut e, &scene, sword, target).expect("attach binds");
    assert!(has_binding(&e, sword, target));

    // single-step undo reverses the attach.
    assert!(e.undo());
    assert!(!has_binding(&e, sword, target));
}

#[test]
fn marketplace_apply_survives_export_then_replay() {
    let log = Log::open(tmp("marketplace"), capscene::fingerprint(N));
    log.clear();
    let handle = "mtkasset:demo-sword";

    // run A: resolve + apply the sword, persist the marketplace record.
    let (mut a, scene_a) = seeded();
    let entry = LocalCatalog::builtin().get("forge:rusty-sword").unwrap();
    let sword =
        capscene::apply_marketplace_entry(&mut a, &scene_a, &entry, [2.0, 0.0, 0.0], Some(handle))
            .unwrap();
    log.append(&Record::ApplyMarketplace {
        entry_id: entry.id.clone(),
        pos: [2.0, 0.0, 0.0],
        mesh: Some(handle.to_string()),
    });
    drop(a); // close

    // run B: fresh deterministic seed + replay (a true close→reopen).
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut b = Engine::new(world, 1);
    capscene::seed(&mut b, &scene, N).expect("re-seed");
    b.clear_history();
    let (applied, skipped) = log.replay(&mut b, &scene, &HashMap::new());
    b.clear_history();

    assert_eq!((applied, skipped), (1, 0), "the marketplace apply replayed");
    assert!(
        b.entity_exists(sword),
        "the marketplace object survived reload"
    );
    let comps = b.components_of(sword);
    assert!(comps.contains_key("Weapon"), "with its component");
    assert_eq!(
        comps.get("MeshRenderer").and_then(|m| m.get("mesh")),
        Some(&FieldValue::Str(handle.to_string())),
        "and its mesh handle (re-applied deterministically from the catalog by id)"
    );
    log.clear();
}
