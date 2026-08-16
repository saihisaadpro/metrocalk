//! Shape Studio (Build sub-engine) — the landing/combine/meld paths, headless. This is the exact chain
//! the live `.exe`'s shape commands run (spawn → land · combine world meshes → boolean → land+retire ·
//! meld → SDF bake → land+retire · undo restores everything as ONE step), proven without a GPU. Written
//! after the first live run crashed the app inside `shape_combine` — whatever that was, it must
//! reproduce (and stay fixed) here.

// A landing position is the engine's f32 world coordinate; the f64→f32 narrowing is the API's.
#![allow(clippy::cast_possible_truncation)]

use metrocalk_core::{Engine, FieldValue};
use metrocalk_ecs::FlecsWorld;

use metrocalk_editor_shell::capscene::{CapResolver, CapScene, MESH_FIELD};
use metrocalk_editor_shell::shape_forge::{
    bake_meld, bake_mesh_result, bake_shape, combine_meshes, land_combined_asset, land_shape_asset,
    meld_field, recentre, shape_material, transform_mesh_world, ShapeRecipe,
};

fn engine_with_resolver() -> (Engine<FlecsWorld>, CapScene) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    engine.set_capability_resolver(Box::new(CapResolver::from_scene(&scene)));
    (engine, scene)
}

fn field_str(
    engine: &Engine<FlecsWorld>,
    id: metrocalk_core::EntityId,
    comp: &str,
    field: &str,
) -> Option<String> {
    engine
        .components_of(id)
        .get(comp)
        .and_then(|c| match c.get(field) {
            Some(FieldValue::Str(s)) => Some(s.clone()),
            _ => None,
        })
}

#[test]
fn spawn_lands_a_complete_shape_entity_and_one_undo_removes_it() {
    let (mut engine, scene) = engine_with_resolver();
    let recipe = ShapeRecipe::parametric("torus").expect("torus is in the catalog");
    let built = bake_shape(&recipe).expect("bakes");

    let id = land_shape_asset(
        &mut engine,
        &scene,
        &built.landing(),
        "Ring",
        [1.0, 0.0, 2.0],
    )
    .expect("lands");
    assert_eq!(
        field_str(&engine, id, "MeshRenderer", MESH_FIELD).as_deref(),
        Some(built.handle.as_str()),
        "the entity carries the cooked mesh handle"
    );
    let source = field_str(&engine, id, "ShapeRecipe", "source").expect("editable source");
    assert_eq!(
        ShapeRecipe::from_json(&source).unwrap(),
        recipe,
        "the recipe round-trips"
    );

    assert!(engine.undo(), "one Ctrl-Z");
    assert!(!engine.entity_exists(id), "the spawn was one transaction");
}

/// The live crash case, on its fixed path: a default sphere carved out of a box goes through FIELD
/// space (both sources are unrotated shape primitives — ADR-070's "CSG is free" on fields), lands as
/// one replace-transaction, and one undo restores both sources.
#[test]
fn combine_box_minus_full_res_sphere_lands_and_one_undo_restores_both() {
    let (mut engine, scene) = engine_with_resolver();

    let box_recipe = ShapeRecipe::parametric("box").unwrap();
    let ball_recipe = ShapeRecipe::parametric("sphere").unwrap();
    let box_built = bake_shape(&box_recipe).unwrap();
    let ball_built = bake_shape(&ball_recipe).unwrap();
    let a = land_shape_asset(
        &mut engine,
        &scene,
        &box_built.landing(),
        "Box",
        [12.0, 0.0, 0.0],
    )
    .unwrap();
    let b = land_shape_asset(
        &mut engine,
        &scene,
        &ball_built.landing(),
        "Sphere",
        [12.5, 0.4, 0.3],
    )
    .unwrap();

    // The field path the engine arm takes for two unrotated shape primitives.
    let fa = meld_field(
        &box_recipe,
        "Box",
        [12.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
        1.0,
    )
    .unwrap();
    let fb = meld_field(
        &ball_recipe,
        "Sphere",
        [12.5, 0.4, 0.3],
        [0.0, 0.0, 0.0, 1.0],
        1.0,
    )
    .unwrap();
    let field = fa.difference(fb);
    let recipe = ShapeRecipe {
        v: 1,
        kind: "carve".into(),
        params: std::collections::BTreeMap::new(),
        profile: Vec::new(),
        sources: vec![a.to_loro_key(), b.to_loro_key()],
    };
    let (built, centre, _tol) = bake_meld(&recipe, &field, 72, shape_material("carve"))
        .expect("the field carve bakes clean");
    assert!(built.triangles > 500, "a real carved surface");

    let id = land_combined_asset(
        &mut engine,
        &scene,
        &built.landing(),
        "Carved shape of Box and Sphere",
        [centre[0] as f32, centre[1] as f32, centre[2] as f32],
        [a, b],
    )
    .expect("the replace transaction commits");

    assert!(engine.entity_exists(id), "the result exists");
    assert!(
        !engine.entity_exists(a) && !engine.entity_exists(b),
        "both sources retired"
    );

    // ONE undo: the result vanishes AND both sources come back with their fields intact.
    assert!(engine.undo(), "undo runs");
    assert!(!engine.entity_exists(id), "undo removed the result");
    assert!(
        engine.entity_exists(a) && engine.entity_exists(b),
        "undo resurrected both sources"
    );
    assert_eq!(
        field_str(&engine, a, "MeshRenderer", MESH_FIELD).as_deref(),
        Some(box_built.handle.as_str()),
        "the resurrected box still has its mesh"
    );
}

/// The exact mesh boolean stays the path for PLANAR geometry, where it is genuinely exact.
#[test]
fn exact_mesh_combine_still_carves_planar_shapes() {
    let box_built = bake_shape(&ShapeRecipe::parametric("box").unwrap()).unwrap();
    let wedge_built = bake_shape(&ShapeRecipe::parametric("wedge").unwrap()).unwrap();
    let wa = metrocalk_editor_shell::csg_intent::mesh_asset_to_trimesh(&box_built.asset);
    let wb = transform_mesh_world(
        &metrocalk_editor_shell::csg_intent::mesh_asset_to_trimesh(&wedge_built.asset),
        1.0,
        [0.0; 3],
        1.0,
        [0.0, 0.0, 0.0, 1.0],
        [0.3, 0.2, 0.1],
    );
    let out = combine_meshes(&wa, &wb, "carve").expect("planar carve is exact and clean");
    assert!(out.triangles.len() > 10, "a real carved surface");
    let (local, _) = recentre(&out);
    let recipe = ShapeRecipe {
        v: 1,
        kind: "carve".into(),
        params: std::collections::BTreeMap::new(),
        profile: Vec::new(),
        sources: Vec::new(),
    };
    let built = bake_mesh_result(&recipe, &local, shape_material("carve"));
    assert!(built.triangles > 10);
}

/// The honest ceiling, pinned: full-resolution curved meshes exceed the exact boolean's fragment
/// budget and are REFUSED in plain language (previously this was a process-killing stack overflow).
#[test]
fn exact_mesh_combine_refuses_fine_curvature_in_plain_language() {
    let ball = bake_shape(&ShapeRecipe::parametric("sphere").unwrap()).unwrap();
    let a = metrocalk_editor_shell::csg_intent::mesh_asset_to_trimesh(&ball.asset);
    let b = transform_mesh_world(
        &a,
        1.0,
        [0.0; 3],
        1.0,
        [0.0, 0.0, 0.0, 1.0],
        [0.4, 0.1, 0.2],
    );
    let err = combine_meshes(&a, &b, "carve").expect_err("full-res spheres exceed the budget");
    let msg = err.to_string();
    assert!(msg.contains("smoothness"), "points at the fix: {msg}");
}

/// Every ordered pair of low-smoothness catalog shapes survives every combine verb — no panics, no
/// aborts: either a clean solid or a plain-language refusal. (Low smoothness keeps the exact-BSP
/// fuzz fast; the full-resolution curved case is pinned separately above.)
#[test]
fn every_shape_pair_combines_or_refuses_cleanly() {
    let kinds = [
        "box", "sphere", "cylinder", "cone", "torus", "capsule", "wedge", "prism",
    ];
    let meshes: Vec<_> = kinds
        .iter()
        .map(|k| {
            let mut recipe = ShapeRecipe::parametric(k).unwrap();
            for key in ["segments", "sides"] {
                if let Some(v) = recipe.params.get_mut(key) {
                    *v = v.clamp(8.0, 10.0);
                }
            }
            let built = bake_shape(&recipe).unwrap();
            metrocalk_editor_shell::csg_intent::mesh_asset_to_trimesh(&built.asset)
        })
        .collect();
    for (i, a) in meshes.iter().enumerate() {
        for (j, b) in meshes.iter().enumerate() {
            // Overlap them for a real interaction.
            let wb =
                transform_mesh_world(b, 1.0, [0.0; 3], 1.0, [0.0, 0.0, 0.0, 1.0], [0.3, 0.2, 0.1]);
            for op in ["union", "carve", "intersect"] {
                match combine_meshes(a, &wb, op) {
                    Ok(out) => assert!(!out.triangles.is_empty(), "{}-{op}-{}", kinds[i], kinds[j]),
                    Err(e) => {
                        // A refusal is acceptable; a panic/abort never is (that is what this loop proves).
                        eprintln!("refused {}-{op}-{}: {e}", kinds[i], kinds[j]);
                    }
                }
            }
        }
    }
}

#[test]
fn meld_two_spheres_lands_one_blob_and_undo_restores_both() {
    let (mut engine, scene) = engine_with_resolver();
    let s = ShapeRecipe::parametric("sphere").unwrap();
    let built_a = bake_shape(&s).unwrap();
    let a = land_shape_asset(
        &mut engine,
        &scene,
        &built_a.landing(),
        "Sphere",
        [16.0, 0.0, 0.0],
    )
    .unwrap();
    let b = land_shape_asset(
        &mut engine,
        &scene,
        &built_a.landing(),
        "Sphere",
        [16.6, 0.0, 0.0],
    )
    .unwrap();

    let fa = meld_field(&s, "A", [16.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], 1.0).unwrap();
    let fb = meld_field(&s, "B", [16.6, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], 1.0).unwrap();
    let field = fa.smooth_union(fb, 0.3);
    let recipe = ShapeRecipe {
        v: 1,
        kind: "meld".into(),
        params: [("k".to_string(), 0.3)].into_iter().collect(),
        profile: Vec::new(),
        sources: vec![a.to_loro_key(), b.to_loro_key()],
    };
    let (built, centre, _tol) =
        bake_meld(&recipe, &field, 64, shape_material("meld")).expect("bakes");
    assert!(built.triangles > 200, "a real blob");

    let id = land_combined_asset(
        &mut engine,
        &scene,
        &built.landing(),
        "Meld of Sphere and Sphere",
        [centre[0] as f32, centre[1] as f32, centre[2] as f32],
        [a, b],
    )
    .expect("lands");
    assert!(!engine.entity_exists(a) && !engine.entity_exists(b));
    assert!(engine.undo(), "undo runs");
    assert!(engine.entity_exists(a) && engine.entity_exists(b) && !engine.entity_exists(id));
}
