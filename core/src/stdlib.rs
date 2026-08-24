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
            .field("frame", Integer, false)
            .field("layer", Integer, false)
            .field("flipX", Boolean, false)
            .field("flipY", Boolean, false)
            .field("opacity", Number, false)
            .field("tintR", Number, false)
            .field("tintG", Number, false)
            .field("tintB", Number, false)
            .field("tintA", Number, false)
            .requires("Spatial")
            .provides("Renderable")
            .tag("2d")
            .tag("render")
            .build(),
        ComponentMeta::builder("UiStyle")
            .category("UI")
            .field("x", Number, false)
            .field("y", Number, false)
            .field("width", Number, false)
            .field("height", Number, false)
            .field("scale", Number, false)
            .field("opacity", Number, false)
            .field("r", Number, false)
            .field("g", Number, false)
            .field("b", Number, false)
            .field("a", Number, false)
            .field("visible", Boolean, false)
            .provides("UIElement")
            .tag("ui")
            .tag("layout")
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
        // Pipe Forge's durable procedural source. The cooked mesh stays in the content-addressed asset
        // repository; the lightweight recipe JSON + build facts keep the result inspectable and editable.
        // Terrain (M19, ADR-104). A terrain is a RECIPE, not a mesh: the document holds the seed, layer
        // stack, sculpt strokes, splines and rules, and every chunk mesh / splat texture / collider / nav
        // grid is derived from it by a pure function. `source` is the recipe JSON minus its strokes; the
        // strokes ride their own packed field so a brush gesture writes ~40 bytes per dab instead of
        // rewriting the whole recipe (undo granularity is per gesture, and history stays small). The summary
        // fields are projections for the inspector — the recipe is the truth.
        ComponentMeta::builder("TerrainRecipe")
            .category("Terrain")
            .field("source", Str, true)
            .field("strokes", Str, false)
            .field("preset", Str, false)
            .field("seed", Integer, false)
            .field("worldSizeM", Number, false)
            .field("chunkSizeM", Number, false)
            .field("layerCount", Integer, false)
            .field("strokeCount", Integer, false)
            .requires("Spatial")
            .provides("Terrain")
            .provides("ProceduralAsset")
            .tag("3d")
            .tag("terrain")
            .tag("procedural")
            .alias("Landscape")
            .alias("Ground")
            .ui_hint("source", "editable terrain recipe")
            .ui_hint("strokes", "packed sculpt strokes")
            .build(),
        ComponentMeta::builder("PipeRecipe")
            .category("Asset")
            .field("source", Str, true)
            .field("version", Integer, true)
            .field("kit", Str, true)
            .field("diameterCm", Number, true)
            .field("lengthM", Number, false)
            .field("triangles", Integer, false)
            .field("textureResolution", Integer, false)
            .requires("Spatial")
            .provides("ProceduralAsset")
            .tag("3d")
            .tag("procedural")
            .tag("asset")
            .ui_hint("source", "editable Pipe Forge recipe")
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
        // directional/spot aim along `dir*` (default straight down). This renderer has one directional
        // shadow map: `castShadows` is meaningful for directional lights; authored point/spot lights retain
        // `false` until those shadow modes exist. Authoring a light is one undoable component commit (it
        // rides the registry like any other component); the per-frame LIT RESULT is a render projection
        // (never Loro), per ADR-021.
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
        // Gameplay roles (the Build→Play bridge). ONE component turns any asset into a live gameplay
        // participant: `role` names the archetype (collectible|solid|prop|spinner), `radius` is the
        // touch trigger's reach in metres, `points` what collecting it scores, and `active` is the
        // runtime liveness flag — a Play-mode rule flips it to false and the play overlay hides the
        // entity (RuntimeState only; the authored document never changes during Play).
        ComponentMeta::builder("GameRole")
            .category("Gameplay")
            .field("role", Str, true)
            .field("radius", Number, false)
            .field("points", Integer, false)
            .field("active", Boolean, false)
            // Companion brain tuning (role == "companion"): plain numeric fields so the
            // Inspector edits them like any other component — no bespoke UI needed.
            // `radius` doubles as the companion's aggro reach; for a waypoint, `points`
            // is its order in the patrol chain.
            .field("speed", Number, false)
            .field("range", Number, false)
            .field("follow", Number, false)
            .requires("Spatial")
            .provides("Gameplay")
            .tag("gameplay")
            .tag("role")
            .alias("Role")
            .ui_hint(
                "role",
                "enum: collectible|solid|prop|spinner|companion|enemy|waypoint|player",
            )
            .ui_hint("radius", "touch trigger / aggro reach, metres")
            .ui_hint("speed", "companion move speed, metres per second")
            .ui_hint("range", "companion attack reach, metres")
            .ui_hint("follow", "companion follow stand-off distance, metres")
            .build(),
        // The user's **"only if"** clauses for this object (conditionals). The clauses live HERE, on the
        // entity they are about — never as a per-entity copy of the role's rule. At Play-start the shared
        // `$subject` rule is expanded per entity and each object's clauses are appended to its own copy of
        // the recording, so the document still holds exactly one pickup rule and one defeat rule no matter
        // how many objects carry conditions. `source` is canonical JSON over the core `Condition` type (the
        // `ShapeRecipe.source` / `TerrainRecipe.source` precedent) — written only by the validated
        // `set_condition` command, so the registry's typo-proofing still applies at the one moment it matters.
        ComponentMeta::builder("PlayIf")
            .category("Gameplay")
            .field("source", Str, true)
            .field("join", Str, false)
            .requires("Spatial")
            .provides("PlayCondition")
            .tag("gameplay")
            .tag("logic")
            .alias("OnlyIf")
            .ui_hint("source", "the \"only if\" clauses this object adds to its role's rule")
            .ui_hint("join", "enum: all|any")
            .build(),
        // CINEMATICS. A cutscene is a RECIPE — an ordered list of shots (subject + size + angle + move
        // + seconds) — not baked keyframes, because the timeline can only author scalar channels (a
        // keyed rotation would lerp four quaternion components independently and take the wrong path)
        // and a baked pose frames where the subject WAS. The camera pose is solved per tick against the
        // live subject. Registering this component is also what lets an ordinary `SetField` rule start a
        // cutscene, so the closed action vocabulary never has to grow.
        ComponentMeta::builder("Cinematic")
            .category("Gameplay")
            .field("source", Str, true)
            .field("playing", Boolean, false)
            .field("seconds", Number, false)
            .requires("Spatial")
            .provides("Cinematic")
            .tag("gameplay")
            .tag("camera")
            .alias("Cutscene")
            .ui_hint("source", "the shots in this cutscene")
            .ui_hint("playing", "set true by a rule to roll the cutscene")
            .build(),
        // Visual effects. An effect is a RECIPE too, and for a sharper reason than the others: a
        // particle here is a pure function of (recipe, index, time), so nothing is stored per frame,
        // a replay is bit-identical, and the timeline can scrub to any second and get the right
        // picture. `playing` is flipped by an ordinary `SetField` rule, so — exactly as with
        // `Cinematic` — the closed action vocabulary gains fire, sparks and explosions without
        // gaining a verb.
        ComponentMeta::builder("Vfx")
            .category("Gameplay")
            .field("source", Str, true)
            .field("playing", Boolean, false)
            .field("intensity", Number, false)
            .requires("Spatial")
            .provides("Vfx")
            .tag("gameplay")
            .tag("vfx")
            .alias("Effect")
            .alias("Particles")
            .ui_hint("source", "the effect layers on this object")
            .ui_hint("playing", "set true by a rule to start the effect")
            .ui_hint("intensity", "1 is the authored strength; 2 is twice as much")
            .build(),
        // Shape Studio (Build sub-engine). A parametric or drawn shape is a RECIPE, not a mesh: the
        // document holds the kind + its parameters (+ the drawn outline) as canonical JSON, and the
        // watertight mesh is derived from it by a pure function in the shell. Editing a parameter
        // re-bakes the mesh and swaps the handle in one undoable commit; the persisted artifact for a
        // parametric shape is the recipe itself, so a missing blob rebuilds deterministically on load.
        ComponentMeta::builder("ShapeRecipe")
            .category("Props")
            .field("source", Str, true)
            .field("version", Integer, true)
            .field("kind", Str, true)
            .field("triangles", Integer, false)
            .requires("Spatial")
            .provides("ProceduralAsset")
            .tag("3d")
            .tag("procedural")
            .tag("shape")
            .alias("Shape")
            .alias("Solid")
            .alias("Primitive")
            .ui_hint(
                "kind",
                "enum: box|sphere|cylinder|cone|torus|capsule|wedge|prism|extrude|revolve|union|carve|intersect|meld",
            )
            .ui_hint("source", "editable shape recipe")
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
        // Fired AUTOMATICALLY by the Play loop when a moving physics body comes within a GameRole
        // entity's trigger radius — the first event the simulation emits by itself (the ADR-049
        // "live input routing" seam, now real for touch).
        EventMeta::new(
            "Touched",
            "a moving object touched this entity (fired live during Play)",
        ),
        // Fired AUTOMATICALLY by a companion's attack landing (in range + off cooldown) —
        // the second self-emitted Play event; the defeat consequence stays an authored rule.
        EventMeta::new(
            "Struck",
            "a companion's attack landed on this entity (fired live during Play)",
        ),
        // TIME. Fired AUTOMATICALLY by the Play clock when this entity's countdown elapses — the
        // first event that comes from nothing but the passage of time, which is what makes fuses,
        // crumbling platforms and timed gates expressible at all.
        EventMeta::new(
            "After",
            "this entity's countdown finished (fired live during Play)",
        ),
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
        // HEALTH. `Damage`/`Heal` are `AdjustCounter` with the two rules every game expects and no
        // author should have to re-derive: never below zero, never above `maxHp`. Making them their own
        // verbs is what lets a hazard say "hurt whoever stepped on me" in one clause.
        ActionMeta::new("Damage", "subtract from a health field, never below zero"),
        ActionMeta::new("Heal", "add to a health field, never above its maximum"),
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
