//! A small standard library of real component kinds — the seed catalog the compatibility query and
//! (later) describe-to-create operate over. Built via the [`ComponentMeta`] builder (less magic than
//! a derive macro, and the same runtime path a plugin or marketplace component uses).
//!
//! Capabilities form the relational web: e.g. `Sprite` and `MeshRenderer` both *provide* `Renderable`
//! and *require* `Spatial`; `HealthBar` *requires* + *observes* `Health` and *provides* `UIElement`.

use crate::registry::{ComponentMeta, FieldType};

/// The standard component kinds (12). Registering all of them populates the relational catalog.
#[allow(clippy::too_many_lines)] // a flat data table of component definitions, not branching logic
pub fn standard_components() -> Vec<ComponentMeta> {
    use FieldType::{Boolean, Integer, Number, String as Str};
    let asset = Some("asset");

    vec![
        ComponentMeta::builder("Transform")
            .category("Props")
            .field("px", Number, true)
            .field("py", Number, true)
            .field("pz", Number, true)
            .field("rx", Number, false)
            .field("ry", Number, false)
            .field("rz", Number, false)
            .field("sx", Number, false)
            .field("sy", Number, false)
            .field("sz", Number, false)
            .provides("Spatial")
            .tag("core")
            .tag("transform")
            .build(),
        ComponentMeta::builder("Health")
            .category("Gameplay")
            .field("hp", Integer, true)
            .field("maxHp", Integer, true)
            .field("regen", Number, false)
            .provides("Health")
            .tag("stats")
            .tag("combat")
            .alias("HP")
            .alias("HitPoints")
            .ui_hint("hp", "slider 0..maxHp")
            .build(),
        ComponentMeta::builder("HealthBar")
            .category("UI")
            .field("width", Number, false)
            .field("anchor", Str, false)
            .requires("Health")
            .observes("Health")
            .provides("UIElement")
            .tag("ui")
            .tag("hud")
            .alias("HP bar")
            .build(),
        ComponentMeta::builder("Sprite")
            .category("Props")
            .field_fmt("texture", Str, true, asset)
            .field("layer", Integer, false)
            .field("flipX", Boolean, false)
            .requires("Spatial")
            .provides("Renderable")
            .tag("2d")
            .tag("render")
            .build(),
        ComponentMeta::builder("MeshRenderer")
            .category("Props")
            .field_fmt("mesh", Str, true, asset)
            .field_fmt("material", Str, false, asset)
            .field("castShadows", Boolean, false)
            .requires("Spatial")
            .provides("Renderable")
            .tag("3d")
            .tag("render")
            .build(),
        ComponentMeta::builder("RigidBody")
            .category("Gameplay")
            .field("mass", Number, true)
            .field("kinematic", Boolean, false)
            .field("drag", Number, false)
            .requires("Spatial")
            .provides("Physics")
            .tag("physics")
            .alias("Rigidbody")
            .build(),
        ComponentMeta::builder("Collider")
            .category("Gameplay")
            .field("shape", Str, true)
            .field("isTrigger", Boolean, false)
            .field("friction", Number, false)
            .requires("Spatial")
            .observes("Physics")
            .provides("Collision")
            .tag("physics")
            .tag("collision")
            .build(),
        ComponentMeta::builder("AudioSource")
            .category("Audio")
            .field_fmt("clip", Str, true, asset)
            .field("volume", Number, false)
            .field("looping", Boolean, false)
            .requires("Spatial")
            .provides("Audio")
            .tag("audio")
            .alias("Sound")
            .build(),
        ComponentMeta::builder("Light")
            .category("Props")
            .field("intensity", Number, true)
            .field_fmt("color", Str, false, Some("color"))
            .field("range", Number, false)
            .requires("Spatial")
            .provides("Lighting")
            .tag("3d")
            .tag("light")
            .build(),
        ComponentMeta::builder("Camera")
            .category("Props")
            .field("fov", Number, false)
            .field("near", Number, false)
            .field("far", Number, false)
            .requires("Spatial")
            .provides("View")
            .tag("3d")
            .tag("camera")
            .build(),
        ComponentMeta::builder("Animator")
            .category("Gameplay")
            .field_fmt("controller", Str, true, asset)
            .field("speed", Number, false)
            .requires("Spatial")
            .observes("Spatial")
            .provides("Animation")
            .tag("animation")
            .build(),
        ComponentMeta::builder("Script")
            .category("Logic")
            .field_fmt("source", Str, true, asset)
            .field("enabled", Boolean, false)
            .provides("Behavior")
            .tag("logic")
            .tag("code")
            .alias("Behavior")
            .build(),
    ]
}
