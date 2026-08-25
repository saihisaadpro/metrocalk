//! M19 (ADR-104) — the terrain authoring seam: recipe edits as undoable transactions, and derived
//! artefacts as content-addressed assets.
//!
//! This module is where [`metrocalk_terrain`] meets the document. It owns three responsibilities and
//! nothing else:
//!
//! 1. **Authoring.** Every terrain change — a slider, a brush gesture, a road, a preset swap — arrives as
//!    one [`TerrainEdit`] and lands as exactly **one** `engine.commit`, so it is one undo step and one CRDT
//!    transaction (invariant 3). The recipe itself is the only thing stored.
//! 2. **Landing derived geometry.** A built chunk becomes a `MeshAsset` in the content-addressed store, the
//!    same path a CSG carve or an SDF bake takes ([`crate::csg_intent`], [`crate::sdf_intent`]). Geometry
//!    never enters the Loro doc (invariant 2) — two identical flat chunks even dedup to one handle.
//! 3. **Adapting to the neighbours.** A terrain chunk collider becomes a `physics::ColliderShape::Heightfield`
//!    on a Fixed body; a scatter prototype gets a software-rasterized impostor billboard.
//!
//! ## Why the strokes live in their own field
//!
//! A recipe with a full layer stack, biomes and scatter rules is tens of kilobytes of JSON. If a brush dab
//! rewrote all of it, a sculpting session would push megabytes through the undo history for a few hundred
//! bytes of actual change. So the component keeps two fields: `source` (the recipe with an empty stroke
//! list) and `strokes` (a packed, quantized text encoding, about forty bytes per dab). A gesture writes only
//! `strokes`; a parameter change writes only `source`. Packing quantizes to millimetres, which also makes
//! the stored value canonical — a stroke round-trips through the document to exactly itself.

// Geometry and rasterization code: this module converts between document counts (usize), field values
// (f32) and texel coordinates constantly. Every conversion is deliberate and bounded by construction — the
// counts are collection lengths and the coordinates are clamped to a texture whose side is at most 1024.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)]

use std::collections::BTreeMap;

use metrocalk_assets::mesh::{Material, MeshAsset, Primitive, Texture};
use metrocalk_assets::{AssetId, AssetStore};
use metrocalk_core::pipeline::{FieldValue, Op, PipelineError};
use metrocalk_core::{Engine, EntityId};
use metrocalk_ecs::flecs_backend::FlecsWorld;
use metrocalk_interchange::{BodyDesc, BodyKind, ColliderDesc, ColliderShape};
use metrocalk_terrain::collision::ChunkCollider;
use metrocalk_terrain::material::ChunkTextures;
use metrocalk_terrain::mesh::{ChunkMesh, MeshData};
use metrocalk_terrain::operation::{self, Rect, TerrainOperation};
use metrocalk_terrain::proto::{self as proto_mod, ProtoKind};
use metrocalk_terrain::recipe::{
    BiomeRule, Layer, MaterialLayer, ScatterProto, ScatterRule, SplineDef, Stroke, StrokeKind,
    TerrainRecipe, WaterConfig,
};
use metrocalk_terrain::sculpt::{stroke_run, Brush};
use metrocalk_terrain::stage::{Stage, StageMask};
use metrocalk_terrain::{
    describe, preset, validate, HeightGrid, LodPolicy, MemoryBudget, Terrain, TerrainError,
};
use serde::{Deserialize, Serialize};

use crate::capscene::CapScene;

/// The component that carries a terrain.
pub const TERRAIN_COMPONENT: &str = "TerrainRecipe";
/// The field holding the recipe JSON (strokes excluded).
pub const SOURCE_FIELD: &str = "source";
/// The field holding the packed sculpt strokes.
pub const STROKES_FIELD: &str = "strokes";

/// Something the caller must see rather than get a silently wrong terrain for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerrainIntentError {
    /// The entity has no `TerrainRecipe` component.
    NotATerrain(String),
    /// The stored recipe could not be parsed — the string says why.
    Corrupt(String),
    /// The named preset does not exist.
    UnknownPreset(String),
    /// The edit refers to an index that does not exist (layer, biome, material, spline, rule).
    NoSuchIndex {
        /// What kind of thing was addressed.
        kind: &'static str,
        /// The index asked for.
        index: usize,
        /// How many exist.
        len: usize,
    },
    /// The edit would produce a recipe that cannot build; the string lists the blocking issues.
    Rejected(String),
    /// The commit pipeline refused the transaction.
    Pipeline(String),
}

impl std::fmt::Display for TerrainIntentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotATerrain(id) => write!(f, "entity {id} is not a terrain"),
            Self::Corrupt(why) => write!(f, "the stored terrain recipe is unreadable: {why}"),
            Self::UnknownPreset(id) => write!(f, "there is no terrain preset called '{id}'"),
            Self::NoSuchIndex { kind, index, len } => {
                write!(f, "{kind} {index} does not exist (there are {len})")
            }
            Self::Rejected(why) => write!(f, "that edit would break the terrain: {why}"),
            Self::Pipeline(why) => write!(f, "the terrain edit could not be committed: {why}"),
        }
    }
}

impl std::error::Error for TerrainIntentError {}

impl From<PipelineError> for TerrainIntentError {
    fn from(e: PipelineError) -> Self {
        Self::Pipeline(e.to_string())
    }
}

/// What an edit actually changed: where, and which derived stages it invalidated.
///
/// Returned by [`apply_to_recipe`] rather than guessed at afterwards. `region: None` means "the whole
/// world" — the honest answer for a seed change or a preset swap, and one that must never be produced by
/// default for an edit that really was local.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Effect {
    /// The world rectangle that changed, or `None` for the whole world.
    pub region: Option<Rect>,
    /// The derived stages to rebuild — already the transitive closure.
    pub stages: StageMask,
}

impl Effect {
    /// Everything, everywhere — for edits whose reach is genuinely global.
    #[must_use]
    pub fn world() -> Self {
        Self {
            region: None,
            stages: StageMask::all(),
        }
    }

    /// A bounded change.
    #[must_use]
    pub fn local(region: Rect, stages: StageMask) -> Self {
        Self {
            region: Some(region),
            stages,
        }
    }

    /// The rectangle as the runtime's `(min, max)` pair.
    #[must_use]
    pub fn bounds(&self) -> Option<([f32; 2], [f32; 2])> {
        self.region.map(|r| (r.min, r.max))
    }
}

/// One authoring operation. Every variant is one undo step.
///
/// Modelling edits as data rather than as a dozen commands is what keeps the Tauri surface to a single
/// command, keeps the UI honest (it can only ask for things that exist), and makes the whole authoring
/// surface testable headlessly without a window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum TerrainEdit {
    /// Replace the whole recipe with a preset, keeping the entity.
    ApplyPreset {
        /// Preset id from [`preset::all`].
        id: String,
    },
    /// Apply a list of resolved terrain operations — the LOCAL edit path (ADR-106).
    ///
    /// One `Apply` is one undo step however many operations it holds, because one sentence is one thing the
    /// author did. Each operation is resolved against the recipe *as it evolves*, so "widen this valley and
    /// make it traversable" both act on the same landform.
    Apply {
        /// The operations, in order.
        ops: Vec<TerrainOperation>,
    },
    /// Replace the whole recipe with the one a plain-language description asks for.
    ///
    /// One edit, therefore one undo step: re-describing a world is as reversible as nudging a slider. The
    /// result is an ordinary recipe — nothing about it is marked "generated", and every layer, material and
    /// rule it produced is editable in the panel afterwards.
    Describe {
        /// The description, as the author typed it.
        text: String,
    },
    /// Change the master seed — a different world with the same art direction.
    SetSeed {
        /// New seed.
        seed: u64,
    },
    /// Rename the terrain.
    Rename {
        /// New name.
        name: String,
    },
    /// Resize the world and/or its chunks.
    SetExtent {
        /// World edge length in metres.
        world_size_m: f32,
        /// Chunk edge length in metres.
        chunk_size_m: f32,
        /// Vertices per chunk edge (must be `2^k + 1`).
        chunk_verts: u32,
    },
    /// Append a layer.
    AddLayer {
        /// The layer to add.
        layer: Box<Layer>,
    },
    /// Replace a layer.
    SetLayer {
        /// Index into the stack.
        index: usize,
        /// The replacement.
        layer: Box<Layer>,
    },
    /// Remove a layer.
    RemoveLayer {
        /// Index into the stack.
        index: usize,
    },
    /// Enable or disable a layer without removing it.
    ToggleLayer {
        /// Index into the stack.
        index: usize,
        /// New enabled state.
        enabled: bool,
    },
    /// Move a layer up or down the stack (order is meaning here, so this is a real edit).
    MoveLayer {
        /// Index into the stack.
        index: usize,
        /// Positions to move by; negative moves towards the base.
        delta: i32,
    },
    /// Record a brush gesture as a run of dabs — one gesture, one undo step.
    Sculpt {
        /// Brush settings.
        brush: Brush,
        /// Gesture start in world XZ.
        from: [f32; 2],
        /// Gesture end in world XZ.
        to: [f32; 2],
    },
    /// Record a whole traced gesture — the viewport brush's commit.
    ///
    /// Carries the *path*, not the dabs, so the committed strokes are produced by the same `stroke_run` the
    /// live preview used: what you saw while painting is what lands in the document, exactly.
    SculptPath {
        /// Brush settings.
        brush: Brush,
        /// World XZ points the cursor traced, in order.
        path: Vec<[f32; 2]>,
    },
    /// Discard every recorded stroke.
    ClearStrokes,
    /// Undo the last recorded gesture's dabs (a convenience for a brush-heavy session; ordinary Ctrl-Z
    /// already covers it because a gesture is one commit).
    DropLastStrokes {
        /// How many dabs to drop.
        count: usize,
    },
    /// Add a road, river or pad.
    AddSpline {
        /// The spline to add.
        spline: Box<SplineDef>,
    },
    /// Replace a spline.
    SetSpline {
        /// Index into the spline list.
        index: usize,
        /// The replacement.
        spline: Box<SplineDef>,
    },
    /// Remove a spline.
    RemoveSpline {
        /// Index into the spline list.
        index: usize,
    },
    /// Add a ground material.
    AddMaterial {
        /// The material to add.
        material: Box<MaterialLayer>,
    },
    /// Replace a ground material.
    SetMaterial {
        /// Index into the material list.
        index: usize,
        /// The replacement.
        material: Box<MaterialLayer>,
    },
    /// Add a biome rule.
    AddBiome {
        /// The rule to add.
        biome: Box<BiomeRule>,
    },
    /// Replace a biome rule.
    SetBiome {
        /// Index into the biome list.
        index: usize,
        /// The replacement.
        biome: Box<BiomeRule>,
    },
    /// Remove a biome rule.
    RemoveBiome {
        /// Index into the biome list.
        index: usize,
    },
    /// Add a scatter prototype.
    AddProto {
        /// The prototype to add.
        proto: Box<ScatterProto>,
    },
    /// Bind (or rebind) a prototype's mesh asset — the step that brings a preset's scatter rules to life.
    BindProto {
        /// Index into the prototype list.
        index: usize,
        /// Asset handle for the full-detail mesh.
        mesh_key: String,
        /// Asset handles for coarser LODs, nearest first.
        lod_keys: Vec<String>,
        /// Asset handle for the impostor billboard.
        impostor_key: Option<String>,
    },
    /// Add a scatter rule.
    AddScatter {
        /// The rule to add.
        rule: Box<ScatterRule>,
    },
    /// Replace a scatter rule.
    SetScatter {
        /// Index into the rule list.
        index: usize,
        /// The replacement.
        rule: Box<ScatterRule>,
    },
    /// Remove a scatter rule.
    RemoveScatter {
        /// Index into the rule list.
        index: usize,
    },
    /// Replace the water settings.
    SetWater {
        /// The new settings.
        water: WaterConfig,
    },
    /// Replace the LOD/view policy.
    SetLod {
        /// The new policy.
        lod: LodPolicy,
    },
    /// Replace the memory ceilings.
    SetBudget {
        /// The new budget.
        budget: MemoryBudget,
    },
}

impl TerrainEdit {
    /// The undo label this edit commits under — what the user sees in the history.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::ApplyPreset { .. } => "terrain-preset",
            Self::Describe { .. } => "terrain-describe",
            Self::Apply { .. } => "terrain-edit",
            Self::SetSeed { .. } => "terrain-seed",
            Self::Rename { .. } => "terrain-rename",
            Self::SetExtent { .. } => "terrain-extent",
            Self::AddLayer { .. } => "terrain-add-layer",
            Self::SetLayer { .. } => "terrain-layer",
            Self::RemoveLayer { .. } => "terrain-remove-layer",
            Self::ToggleLayer { .. } => "terrain-toggle-layer",
            Self::MoveLayer { .. } => "terrain-move-layer",
            Self::Sculpt { .. } | Self::SculptPath { .. } => "terrain-sculpt",
            Self::ClearStrokes | Self::DropLastStrokes { .. } => "terrain-strokes",
            Self::AddSpline { .. } | Self::SetSpline { .. } | Self::RemoveSpline { .. } => {
                "terrain-spline"
            }
            Self::AddMaterial { .. } | Self::SetMaterial { .. } => "terrain-material",
            Self::AddBiome { .. } | Self::SetBiome { .. } | Self::RemoveBiome { .. } => {
                "terrain-biome"
            }
            Self::AddProto { .. } | Self::BindProto { .. } => "terrain-prototype",
            Self::AddScatter { .. } | Self::SetScatter { .. } | Self::RemoveScatter { .. } => {
                "terrain-scatter"
            }
            Self::SetWater { .. } => "terrain-water",
            Self::SetLod { .. } => "terrain-lod",
            Self::SetBudget { .. } => "terrain-budget",
        }
    }

    /// Whether this edit only touches the stroke list — used to write the smaller of the two fields.
    #[must_use]
    pub fn strokes_only(&self) -> bool {
        matches!(
            self,
            Self::Sculpt { .. }
                | Self::SculptPath { .. }
                | Self::ClearStrokes
                | Self::DropLastStrokes { .. }
        )
    }

    /// The world rectangle this edit can change, or `None` when it could change anything.
    ///
    /// The streaming runtime uses this to re-stream only what moved. A brush dab changes a few metres of
    /// ground; re-streaming a four-kilometre world for it would turn every paint stroke into a stall, and
    /// guessing the region would be worse than not trying — so anything whose reach is not obviously bounded
    /// answers `None` and pays for a full rebuild.
    #[must_use]
    pub fn dirty_region(&self) -> Option<([f32; 2], [f32; 2])> {
        match self {
            Self::Sculpt { brush, from, to } => {
                let r = brush.radius_m.max(0.01) + 1.0;
                Some((
                    [from[0].min(to[0]) - r, from[1].min(to[1]) - r],
                    [from[0].max(to[0]) + r, from[1].max(to[1]) + r],
                ))
            }
            Self::SculptPath { brush, path } => {
                if path.is_empty() {
                    return Some(([0.0, 0.0], [0.0, 0.0]));
                }
                let r = brush.radius_m.max(0.01) + 1.0;
                let mut min = [f32::MAX; 2];
                let mut max = [f32::MIN; 2];
                for p in path {
                    min[0] = min[0].min(p[0] - r);
                    min[1] = min[1].min(p[1] - r);
                    max[0] = max[0].max(p[0] + r);
                    max[1] = max[1].max(p[1] + r);
                }
                Some((min, max))
            }
            Self::AddSpline { spline } | Self::SetSpline { spline, .. } => {
                if spline.points.is_empty() {
                    return Some(([0.0, 0.0], [0.0, 0.0]));
                }
                let r = spline.width_m * 0.5 + spline.falloff_m + spline.clear_scatter_m + 2.0;
                let mut min = [f32::MAX; 2];
                let mut max = [f32::MIN; 2];
                for p in &spline.points {
                    min[0] = min[0].min(p[0] - r);
                    min[1] = min[1].min(p[2] - r);
                    max[0] = max[0].max(p[0] + r);
                    max[1] = max[1].max(p[2] + r);
                }
                Some((min, max))
            }
            // Everything else can move the whole field: a layer, a seed, a biome, a budget.
            _ => None,
        }
    }
}

/// Quantum for packed stroke coordinates: one millimetre. Finer than any brush can be aimed, and it makes
/// the stored value canonical so a stroke round-trips to exactly itself.
const STROKE_QUANTUM: f32 = 1000.0;

fn q(v: f32) -> f32 {
    (v * STROKE_QUANTUM).round() / STROKE_QUANTUM
}

fn kind_char(k: StrokeKind) -> char {
    match k {
        StrokeKind::Raise => 'r',
        StrokeKind::Smooth => 's',
        StrokeKind::Flatten => 'f',
        StrokeKind::Noise => 'n',
    }
}

fn char_kind(c: char) -> Option<StrokeKind> {
    match c {
        'r' => Some(StrokeKind::Raise),
        's' => Some(StrokeKind::Smooth),
        'f' => Some(StrokeKind::Flatten),
        'n' => Some(StrokeKind::Noise),
        _ => None,
    }
}

/// Pack strokes into the compact text form the document stores. Quantizes to millimetres.
#[must_use]
pub fn pack_strokes(strokes: &[Stroke]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(strokes.len() * 40);
    for s in strokes {
        if !out.is_empty() {
            out.push(';');
        }
        out.push(kind_char(s.kind));
        out.push(':');
        // `{}` on a quantized f32 is the shortest exact representation, so this round-trips.
        // `write!` into the buffer rather than allocating a `String` per dab: a long sculpting session packs
        // thousands of these on every gesture.
        let _ = write!(
            out,
            "{}:{}:{}:{}:{}:{}",
            q(s.x),
            q(s.z),
            q(s.radius_m),
            q(s.strength),
            q(s.hardness),
            q(s.target_m)
        );
    }
    out
}

/// Parse the packed form. Malformed entries are skipped rather than failing the whole load — a corrupt tail
/// should cost the last dab, not the terrain.
#[must_use]
pub fn unpack_strokes(packed: &str) -> Vec<Stroke> {
    let mut out = Vec::new();
    for entry in packed.split(';').filter(|e| !e.is_empty()) {
        let mut parts = entry.split(':');
        let Some(kind) = parts
            .next()
            .and_then(|k| k.chars().next())
            .and_then(char_kind)
        else {
            continue;
        };
        let nums: Vec<f32> = parts.filter_map(|p| p.parse::<f32>().ok()).collect();
        if nums.len() != 6 {
            continue;
        }
        out.push(Stroke {
            kind,
            x: nums[0],
            z: nums[1],
            radius_m: nums[2],
            strength: nums[3],
            hardness: nums[4],
            target_m: nums[5],
        });
    }
    out
}

/// Every terrain entity in the scene, in document order.
#[must_use]
pub fn terrains(engine: &Engine<FlecsWorld>) -> Vec<EntityId> {
    engine
        .entity_ids()
        .into_iter()
        .filter(|id| engine.components_of(*id).contains_key(TERRAIN_COMPONENT))
        .collect()
}

/// The scene's terrain, if it has exactly one — the common case, and what a command with no explicit target
/// should act on.
#[must_use]
pub fn only_terrain(engine: &Engine<FlecsWorld>) -> Option<EntityId> {
    let all = terrains(engine);
    if all.len() == 1 {
        all.into_iter().next()
    } else {
        None
    }
}

/// Read an entity's terrain recipe.
///
/// # Errors
/// [`TerrainIntentError::NotATerrain`] when the component is absent, [`TerrainIntentError::Corrupt`] when the
/// stored JSON cannot be parsed.
pub fn read(
    engine: &Engine<FlecsWorld>,
    id: EntityId,
) -> Result<TerrainRecipe, TerrainIntentError> {
    let comps = engine.components_of(id);
    let rec = comps
        .get(TERRAIN_COMPONENT)
        .ok_or_else(|| TerrainIntentError::NotATerrain(id.to_string()))?;
    let source = match rec.get(SOURCE_FIELD) {
        Some(FieldValue::Str(s)) => s.clone(),
        _ => return Err(TerrainIntentError::NotATerrain(id.to_string())),
    };
    let mut recipe: TerrainRecipe =
        serde_json::from_str(&source).map_err(|e| TerrainIntentError::Corrupt(e.to_string()))?;
    if let Some(FieldValue::Str(packed)) = rec.get(STROKES_FIELD) {
        recipe.strokes = unpack_strokes(packed);
    }
    Ok(recipe)
}

/// The ops that write a recipe into the document.
///
/// `strokes_only` writes just the packed stroke field, which is what makes a brush gesture cheap. The
/// summary fields are refreshed either way because they are what the inspector reads.
fn write_ops(
    id: EntityId,
    recipe: &TerrainRecipe,
    strokes_only: bool,
) -> Result<Vec<Op>, TerrainIntentError> {
    let mut ops = Vec::with_capacity(8);
    ops.push(Op::SetField {
        entity: id,
        component: TERRAIN_COMPONENT.into(),
        field: STROKES_FIELD.into(),
        value: FieldValue::Str(pack_strokes(&recipe.strokes)),
    });
    ops.push(Op::SetField {
        entity: id,
        component: TERRAIN_COMPONENT.into(),
        field: "strokeCount".into(),
        value: FieldValue::Integer(recipe.strokes.len() as i64),
    });
    if !strokes_only {
        // The stroke list is stored once, in its own field — not twice.
        let mut without = recipe.clone();
        without.strokes.clear();
        let json = serde_json::to_string(&without)
            .map_err(|e| TerrainIntentError::Corrupt(e.to_string()))?;
        ops.push(Op::SetField {
            entity: id,
            component: TERRAIN_COMPONENT.into(),
            field: SOURCE_FIELD.into(),
            value: FieldValue::Str(json),
        });
        for (field, value) in [
            ("seed", FieldValue::Integer(recipe.seed as i64)),
            (
                "worldSizeM",
                FieldValue::Number(f64::from(recipe.world_size_m)),
            ),
            (
                "chunkSizeM",
                FieldValue::Number(f64::from(recipe.chunk_size_m)),
            ),
            (
                "layerCount",
                FieldValue::Integer(recipe.layers.len() as i64),
            ),
        ] {
            ops.push(Op::SetField {
                entity: id,
                component: TERRAIN_COMPONENT.into(),
                field: field.into(),
                value,
            });
        }
    }
    Ok(ops)
}

/// Create a terrain entity from a preset, as one undoable transaction.
///
/// # Errors
/// [`TerrainIntentError::UnknownPreset`] for an unknown id; propagates a pipeline failure.
pub fn create_from_preset(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    preset_id: &str,
    position: [f32; 3],
) -> Result<(EntityId, TerrainRecipe), TerrainIntentError> {
    let recipe = preset::by_id(preset_id)
        .ok_or_else(|| TerrainIntentError::UnknownPreset(preset_id.to_string()))?;
    create_with(engine, scene, recipe, Some(preset_id), position)
}

/// Create a terrain entity from a plain-language description, as one undoable transaction.
///
/// Returns the reading alongside the recipe so the caller can show the author **what was understood and what
/// was ignored**. A generator that quietly drops half the sentence is the thing that makes these features
/// feel unreliable; saying so costs one field.
///
/// # Errors
/// Propagates a pipeline failure. A description is never itself an error — an unreadable one produces a
/// usable default world plus a reading that says nothing was understood, because failing here would make
/// "describe your terrain" something the author can get wrong.
pub fn create_from_description(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    text: &str,
    position: [f32; 3],
) -> Result<(EntityId, TerrainRecipe, describe::Reading), TerrainIntentError> {
    let (recipe, reading) = describe::compile(text);
    // No `preset` marker: this terrain came from a sentence, and the sentence travels *inside* the recipe
    // (`TerrainRecipe::description`) so it round-trips through the document like any other authored value.
    let (id, recipe) = create_with(engine, scene, recipe, None, position)?;
    Ok((id, recipe, reading))
}

/// The shared creation path: place the entity, write the recipe, record which preset it came from (if any),
/// and pair the caps.
fn create_with(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    recipe: TerrainRecipe,
    preset_id: Option<&str>,
    position: [f32; 3],
) -> Result<(EntityId, TerrainRecipe), TerrainIntentError> {
    let id = engine.alloc_entity_id();
    // A terrain covers `[0, world_size]`, so an entity dropped at the caller's origin sits at the corner of
    // the land it describes — and framing it would fly the camera to the edge of the map. Centre it instead;
    // the chunks take their positions from the recipe, so this transform is purely where the *entity* is.
    let centre = recipe.world_size_m * 0.5;
    let position = [
        if position[0] == 0.0 {
            centre
        } else {
            position[0]
        },
        position[1],
        if position[2] == 0.0 {
            centre
        } else {
            position[2]
        },
    ];
    let mut ops = vec![Op::CreateEntity { id, parent: None }];
    for (f, v) in [("x", position[0]), ("y", position[1]), ("z", position[2])] {
        ops.push(Op::SetField {
            entity: id,
            component: "Transform".into(),
            field: f.into(),
            value: FieldValue::Number(f64::from(v)),
        });
    }
    ops.extend(write_ops(id, &recipe, false)?);
    if let Some(preset_id) = preset_id {
        ops.push(Op::SetField {
            entity: id,
            component: TERRAIN_COMPONENT.into(),
            field: "preset".into(),
            value: FieldValue::Str(preset_id.to_string()),
        });
    }
    // The capability pairs, so the terrain participates in binding-by-intent like any other entity.
    for cap in ["Terrain", "ProceduralAsset"] {
        if let Some(&c) = scene.caps.get(&metrocalk_core::caps::canonical(cap)) {
            ops.push(Op::AddPair {
                entity: id,
                rel: scene.rels.provides,
                target: c,
            });
        }
    }
    engine.commit("terrain-create", ops)?;
    Ok((id, recipe))
}

/// Apply one edit to a terrain, as one undoable transaction, and return the recipe as the document now holds
/// it (so a caller always sees the quantized, canonical values rather than what it asked for).
///
/// Validation runs **before** the commit: an edit that would make the terrain unbuildable is rejected with
/// its reasons rather than committed and then found to be broken.
///
/// # Errors
/// Any [`TerrainIntentError`]; the recipe is left untouched on every failure path.
pub fn apply_edit(
    engine: &mut Engine<FlecsWorld>,
    id: EntityId,
    edit: &TerrainEdit,
) -> Result<(TerrainRecipe, Effect), TerrainIntentError> {
    let mut recipe = read(engine, id)?;
    // The edit itself reports what it touched, so no caller has to re-derive it — and a bounded edit can no
    // longer silently fall back to a whole-world rebuild because the information was not carried across.
    let effect = apply_to_recipe(&mut recipe, edit)?;
    let report = validate::validate(&recipe);
    if report.has_blocking() {
        return Err(TerrainIntentError::Rejected(report.summary()));
    }
    let ops = write_ops(id, &recipe, edit.strokes_only())?;
    engine.commit(edit.label(), ops)?;
    Ok((read(engine, id)?, effect))
}

/// Mutate a recipe in memory. Separated from the commit so it is testable without an engine, and so a UI can
/// preview an edit before committing it.
///
/// # Errors
/// [`TerrainIntentError::UnknownPreset`] or [`TerrainIntentError::NoSuchIndex`].
#[allow(clippy::too_many_lines)] // a flat dispatch over an edit vocabulary; splitting it would only scatter it
pub fn apply_to_recipe(
    recipe: &mut TerrainRecipe,
    edit: &TerrainEdit,
) -> Result<Effect, TerrainIntentError> {
    /// Bounds-check an index against a collection length.
    fn bound(kind: &'static str, index: usize, len: usize) -> Result<(), TerrainIntentError> {
        if index < len {
            Ok(())
        } else {
            Err(TerrainIntentError::NoSuchIndex { kind, index, len })
        }
    }

    // Captured BEFORE the edit runs: the ground it is about to stop affecting.
    let prior = prior_bounds(recipe, edit);
    let mut applied_effect: Option<Effect> = None;

    match edit {
        TerrainEdit::ApplyPreset { id } => {
            let next =
                preset::by_id(id).ok_or_else(|| TerrainIntentError::UnknownPreset(id.clone()))?;
            *recipe = next;
        }
        TerrainEdit::Describe { text } => {
            let (next, _) = describe::compile(text);
            *recipe = next;
        }
        TerrainEdit::SetSeed { seed } => recipe.seed = *seed,
        TerrainEdit::Rename { name } => recipe.name.clone_from(name),
        TerrainEdit::SetExtent {
            world_size_m,
            chunk_size_m,
            chunk_verts,
        } => {
            recipe.world_size_m = *world_size_m;
            recipe.chunk_size_m = *chunk_size_m;
            recipe.chunk_verts = *chunk_verts;
        }
        TerrainEdit::AddLayer { layer } => recipe.layers.push((**layer).clone()),
        TerrainEdit::SetLayer { index, layer } => {
            bound("layer", *index, recipe.layers.len())?;
            recipe.layers[*index] = (**layer).clone();
        }
        TerrainEdit::RemoveLayer { index } => {
            bound("layer", *index, recipe.layers.len())?;
            recipe.layers.remove(*index);
        }
        TerrainEdit::ToggleLayer { index, enabled } => {
            bound("layer", *index, recipe.layers.len())?;
            recipe.layers[*index].enabled = *enabled;
        }
        TerrainEdit::MoveLayer { index, delta } => {
            bound("layer", *index, recipe.layers.len())?;
            let len = recipe.layers.len() as i64;
            let target = (*index as i64 + i64::from(*delta)).clamp(0, len - 1) as usize;
            let layer = recipe.layers.remove(*index);
            recipe.layers.insert(target, layer);
        }
        TerrainEdit::Sculpt { brush, from, to } => {
            recipe.strokes.extend(stroke_run(brush, *from, *to));
        }
        TerrainEdit::SculptPath { brush, path } => {
            if path.len() == 1 {
                recipe.strokes.push(brush.dab(path[0][0], path[0][1]));
            } else {
                for w in path.windows(2) {
                    recipe.strokes.extend(stroke_run(brush, w[0], w[1]));
                }
            }
        }
        TerrainEdit::ClearStrokes => recipe.strokes.clear(),
        TerrainEdit::DropLastStrokes { count } => {
            let keep = recipe.strokes.len().saturating_sub(*count);
            recipe.strokes.truncate(keep);
        }
        TerrainEdit::AddSpline { spline } => recipe.splines.push((**spline).clone()),
        TerrainEdit::SetSpline { index, spline } => {
            bound("spline", *index, recipe.splines.len())?;
            recipe.splines[*index] = (**spline).clone();
        }
        TerrainEdit::RemoveSpline { index } => {
            bound("spline", *index, recipe.splines.len())?;
            recipe.splines.remove(*index);
        }
        TerrainEdit::AddMaterial { material } => recipe.materials.push((**material).clone()),
        TerrainEdit::SetMaterial { index, material } => {
            bound("material", *index, recipe.materials.len())?;
            recipe.materials[*index] = (**material).clone();
        }
        TerrainEdit::AddBiome { biome } => recipe.biomes.push((**biome).clone()),
        TerrainEdit::SetBiome { index, biome } => {
            bound("biome", *index, recipe.biomes.len())?;
            recipe.biomes[*index] = (**biome).clone();
        }
        TerrainEdit::RemoveBiome { index } => {
            bound("biome", *index, recipe.biomes.len())?;
            recipe.biomes.remove(*index);
            // Every scatter rule names the biomes it applies to BY POSITION, and a `Vec::remove` shifts
            // every position above it down by one. Without this, deleting "Shore" silently re-points the
            // woodland at what used to be the uplands and the boulders at nothing — a world that is wrong
            // in a way no error reports and no validator catches, because every index is still in range.
            for rule in &mut recipe.scatter {
                if rule.biomes.is_empty() {
                    // Already means "every biome"; there is no position to shift.
                    continue;
                }
                rule.biomes.retain(|b| *b != *index);
                for b in &mut rule.biomes {
                    if *b > *index {
                        *b -= 1;
                    }
                }
                if rule.biomes.is_empty() {
                    // The rule named ONLY the biome that just went. An empty list means "every biome", so
                    // leaving it empty would promote a niche rule to carpeting the whole world — a louder
                    // wrong than the one being fixed. Disable it instead, where the author can see it.
                    rule.enabled = false;
                }
            }
        }
        TerrainEdit::AddProto { proto } => recipe.protos.push((**proto).clone()),
        TerrainEdit::BindProto {
            index,
            mesh_key,
            lod_keys,
            impostor_key,
        } => {
            bound("prototype", *index, recipe.protos.len())?;
            let p = &mut recipe.protos[*index];
            p.mesh_key.clone_from(mesh_key);
            p.lod_keys.clone_from(lod_keys);
            p.impostor_key.clone_from(impostor_key);
        }
        TerrainEdit::AddScatter { rule } => recipe.scatter.push((**rule).clone()),
        TerrainEdit::SetScatter { index, rule } => {
            bound("scatter rule", *index, recipe.scatter.len())?;
            recipe.scatter[*index] = (**rule).clone();
        }
        TerrainEdit::RemoveScatter { index } => {
            bound("scatter rule", *index, recipe.scatter.len())?;
            recipe.scatter.remove(*index);
        }
        TerrainEdit::SetWater { water } => recipe.water = *water,
        TerrainEdit::SetLod { lod } => recipe.lod = *lod,
        TerrainEdit::SetBudget { budget } => recipe.budget = *budget,
        TerrainEdit::Apply { ops } => {
            // Resolved against the recipe AS IT EVOLVES, so a second operation in the same sentence sees
            // what the first one did — "widen this valley and make it traversable" acts on one landform.
            let mut region: Option<Rect> = None;
            let mut stages = StageMask::none();
            for op in ops {
                let resolved = operation::resolve(op, recipe)
                    .map_err(|r| TerrainIntentError::Rejected(refusal_text(&r)))?;
                let applied = operation::apply(&resolved, recipe)
                    .map_err(|r| TerrainIntentError::Rejected(refusal_text(&r)))?;
                region = Some(region.map_or(applied.region, |r| r.union(applied.region)));
                stages = stages.union(applied.stages);
            }
            applied_effect = Some(match region {
                Some(r) => Effect::local(r, stages),
                // No operations at all is a no-op, not a world rebuild.
                None => Effect {
                    region: Some(Rect::new([0.0, 0.0], [0.0, 0.0])),
                    stages: StageMask::none(),
                },
            });
        }
    }

    // An operation list already reported exactly what it touched. Everything else is classified here, with
    // the PRIOR bounds unioned in — moving or shrinking a road must rebuild the ground it vacated, which is
    // the bug that left old corridors carved into the world.
    let mut effect = applied_effect.unwrap_or_else(|| effect_of(edit, recipe));
    if let (Some(prior), Some(now)) = (prior, effect.region) {
        effect.region = Some(now.union(prior));
    }
    Ok(effect)
}

/// A refusal rendered as the one-line reason the author sees.
fn refusal_text(r: &operation::Refusal) -> String {
    match &r.suggestion {
        Some(s) => format!("{} — {s}", r.reason),
        None => r.reason.clone(),
    }
}

/// The world rectangle an edit is about to *stop* affecting, captured before it runs.
///
/// Without this a narrowed feature, a moved road or a removed river leaves its old shape baked into the
/// resident chunks: the new footprint is rebuilt, the vacated ground is not.
fn prior_bounds(recipe: &TerrainRecipe, edit: &TerrainEdit) -> Option<Rect> {
    let spline_rect = |s: &SplineDef| {
        let pad = s.width_m * 0.5 + s.falloff_m + s.clear_scatter_m + 4.0;
        let mut r: Option<Rect> = None;
        for p in &s.points {
            let b = Rect::around([p[0], p[2]], pad);
            r = Some(r.map_or(b, |x: Rect| x.union(b)));
        }
        r
    };
    match edit {
        TerrainEdit::SetSpline { index, .. } | TerrainEdit::RemoveSpline { index } => {
            recipe.splines.get(*index).and_then(spline_rect)
        }
        TerrainEdit::Apply { ops } => {
            // Whatever the operations are about to touch, as it stands now.
            let mut acc: Option<Rect> = None;
            for op in ops {
                if let Ok(res) = operation::resolve(op, recipe) {
                    acc = Some(acc.map_or(res.region, |a: Rect| a.union(res.region)));
                }
            }
            acc
        }
        _ => None,
    }
}

/// Which region and which derived stages an edit reaches.
fn effect_of(edit: &TerrainEdit, recipe: &TerrainRecipe) -> Effect {
    let all = StageMask::all();
    match edit {
        // Naming something derives nothing.
        TerrainEdit::Rename { .. } => Effect {
            region: Some(Rect::new([0.0, 0.0], [0.0, 0.0])),
            stages: StageMask::none(),
        },
        TerrainEdit::Sculpt { brush, from, to } => {
            let r = brush.radius_m.max(0.01) + 1.0;
            Effect::local(
                Rect::new([from[0] - r, from[1] - r], [to[0] + r, to[1] + r]).union(Rect::new(
                    [to[0] - r, to[1] - r],
                    [from[0] + r, from[1] + r],
                )),
                StageMask::of(&[Stage::Elevation]).closure(),
            )
        }
        TerrainEdit::SculptPath { brush, path } => {
            let r = brush.radius_m.max(0.01) + 1.0;
            let mut acc: Option<Rect> = None;
            for p in path {
                let b = Rect::around(*p, r);
                acc = Some(acc.map_or(b, |a: Rect| a.union(b)));
            }
            acc.map_or_else(Effect::world, |b| {
                Effect::local(b, StageMask::of(&[Stage::Elevation]).closure())
            })
        }
        TerrainEdit::AddSpline { spline } | TerrainEdit::SetSpline { spline, .. } => {
            let pad = spline.width_m * 0.5 + spline.falloff_m + spline.clear_scatter_m + 4.0;
            let mut acc: Option<Rect> = None;
            for p in &spline.points {
                let b = Rect::around([p[0], p[2]], pad);
                acc = Some(acc.map_or(b, |a: Rect| a.union(b)));
            }
            acc.map_or_else(Effect::world, |b| {
                Effect::local(b, StageMask::of(&[Stage::Elevation]).closure())
            })
        }
        // Water moves shores and wading, not the ground.
        TerrainEdit::SetWater { .. } => Effect {
            region: None,
            stages: StageMask::of(&[Stage::Hydrology]).closure(),
        },
        // Look-only edits: repaint, do not re-mesh.
        TerrainEdit::AddMaterial { .. } | TerrainEdit::SetMaterial { .. } => Effect {
            region: None,
            stages: StageMask::of(&[Stage::Materials]).closure(),
        },
        TerrainEdit::AddBiome { .. }
        | TerrainEdit::SetBiome { .. }
        | TerrainEdit::RemoveBiome { .. } => Effect {
            region: None,
            stages: StageMask::of(&[Stage::Biomes]).closure(),
        },
        TerrainEdit::AddScatter { .. }
        | TerrainEdit::SetScatter { .. }
        | TerrainEdit::RemoveScatter { .. }
        | TerrainEdit::AddProto { .. }
        | TerrainEdit::BindProto { .. } => Effect {
            region: None,
            stages: StageMask::of(&[Stage::Vegetation]).closure(),
        },
        // Everything else can move the whole field: a layer, a seed, an extent, a preset, a description.
        _ => {
            let _ = recipe;
            Effect {
                region: None,
                stages: all,
            }
        }
    }
}

/// Compile a terrain for rendering/simulation, reusing any cached erosion bakes.
///
/// # Errors
/// Propagates [`TerrainError`] — an unbuildable recipe or a missing imported heightmap.
pub fn compile(
    recipe: TerrainRecipe,
    inputs: BTreeMap<String, HeightGrid>,
    cache: &BTreeMap<String, HeightGrid>,
) -> Result<Terrain, TerrainError> {
    Terrain::compile_with_bakes(recipe, inputs, cache)
}

/// Turn a built chunk into a `MeshAsset` with its baked textures attached.
///
/// The result goes through the ordinary asset store and the ordinary instanced mesh draw — a terrain chunk is
/// not a special case anywhere downstream of here.
#[must_use]
pub fn chunk_mesh_asset(
    name: &str,
    mesh: &ChunkMesh,
    textures: Option<&ChunkTextures>,
) -> MeshAsset {
    let (roughness, metallic) = textures.map_or((0.88, 0.0), |t| (t.roughness, t.metallic));
    let mut asset = MeshAsset {
        name: name.to_string(),
        primitives: vec![Primitive {
            positions: mesh.data.positions.clone(),
            normals: mesh.data.normals.clone(),
            uvs: mesh.data.uvs.clone(),
            tangents: Vec::new(),
            indices: mesh.data.indices.clone(),
            material: 0,
            joints: Vec::new(),
            weights: Vec::new(),
        }],
        materials: vec![Material {
            // White base colour so the baked splat texture is not tinted twice.
            base_color: [1.0, 1.0, 1.0, 1.0],
            metallic,
            roughness,
            base_color_texture: None,
            metallic_roughness_texture: None,
            normal_texture: None,
            occlusion_texture: None,
            curvature_texture: None,
        }],
        textures: Vec::new(),
        skeleton: None,
    };
    if let Some(t) = textures {
        asset.textures.push(Texture {
            width: t.res,
            height: t.res,
            rgba8: t.albedo_rgba8.clone(),
        });
        asset.materials[0].base_color_texture = Some(0);
        if let Some(n) = &t.normal_rgba8 {
            asset.textures.push(Texture {
                width: t.res,
                height: t.res,
                rgba8: n.clone(),
            });
            asset.materials[0].normal_texture = Some(1);
        }
    } else {
        // Untextured chunks would otherwise render pure white; without a bake there is nothing to sample, so
        // fall back to a neutral ground colour rather than a blown-out surface.
        asset.materials[0].base_color = [0.32, 0.34, 0.26, 1.0];
    }
    asset
}

/// Deterministic content bytes for a chunk asset — geometry plus texture bytes, so a re-bake at a different
/// resolution is a different asset and an identical chunk dedups.
fn chunk_content_bytes(mesh: &ChunkMesh, textures: Option<&ChunkTextures>) -> Vec<u8> {
    let d = &mesh.data;
    let mut b = Vec::with_capacity(d.positions.len() * 12 + d.indices.len() * 4 + 32);
    b.extend_from_slice(&(d.positions.len() as u64).to_le_bytes());
    b.extend_from_slice(&(d.indices.len() as u64).to_le_bytes());
    b.push(mesh.lod);
    for p in &d.positions {
        for c in p {
            b.extend_from_slice(&c.to_le_bytes());
        }
    }
    for i in &d.indices {
        b.extend_from_slice(&i.to_le_bytes());
    }
    if let Some(t) = textures {
        b.extend_from_slice(&t.res.to_le_bytes());
        b.extend_from_slice(&t.albedo_rgba8);
        if let Some(n) = &t.normal_rgba8 {
            b.extend_from_slice(n);
        }
    }
    b
}

/// Store a built chunk in the content-addressed asset store and return its handle.
#[must_use]
pub fn store_chunk(
    store: &mut AssetStore,
    mesh: &ChunkMesh,
    textures: Option<&ChunkTextures>,
) -> String {
    let name = format!(
        "terrain-chunk-{}-{}-lod{}",
        mesh.coord.x, mesh.coord.z, mesh.lod
    );
    let asset = chunk_mesh_asset(&name, mesh, textures);
    let id = AssetId::of_bytes(&chunk_content_bytes(mesh, textures));
    let handle = id.as_str().to_string();
    store.insert(id, asset);
    handle
}

/// Store a plain [`MeshData`] (a road ribbon, a water surface, an impostor) with a flat material.
#[must_use]
pub fn store_mesh_data(
    store: &mut AssetStore,
    data: &MeshData,
    name: &str,
    albedo: [f32; 4],
    roughness: f32,
    texture: Option<Texture>,
) -> String {
    let mut asset = MeshAsset {
        name: name.to_string(),
        primitives: vec![Primitive {
            positions: data.positions.clone(),
            normals: data.normals.clone(),
            uvs: data.uvs.clone(),
            tangents: Vec::new(),
            indices: data.indices.clone(),
            material: 0,
            joints: Vec::new(),
            weights: Vec::new(),
        }],
        materials: vec![Material {
            base_color: albedo,
            metallic: 0.0,
            roughness,
            base_color_texture: None,
            metallic_roughness_texture: None,
            normal_texture: None,
            occlusion_texture: None,
            curvature_texture: None,
        }],
        textures: Vec::new(),
        skeleton: None,
    };
    let mut bytes = Vec::with_capacity(data.positions.len() * 12 + 16);
    bytes.extend_from_slice(name.as_bytes());
    for p in &data.positions {
        for c in p {
            bytes.extend_from_slice(&c.to_le_bytes());
        }
    }
    for i in &data.indices {
        bytes.extend_from_slice(&i.to_le_bytes());
    }
    if let Some(t) = texture {
        bytes.extend_from_slice(&t.rgba8);
        asset.textures.push(t);
        asset.materials[0].base_color_texture = Some(0);
        asset.materials[0].base_color = [1.0, 1.0, 1.0, 1.0];
    }
    let id = AssetId::of_bytes(&bytes);
    let handle = id.as_str().to_string();
    store.insert(id, asset);
    handle
}

/// A generated stand-in ready to be registered with the host's asset runtime.
#[derive(Clone, Debug, PartialEq)]
pub struct InstalledProto {
    /// Which prototype in the recipe this belongs to.
    pub proto_index: usize,
    /// Content-addressed handle of the full-detail mesh.
    pub mesh_key: String,
    /// Handles of the coarser LODs, nearest first.
    pub lod_keys: Vec<String>,
    /// Handle of the impostor billboard.
    pub impostor_key: String,
    /// Every `(handle, asset)` pair the caller must register — full mesh, LODs and impostor.
    pub assets: Vec<(String, MeshAsset)>,
}

/// Match a recipe prototype to the stand-in that fits its name, so a preset's "Conifer" gets a conifer.
fn stand_in_for(name: &str) -> ProtoKind {
    let n = name.to_ascii_lowercase();
    if n.contains("conifer") || n.contains("pine") || n.contains("spruce") {
        ProtoKind::Conifer
    } else if n.contains("palm") {
        ProtoKind::Palm
    } else if n.contains("grass") || n.contains("tuft") || n.contains("clutter") {
        ProtoKind::GrassTuft
    } else if n.contains("boulder")
        || n.contains("rock")
        || n.contains("slab")
        || n.contains("stone")
    {
        ProtoKind::Boulder
    } else if n.contains("desert") || n.contains("scrub") {
        ProtoKind::DesertShrub
    } else if n.contains("shrub") || n.contains("brush") || n.contains("bush") {
        ProtoKind::Shrub
    } else {
        ProtoKind::BroadleafTree
    }
}

/// Generate, store and bind stand-in meshes for every unbound prototype in `recipe`.
///
/// A preset that declares a woodland and then places nothing — because the project happens to have no tree
/// asset — teaches the author that the feature is broken. Generating a stand-in makes a new terrain arrive
/// populated, and because the recipe references the result by handle, replacing it with real art later is a
/// one-field edit rather than a migration.
///
/// Each prototype gets a full mesh, two decimated LODs and a billboard-cloud impostor whose texture is
/// software-rasterized from the full mesh: the whole distance chain, generated.
pub fn install_stand_in_protos(
    store: &mut AssetStore,
    recipe: &mut TerrainRecipe,
) -> Vec<InstalledProto> {
    let mut out = Vec::new();
    for (i, proto) in recipe.protos.iter_mut().enumerate() {
        if !proto.mesh_key.is_empty() {
            continue;
        }
        let (installed, radius_m, height_m) = generate_stand_in(store, i, &proto.name);
        proto.mesh_key.clone_from(&installed.mesh_key);
        proto.lod_keys.clone_from(&installed.lod_keys);
        proto.impostor_key = Some(installed.impostor_key.clone());
        proto.radius_m = radius_m;
        proto.height_m = height_m;
        out.push(installed);
    }
    out
}

/// Regenerate the stand-in chain for **every** prototype, without touching the recipe.
///
/// This is the reopen path. The generated meshes live only in the host's in-memory `AssetStore`, and the
/// `.mtk` file holds the Loro document alone — so a saved project reopens with prototypes still bound to
/// handles that now resolve to nothing, and the world comes back bald with no error anywhere. Persisting
/// the bytes would work, but it is unnecessary: `proto::build` is a pure function of the prototype's own
/// name and fixed constants, and every handle is a content address over the bytes it produced, so running
/// generation again yields **byte-identical handles**. Regeneration is therefore a faithful restore rather
/// than an approximation of one.
///
/// Deliberately does not mutate `recipe`: the keys are already correct in the document, and writing them
/// again would dirty a project the author has only just opened.
#[must_use]
pub fn regenerate_stand_in_protos(
    store: &mut AssetStore,
    recipe: &TerrainRecipe,
) -> Vec<InstalledProto> {
    recipe
        .protos
        .iter()
        .enumerate()
        .map(|(i, proto)| generate_stand_in(store, i, &proto.name).0)
        .collect()
}

/// The whole distance chain for one prototype: full mesh, two LODs and a billboard-cloud impostor.
///
/// Returns the handles alongside the built radius and height, so the binding path can write those into the
/// recipe and the restore path can ignore them.
fn generate_stand_in(
    store: &mut AssetStore,
    index: usize,
    name: &str,
) -> (InstalledProto, f32, f32) {
    let kind = stand_in_for(name);
    let built = proto_mod::build(kind);
    let mut assets = Vec::new();

    let full = proto_asset(&built.data, &built, kind.id(), "full", true);
    let mesh_key = store_named(store, kind.id(), "full", &full);
    assets.push((mesh_key.clone(), full.clone()));

    // Two LODs, each roughly halving the silhouette's detail. `decimate` refuses to return an empty
    // mesh, so a sparse prototype degrades rather than disappearing.
    let mut lod_keys = Vec::new();
    for (n, frac) in [(1u32, 0.22f32), (2, 0.45)] {
        let coarse = proto_mod::decimate(&built.data, built.radius_m.max(0.05) * frac);
        let asset = proto_asset(&coarse, &built, kind.id(), "lod", false);
        let key = store_named(store, kind.id(), &format!("lod{n}"), &asset);
        lod_keys.push(key.clone());
        assets.push((key, asset));
    }

    // The impostor: intersecting quads carrying a texture baked from the full mesh's own silhouette.
    let cross =
        metrocalk_terrain::scatter::impostor_cross_mesh(built.radius_m, built.height_m, 3, true);
    let texture = bake_impostor_texture(&full, 96);
    let mut impostor = mesh_data_asset(&cross, [1.0, 1.0, 1.0, 1.0], 0.9);
    impostor.name = format!("terrain-proto-{}-impostor", kind.id());
    impostor.textures.push(texture);
    impostor.materials[0].base_color_texture = Some(0);
    let impostor_key = store_named(store, kind.id(), "impostor", &impostor);
    assets.push((impostor_key.clone(), impostor));

    (
        InstalledProto {
            proto_index: index,
            mesh_key,
            lod_keys,
            impostor_key,
            assets,
        },
        built.radius_m,
        built.height_m,
    )
}

/// Turn generated geometry into an asset: dark stem below, foliage above.
///
/// Two materials rather than one because that split is what stops a generated tree reading as a single
/// green lump, and the renderer already draws one submesh per material so it costs nothing extra. A
/// decimated copy has its own vertex numbering, so `split` only applies to the full mesh.
fn proto_asset(
    data: &MeshData,
    built: &proto_mod::ProtoMesh,
    id: &str,
    suffix: &str,
    split_materials: bool,
) -> MeshAsset {
    let split = built.canopy_start as usize;
    let two = split_materials && split > 0 && split < data.positions.len();
    let mut primitives = Vec::new();
    let mut materials = Vec::new();
    if two {
        let mut stem = Vec::new();
        let mut canopy = Vec::new();
        for t in data.indices.as_chunks::<3>().0 {
            let dst = if (t[0] as usize) < split {
                &mut stem
            } else {
                &mut canopy
            };
            dst.extend_from_slice(t);
        }
        primitives.push(primitive_from(data, stem, 0));
        primitives.push(primitive_from(data, canopy, 1));
        materials.push(flat_material(rgba(built.stem_color), 0.92));
        materials.push(flat_material(rgba(built.canopy_color), 0.85));
    } else {
        primitives.push(primitive_from(data, data.indices.clone(), 0));
        materials.push(flat_material(rgba(built.canopy_color), 0.88));
    }
    MeshAsset {
        name: format!("terrain-proto-{id}-{suffix}"),
        primitives,
        materials,
        textures: Vec::new(),
        skeleton: None,
    }
}

fn rgba(c: [f32; 3]) -> [f32; 4] {
    [c[0], c[1], c[2], 1.0]
}

fn flat_material(base_color: [f32; 4], roughness: f32) -> Material {
    Material {
        base_color,
        metallic: 0.0,
        roughness,
        base_color_texture: None,
        metallic_roughness_texture: None,
        normal_texture: None,
        occlusion_texture: None,
        curvature_texture: None,
    }
}

fn primitive_from(data: &MeshData, indices: Vec<u32>, material: usize) -> Primitive {
    Primitive {
        positions: data.positions.clone(),
        normals: data.normals.clone(),
        uvs: data.uvs.clone(),
        tangents: Vec::new(),
        indices,
        material,
        joints: Vec::new(),
        weights: Vec::new(),
    }
}

/// A plain single-material asset from raw geometry.
fn mesh_data_asset(data: &MeshData, base_color: [f32; 4], roughness: f32) -> MeshAsset {
    MeshAsset {
        name: "terrain-mesh".into(),
        primitives: vec![primitive_from(data, data.indices.clone(), 0)],
        materials: vec![flat_material(base_color, roughness)],
        textures: Vec::new(),
        skeleton: None,
    }
}

/// Store an asset under a content address derived from its identity and geometry.
fn store_named(store: &mut AssetStore, id: &str, suffix: &str, asset: &MeshAsset) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(id.as_bytes());
    bytes.extend_from_slice(suffix.as_bytes());
    for prim in &asset.primitives {
        for p in &prim.positions {
            for c in p {
                bytes.extend_from_slice(&c.to_le_bytes());
            }
        }
        for i in &prim.indices {
            bytes.extend_from_slice(&i.to_le_bytes());
        }
    }
    for t in &asset.textures {
        bytes.extend_from_slice(&t.rgba8);
    }
    let aid = AssetId::of_bytes(&bytes);
    let handle = aid.as_str().to_string();
    store.insert(aid, asset.clone());
    handle
}

/// The physics body and collider for a terrain chunk.
///
/// Fixed body at the chunk centre with `y = 0`, because the height field carries absolute world heights —
/// attaching it at the chunk's mean height would add that height twice.
#[must_use]
pub fn chunk_body_and_collider(collider: &ChunkCollider) -> (BodyDesc, ColliderDesc) {
    let t = collider.body_translation();
    let body = BodyDesc::new(
        BodyKind::Fixed,
        [f64::from(t[0]), f64::from(t[1]), f64::from(t[2])],
    );
    let desc = ColliderDesc {
        shape: ColliderShape::Heightfield {
            rows: collider.rows,
            cols: collider.cols,
            heights: collider.heights.iter().map(|h| f64::from(*h)).collect(),
            scale: [f64::from(collider.size_m), 1.0, f64::from(collider.size_m)],
        },
        density: 1.0,
        // Ground friction that a character controller can actually walk on; no bounce.
        friction: 0.9,
        restitution: 0.0,
    };
    (body, desc)
}

/// Bake an impostor billboard texture by software-rasterizing a prototype mesh from the side.
///
/// A GPU bake would need the render thread, a target, a readback and a frame of latency. This is a
/// z-buffered orthographic rasterizer over the mesh's own vertex colours and a fixed key light: about a
/// millisecond for a tree, deterministic, testable headlessly, and available at import time rather than at
/// first draw. Alpha is 0 where nothing was drawn, which is what makes the billboard read as foliage rather
/// than as a rectangle.
#[must_use]
pub fn bake_impostor_texture(asset: &MeshAsset, size: u32) -> Texture {
    let size = size.clamp(8, 1024);
    let n = (size * size) as usize;
    let mut rgba = vec![0u8; n * 4];
    let mut depth = vec![f32::MAX; n];
    let bounds = asset.bounds();
    let (lo, hi) = (bounds.min, bounds.max);
    let span_x = (hi[0] - lo[0]).max(1e-4);
    let span_y = (hi[1] - lo[1]).max(1e-4);
    // Fit the taller axis so the silhouette is not stretched, and keep a texel of margin.
    let span = span_x.max(span_y) * 1.02;
    let cx = f32::midpoint(lo[0], hi[0]);
    let cy = f32::midpoint(lo[1], hi[1]);
    // A key light from the front-upper-left, matching how foliage is usually authored.
    let light = normalize3([-0.4, 0.7, 0.6]);

    for prim in &asset.primitives {
        let mat = asset.materials.get(prim.material);
        let base = mat.map_or([0.4, 0.5, 0.3], |m| {
            [m.base_color[0], m.base_color[1], m.base_color[2]]
        });
        for tri in prim.indices.as_chunks::<3>().0 {
            let vs: Vec<[f32; 3]> = tri
                .iter()
                .filter_map(|i| prim.positions.get(*i as usize).copied())
                .collect();
            if vs.len() != 3 {
                continue;
            }
            // Project: X across, Y up (flipped for texture space), Z is depth.
            let project = |p: [f32; 3]| -> [f32; 3] {
                let u = (p[0] - cx) / span + 0.5;
                let v = 0.5 - (p[1] - cy) / span;
                [u * size as f32, v * size as f32, p[2]]
            };
            let p0 = project(vs[0]);
            let p1 = project(vs[1]);
            let p2 = project(vs[2]);
            let nrm = tri_normal(vs[0], vs[1], vs[2]);
            // Two-sided: foliage cards face both ways, and a back-facing leaf should still be lit.
            let lambert = (nrm[0] * light[0] + nrm[1] * light[1] + nrm[2] * light[2]).abs();
            let shade = 0.35 + 0.65 * lambert;
            raster_triangle(
                &mut rgba,
                &mut depth,
                size,
                [p0, p1, p2],
                [
                    (base[0] * shade).min(1.0),
                    (base[1] * shade).min(1.0),
                    (base[2] * shade).min(1.0),
                ],
            );
        }
    }
    Texture {
        width: size,
        height: size,
        rgba8: rgba,
    }
}

fn tri_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    normalize3([
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ])
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let l2 = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
    if l2 <= 1e-20 {
        return [0.0, 1.0, 0.0];
    }
    let inv = 1.0 / l2.sqrt();
    [v[0] * inv, v[1] * inv, v[2] * inv]
}

/// Fill one triangle with a flat colour, z-tested. Barycentric, half-open on the max edges so adjacent
/// triangles neither double-shade nor leave a gap.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn raster_triangle(
    rgba: &mut [u8],
    depth: &mut [f32],
    size: u32,
    p: [[f32; 3]; 3],
    colour: [f32; 3],
) {
    let min_x = p
        .iter()
        .map(|v| v[0])
        .fold(f32::MAX, f32::min)
        .floor()
        .max(0.0) as u32;
    let max_x =
        (p.iter().map(|v| v[0]).fold(f32::MIN, f32::max).ceil()).clamp(0.0, size as f32) as u32;
    let min_y = p
        .iter()
        .map(|v| v[1])
        .fold(f32::MAX, f32::min)
        .floor()
        .max(0.0) as u32;
    let max_y =
        (p.iter().map(|v| v[1]).fold(f32::MIN, f32::max).ceil()).clamp(0.0, size as f32) as u32;
    let area =
        (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
    if area.abs() < 1e-9 {
        return;
    }
    for y in min_y..max_y {
        for x in min_x..max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let w0 = ((p[1][0] - px) * (p[2][1] - py) - (p[2][0] - px) * (p[1][1] - py)) / area;
            let w1 = ((p[2][0] - px) * (p[0][1] - py) - (p[0][0] - px) * (p[2][1] - py)) / area;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let z = w0 * p[0][2] + w1 * p[1][2] + w2 * p[2][2];
            let idx = (y * size + x) as usize;
            // Nearest wins; the camera looks down -Z, so a larger Z is nearer.
            if -z < depth[idx] {
                depth[idx] = -z;
                let o = idx * 4;
                rgba[o] = (colour[0] * 255.0 + 0.5) as u8;
                rgba[o + 1] = (colour[1] * 255.0 + 0.5) as u8;
                rgba[o + 2] = (colour[2] * 255.0 + 0.5) as u8;
                rgba[o + 3] = 255;
            }
        }
    }
}

#[cfg(test)]
mod tests;
