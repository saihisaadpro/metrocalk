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
