//! A small standard library of real component kinds — the seed catalog the compatibility query and
//! (later) describe-to-create operate over. Built via the [`ComponentMeta`] builder (less magic than
//! a derive macro, and the same runtime path a plugin or marketplace component uses).
//!
//! Capabilities form the relational web: e.g. `Sprite` and `MeshRenderer` both *provide* `Renderable`
//! and *require* `Spatial`; `HealthBar` *requires* + *observes* `Health` and *provides* `UIElement`.

use crate::registry::{ActionMeta, ComponentMeta, EventMeta, FieldType, PluginMeta};

/// The standard component kinds. Registering all of them populates the relational catalog.
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
        // Physics (M8.2, ADR-021): metadata for the registry/intent system; the live simulation behind
        // these rides the project-owned `Physics` trait (`/physics`). `kind`/`shape` are String fields
        // (FieldType is scalar-only) carrying a closed vocab via the ui_hint — the sync seam maps them to
        // `physics::BodyKind` / `ColliderShape`. Collider `requires("Physics")` so it rides the M3.1
        // reveal as a one-click attach onto a RigidBody (which provides "Physics") — "this body needs a
        // collider, add one?".
        ComponentMeta::builder("RigidBody")
            .category("Gameplay")
            .field("kind", Str, true)
            .field("mass", Number, false)
            .field("linearDamping", Number, false)
            .field("angularDamping", Number, false)
            .field("gravityScale", Number, false)
            .requires("Spatial")
            .provides("Physics")
            .tag("physics")
            .alias("Rigidbody")
            .ui_hint(
                "kind",
                "enum: dynamic|fixed|kinematicPosition|kinematicVelocity",
            )
            .build(),
        ComponentMeta::builder("Collider")
            .category("Gameplay")
            .field("shape", Str, true)
            .field("isTrigger", Boolean, false)
            .field("density", Number, false)
            .field("friction", Number, false)
            .field("restitution", Number, false)
            // Flat scalar shape params (no Vec3 FieldType) — read per `shape` at the sync seam.
            .field("radius", Number, false)
            .field("halfX", Number, false)
            .field("halfY", Number, false)
            .field("halfZ", Number, false)
            .field("halfHeight", Number, false)
            .requires("Spatial")
            .requires("Physics")
            .provides("Collision")
            .tag("physics")
            .tag("collision")
            .ui_hint(
                "shape",
                "enum: ball|cuboid|capsule|convexHull|triMesh|convexDecomposition|voxels|sdf",
            )
            .build(),
        ComponentMeta::builder("Joint")
            .category("Gameplay")
            .field("kind", Str, true)
            .field_fmt("bodyA", Str, true, Some("entity-ref"))
            .field_fmt("bodyB", Str, true, Some("entity-ref"))
            .requires("Physics")
            .provides("Joint")
            .tag("physics")
            .tag("joint")
            .ui_hint("kind", "enum: revolute|fixed|spherical")
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
        // M11.3 (ADR-042): a real, authored light. `kind` picks Directional/Point/Spot; `r/g/b` is the linear
        // colour, `intensity` the strength; point/spot use the entity Transform's position + `range` falloff;
        // directional/spot aim along `dir*` (default straight down). `castShadows` (consumed by the shadow
        // pass) defaults off. Authoring a light is one undoable component commit (it rides the registry like
        // any other component); the per-frame LIT RESULT is a render projection (never Loro), per ADR-021.
        ComponentMeta::builder("Light")
            .category("Props")
            .field("kind", Str, false)
            .field("intensity", Number, true)
            .field("r", Number, false)
            .field("g", Number, false)
            .field("b", Number, false)
            .field("range", Number, false)
            .field("dirX", Number, false)
            .field("dirY", Number, false)
            .field("dirZ", Number, false)
            .field("castShadows", Boolean, false)
            .requires("Spatial")
            .provides("Lighting")
            .tag("3d")
            .tag("light")
            .ui_hint("kind", "enum: directional|point|spot")
            .build(),
        // M11.4 (ADR-043): a scene camera — the view the *game* renders, distinct from the editor fly-cam.
        // `fov`/`near`/`far` + position via the entity Transform; `active` picks which one Play / look-through
        // renders from. Authoring a camera is one undoable component commit (rides the registry); the editor
        // fly-cam stays render/tool state (never Loro). The look-through view-proj is a render projection.
        ComponentMeta::builder("Camera")
            .category("Props")
            .field("fov", Number, false)
            .field("near", Number, false)
            .field("far", Number, false)
            .field("active", Boolean, false)
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
        // ── M12.1 (ADR-045) rule-target primitives — the counters / quest-state / effect components a Rule
        // reads + mutates through the typed vocabulary (the building blocks of the test-5 conditional). ──
        ComponentMeta::builder("KillCounter")
            .category("Logic")
            .field("count", Integer, true)
            .provides("Counter")
            .tag("quest")
            .tag("counter")
            .ui_hint("count", "enemies defeated so far")
            .build(),
        ComponentMeta::builder("QuestState")
            .category("Logic")
            .field("state", Str, true)
            .provides("QuestState")
            .tag("quest")
            .ui_hint(
                "state",
                "the quest phase, e.g. Hunting|ReadyForBoss|FacingBoss",
            )
            .build(),
        ComponentMeta::builder("Zone")
            .category("Logic")
            .field("current", Str, true)
            .provides("Zone")
            .tag("quest")
            .ui_hint("current", "the area the entity is in, e.g. BossArena")
            .build(),
        ComponentMeta::builder("Flammable")
            .category("Gameplay")
            .field("lit", Boolean, true)
            .provides("Flammable")
            .tag("effect")
            .ui_hint("lit", "whether the object is currently on fire")
            .build(),
    ]
}

/// The standard rule **events** — the "When" vocabulary the Rules builder (M12.1 / ADR-045) offers. The
/// `*Entered`/`*Exited` pairs are what the mirror-rule proposer ([`crate::rules::propose_mirror`]) inverts.
#[must_use]
pub fn standard_events() -> Vec<EventMeta> {
    vec![
        EventMeta::new("EnemyDied", "an enemy was defeated"),
        EventMeta::new("EntitySpawned", "an entity was created in the scene"),
        EventMeta::new("EntityDestroyed", "an entity was removed from the scene"),
        EventMeta::new("ZoneEntered", "an entity entered an area / zone"),
        EventMeta::new("ZoneExited", "an entity left an area / zone"),
        EventMeta::new("StateEntered", "a quest/state machine entered a state"),
        EventMeta::new("StateExited", "a quest/state machine left a state"),
    ]
}

/// The standard rule **actions** — the CLOSED "Then" vocabulary (the honest ceiling: verbs over component
/// fields, never free code; genuinely algorithmic behaviour is the M12.3 plugin tier).
#[must_use]
pub fn standard_actions() -> Vec<ActionMeta> {
    vec![
        ActionMeta::new("SetField", "set a component field to a value"),
        ActionMeta::new("AdjustCounter", "add a number to a numeric counter field"),
        // M12.3 (ADR-047) — the honest-ceiling escape: hand off to a sandboxed WASM plugin for genuinely
        // algorithmic behavior (a boss AI, a procedural generator, a custom solver). Still a CLOSED verb —
        // the algorithm is the plugin's, not free code in a Rule.
        ActionMeta::new(
            "RunPlugin",
            "run a sandboxed WASM plugin for algorithmic behavior (the honest ceiling)",
        ),
    ]
}

/// The standard **WASM-plugin** components (M12.3 / ADR-047) — the algorithmic escape a `RunPlugin` rule
/// action invokes. The example `arrange` plugin is a deterministic procedural arrangement (so it's eligible
/// for the Play/replay lockstep path). Registering a plugin makes it referenceable + typed (reveal/explain);
/// the host (`/plugins`) loads each by name from its sandboxed `.wasm`.
#[must_use]
pub fn standard_plugins() -> Vec<PluginMeta> {
    vec![PluginMeta::new(
        "arrange",
        "deterministically arrange entities in a procedural spiral",
        true,
    )]
}
