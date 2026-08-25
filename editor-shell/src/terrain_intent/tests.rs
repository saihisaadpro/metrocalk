//! Headless tests for the terrain authoring seam: every edit is one undoable transaction, the recipe
//! round-trips through the document exactly, derived geometry lands content-addressed, and the physics and
//! impostor adapters produce something a downstream system can actually use.

use super::*;
use metrocalk_ecs::FlecsWorld as Flecs;
use metrocalk_terrain::recipe::{Blend, ChunkCoord, LayerKind, SplineKind};
use metrocalk_terrain::stream::{build_chunk, ChunkRequest};
use metrocalk_terrain::{mesh as tmesh, nav::NavConfig};

/// A live engine with the standard registry interned, as the shell builds it.
fn engine_with_scene() -> (Engine<FlecsWorld>, CapScene) {
    let mut world = Flecs::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    engine.clear_history();
    (engine, scene)
}

fn create(engine: &mut Engine<FlecsWorld>, scene: &CapScene, preset: &str) -> EntityId {
    create_from_preset(engine, scene, preset, [0.0, 0.0, 0.0])
        .expect("preset creates")
        .0
}

#[test]
fn a_terrain_is_created_from_a_preset_in_one_transaction() {
    let (mut engine, scene) = engine_with_scene();
    let before = engine.entity_ids().len();
    let id = create(&mut engine, &scene, "rolling-hills");
    assert_eq!(engine.entity_ids().len(), before + 1);
    let recipe = read(&engine, id).expect("readable");
    assert_eq!(recipe.name, "Rolling Hills");
    assert!(recipe.layers.len() >= 3);
    assert!(!recipe.biomes.is_empty());
    // The summary fields the inspector reads are populated from the recipe, not guessed.
    let comps = engine.components_of(id);
    let rec = &comps[TERRAIN_COMPONENT];
    assert_eq!(
        rec.get("preset"),
        Some(&FieldValue::Str("rolling-hills".into()))
    );
    assert_eq!(
        rec.get("layerCount"),
        Some(&FieldValue::Integer(recipe.layers.len() as i64))
    );
    // And it is one undo step: undoing removes the terrain entirely.
    assert!(engine.undo(), "undo");
    assert_eq!(engine.entity_ids().len(), before);
}

#[test]
fn an_unknown_preset_is_refused_by_name() {
    let (mut engine, scene) = engine_with_scene();
    let err = create_from_preset(&mut engine, &scene, "mars", [0.0; 3]).expect_err("must refuse");
    assert!(matches!(err, TerrainIntentError::UnknownPreset(_)));
    assert!(err.to_string().contains("mars"));
}

#[test]
fn the_recipe_round_trips_through_the_document() {
    let (mut engine, scene) = engine_with_scene();
    let id = create(&mut engine, &scene, "alpine");
    let recipe = read(&engine, id).expect("read");
    // Write it back unchanged and read again: identical, including every layer and rule.
    let again = apply_edit(
        &mut engine,
        id,
        &TerrainEdit::Rename {
            name: recipe.name.clone(),
        },
    )
    .expect("rename")
    .0;
    assert_eq!(recipe.layers, again.layers);
    assert_eq!(recipe.biomes, again.biomes);
    assert_eq!(recipe.materials, again.materials);
    assert_eq!(recipe.scatter, again.scatter);
    assert_eq!(recipe.lod, again.lod);
    assert_eq!(recipe.digest(), again.digest(), "the digest must be stable");
}

#[test]
fn a_sculpt_gesture_is_one_undo_step_and_writes_only_the_stroke_field() {
    let (mut engine, scene) = engine_with_scene();
    let id = create(&mut engine, &scene, "flat");
    let source_before = match engine.components_of(id)[TERRAIN_COMPONENT][SOURCE_FIELD].clone() {
        FieldValue::Str(s) => s,
        other => panic!("source should be a string, got {other:?}"),
    };
    let brush = Brush {
        radius_m: 20.0,
        strength: 4.0,
        spacing: 0.25,
        ..Brush::default()
    };
    let after = apply_edit(
        &mut engine,
        id,
        &TerrainEdit::Sculpt {
            brush,
            from: [100.0, 100.0],
            to: [200.0, 100.0],
        },
    )
    .expect("sculpt")
    .0;
    assert_eq!(after.strokes.len(), 20, "100 m at 5 m spacing");
    // The expensive field was not touched, which is the whole point of splitting it.
    let source_after = match engine.components_of(id)[TERRAIN_COMPONENT][SOURCE_FIELD].clone() {
        FieldValue::Str(s) => s,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        source_before, source_after,
        "a brush dab rewrote the whole recipe"
    );
    // One gesture, one undo step.
    assert!(engine.undo(), "undo");
    assert!(read(&engine, id).expect("read").strokes.is_empty());
}

#[test]
fn strokes_survive_the_packed_encoding_exactly() {
    let strokes = vec![
        Stroke {
            kind: StrokeKind::Raise,
            x: 123.456,
            z: -78.9,
            radius_m: 12.5,
            strength: -3.25,
            hardness: 0.75,
            target_m: 0.0,
        },
        Stroke {
            kind: StrokeKind::Flatten,
            x: 0.001,
            z: 4096.0,
            radius_m: 8.0,
            strength: 1.0,
            hardness: 0.0,
            target_m: 17.5,
        },
        Stroke {
            kind: StrokeKind::Smooth,
            x: -1.5,
            z: 2.5,
            radius_m: 30.0,
            strength: 0.6,
            hardness: 1.0,
            target_m: 0.5,
        },
        Stroke {
            kind: StrokeKind::Noise,
            x: 10.0,
            z: 10.0,
            radius_m: 5.0,
            strength: 2.0,
            hardness: 0.5,
            target_m: 6.0,
        },
    ];
    let packed = pack_strokes(&strokes);
    let back = unpack_strokes(&packed);
    assert_eq!(
        strokes, back,
        "the packed form must be lossless at millimetre precision"
    );
    // Compact enough to be worth the trouble: well under 50 bytes a dab.
    assert!(
        packed.len() / strokes.len() < 50,
        "packed form is {} bytes per stroke",
        packed.len() / strokes.len()
    );
    // Re-packing is idempotent, so a stroke read from the document writes back unchanged.
    assert_eq!(packed, pack_strokes(&back));
}

#[test]
fn a_corrupt_stroke_entry_costs_one_dab_not_the_terrain() {
    let good = pack_strokes(&[Stroke {
        kind: StrokeKind::Raise,
        x: 1.0,
        z: 2.0,
        radius_m: 3.0,
        strength: 4.0,
        hardness: 0.5,
        target_m: 0.0,
    }]);
    let mangled = format!("{good};zzz:nonsense;r:1:2:3");
    let back = unpack_strokes(&mangled);
    assert_eq!(back.len(), 1, "the intact dab must survive: {back:?}");
}

#[test]
fn edits_that_would_break_the_terrain_are_rejected_before_committing() {
    let (mut engine, scene) = engine_with_scene();
    let id = create(&mut engine, &scene, "flat");
    let before = read(&engine, id).expect("read");
    // 50 vertices per edge is not 2^k + 1, so LODs could not subsample it and chunks would crack.
    let err = apply_edit(
        &mut engine,
        id,
        &TerrainEdit::SetExtent {
            world_size_m: 1024.0,
            chunk_size_m: 64.0,
            chunk_verts: 50,
        },
    )
    .expect_err("must be rejected");
    assert!(matches!(err, TerrainIntentError::Rejected(_)));
    assert!(err.to_string().contains("power of two"), "{err}");
    // And the document is untouched — a rejected edit is not a partial edit.
    assert_eq!(read(&engine, id).expect("read"), before);
}

#[test]
fn out_of_range_indices_are_named_not_panicked_on() {
    let mut recipe = preset::by_id("flat").expect("preset");
    let err = apply_to_recipe(&mut recipe, &TerrainEdit::RemoveLayer { index: 9 })
        .expect_err("must refuse");
    assert!(matches!(
        err,
        TerrainIntentError::NoSuchIndex {
            kind: "layer",
            index: 9,
            len: 1
        }
    ));
    assert!(err.to_string().contains("there are 1"));
}

#[test]
fn layer_order_is_editable_because_order_is_meaning() {
    let mut recipe = preset::by_id("rolling-hills").expect("preset");
    let names: Vec<String> = recipe.layers.iter().map(|l| l.name.clone()).collect();
    apply_to_recipe(
        &mut recipe,
        &TerrainEdit::MoveLayer {
            index: 3,
            delta: -2,
        },
    )
    .expect("move");
    let moved: Vec<String> = recipe.layers.iter().map(|l| l.name.clone()).collect();
    assert_eq!(moved[1], names[3], "the layer moved down the stack");
    assert_eq!(moved[3], names[2]);
    // Clamped rather than out of range.
    apply_to_recipe(
        &mut recipe,
        &TerrainEdit::MoveLayer {
            index: 0,
            delta: 99,
        },
    )
    .expect("move");
    assert_eq!(recipe.layers.len(), names.len());
}

#[test]
fn binding_a_prototype_brings_a_presets_scatter_rules_to_life() {
    let (mut engine, scene) = engine_with_scene();
    let id = create(&mut engine, &scene, "rolling-hills");
    let recipe = read(&engine, id).expect("read");
    assert!(
        recipe.protos.iter().all(|p| p.mesh_key.is_empty()),
        "a preset ships unbound prototypes"
    );
    let terrain = Terrain::compile(recipe, BTreeMap::new()).expect("compile");
    // Scanned over a patch rather than one chunk: whether any single chunk is woodland depends on the
    // seed's coastline, and the property under test is "the rule came to life", not "chunk (8,8) is a
    // forest".
    let count = |t: &Terrain| -> usize {
        let mut n = 0;
        for cz in 6..14 {
            for cx in 6..14 {
                let s = t.sample_chunk(ChunkCoord::new(cx, cz)).expect("chunk");
                n += metrocalk_terrain::scatter::scatter_chunk(t, &s).len();
            }
        }
        n
    };
    assert_eq!(count(&terrain), 0, "unbound prototypes must place nothing");

    let bound = apply_edit(
        &mut engine,
        id,
        &TerrainEdit::BindProto {
            index: 0,
            mesh_key: "mesh:tree".into(),
            lod_keys: vec!["mesh:tree1".into()],
            impostor_key: Some("mesh:tree_imp".into()),
        },
    )
    .expect("bind")
    .0;
    let terrain2 = Terrain::compile(bound, BTreeMap::new()).expect("compile");
    assert!(
        count(&terrain2) > 0,
        "binding a mesh should populate the woodland rule"
    );
}

#[test]
fn a_road_added_through_the_seam_reshapes_the_compiled_terrain() {
    let (mut engine, scene) = engine_with_scene();
    let id = create(&mut engine, &scene, "rolling-hills");
    let plain =
        Terrain::compile(read(&engine, id).expect("read"), BTreeMap::new()).expect("compile");
    let before = plain.height(1024.0, 1024.0);

    let with_road = apply_edit(
        &mut engine,
        id,
        &TerrainEdit::AddSpline {
            spline: Box::new(SplineDef {
                name: "Main Road".into(),
                kind: SplineKind::Road,
                points: vec![[900.0, 0.0, 1024.0], [1150.0, 0.0, 1024.0]],
                width_m: 12.0,
                falloff_m: 8.0,
                material_layer: Some(4),
                ..SplineDef::default()
            }),
        },
    )
    .expect("add road")
    .0;
    let roaded = Terrain::compile(with_road, BTreeMap::new()).expect("compile");
    // On the corridor the surface is graded, so it differs from the raw field, and it is flat across.
    let on = roaded.height(1024.0, 1024.0);
    let across = roaded.height(1024.0, 1028.0);
    assert!(
        (on - across).abs() < 0.5,
        "corridor is not flat: {on} vs {across}"
    );
    assert!(
        (on - before).abs() > 1e-4 || (across - roaded.height(1024.0, 1060.0)).abs() > 1e-4,
        "the road changed nothing"
    );
}

#[test]
fn a_chunk_lands_as_a_content_addressed_textured_mesh() {
    let recipe = preset::by_id("rolling-hills").expect("preset");
    let terrain = Terrain::compile(recipe, BTreeMap::new()).expect("compile");
    let req = ChunkRequest {
        coord: ChunkCoord::new(4, 4),
        lod: 0,
        want_textures: true,
        want_normals: true,
        want_scatter: false,
        collider_lod: Some(0),
        want_nav: true,
        priority: 0.0,
    };
    let build = build_chunk(&terrain, &req, &NavConfig::default());
    let mut store = AssetStore::new();
    let handle = store_chunk(&mut store, &build.meshes[0], build.textures.as_ref());
    let asset = store.get_str(&handle).expect("stored");
    assert_eq!(asset.primitives.len(), 1);
    assert_eq!(asset.textures.len(), 2, "albedo + detail normal");
    assert_eq!(asset.materials[0].base_color_texture, Some(0));
    assert_eq!(asset.materials[0].normal_texture, Some(1));
    assert!(asset.triangle_count() > 1000);
    assert_eq!(
        asset.primitives[0].uvs.len(),
        asset.primitives[0].positions.len(),
        "the splat bake needs a UV per vertex"
    );
    // Content addressing: the same chunk stores to the same handle, a different LOD to a different one.
    let again = store_chunk(&mut store, &build.meshes[0], build.textures.as_ref());
    assert_eq!(handle, again);
    let coarse = store_chunk(&mut store, &build.meshes[2], build.textures.as_ref());
    assert_ne!(handle, coarse);
}

#[test]
fn identical_chunks_dedup_to_one_asset() {
    // Two flat chunks are byte-identical in local space, so a flat world costs one upload, not one per chunk.
    let recipe = preset::by_id("flat").expect("preset");
    let terrain = Terrain::compile(recipe, BTreeMap::new()).expect("compile");
    let mut store = AssetStore::new();
    let a = terrain.sample_chunk(ChunkCoord::new(2, 2)).expect("chunk");
    let b = terrain.sample_chunk(ChunkCoord::new(9, 5)).expect("chunk");
    let policy = terrain.recipe().lod;
    let ma = tmesh::build_chunk_mesh(&a, 0, &policy);
    let mb = tmesh::build_chunk_mesh(&b, 0, &policy);
    let ha = store_chunk(&mut store, &ma, None);
    let hb = store_chunk(&mut store, &mb, None);
    assert_eq!(ha, hb, "identical flat chunks must share one asset");
    assert_eq!(store.len(), 1);
}

#[test]
fn a_chunk_collider_is_a_height_field_the_physics_world_accepts() {
    use metrocalk_physics::{Physics, PhysicsConfig, RapierPhysics};
    let recipe = preset::by_id("rolling-hills").expect("preset");
    let terrain = Terrain::compile(recipe, BTreeMap::new()).expect("compile");
    let samples = terrain.sample_chunk(ChunkCoord::new(6, 6)).expect("chunk");
    let collider = metrocalk_terrain::chunk_collider(&samples, 0);
    let (body, desc) = chunk_body_and_collider(&collider);
    assert!(
        body.translation[1].abs() < f64::EPSILON,
        "heights are absolute, so the body sits at y=0, not at the terrain's mean height"
    );

    let mut world = RapierPhysics::new(PhysicsConfig::default());
    let ground = world.add_body(&body);
    world
        .add_collider(ground, &desc)
        .expect("the height field must be accepted");

    // A ball dropped over the chunk centre lands on the terrain surface, not through it.
    let c = samples.center_xz();
    let surface = f64::from(terrain.height(c[0], c[1]));
    let start = surface + 4.0;
    let ball = world.add_body(&BodyDesc::new(
        BodyKind::Dynamic,
        [f64::from(c[0]), start, f64::from(c[1])],
    ));
    world
        .add_collider(
            ball,
            &ColliderDesc::new(metrocalk_physics::ColliderShape::Ball { radius: 0.5 }),
        )
        .expect("ball");
    // Four seconds: long enough to fall four metres and settle, short enough that it cannot roll off a
    // 64 m patch of hillside and confuse "sank through" with "left the chunk".
    for _ in 0..240 {
        world.step();
    }
    let p = world.transform(ball).expect("ball exists").0;
    assert!(p[1] < start - 1.0, "the ball never fell: {}", p[1]);
    // Compared against the ground under wherever it ended up, since a hillside legitimately makes it roll.
    let under = f64::from(collider.height_at(p[0] as f32, p[2] as f32));
    assert!(
        p[1] > under - 0.5,
        "the ball sank through the terrain: y={} but the ground under it is {under}",
        p[1]
    );
}

#[test]
fn an_impostor_bake_produces_a_silhouette_with_transparency() {
    // A cross-quad tree stand-in: the bake must cover part of the texture and leave the rest transparent,
    // which is what makes a billboard read as foliage rather than as a rectangle.
    let data = metrocalk_terrain::scatter::impostor_cross_mesh(1.0, 4.0, 2, false);
    let mut store = AssetStore::new();
    let handle = store_mesh_data(&mut store, &data, "tree", [0.2, 0.45, 0.15, 1.0], 0.8, None);
    let asset = store.get_str(&handle).expect("stored").clone();

    let tex = bake_impostor_texture(&asset, 64);
    assert_eq!(tex.width, 64);
    assert_eq!(tex.rgba8.len(), 64 * 64 * 4);
    let pixels = tex.rgba8.as_chunks::<4>().0;
    let opaque = pixels.iter().filter(|p| p[3] > 0).count();
    let total = (tex.width * tex.height) as usize;
    assert!(
        opaque > total / 20,
        "the bake drew almost nothing: {opaque}/{total}"
    );
    assert!(
        opaque < total,
        "the bake filled the whole texture — there is no silhouette"
    );
    // Something green was drawn, i.e. the material colour and the lighting both reached the pixels.
    let greenest = pixels
        .iter()
        .filter(|p| p[3] > 0)
        .map(|p| p[1])
        .max()
        .unwrap_or(0);
    assert!(greenest > 40, "the drawn pixels are black: {greenest}");
    // Deterministic.
    assert_eq!(tex, bake_impostor_texture(&asset, 64));
}

#[test]
fn terrains_are_discoverable_in_the_scene() {
    let (mut engine, scene) = engine_with_scene();
    assert!(terrains(&engine).is_empty());
    assert_eq!(only_terrain(&engine), None);
    let id = create(&mut engine, &scene, "flat");
    assert_eq!(terrains(&engine), vec![id]);
    assert_eq!(only_terrain(&engine), Some(id));
    // Two terrains means a command must be told which one.
    let _second = create(&mut engine, &scene, "dunes");
    assert_eq!(terrains(&engine).len(), 2);
    assert_eq!(only_terrain(&engine), None);
}

#[test]
fn reading_a_non_terrain_entity_explains_itself() {
    let (mut engine, scene) = engine_with_scene();
    let _ = &scene;
    let id = crate::capscene::create_entity(&mut engine, [0.0; 3], "Not Terrain").expect("create");
    let err = read(&engine, id).expect_err("must refuse");
    assert!(matches!(err, TerrainIntentError::NotATerrain(_)));
}

#[test]
fn a_preset_swap_replaces_the_whole_recipe_in_one_step() {
    let (mut engine, scene) = engine_with_scene();
    let id = create(&mut engine, &scene, "flat");
    let swapped = apply_edit(
        &mut engine,
        id,
        &TerrainEdit::ApplyPreset {
            id: "canyon".into(),
        },
    )
    .expect("swap")
    .0;
    assert_eq!(swapped.name, "Canyon Mesa");
    assert!(engine.undo(), "undo");
    assert_eq!(read(&engine, id).expect("read").name, "Flat Ground");
}

#[test]
fn every_edit_variant_survives_a_json_round_trip() {
    // The Tauri command carries these as JSON, so a variant that does not serialize is a dead control.
    let edits = vec![
        TerrainEdit::ApplyPreset { id: "flat".into() },
        TerrainEdit::SetSeed { seed: 42 },
        TerrainEdit::Rename { name: "X".into() },
        TerrainEdit::SetExtent {
            world_size_m: 1024.0,
            chunk_size_m: 64.0,
            chunk_verts: 65,
        },
        TerrainEdit::AddLayer {
            layer: Box::new(
                Layer::new("L", LayerKind::Constant { height: 1.0 }).with_blend(Blend::Add),
            ),
        },
        TerrainEdit::ToggleLayer {
            index: 0,
            enabled: false,
        },
        TerrainEdit::MoveLayer { index: 0, delta: 1 },
        TerrainEdit::Sculpt {
            brush: Brush::default(),
            from: [0.0, 0.0],
            to: [1.0, 1.0],
        },
        TerrainEdit::ClearStrokes,
        TerrainEdit::DropLastStrokes { count: 3 },
        TerrainEdit::AddSpline {
            spline: Box::new(SplineDef::default()),
        },
        TerrainEdit::AddMaterial {
            material: Box::new(MaterialLayer::new("M", [0.5; 3], 0.8)),
        },
        TerrainEdit::AddBiome {
            biome: Box::new(BiomeRule::by_height("B", [0.0, 1.0, 2.0, 3.0], 0)),
        },
        TerrainEdit::AddProto {
            proto: Box::new(ScatterProto::new("P", "k", 1.0, 2.0)),
        },
        TerrainEdit::BindProto {
            index: 0,
            mesh_key: "k".into(),
            lod_keys: vec![],
            impostor_key: None,
        },
        TerrainEdit::AddScatter {
            rule: Box::new(ScatterRule::new("R", 0, 10.0)),
        },
        TerrainEdit::SetWater {
            water: WaterConfig::default(),
        },
        TerrainEdit::SetLod {
            lod: LodPolicy::default(),
        },
        TerrainEdit::SetBudget {
            budget: MemoryBudget::default(),
        },
    ];
    for e in edits {
        let json = serde_json::to_string(&e).expect("serialize");
        let back: TerrainEdit = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(e, back, "round trip failed for {json}");
        assert!(!e.label().is_empty());
    }
}

#[test]
fn a_terrain_is_created_from_a_description_and_can_be_re_described() {
    // The exact live sequence: describe once into an empty scene, then describe something completely
    // different over it. The second call goes through `apply_edit`, which validates BEFORE committing — so a
    // description that synthesises an invalid recipe surfaces here rather than as a refusal in the app.
    let (mut engine, scene) = engine_with_scene();
    let (id, recipe, reading) = create_from_description(
        &mut engine,
        &scene,
        "a 3 km eroded alpine valley with a river, a road and dense conifer forest",
        [0.0; 3],
    )
    .expect("a description creates a terrain");
    assert_eq!(reading.brief.landform, describe::Landform::Mountains);
    assert_eq!(recipe.splines.len(), 2, "a river and a road");
    // 3 000 m snapped to whole 64 m chunks: 3000/64 = 46.875 -> 47 chunks -> 3 008 m.
    assert!(
        (recipe.world_size_m - 3008.0).abs() < 1e-3,
        "{}",
        recipe.world_size_m
    );
    // The description is recorded ON THE RECIPE, so reopening the project shows what was asked for — and it
    // must NOT have been written over the `source` field, which is where the recipe JSON itself lives.
    assert!(recipe.description.contains("alpine valley"));
    assert_eq!(
        read(&engine, id).expect("readable").description,
        recipe.description
    );

    // Re-describe: a completely different world, one undo step, and it must NOT be rejected.
    let next = apply_edit(
        &mut engine,
        id,
        &TerrainEdit::Describe {
            text: "a small lush tropical archipelago with beaches, palms and unicorns".into(),
        },
    )
    .expect("re-describing must be accepted")
    .0;
    assert!(
        (next.world_size_m - 1024.0).abs() < 1e-3,
        "{}",
        next.world_size_m
    );
    assert!(next.water.enabled, "an archipelago has a sea");
    assert!(
        next.protos.iter().any(|p| p.name.contains("Palm")),
        "tropical species: {:?}",
        next.protos.iter().map(|p| &p.name).collect::<Vec<_>>()
    );
    // And undo puts the alpine valley back, whole.
    assert!(engine.undo(), "undo");
    let back = read(&engine, id).expect("readable");
    assert!(
        (back.world_size_m - 3008.0).abs() < 1e-3,
        "{}",
        back.world_size_m
    );
    assert_eq!(back.splines.len(), 2);
}

// ── M19.2 (ADR-106) — the local edit path: language changes PART of a world ────────────────────────────

#[test]
fn a_local_operation_changes_one_landform_and_reports_a_bounded_region() {
    use metrocalk_terrain::feature::{FeatureKind, Skeleton, WorldFeature};
    use metrocalk_terrain::operation::{Target, TerrainOperation, Verb};
    use metrocalk_terrain::stage::Stage;

    let (mut engine, scene) = engine_with_scene();
    let id = create(&mut engine, &scene, "rolling-hills");

    // Give the world a named landform, as a description would.
    let mut recipe = read(&engine, id).expect("read");
    let mut m = WorldFeature::new(
        1,
        "North Ridge",
        FeatureKind::Mountain,
        Skeleton::Point { x: 700.0, z: 700.0 },
    );
    m.extent_m = 200.0;
    m.amplitude_m = 150.0;
    recipe.features.push(m);
    recipe.next_feature_id = 2;
    let before = {
        let t = Terrain::compile(recipe.clone(), BTreeMap::new()).expect("compile");
        (t.height(700.0, 700.0), t.height(1800.0, 1800.0))
    };
    let ops = write_ops(id, &recipe, false).expect("ops");
    engine.commit("seed-feature", ops).expect("commit");

    // "raise North Ridge by 100 m"
    let mut op = TerrainOperation::new(
        Verb::Raise,
        Target::Named {
            name: "north ridge".into(),
        },
    );
    op.params.delta_m = Some(100.0);
    let (after_recipe, effect) =
        apply_edit(&mut engine, id, &TerrainEdit::Apply { ops: vec![op] }).expect("apply");

    // The landform moved...
    assert!((after_recipe.features[0].amplitude_m - 250.0).abs() < 1e-3);
    let after = {
        let t = Terrain::compile(after_recipe.clone(), BTreeMap::new()).expect("compile");
        (t.height(700.0, 700.0), t.height(1800.0, 1800.0))
    };
    assert!(
        after.0 > before.0 + 50.0,
        "the ridge rose: {:?} -> {:?}",
        before.0,
        after.0
    );
    // ...and the rest of the world did NOT. This is the property that makes local editing worth having.
    assert!(
        (after.1 - before.1).abs() < 1e-4,
        "ground a kilometre away moved: {} -> {}",
        before.1,
        after.1
    );

    // The reported region is bounded, and it covers the landform.
    let (min, max) = effect.bounds().expect("a local edit must report a region");
    assert!(
        max[0] - min[0] < after_recipe.world_size_m,
        "not a whole-world rebuild"
    );
    assert!(min[0] <= 700.0 && max[0] >= 700.0 && min[1] <= 700.0 && max[1] >= 700.0);
    // And it rebuilds the ground and everything downstream, but the graph decided that — not a default.
    assert!(effect.stages.has(Stage::Elevation) && effect.stages.has(Stage::Lod));

    // One undo step puts it back.
    assert!(engine.undo(), "undo");
    assert!((read(&engine, id).expect("read").features[0].amplitude_m - 150.0).abs() < 1e-3);
}

#[test]
fn a_local_operation_preserves_every_hand_edit_that_a_re_describe_would_destroy() {
    use metrocalk_terrain::feature::{FeatureKind, Skeleton, WorldFeature};
    use metrocalk_terrain::operation::{Target, TerrainOperation, Verb};

    let (mut engine, scene) = engine_with_scene();
    let id = create(&mut engine, &scene, "rolling-hills");
    // Sculpt something by hand.
    apply_edit(
        &mut engine,
        id,
        &TerrainEdit::Sculpt {
            brush: Brush {
                radius_m: 30.0,
                strength: 8.0,
                ..Brush::default()
            },
            from: [300.0, 300.0],
            to: [340.0, 300.0],
        },
    )
    .expect("sculpt");
    let strokes = read(&engine, id).expect("read").strokes.len();
    assert!(strokes > 0);

    // A LOCAL operation keeps them...
    let mut op = TerrainOperation::new(
        Verb::Plant,
        Target::Here {
            at: [1200.0, 1200.0],
            radius_m: 200.0,
        },
    );
    op.params.kind = Some(FeatureKind::Forest);
    let (kept, _) =
        apply_edit(&mut engine, id, &TerrainEdit::Apply { ops: vec![op] }).expect("plant");
    assert_eq!(
        kept.strokes.len(),
        strokes,
        "a local edit destroyed hand sculpting"
    );
    assert!(kept.features.iter().any(|f| f.kind == FeatureKind::Forest));

    // ...whereas a re-describe replaces the world, which is why it must not be the only modify path.
    let (replaced, effect) = apply_edit(
        &mut engine,
        id,
        &TerrainEdit::Describe {
            text: "a flat desert".into(),
        },
    )
    .expect("describe");
    assert_eq!(
        replaced.strokes.len(),
        0,
        "a re-describe is a new world, by design"
    );
    assert!(
        effect.region.is_none(),
        "and it honestly reports a whole-world rebuild"
    );
    let _ = WorldFeature::new(
        9,
        "x",
        FeatureKind::Hill,
        Skeleton::Point { x: 0.0, z: 0.0 },
    );
}

#[test]
fn planting_a_forest_does_not_ask_the_world_to_re_mesh() {
    use metrocalk_terrain::feature::FeatureKind;
    use metrocalk_terrain::operation::{Target, TerrainOperation, Verb};
    use metrocalk_terrain::stage::Stage;

    let (mut engine, scene) = engine_with_scene();
    let id = create(&mut engine, &scene, "rolling-hills");
    let mut op = TerrainOperation::new(
        Verb::Plant,
        Target::Here {
            at: [500.0, 500.0],
            radius_m: 150.0,
        },
    );
    op.params.kind = Some(FeatureKind::Forest);
    let (_, effect) =
        apply_edit(&mut engine, id, &TerrainEdit::Apply { ops: vec![op] }).expect("plant");
    assert!(effect.stages.has(Stage::Vegetation));
    assert!(
        !effect.stages.has(Stage::Elevation),
        "planting must not re-run the ground: {}",
        effect.stages.describe()
    );
    assert!(!effect.stages.has(Stage::Collision) && !effect.stages.has(Stage::Navigation));
}

#[test]
fn moving_a_river_rebuilds_the_channel_it_left_behind() {
    // The confirmed bug this fixes: SetSpline reported only the NEW corridor, so the old carve stayed in
    // the resident chunks and the world kept a ghost river.
    let (mut engine, scene) = engine_with_scene();
    let id = create(&mut engine, &scene, "rolling-hills");
    let river = SplineDef {
        name: "Water".into(),
        kind: metrocalk_terrain::recipe::SplineKind::River,
        points: vec![[200.0, 0.0, 200.0], [400.0, 0.0, 400.0]],
        width_m: 12.0,
        falloff_m: 10.0,
        depth_m: 3.0,
        use_point_height: false,
        material_layer: None,
        clear_scatter_m: 6.0,
        enabled: true,
    };
    apply_edit(
        &mut engine,
        id,
        &TerrainEdit::AddSpline {
            spline: Box::new(river.clone()),
        },
    )
    .expect("add");

    // Move it a long way.
    let mut moved = river;
    moved.points = vec![[1500.0, 0.0, 1500.0], [1700.0, 0.0, 1700.0]];
    let (_, effect) = apply_edit(
        &mut engine,
        id,
        &TerrainEdit::SetSpline {
            index: 0,
            spline: Box::new(moved),
        },
    )
    .expect("move");

    let (min, max) = effect.bounds().expect("bounded");
    // The NEW corridor...
    assert!(
        min[0] <= 1500.0 && max[0] >= 1700.0,
        "new corridor not covered: {min:?}..{max:?}"
    );
    // ...AND the old one, which is the whole point.
    assert!(
        min[0] <= 200.0 && min[1] <= 200.0,
        "the vacated channel was not rebuilt: {min:?}"
    );
}

#[test]
fn removing_a_biome_repoints_every_scatter_rule_that_named_one_above_it() {
    // The silent corruption this exists for: scatter rules name biomes BY POSITION, and `Vec::remove`
    // shifts every position above the hole down by one. Deleting "Shore" therefore left the woodland
    // pointing at what used to be the uplands — a world that is wrong everywhere and in range everywhere,
    // so no validator and no error could catch it.
    let (mut engine, scene) = engine_with_scene();
    let id = create(&mut engine, &scene, "rolling-hills");
    let before = read(&engine, id).expect("recipe");
    assert!(before.biomes.len() >= 4, "{:?}", before.biomes.len());

    // Name each rule's biomes by the biome NAME, so the assertion is about meaning, not about numbers.
    let named = |r: &TerrainRecipe| -> Vec<Vec<String>> {
        r.scatter
            .iter()
            .map(|s| {
                s.biomes
                    .iter()
                    .filter_map(|b| r.biomes.get(*b).map(|x| x.name.clone()))
                    .collect()
            })
            .collect()
    };
    let meaning_before = named(&before);
    let doomed = before.biomes[0].name.clone();

    apply_edit(&mut engine, id, &TerrainEdit::RemoveBiome { index: 0 }).expect("remove");
    let after = read(&engine, id).expect("recipe");
    assert_eq!(after.biomes.len(), before.biomes.len() - 1);

    // Every surviving reference still means the same biome it meant before.
    for (i, (was, now)) in meaning_before.iter().zip(named(&after).iter()).enumerate() {
        let expected: Vec<&String> = was.iter().filter(|n| **n != doomed).collect();
        let got: Vec<&String> = now.iter().collect();
        assert_eq!(expected, got, "scatter[{i}] changed meaning");
    }

    // Every index is still addressable — the shift cannot have pushed one off the end either.
    for (i, s) in after.scatter.iter().enumerate() {
        for b in &s.biomes {
            assert!(
                *b < after.biomes.len(),
                "scatter[{i}] biome {b} out of range"
            );
        }
    }
}

#[test]
fn a_rule_that_named_only_the_removed_biome_is_disabled_not_promoted_to_everywhere() {
    // An empty biome list means "every biome". So the naive fix — drop the dead index and stop — turns a
    // rule that applied to one niche biome into one that carpets the entire world. Louder than the bug it
    // replaces, and just as silent.
    let (mut engine, scene) = engine_with_scene();
    let id = create(&mut engine, &scene, "rolling-hills");
    let mut recipe = read(&engine, id).expect("recipe");
    let only = recipe.biomes.len() - 1;
    let mut rule = recipe.scatter[0].clone();
    rule.name = "Cliff Moss".into();
    rule.biomes = vec![only];
    rule.enabled = true;
    recipe.scatter.push(rule);
    apply_edit(
        &mut engine,
        id,
        &TerrainEdit::AddScatter {
            rule: Box::new(recipe.scatter.last().expect("just pushed").clone()),
        },
    )
    .expect("add");

    apply_edit(&mut engine, id, &TerrainEdit::RemoveBiome { index: only }).expect("remove");
    let after = read(&engine, id).expect("recipe");
    let moss = after
        .scatter
        .iter()
        .find(|s| s.name == "Cliff Moss")
        .expect("the rule survives");
    assert!(moss.biomes.is_empty(), "its only biome is gone");
    assert!(
        !moss.enabled,
        "so it must be OFF, not applied to every biome"
    );
}

#[test]
fn stand_in_meshes_regenerate_to_byte_identical_handles_after_a_reopen() {
    // The reopen path depends on exactly this property. The `.mtk` holds the document alone, so the
    // generated vegetation meshes are gone on the next launch and the recipe's handles resolve to nothing
    // — a world that comes back bald with no error. Regenerating is only a faithful restore if it lands on
    // the SAME content addresses, which is what this pins.
    let (mut engine, scene) = engine_with_scene();
    let id = create(&mut engine, &scene, "rolling-hills");
    let mut store = AssetStore::default();
    let mut recipe = read(&engine, id).expect("recipe");
    let installed = install_stand_in_protos(&mut store, &mut recipe);
    assert!(!installed.is_empty(), "the preset declares prototypes");

    // A fresh process: a new store, and the recipe as the document persisted it.
    let mut reopened = AssetStore::default();
    let again = regenerate_stand_in_protos(&mut reopened, &recipe);
    assert_eq!(
        again.len(),
        recipe.protos.len(),
        "every prototype, bound or not"
    );
    for first in &installed {
        let second = again
            .iter()
            .find(|p| p.proto_index == first.proto_index)
            .expect("the same prototype");
        assert_eq!(first.mesh_key, second.mesh_key, "full mesh handle");
        assert_eq!(first.lod_keys, second.lod_keys, "LOD handles");
        assert_eq!(first.impostor_key, second.impostor_key, "impostor handle");
    }
    // And the handles the document carries are exactly the ones regeneration produces, which is what makes
    // the restore a lookup rather than a re-bind.
    for proto in &recipe.protos {
        assert!(
            again.iter().any(|p| p.mesh_key == proto.mesh_key),
            "no regenerated mesh matches {}",
            proto.mesh_key
        );
    }
}
