//! M8.5 live wiring — a parsed URDF / USD-Physics import becomes **registry-component entities** through
//! the commit pipeline (one undoable transaction), proven headless: import a 2-link URDF arm → two
//! physics entities (Transform + RigidBody + Collider) + a Joint, every assumption explained, units SI;
//! the import is undoable like any scene edit.

use metrocalk_core::{Engine, EntityId, FieldValue};
use metrocalk_ecs::FlecsWorld;

use metrocalk_editor_shell::capscene::{self, CapScene};
use metrocalk_interchange::{Interchange, UrdfInterchange};

const ARM: &str = r#"
<robot name="arm">
  <link name="base">
    <inertial><mass value="5.0"/><inertia ixx="1" ixy="0" ixz="0" iyy="1" iyz="0" izz="1"/></inertial>
    <collision><geometry><box size="0.4 0.4 0.2"/></geometry></collision>
  </link>
  <link name="upper">
    <inertial><mass value="2.0"/><inertia ixx="1" ixy="0" ixz="0" iyy="1" iyz="0" izz="1"/></inertial>
    <collision><geometry><sphere radius="0.15"/></geometry></collision>
  </link>
  <joint name="shoulder" type="revolute">
    <parent link="base"/><child link="upper"/>
    <origin xyz="0 0 0.6" rpy="0 0 0"/><axis xyz="0 1 0"/>
    <limit lower="-1.0" upper="1.0" effort="100" velocity="1"/>
  </joint>
</robot>
"#;

fn components(
    engine: &Engine<FlecsWorld>,
    id: EntityId,
) -> std::collections::HashMap<String, std::collections::HashMap<String, FieldValue>> {
    engine.components_of(id)
}

#[test]
fn urdf_import_becomes_registry_components_in_one_undoable_tx() {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);

    let import = UrdfInterchange.import(ARM.as_bytes()).expect("parse URDF");
    assert!(
        import.notes.iter().any(|n| n.feature.contains("limit")),
        "the unenforced limit is explained"
    );

    let before = engine.entity_ids().len();
    let ids = capscene::import_scene(&mut engine, &scene, &import).expect("import");
    assert_eq!(ids.len(), 2, "two links → two body entities");

    // base = the root → Fixed + a cuboid collider; upper = Dynamic + a ball; both carry RigidBody+Collider.
    let base = ids
        .iter()
        .copied()
        .find(|id| {
            components(&engine, *id)
                .get("RigidBody")
                .and_then(|m| m.get("kind"))
                == Some(&FieldValue::Str("fixed".into()))
        })
        .expect("a Fixed root body");
    let bc = components(&engine, base);
    assert_eq!(
        bc.get("Collider").and_then(|m| m.get("shape")),
        Some(&FieldValue::Str("cuboid".into()))
    );
    assert_eq!(
        bc.get("RigidBody").and_then(|m| m.get("mass")),
        Some(&FieldValue::Number(5.0))
    );

    let upper = ids.iter().copied().find(|id| *id != base).unwrap();
    let uc = components(&engine, upper);
    assert_eq!(
        uc.get("RigidBody").and_then(|m| m.get("kind")),
        Some(&FieldValue::Str("dynamic".into()))
    );
    assert_eq!(
        uc.get("Collider").and_then(|m| m.get("shape")),
        Some(&FieldValue::Str("ball".into()))
    );
    // The upper link is FK-placed at the joint origin (z = 0.6).
    assert_eq!(
        uc.get("Transform").and_then(|m| m.get("z")),
        Some(&FieldValue::Number(0.6))
    );

    // A Joint entity references the two bodies (inspectable).
    let joint = engine
        .entity_ids()
        .into_iter()
        .find(|id| components(&engine, *id).contains_key("Joint"))
        .expect("a Joint entity");
    let jc = components(&engine, joint);
    assert_eq!(
        jc.get("Joint").and_then(|m| m.get("kind")),
        Some(&FieldValue::Str("revolute".into()))
    );
    assert_eq!(
        jc.get("Joint").and_then(|m| m.get("bodyA")),
        Some(&FieldValue::Str(base.to_loro_key()))
    );

    // ONE undoable transaction — Ctrl-Z peels the whole import back.
    assert!(engine.entity_ids().len() > before);
    assert!(engine.undo(), "the import is undoable");
    assert_eq!(
        engine.entity_ids().len(),
        before,
        "undo removed every imported entity"
    );
}
