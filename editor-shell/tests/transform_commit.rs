//! M9.1 gizmo commit — `capscene::set_transform` writes the full TRS (position + rotation quat + uniform
//! scale) as ONE undoable transaction, and the renderer/HUD read it back. Proven headless: commit a moved/
//! rotated/scaled pose → every field round-trips through `components_of` → undo reverts the whole pose
//! atomically. (The interaction math is tested in `/gizmo`; this covers the commit + persistence shape.)

use metrocalk_core::{Engine, EntityId, FieldValue};
use metrocalk_ecs::FlecsWorld;

use metrocalk_editor_shell::capscene::{self, CapScene};
use metrocalk_interchange::Interchange;

fn field(engine: &Engine<FlecsWorld>, id: EntityId, f: &str) -> Option<f64> {
    match engine
        .components_of(id)
        .get("Transform")
        .and_then(|m| m.get(f))
    {
        Some(FieldValue::Number(n)) => Some(*n),
        _ => None,
    }
}

#[test]
fn set_transform_writes_full_trs_in_one_undoable_tx() {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);

    // Import a body to move (any entity with a Transform works; a URDF link is convenient).
    let import = metrocalk_interchange::UrdfInterchange
        .import(
            br#"<robot name="r"><link name="l"><inertial><mass value="1"/><inertia ixx="1" ixy="0" ixz="0" iyy="1" iyz="0" izz="1"/></inertial><collision><geometry><box size="1 1 1"/></geometry></collision></link></robot>"#,
        )
        .unwrap();
    let id = capscene::import_scene(&mut engine, &scene, &import).unwrap()[0];

    // A 90°-about-Y rotation (quat [0, sin45, 0, cos45]), moved to (3,4,5), scaled 2×.
    let s = std::f32::consts::FRAC_1_SQRT_2;
    let rot = [0.0, s, 0.0, s];
    capscene::set_transform(&mut engine, id, [3.0, 4.0, 5.0], rot, 2.0).expect("commit");

    // Every field round-trips.
    assert_eq!(field(&engine, id, "x"), Some(3.0));
    assert_eq!(field(&engine, id, "y"), Some(4.0));
    assert_eq!(field(&engine, id, "z"), Some(5.0));
    assert!((field(&engine, id, "qw").unwrap() - f64::from(rot[3])).abs() < 1e-6);
    assert!((field(&engine, id, "qy").unwrap() - f64::from(rot[1])).abs() < 1e-6);
    assert_eq!(field(&engine, id, "scale"), Some(2.0));

    // ONE undoable transaction — Ctrl-Z reverts the WHOLE pose (position + rotation + scale) atomically.
    assert!(engine.undo(), "the transform commit is undoable");
    // Back to the imported pose (x ≈ 0, scale field gone or the prior value).
    assert_ne!(
        field(&engine, id, "x"),
        Some(3.0),
        "undo reverted the position"
    );
    assert_ne!(
        field(&engine, id, "scale"),
        Some(2.0),
        "undo reverted the scale"
    );
}

/// **ADR-172 — the declaration and the storage, compared.**
///
/// `core/src/stdlib.rs` states what a `Transform` is; `capscene::set_transform` states it again by
/// writing one, and `capscene::local_transform` a third time by reading one. From M1.3 until
/// 2026-08-28 the first of those said `px/py/pz` + Euler `rx/ry/rz` + non-uniform `sx/sy/sz` and the
/// other two said `x/y/z` + a quaternion + a uniform scale, and **nothing compared them** — so the
/// Inspector's curated table and its vector rows were both built against fields no entity has ever
/// carried, and the gate written to keep the editor honest (`check-registry-vocab.mjs`) compared the
/// editor to the half that was wrong.
///
/// This is the comparison. It is deliberately BEHAVIOURAL rather than a second hand-written list:
/// the key set comes from actually committing a pose through the only writer and reading the
/// document back, so re-typing a field name in `set_transform`, in `local_transform`, or in the
/// registry block turns it red.
#[test]
fn stdlib_transform_matches_the_stored_transform() {
    use std::collections::BTreeSet;

    let mut world = FlecsWorld::new();
    let _scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    let id = capscene::create_entity(&mut engine, [1.0, 2.0, 3.0], "Probe").expect("create");
    let s = std::f32::consts::FRAC_1_SQRT_2;
    capscene::set_transform(&mut engine, id, [1.0, 2.0, 3.0], [0.0, s, 0.0, s], 2.0).expect("pose");

    let stored: BTreeSet<String> = engine
        .components_of(id)
        .get("Transform")
        .expect("the probe carries a Transform")
        .keys()
        .cloned()
        .collect();

    let declared: BTreeSet<String> = metrocalk_core::stdlib::standard_components()
        .into_iter()
        .find(|meta| meta.name == "Transform")
        .expect("the core registers a Transform")
        .fields
        .iter()
        .map(|field| field.name.clone())
        .collect();

    assert_eq!(
        stored, declared,
        "the fields a committed Transform holds and the fields `stdlib.rs` declares must be the \
         same set — a registry that describes storage nothing writes is what ADR-172 repaired"
    );

    // And the READER sees every one of them: a declaration both halves agree about is still useless
    // if `local_transform` is looking somewhere else.
    let read = capscene::local_transform(&engine, id);
    assert!((f64::from(read.translation[0]) - 1.0).abs() < 1e-6, "x");
    assert!((f64::from(read.translation[1]) - 2.0).abs() < 1e-6, "y");
    assert!((f64::from(read.translation[2]) - 3.0).abs() < 1e-6, "z");
    assert!((read.rotation[1] - s).abs() < 1e-6, "qy");
    assert!((read.rotation[3] - s).abs() < 1e-6, "qw");
    assert!((read.scale[0] - 2.0).abs() < 1e-6, "scale");
}
