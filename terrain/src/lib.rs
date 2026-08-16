//! # `metrocalk-terrain` (M19 / ADR-104) — terrain as a recipe
//!
//! **A terrain is a recipe, not a mesh.** The Loro document holds one small [`TerrainRecipe`] — a seed, an
//! ordered layer stack, sculpt strokes, road/river splines, biome/material/scatter rules and budgets. Every
//! heavy artefact is *derived* from it by a pure function of `(recipe, chunk, lod)`:
//!
//! | Artefact | Produced by | Consumed by |
//! |---|---|---|
//! | chunk mesh (+ LODs, + skirts, + measured geometric error) | [`mesh::build_chunk_mesh`] | the wgpu mesh pass, via a content-addressed `MeshAsset` |
//! | splat albedo + normal texture | [`material::bake_chunk_textures`] | the same submesh's texture bindings |
//! | scattered instances (vegetation/props, per-LOD) | [`scatter::scatter_chunk`] | the instanced draw path |
//! | height-field collider (+ physics LOD) | [`collision::chunk_collider`] | `physics::ColliderShape::Heightfield` |
//! | navigation grid + paths | [`nav`] | gameplay/agents |
//! | which chunks exist at which LOD, and when | [`stream::Streamer`] | the host's worker pool |
//!
//! ## Why this shape
//!
//! Editing a *recipe* is what makes terrain "highly dynamic, editable, reusable and deterministic". A
//! sculpt stroke is 40 bytes in the document, undoable through the ordinary commit pipeline, mergeable
//! under CRDT rules, and replayable at any resolution forever. Editing a *baked heightmap* would mean
//! megabytes of binary churn per brush dab, no meaningful undo granularity, and a document that cannot
//! merge. The recipe is also the reuse unit: copy it and you have the same world; change the seed and you
//! have a different one with the same art direction.
//!
//! Two properties fall out of pointwise evaluation and are load-bearing for quality:
//!
//! * **No seams, ever.** [`Terrain::height`] is a pure function of world position. Two adjacent chunks at
//!   two different LODs evaluate the *same expression* at a shared edge vertex, so they agree bit-exactly.
//!   Cracks can then only come from *missing* geometry across a T-junction, which the skirts cover, and
//!   normals come from the same analytic gradient at every LOD so lighting does not seam either.
//! * **Any resolution, any order.** The same recipe yields a 0.25 m authoring grid and a 32 m distant
//!   chunk; chunks can be built on any thread in any order and still produce identical bytes.
//!
//! ## The one coupled stage, handled honestly
//!
//! Hydraulic erosion is *not* pointwise — a droplet carries sediment across cells. It is therefore a
//! **bake**: [`erosion::bake`] produces a height-offset [`grid::HeightGrid`] which the field then samples
//! pointwise like any other layer. To keep even that stage order-independent it runs in **epochs**: within
//! an epoch every droplet reads the same frozen grid and accumulates into a separate delta buffer, so
//! droplets are independent and the work divides across any number of threads or tiles; between epochs the
//! deltas are applied, which is what lets channels deepen and feed back on themselves. Droplet spawning is
//! keyed on **world** cell coordinates, so it does not depend on which region is being processed. Both
//! properties are asserted by test (`erosion::tests::droplet_order_does_not_change_an_epoch` and
//! `bake_is_bit_reproducible`), not merely claimed.
//!
//! ## What this crate deliberately does not do
//!
//! It owns no threads, no files, no GPU and no mesh/image formats. `build_chunk` is a pure function; the
//! host runs it on a worker pool, turns the result into a content-addressed `MeshAsset`, and uploads it.
//! That is what keeps the crate `wasm32`-portable and what keeps the renderer unaware terrain exists —
//! a terrain chunk is just another instanced textured mesh.
//!
//! ## References
//! Ulrich, *Rendering Massive Terrains using Chunked Level of Detail Control* (SIGGRAPH 2002) — the
//! measured-geometric-error/screen-space-error LOD selection used in [`mesh`] and [`stream`];
//! Strugar, *Continuous Distance-Dependent Level of Detail* (JGT 2009) — the vertex-morph refinement named
//! as a seam in [`mesh::LodError`]'s docs; Musgrave/Kolb/Mace (1989) and Mei/Decaudin/Hu (2007) for the
//! erosion models; Gribb/Hartmann for the frustum-plane extraction in [`stream`].

#![allow(
    // Terrain code converts between world metres (f32), grid cells (usize) and integer hash lattices
    // (i32/i64) constantly. Every conversion here is deliberate and range-checked by construction.
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    // Geometry maths reads better as `x`/`z`/`h`/`w` than as invented long names.
    clippy::many_single_char_names,
    clippy::similar_names,
    // Field/mesh builders legitimately take many small scalar parameters; grouping them into structs for
    // the lint's sake would add indirection without adding meaning.
    clippy::too_many_arguments,
    // Recipe structs are plain data tables with several independent boolean switches.
    clippy::struct_excessive_bools,
    // Exact float comparison is used only against values this crate itself produced (0.0 sentinels,
    // quantized cell keys), never against measured quantities.
    clippy::float_cmp
)]

pub mod biome;
pub mod collision;
pub mod describe;
pub mod erosion;
pub mod feature;
pub mod field;
pub mod grid;
pub mod material;
pub mod mesh;
pub mod nav;
pub mod noise;
pub mod operation;
pub mod plan;
pub mod preset;
pub mod profile;
pub mod proto;
pub mod recipe;
pub mod scatter;
pub mod sculpt;
pub mod spline;
pub mod stage;
pub mod stream;
pub mod validate;

pub use biome::{BiomeSample, BiomeWeights};
pub use collision::{chunk_collider, ChunkCollider};
pub use field::{SurfaceSample, Terrain};
pub use grid::HeightGrid;
pub use mesh::{build_chunk_mesh, ChunkMesh, LodError};
pub use nav::{NavGrid, PathResult};
pub use profile::{BuildTimings, TerrainStats};
pub use proto::{ProtoKind, ProtoMesh};
pub use recipe::{
    BiomeRule, Blend, ChunkCoord, HeightMask, Layer, LayerKind, LodPolicy, MaterialLayer,
    MemoryBudget, ScatterProto, ScatterRule, SplineDef, SplineKind, Stroke, StrokeKind,
    TerrainRecipe, WaterConfig,
};
pub use scatter::{ScatterInstance, ScatterOutput};
pub use stream::{ChunkBuild, ChunkRequest, StreamPlan, Streamer, ViewParams};
pub use validate::{Severity, ValidationIssue, ValidationReport};

/// Something the caller must see rather than get a silently wrong terrain for.
///
/// Every variant carries the *reason* in plain language, because a terrain that quietly clamps a bad
/// parameter produces a world the author cannot explain. This mirrors the house rule the CSG and SDF
/// bakers follow: `Blocked`-with-a-reason, never a silent crack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerrainError {
    /// The recipe cannot produce a terrain at all; the string lists the blocking issues.
    InvalidRecipe(String),
    /// A layer referenced a baked grid (imported heightmap / erosion bake) the caller did not supply.
    MissingBakedGrid {
        /// Index of the layer in [`TerrainRecipe::layers`].
        layer: usize,
        /// The key the layer asked for.
        key: String,
    },
    /// A chunk coordinate lies outside the recipe's world bounds.
    ChunkOutOfBounds(ChunkCoord),
    /// The request would exceed a hard budget; the string says which and by how much.
    BudgetExceeded(String),
}

impl std::fmt::Display for TerrainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRecipe(why) => write!(f, "terrain recipe is not buildable: {why}"),
            Self::MissingBakedGrid { layer, key } => write!(
                f,
                "layer {layer} needs the baked grid '{key}', which was not supplied"
            ),
            Self::ChunkOutOfBounds(c) => {
                write!(f, "chunk ({}, {}) is outside the world bounds", c.x, c.z)
            }
            Self::BudgetExceeded(why) => write!(f, "terrain budget exceeded: {why}"),
        }
    }
}

impl std::error::Error for TerrainError {}
