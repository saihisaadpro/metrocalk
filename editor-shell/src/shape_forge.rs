//! Shape Studio — the Build sub-engine's creation substrate (parametric primitives, draw-to-3D and the
//! shape recipe schema).
//!
//! **A shape is a RECIPE, not a mesh** (the ADR-089/ADR-104 discipline): the scene document holds one
//! `ShapeRecipe` component (kind + parameters + an optional drawn profile) and the watertight triangle
//! mesh is DERIVED from it by a pure function in here. The persisted artifact for a parametric or drawn
//! shape is the *recipe itself* — a missing blob rebuilds deterministically on load, so a shape can never
//! "die with the process" (the M19.3 lesson). Results of mesh-level operations (an exact CSG combine)
//! are not re-derivable from parameters alone, so those persist as a *mesh-form* artifact instead; the
//! magic router distinguishes the two forms.
//!
//! Determinism: every builder is fixed-order pure `f64` arithmetic (std trig is deterministic per
//! platform — the same honesty standard as Pipe Forge's sweep); the same recipe produces a bit-identical
//! [`TriMesh`] and therefore the same content-addressed handle. Every mesh that leaves this module is
//! gated by the always-on M13.2 validator — a degenerate result is a plain-language refusal, never a
//! silent crack.

#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
// Geometry code speaks in the standard one-letter math names (r/h/u/v/w, quad corners a..d); renaming
// them to satisfy the lint would make the maths harder to check against the references, not easier.
#![allow(clippy::many_single_char_names)]

use std::collections::BTreeMap;
use std::f64::consts::{PI, TAU};
use std::fmt;

use metrocalk_assets::{AssetId, Material, MeshAsset, Primitive};
use metrocalk_core::caps::canonical;
use metrocalk_core::variant::INSTANCE_META;
use metrocalk_core::{Engine, EntityId, FieldValue, Op, PipelineError};
use metrocalk_csg::{validate, TriMesh};
use metrocalk_sdf::{Axis, Sdf};
use serde::{Deserialize, Serialize};

use crate::capscene::{CapScene, MESH_FIELD, NAME_FIELD};

/// Self-describing prefix for a persisted shape artifact (routed by the blob loader like `MTKPIPE1`).
pub const SHAPE_ARTIFACT_MAGIC: &[u8; 8] = b"MTKSHAP1";
/// Current recipe schema version.
pub const SHAPE_RECIPE_VERSION: u32 = 1;
/// The component kind the scene document stores.
pub const SHAPE_COMPONENT: &str = "ShapeRecipe";

const FORM_RECIPE: u8 = 1;
const FORM_MESH: u8 = 2;

const MAX_PROFILE_POINTS: usize = 256;
const MAX_COORD_M: f64 = 100.0;
const MIN_REVOLVE_R: f64 = 0.01;
/// Hostile length prefixes must not turn a blob load into an unbounded allocation.
const MAX_ARTIFACT_BYTES: usize = 32 * 1024 * 1024;

// ============================================================================================
// The recipe — the durable, editable source of a shape
// ============================================================================================

/// The editable source of a Shape Studio shape. `params` is a flat, sorted map (deterministic JSON);
/// `profile` carries the drawn outline for `extrude` (ground-plan `(x, z)` metres) and `revolve`
/// (`(radius, y)` metres, spun around the vertical axis).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShapeRecipe {
    /// Schema version ([`SHAPE_RECIPE_VERSION`]).
    pub v: u32,
    /// The shape kind: `box|sphere|cylinder|cone|torus|capsule|wedge|prism|extrude|revolve|union|carve|intersect|meld`.
    pub kind: String,
    /// Flat scalar parameters (sorted map ⇒ canonical JSON).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, f64>,
    /// The drawn outline (extrude/revolve only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profile: Vec<[f64; 2]>,
    /// For combine/meld results: the content-addressed source handles this result was built from
    /// (provenance — the mesh-form artifact carries the actual geometry).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
}

impl ShapeRecipe {
    /// A parametric recipe with the catalog's default parameters for `kind`.
    #[must_use]
    pub fn parametric(kind: &str) -> Option<Self> {
        let spec = shape_specs().into_iter().find(|s| s.kind == kind)?;
        Some(Self {
            v: SHAPE_RECIPE_VERSION,
            kind: kind.to_string(),
            params: spec
                .params
                .iter()
                .map(|p| (p.key.to_string(), p.default))
                .collect(),
            profile: Vec::new(),
            sources: Vec::new(),
        })
    }

    /// Canonical JSON (sorted params, fixed field order) — what the document's `source` field stores
    /// and what the recipe-form artifact hashes.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Parse a recipe from the document's `source` field.
    ///
    /// # Errors
    /// A plain-language reason when the JSON is not a valid recipe.
    pub fn from_json(json: &str) -> Result<Self, ShapeError> {
        serde_json::from_str(json)
            .map_err(|e| ShapeError::BadRecipe(format!("the shape recipe did not parse: {e}")))
    }

    /// Read a parameter with a default.
    #[must_use]
    pub fn param(&self, key: &str, default: f64) -> f64 {
        self.params.get(key).copied().unwrap_or(default)
    }
}

/// A shape build failed in a way the author must see — never a silent crack, never a panic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShapeError {
    /// Unknown kind string.
    UnknownKind(String),
    /// A parameter is out of its allowed range.
    BadParam(String),
    /// The drawn outline can't become a solid (too few points, self-intersecting, zero area…).
    BadProfile(String),
    /// The recipe JSON didn't parse.
    BadRecipe(String),
    /// The geometry pipeline refused to produce a clean solid.
    Blocked(String),
}

impl fmt::Display for ShapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownKind(k) => write!(f, "there is no shape called \"{k}\""),
            Self::BadParam(m) | Self::BadProfile(m) | Self::BadRecipe(m) | Self::Blocked(m) => {
                write!(f, "{m}")
            }
        }
    }
}

impl std::error::Error for ShapeError {}

// ============================================================================================
// The catalog — what the Build panel offers, with plain-language parameter specs
// ============================================================================================

/// One tunable parameter of a shape, in the author's language.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapeParamSpec {
    /// Recipe key.
    pub key: &'static str,
    /// Plain-language label ("Radius", "Smoothness").
    pub label: &'static str,
    /// Minimum accepted value.
    pub min: f64,
    /// Maximum accepted value.
    pub max: f64,
    /// UI nudge step.
    pub step: f64,
    /// Default value.
    pub default: f64,
    /// Whole numbers only (segment counts).
    pub integer: bool,
    /// Unit shown after the value ("m" or "" for counts).
    pub unit: &'static str,
}

/// One creatable shape: identity, copy and its parameter specs.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapeSpec {
    /// Recipe kind.
    pub kind: &'static str,
    /// Card label.
    pub label: &'static str,
    /// One line on what it is for.
    pub blurb: &'static str,
    /// Card glyph.
    pub icon: &'static str,
    /// Tunable parameters, in display order.
    pub params: Vec<ShapeParamSpec>,
}

#[allow(clippy::too_many_arguments)] // a data-table row constructor, not branching logic
const fn p(
    key: &'static str,
    label: &'static str,
    min: f64,
    max: f64,
    step: f64,
    default: f64,
    integer: bool,
    unit: &'static str,
) -> ShapeParamSpec {
    ShapeParamSpec {
        key,
        label,
        min,
        max,
        step,
        default,
        integer,
        unit,
    }
}

/// The parametric shapes the Build panel offers, with their parameter specs.
#[must_use]
#[allow(clippy::too_many_lines)] // a flat data table of shape definitions, not branching logic
pub fn shape_specs() -> Vec<ShapeSpec> {
    vec![
        ShapeSpec {
            kind: "box",
            label: "Box",
            blurb: "A rectangular block",
            icon: "▧",
            params: vec![
                p("width", "Width", 0.05, 50.0, 0.1, 1.0, false, "m"),
                p("height", "Height", 0.05, 50.0, 0.1, 1.0, false, "m"),
                p("depth", "Depth", 0.05, 50.0, 0.1, 1.0, false, "m"),
            ],
        },
        ShapeSpec {
            kind: "sphere",
            label: "Sphere",
            blurb: "A ball",
            icon: "●",
            params: vec![
                p("radius", "Radius", 0.05, 25.0, 0.05, 0.5, false, "m"),
                p("segments", "Smoothness", 8.0, 96.0, 4.0, 32.0, true, ""),
            ],
        },
        ShapeSpec {
            kind: "cylinder",
            label: "Cylinder",
            blurb: "A round column",
            icon: "⬤",
            params: vec![
                p("radius", "Radius", 0.05, 25.0, 0.05, 0.5, false, "m"),
                p("height", "Height", 0.05, 50.0, 0.1, 1.0, false, "m"),
                p("segments", "Smoothness", 3.0, 96.0, 1.0, 32.0, true, ""),
            ],
        },
        ShapeSpec {
            kind: "cone",
            label: "Cone",
            blurb: "A point over a round base",
            icon: "▲",
            params: vec![
                p("radius", "Radius", 0.05, 25.0, 0.05, 0.5, false, "m"),
                p("height", "Height", 0.05, 50.0, 0.1, 1.0, false, "m"),
                p("segments", "Smoothness", 3.0, 96.0, 1.0, 32.0, true, ""),
            ],
        },
        ShapeSpec {
            kind: "torus",
            label: "Ring",
            blurb: "A doughnut",
            icon: "◍",
            params: vec![
                p("radius", "Ring radius", 0.1, 25.0, 0.05, 0.5, false, "m"),
                p("thickness", "Thickness", 0.02, 10.0, 0.02, 0.18, false, "m"),
                p("segments", "Smoothness", 8.0, 96.0, 4.0, 40.0, true, ""),
            ],
        },
        ShapeSpec {
            kind: "capsule",
            label: "Capsule",
            blurb: "A pill — a cylinder with round ends",
            icon: "▢",
            params: vec![
                p("radius", "Radius", 0.05, 10.0, 0.05, 0.3, false, "m"),
                p("height", "Height", 0.2, 50.0, 0.1, 1.2, false, "m"),
                p("segments", "Smoothness", 8.0, 96.0, 4.0, 32.0, true, ""),
            ],
        },
        ShapeSpec {
            kind: "wedge",
            label: "Wedge",
            blurb: "A ramp",
            icon: "◣",
            params: vec![
                p("width", "Width", 0.05, 50.0, 0.1, 1.0, false, "m"),
                p("height", "Height", 0.05, 50.0, 0.1, 1.0, false, "m"),
                p("depth", "Depth", 0.05, 50.0, 0.1, 1.0, false, "m"),
            ],
        },
        ShapeSpec {
            kind: "prism",
            label: "Prism",
            blurb: "A column with flat sides",
            icon: "⬢",
            params: vec![
                p("radius", "Radius", 0.05, 25.0, 0.05, 0.5, false, "m"),
                p("height", "Height", 0.05, 50.0, 0.1, 1.0, false, "m"),
                p("sides", "Sides", 3.0, 12.0, 1.0, 6.0, true, ""),
            ],
        },
        ShapeSpec {
            kind: "tube",
            label: "Tube",
            blurb: "A hollow pipe with a wall thickness",
            icon: "◎",
            params: vec![
                p("radius", "Radius", 0.1, 25.0, 0.05, 0.5, false, "m"),
                p("thickness", "Wall", 0.02, 10.0, 0.02, 0.12, false, "m"),
                p("height", "Height", 0.05, 50.0, 0.1, 1.0, false, "m"),
                p("segments", "Smoothness", 8.0, 96.0, 4.0, 32.0, true, ""),
            ],
        },
    ]
}

/// The parameter keys a kind's builder actually reads — the edit allowlist. Anything else written
/// into a recipe would silently mint a new content address for bit-identical geometry (a burned undo
/// step + a stray blob per typo), so an unknown key is a refusal that names itself.
///
/// # Errors
/// [`ShapeError::BadParam`] naming the first unknown key.
pub fn validate_param_keys<'k>(
    kind: &str,
    keys: impl IntoIterator<Item = &'k str>,
) -> Result<(), ShapeError> {
    let allowed: Vec<&'static str> = match kind {
        "extrude" => vec!["height", "taper"],
        "revolve" => vec!["segments"],
        parametric => shape_specs()
            .into_iter()
            .find(|s| s.kind == parametric)
            .map(|s| s.params.iter().map(|p| p.key).collect())
            .unwrap_or_default(),
    };
    for key in keys {
        if !allowed.contains(&key) {
            return Err(ShapeError::BadParam(format!(
                "a {kind} has no parameter called \"{key}\""
            )));
        }
    }
    Ok(())
}

/// Rebuild a shape's display asset from the document's own `ShapeRecipe.source` — the self-heal a
/// missing sidecar blob takes at boot/open (the recipe IS the artifact for parametric/drawn kinds).
/// Mesh-form results (combine/meld) carry geometry that is NOT re-derivable; that loss is named, not
/// silent.
///
/// # Errors
/// A plain-language reason (corrupt source, identity mismatch, or a non-rebuildable mesh-form kind).
pub fn rebuild_from_source(expected: &str, source: &str) -> Result<ShapeBuild, String> {
    let recipe = ShapeRecipe::from_json(source).map_err(|e| e.to_string())?;
    if matches!(
        recipe.kind.as_str(),
        "union" | "carve" | "intersect" | "meld"
    ) {
        return Err(format!(
            "a {} result's geometry lives only in its saved blob — the blob is missing and the \
             geometry cannot be re-derived from parameters",
            recipe.kind
        ));
    }
    let built = bake_shape(&recipe).map_err(|e| e.to_string())?;
    if built.handle != expected {
        return Err("the editable recipe does not match the saved asset identity".to_string());
    }
    Ok(built)
}

/// The per-kind display material: a satin dielectric in a calm, distinct colour, so a fresh scene of
/// shapes reads like a set of crafted objects rather than grey clay. Linear RGB (the shader contract);
/// a display value only — never part of geometry identity (but part of the ARTIFACT identity, so two
/// same-size shapes in different colours keep distinct handles).
#[must_use]
pub fn shape_material(kind: &str) -> Material {
    let base: [f32; 3] = match kind {
        "box" => [0.56, 0.24, 0.14],      // terracotta
        "sphere" => [0.10, 0.26, 0.55],   // azure
        "cylinder" => [0.16, 0.34, 0.16], // sage
        "cone" => [0.62, 0.38, 0.07],     // amber
        "torus" => [0.27, 0.17, 0.44],    // violet
        "capsule" => [0.09, 0.35, 0.36],  // teal
        "wedge" => [0.23, 0.28, 0.38],    // slate
        "prism" => [0.50, 0.18, 0.26],    // rose
        "tube" => [0.44, 0.24, 0.14],     // copper
        "extrude" => [0.12, 0.38, 0.50],  // lagoon
        "revolve" => [0.55, 0.34, 0.11],  // gold
        "meld" => [0.38, 0.22, 0.50],     // orchid
        _ => [0.32, 0.33, 0.36],          // combines: neutral steel satin
    };
    Material {
        base_color: [base[0], base[1], base[2], 1.0],
        metallic: 0.04,
        roughness: 0.48,
        ..Material::default()
    }
}

// ============================================================================================
// The builders — recipe → deterministic watertight TriMesh (base resting on y = 0)
// ============================================================================================

/// Build the mesh a recipe describes. Every result is validator-clean and rests on `y = 0` (so a shape
/// spawned on the ground plane sits on the ground, not half inside it).
///
/// # Errors
/// A plain-language [`ShapeError`] when a parameter is out of range or a drawn outline can't become a
/// solid. Never a panic, never a silent degenerate mesh.
pub fn build_shape(recipe: &ShapeRecipe) -> Result<TriMesh, ShapeError> {
    let mesh = match recipe.kind.as_str() {
        "box" => build_box(recipe)?,
        "sphere" => build_sphere(recipe)?,
        "cylinder" => build_cylinder(recipe, false)?,
        "prism" => build_cylinder(recipe, true)?,
        "cone" => build_cone(recipe)?,
        "torus" => build_torus(recipe)?,
        "capsule" => build_capsule(recipe)?,
        "wedge" => build_wedge(recipe)?,
        "tube" => build_tube(recipe)?,
        "extrude" => build_extrude(recipe)?,
        "revolve" => build_revolve(recipe)?,
        other => return Err(ShapeError::UnknownKind(other.to_string())),
    };
    let mesh = rest_on_ground(mesh);
    let report = validate(&mesh);
    if !(report.watertight && report.manifold && report.oriented) {
        return Err(ShapeError::Blocked(format!(
            "the shape did not produce a clean solid: {}",
            report.explain()
        )));
    }
    Ok(mesh)
}

fn require(cond: bool, msg: &str) -> Result<(), ShapeError> {
    if cond {
        Ok(())
    } else {
        Err(ShapeError::BadParam(msg.to_string()))
    }
}

fn dim(
    recipe: &ShapeRecipe,
    key: &str,
    default: f64,
    min: f64,
    max: f64,
    label: &str,
) -> Result<f64, ShapeError> {
    let v = recipe.param(key, default);
    require(
        v.is_finite() && v >= min && v <= max,
        &format!("{label} must be between {min} and {max} metres"),
    )?;
    Ok(v)
}

fn segs(
    recipe: &ShapeRecipe,
    key: &str,
    default: f64,
    min: usize,
    max: usize,
) -> Result<usize, ShapeError> {
    let v = recipe.param(key, default);
    require(
        v.is_finite() && v >= min as f64 && v <= max as f64,
        &format!("smoothness must be between {min} and {max}"),
    )?;
    #[allow(clippy::cast_sign_loss)] // guarded ≥ min ≥ 3 above
    Ok(v.round() as usize)
}

/// Translate so the bounding-box floor is exactly y = 0.
fn rest_on_ground(mut mesh: TriMesh) -> TriMesh {
    let min_y = mesh
        .positions
        .iter()
        .map(|position| position[1])
        .fold(f64::INFINITY, f64::min);
    if min_y.is_finite() && min_y != 0.0 {
        for position in &mut mesh.positions {
            position[1] -= min_y;
        }
    }
    mesh
}

fn build_box(recipe: &ShapeRecipe) -> Result<TriMesh, ShapeError> {
    let w = dim(recipe, "width", 1.0, 0.05, 50.0, "width")?;
    let h = dim(recipe, "height", 1.0, 0.05, 50.0, "height")?;
    let d = dim(recipe, "depth", 1.0, 0.05, 50.0, "depth")?;
    Ok(metrocalk_csg::box_mesh(
        [0.0, 0.0, 0.0],
        [w / 2.0, h / 2.0, d / 2.0],
    ))
}

/// UV sphere: shared pole vertices + wrapped rings ⇒ watertight by construction.
fn build_sphere(recipe: &ShapeRecipe) -> Result<TriMesh, ShapeError> {
    let r = dim(recipe, "radius", 0.5, 0.05, 25.0, "radius")?;
    let seg = segs(recipe, "segments", 32.0, 8, 96)?;
    let rings = (seg / 2).max(4);
    let mut positions: Vec<[f64; 3]> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    let top = positions.len() as u32;
    positions.push([0.0, r, 0.0]);
    for i in 1..rings {
        let theta = PI * i as f64 / rings as f64;
        for j in 0..seg {
            let phi = TAU * j as f64 / seg as f64;
            positions.push([
                r * theta.sin() * phi.cos(),
                r * theta.cos(),
                r * theta.sin() * phi.sin(),
            ]);
        }
    }
    let bottom = positions.len() as u32;
    positions.push([0.0, -r, 0.0]);
    let ring = |i: usize, j: usize| -> u32 { 1 + ((i - 1) * seg + (j % seg)) as u32 };
    // Top cap: outward = +Y-ish. With phi increasing CCW seen from +Y, (top, j+1, j) faces outward.
    for j in 0..seg {
        triangles.push([top, ring(1, j + 1), ring(1, j)]);
    }
    for i in 1..rings - 1 {
        for j in 0..seg {
            let a = ring(i, j);
            let b = ring(i, j + 1);
            let c = ring(i + 1, j + 1);
            let d = ring(i + 1, j);
            triangles.push([a, b, c]);
            triangles.push([a, c, d]);
        }
    }
    for j in 0..seg {
        triangles.push([bottom, ring(rings - 1, j), ring(rings - 1, j + 1)]);
    }
    Ok(TriMesh::new(positions, triangles))
}

/// Cylinder (round column) — `prism = true` reads the `sides` parameter instead (a hexagonal column is
/// just a 6-segment cylinder; the crease-aware normal packer keeps its facets crisp).
fn build_cylinder(recipe: &ShapeRecipe, prism: bool) -> Result<TriMesh, ShapeError> {
    let r = dim(recipe, "radius", 0.5, 0.05, 25.0, "radius")?;
    let h = dim(recipe, "height", 1.0, 0.05, 50.0, "height")?;
    let seg = if prism {
        segs(recipe, "sides", 6.0, 3, 12)?
    } else {
        segs(recipe, "segments", 32.0, 3, 96)?
    };
    let (hy, positions, triangles) = tube(r, r, h, seg);
    let _ = hy;
    Ok(TriMesh::new(positions, triangles))
}

/// A capped tube with independent bottom/top radii (cylinder when equal). Both caps are fans around a
/// centre vertex. Returns (half-height, positions, triangles); centred on the origin.
fn tube(r_bottom: f64, r_top: f64, h: f64, seg: usize) -> (f64, Vec<[f64; 3]>, Vec<[u32; 3]>) {
    let hy = h / 2.0;
    let mut positions: Vec<[f64; 3]> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    for j in 0..seg {
        let phi = TAU * j as f64 / seg as f64;
        positions.push([r_bottom * phi.cos(), -hy, r_bottom * phi.sin()]);
    }
    for j in 0..seg {
        let phi = TAU * j as f64 / seg as f64;
        positions.push([r_top * phi.cos(), hy, r_top * phi.sin()]);
    }
    let cb = positions.len() as u32;
    positions.push([0.0, -hy, 0.0]);
    let ct = positions.len() as u32;
    positions.push([0.0, hy, 0.0]);
    let b = |j: usize| (j % seg) as u32;
    let t = |j: usize| (seg + (j % seg)) as u32;
    for j in 0..seg {
        // Side quad, outward. phi increases CCW seen from +Y, so outward winding is (b_j, t_j, t_{j+1}).
        triangles.push([b(j), t(j), t(j + 1)]);
        triangles.push([b(j), t(j + 1), b(j + 1)]);
        // Bottom cap (outward −Y) and top cap (outward +Y).
        triangles.push([cb, b(j), b(j + 1)]);
        triangles.push([ct, t(j + 1), t(j)]);
    }
    (hy, positions, triangles)
}

fn build_cone(recipe: &ShapeRecipe) -> Result<TriMesh, ShapeError> {
    let r = dim(recipe, "radius", 0.5, 0.05, 25.0, "radius")?;
    let h = dim(recipe, "height", 1.0, 0.05, 50.0, "height")?;
    let seg = segs(recipe, "segments", 32.0, 3, 96)?;
    let hy = h / 2.0;
    let mut positions: Vec<[f64; 3]> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    for j in 0..seg {
        let phi = TAU * j as f64 / seg as f64;
        positions.push([r * phi.cos(), -hy, r * phi.sin()]);
    }
    let apex = positions.len() as u32;
    positions.push([0.0, hy, 0.0]);
    let centre = positions.len() as u32;
    positions.push([0.0, -hy, 0.0]);
    let b = |j: usize| (j % seg) as u32;
    for j in 0..seg {
        triangles.push([b(j), apex, b(j + 1)]);
        triangles.push([centre, b(j), b(j + 1)]);
    }
    Ok(TriMesh::new(positions, triangles))
}

fn build_torus(recipe: &ShapeRecipe) -> Result<TriMesh, ShapeError> {
    let ring_r = dim(recipe, "radius", 0.5, 0.1, 25.0, "ring radius")?;
    let tube_r = dim(recipe, "thickness", 0.18, 0.02, 10.0, "thickness")?;
    require(
        tube_r < ring_r,
        "thickness must be smaller than the ring radius (or the hole closes)",
    )?;
    let seg = segs(recipe, "segments", 40.0, 8, 96)?;
    let sides = (seg / 2).clamp(8, 48);
    let mut positions: Vec<[f64; 3]> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    for i in 0..seg {
        let u = TAU * i as f64 / seg as f64;
        for j in 0..sides {
            let v = TAU * j as f64 / sides as f64;
            let radial = ring_r + tube_r * v.cos();
            positions.push([radial * u.cos(), tube_r * v.sin(), radial * u.sin()]);
        }
    }
    let vid = |i: usize, j: usize| ((i % seg) * sides + (j % sides)) as u32;
    for i in 0..seg {
        for j in 0..sides {
            let a = vid(i, j);
            let b = vid(i + 1, j);
            let c = vid(i + 1, j + 1);
            let d = vid(i, j + 1);
            // Outward winding for this (u CW-in-XZ, v CCW-in-radial/Y) parametrisation — asserted by
            // the validator + the signed-volume test.
            triangles.push([a, d, c]);
            triangles.push([a, c, b]);
        }
    }
    Ok(TriMesh::new(positions, triangles))
}

fn build_capsule(recipe: &ShapeRecipe) -> Result<TriMesh, ShapeError> {
    let r = dim(recipe, "radius", 0.3, 0.05, 10.0, "radius")?;
    let h = dim(recipe, "height", 1.2, 0.2, 50.0, "height")?;
    require(
        h > 2.0 * r + 1e-6,
        "height must be more than twice the radius (the two round ends need room)",
    )?;
    let seg = segs(recipe, "segments", 32.0, 8, 96)?;
    let hemi = (seg / 4).max(2);
    let cyl_half = (h - 2.0 * r) / 2.0;
    let mut positions: Vec<[f64; 3]> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    let top_pole = positions.len() as u32;
    positions.push([0.0, cyl_half + r, 0.0]);
    // Rings: top hemisphere (theta 0→π/2 exclusive→inclusive at equator), then bottom hemisphere
    // equator→pole. The two equator rings are distinct (they sit at ±cyl_half) — the straight wall
    // between them is the cylindrical section.
    let mut ring_starts: Vec<u32> = Vec::new();
    for i in 1..=hemi {
        let theta = (PI / 2.0) * i as f64 / hemi as f64;
        ring_starts.push(positions.len() as u32);
        for j in 0..seg {
            let phi = TAU * j as f64 / seg as f64;
            positions.push([
                r * theta.sin() * phi.cos(),
                cyl_half + r * theta.cos(),
                r * theta.sin() * phi.sin(),
            ]);
        }
    }
    for i in 0..hemi {
        let theta = (PI / 2.0) * (1.0 + i as f64 / hemi as f64);
        ring_starts.push(positions.len() as u32);
        for j in 0..seg {
            let phi = TAU * j as f64 / seg as f64;
            positions.push([
                r * theta.sin() * phi.cos(),
                -cyl_half + r * theta.cos(),
                r * theta.sin() * phi.sin(),
            ]);
        }
    }
    let bottom_pole = positions.len() as u32;
    positions.push([0.0, -cyl_half - r, 0.0]);
    let ring = |ri: usize, j: usize| ring_starts[ri] + (j % seg) as u32;
    for j in 0..seg {
        triangles.push([top_pole, ring(0, j + 1), ring(0, j)]);
    }
    for ri in 0..ring_starts.len() - 1 {
        for j in 0..seg {
            let a = ring(ri, j);
            let b = ring(ri, j + 1);
            let c = ring(ri + 1, j + 1);
            let d = ring(ri + 1, j);
            triangles.push([a, b, c]);
            triangles.push([a, c, d]);
        }
    }
    let last = ring_starts.len() - 1;
    for j in 0..seg {
        triangles.push([bottom_pole, ring(last, j), ring(last, j + 1)]);
    }
    Ok(TriMesh::new(positions, triangles))
}

/// A hollow pipe: two concentric walls joined by flat ring caps. Watertight, genus 1.
fn build_tube(recipe: &ShapeRecipe) -> Result<TriMesh, ShapeError> {
    let r_outer = dim(recipe, "radius", 0.5, 0.1, 25.0, "radius")?;
    let wall = dim(recipe, "thickness", 0.12, 0.02, 10.0, "wall thickness")?;
    require(
        wall < r_outer - 0.005,
        "the wall must be thinner than the radius (or the hole closes — use a cylinder)",
    )?;
    let r_inner = r_outer - wall;
    let h = dim(recipe, "height", 1.0, 0.05, 50.0, "height")?;
    let seg = segs(recipe, "segments", 32.0, 8, 96)?;
    let hy = h / 2.0;
    let mut positions: Vec<[f64; 3]> = Vec::with_capacity(seg * 4);
    // Ring layout: [outer-bottom, outer-top, inner-bottom, inner-top], each `seg` long.
    for &(radius, y) in &[(r_outer, -hy), (r_outer, hy), (r_inner, -hy), (r_inner, hy)] {
        for j in 0..seg {
            let phi = TAU * j as f64 / seg as f64;
            positions.push([radius * phi.cos(), y, radius * phi.sin()]);
        }
    }
    let ring = |which: usize, j: usize| (which * seg + (j % seg)) as u32;
    let (ob, ot, ib, it) = (0, 1, 2, 3);
    let mut triangles: Vec<[u32; 3]> = Vec::with_capacity(seg * 8);
    for j in 0..seg {
        // Outer wall faces outward (same winding as the cylinder's side).
        triangles.push([ring(ob, j), ring(ot, j), ring(ot, j + 1)]);
        triangles.push([ring(ob, j), ring(ot, j + 1), ring(ob, j + 1)]);
        // Inner wall faces INWARD (into the bore) — reversed winding.
        triangles.push([ring(ib, j), ring(it, j + 1), ring(it, j)]);
        triangles.push([ring(ib, j), ring(ib, j + 1), ring(it, j + 1)]);
        // Top annulus faces +Y.
        triangles.push([ring(ot, j), ring(it, j), ring(it, j + 1)]);
        triangles.push([ring(ot, j), ring(it, j + 1), ring(ot, j + 1)]);
        // Bottom annulus faces −Y.
        triangles.push([ring(ob, j), ring(ib, j + 1), ring(ib, j)]);
        triangles.push([ring(ob, j), ring(ob, j + 1), ring(ib, j + 1)]);
    }
    Ok(TriMesh::new(positions, triangles))
}

/// A ramp: a right-triangle cross-section (in x/y) extruded along z. The vertical face is at −x.
fn build_wedge(recipe: &ShapeRecipe) -> Result<TriMesh, ShapeError> {
    let w = dim(recipe, "width", 1.0, 0.05, 50.0, "width")?;
    let h = dim(recipe, "height", 1.0, 0.05, 50.0, "height")?;
    let d = dim(recipe, "depth", 1.0, 0.05, 50.0, "depth")?;
    let (hw, hd) = (w / 2.0, d / 2.0);
    let positions: Vec<[f64; 3]> = vec![
        [-hw, 0.0, -hd], // 0 back  bottom left
        [hw, 0.0, -hd],  // 1 back  bottom right
        [-hw, h, -hd],   // 2 back  top    left
        [-hw, 0.0, hd],  // 3 front bottom left
        [hw, 0.0, hd],   // 4 front bottom right
        [-hw, h, hd],    // 5 front top    left
    ];
    let triangles: Vec<[u32; 3]> = vec![
        // Back triangle (outward −Z): CCW seen from −Z.
        [0, 2, 1],
        // Front triangle (outward +Z).
        [3, 4, 5],
        // Bottom (outward −Y).
        [0, 1, 4],
        [0, 4, 3],
        // Vertical left face (outward −X).
        [0, 3, 5],
        [0, 5, 2],
        // Sloped face (outward +X/+Y).
        [1, 2, 5],
        [1, 5, 4],
    ];
    Ok(TriMesh::new(positions, triangles))
}

// ── the drawn shapes ─────────────────────────────────────────────────────────────────────────────────

/// Sanitise a drawn outline: drop duplicate consecutive points, require 3..=256 points inside the
/// coordinate budget, refuse self-intersection and near-zero area, and return the outline in
/// positive-signed-area order (reversing a clockwise drawing — authors draw in either direction).
fn require_outline(cond: bool, msg: &str) -> Result<(), ShapeError> {
    if cond {
        Ok(())
    } else {
        Err(ShapeError::BadProfile(msg.to_string()))
    }
}

fn clean_profile(points: &[[f64; 2]]) -> Result<Vec<[f64; 2]>, ShapeError> {
    let mut pts: Vec<[f64; 2]> = Vec::with_capacity(points.len());
    for point in points {
        require_outline(
            point[0].is_finite() && point[1].is_finite(),
            "the outline contains an unusable point",
        )?;
        require_outline(
            point[0].abs() <= MAX_COORD_M && point[1].abs() <= MAX_COORD_M,
            "the outline is bigger than the 100 m drawing area",
        )?;
        if pts.last().is_none_or(|last: &[f64; 2]| {
            (last[0] - point[0]).abs() > 1e-9 || (last[1] - point[1]).abs() > 1e-9
        }) {
            pts.push(*point);
        }
    }
    // The outline is closed implicitly — a trailing point equal to the first is dropped.
    if pts.len() >= 2 {
        let (first, last) = (pts[0], *pts.last().unwrap_or(&pts[0]));
        if (first[0] - last[0]).abs() < 1e-9 && (first[1] - last[1]).abs() < 1e-9 {
            pts.pop();
        }
    }
    require_outline(
        pts.len() >= 3,
        "draw at least three points to outline a shape",
    )?;
    require_outline(
        pts.len() <= MAX_PROFILE_POINTS,
        "the outline has too many points — keep it under 256",
    )?;
    if self_intersects(&pts) {
        return Err(ShapeError::BadProfile(
            "the outline crosses itself — simplify the drawing so it traces one loop".to_string(),
        ));
    }
    let area2 = signed_area2(&pts);
    require_outline(
        area2.abs() > 1e-4,
        "the outline is too thin to become a solid — open it up a little",
    )?;
    if area2 < 0.0 {
        pts.reverse();
    }
    Ok(pts)
}

/// The area centroid of a simple polygon (the taper's scaling centre — the vertex mean would drift
/// on outlines with uneven point spacing, tilting the frustum).
fn polygon_centroid(pts: &[[f64; 2]]) -> [f64; 2] {
    let mut cx = 0.0;
    let mut cy = 0.0;
    let mut a2 = 0.0;
    for i in 0..pts.len() {
        let p = pts[i];
        let q = pts[(i + 1) % pts.len()];
        let cross = p[0] * q[1] - q[0] * p[1];
        a2 += cross;
        cx += (p[0] + q[0]) * cross;
        cy += (p[1] + q[1]) * cross;
    }
    if a2.abs() < 1e-12 {
        return pts[0]; // degenerate — clean_profile refuses these before we get here
    }
    [cx / (3.0 * a2), cy / (3.0 * a2)]
}

/// Twice the signed area of a closed 2D polygon.
fn signed_area2(pts: &[[f64; 2]]) -> f64 {
    let mut sum = 0.0;
    for i in 0..pts.len() {
        let a = pts[i];
        let b = pts[(i + 1) % pts.len()];
        sum += a[0] * b[1] - b[0] * a[1];
    }
    sum
}

/// O(n²) proper-crossing test between non-adjacent edges (adjacent edges share an endpoint by design).
fn self_intersects(pts: &[[f64; 2]]) -> bool {
    let n = pts.len();
    let seg = |i: usize| (pts[i], pts[(i + 1) % n]);
    for i in 0..n {
        for j in i + 1..n {
            // Skip adjacent edges (they legitimately share a vertex).
            if j == i || (j + 1) % n == i || (i + 1) % n == j {
                continue;
            }
            let (a1, a2) = seg(i);
            let (b1, b2) = seg(j);
            if segments_cross(a1, a2, b1, b2) {
                return true;
            }
        }
    }
    false
}

fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn segments_cross(a1: [f64; 2], a2: [f64; 2], b1: [f64; 2], b2: [f64; 2]) -> bool {
    let d1 = orient(a1, a2, b1);
    let d2 = orient(a1, a2, b2);
    let d3 = orient(b1, b2, a1);
    let d4 = orient(b1, b2, a2);
    (d1 * d2 < 0.0) && (d3 * d4 < 0.0)
}

/// Ear-clipping triangulation of a simple polygon in positive-area order. Returns index triples in
/// polygon order (facing −Y when the polygon lies in the ground plane as `(x, z)`).
fn ear_clip(pts: &[[f64; 2]]) -> Result<Vec<[u32; 3]>, ShapeError> {
    let mut idx: Vec<u32> = (0..pts.len() as u32).collect();
    let mut tris: Vec<[u32; 3]> = Vec::with_capacity(pts.len().saturating_sub(2));
    let mut guard = 0usize;
    while idx.len() > 3 {
        let n = idx.len();
        let mut clipped = false;
        for k in 0..n {
            let ia = idx[(k + n - 1) % n];
            let ib = idx[k];
            let ic = idx[(k + 1) % n];
            let (a, b, c) = (pts[ia as usize], pts[ib as usize], pts[ic as usize]);
            if orient(a, b, c) <= 1e-12 {
                continue; // reflex or collinear corner — not an ear
            }
            let mut contains = false;
            for &other in &idx {
                if other == ia || other == ib || other == ic {
                    continue;
                }
                let q = pts[other as usize];
                if orient(a, b, q) >= -1e-12
                    && orient(b, c, q) >= -1e-12
                    && orient(c, a, q) >= -1e-12
                {
                    contains = true;
                    break;
                }
            }
            if contains {
                continue;
            }
            tris.push([ia, ib, ic]);
            idx.remove(k);
            clipped = true;
            break;
        }
        if !clipped {
            return Err(ShapeError::BadProfile(
                "the outline could not be filled — simplify the drawing".to_string(),
            ));
        }
        guard += 1;
        if guard > MAX_PROFILE_POINTS * 2 {
            return Err(ShapeError::BadProfile(
                "the outline could not be filled — simplify the drawing".to_string(),
            ));
        }
    }
    tris.push([idx[0], idx[1], idx[2]]);
    Ok(tris)
}

/// Extrude a drawn ground-plan outline `(x, z)` up by `height`. `taper` scales the top face about
/// the outline's area centroid (1 = straight walls, smaller = a pyramidal frustum) — uniform scaling
/// about any interior point keeps a simple polygon simple, so the walls never self-cross.
fn build_extrude(recipe: &ShapeRecipe) -> Result<TriMesh, ShapeError> {
    let h = dim(recipe, "height", 1.0, 0.02, 50.0, "height")?;
    let taper = {
        let t = recipe.param("taper", 1.0);
        require(
            t.is_finite() && (0.05..=1.0).contains(&t),
            "taper must be between 0.05 and 1",
        )?;
        t
    };
    let pts = clean_profile(&recipe.profile)?;
    let n = pts.len();
    let cap = ear_clip(&pts)?;
    let centroid = polygon_centroid(&pts);
    let mut positions: Vec<[f64; 3]> = Vec::with_capacity(n * 2);
    for point in &pts {
        positions.push([point[0], 0.0, point[1]]);
    }
    for point in &pts {
        positions.push([
            centroid[0] + (point[0] - centroid[0]) * taper,
            h,
            centroid[1] + (point[1] - centroid[1]) * taper,
        ]);
    }
    let mut triangles: Vec<[u32; 3]> = Vec::with_capacity(cap.len() * 2 + n * 2);
    // Positive-area (x, z) polygon order faces −Y (checked in the module tests): bottom cap as-is,
    // top cap reversed.
    for t in &cap {
        triangles.push([t[0], t[1], t[2]]);
    }
    for t in &cap {
        triangles.push([t[2] + n as u32, t[1] + n as u32, t[0] + n as u32]);
    }
    for i in 0..n {
        let j = (i + 1) % n;
        let (bi, bj) = (i as u32, j as u32);
        let (ti, tj) = (bi + n as u32, bj + n as u32);
        triangles.push([bi, ti, tj]);
        triangles.push([bi, tj, bj]);
    }
    Ok(TriMesh::new(positions, triangles))
}

/// Spin a drawn side-profile `(radius, y)` around the vertical axis. The profile must stay off the
/// axis — the gap becomes the hole through the middle (a doughnut/vase-wall topology, honest and
/// watertight).
fn build_revolve(recipe: &ShapeRecipe) -> Result<TriMesh, ShapeError> {
    let seg = segs(recipe, "segments", 48.0, 8, 128)?;
    let pts = clean_profile(&recipe.profile)?;
    for point in &pts {
        require_outline(
            point[0] >= MIN_REVOLVE_R,
            "keep the outline a little to the right of the centre line — it spins around that axis",
        )?;
    }
    let m = pts.len();
    let mut positions: Vec<[f64; 3]> = Vec::with_capacity(m * seg);
    for j in 0..seg {
        let u = TAU * j as f64 / seg as f64;
        for point in &pts {
            positions.push([point[0] * u.cos(), point[1], point[0] * u.sin()]);
        }
    }
    let vid = |i: usize, j: usize| ((j % seg) * m + (i % m)) as u32;
    let mut triangles: Vec<[u32; 3]> = Vec::with_capacity(m * seg * 2);
    for i in 0..m {
        for j in 0..seg {
            let a = vid(i, j);
            let b = vid(i + 1, j);
            let c = vid(i + 1, j + 1);
            let d = vid(i, j + 1);
            triangles.push([a, b, c]);
            triangles.push([a, c, d]);
        }
    }
    Ok(TriMesh::new(positions, triangles))
}

// ============================================================================================
// Identity + artifact — the recipe IS the durable unit (mesh-form for combine results)
// ============================================================================================

/// What a Shape Studio command reports back to the UI. Exactly one of `created`/`reason` is the
/// story: `created` = the entity landed (placed + selected is the UI's job); `reason` = a
/// plain-language refusal with NO scene change. `message` is the friendly summary either way.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapeReply {
    /// The created/updated entity id, when the operation landed.
    pub created: Option<String>,
    /// The content-addressed mesh handle, when one was baked.
    pub handle: Option<String>,
    /// Triangles in the baked mesh (the legible build fact).
    pub triangles: usize,
    /// Wall-clock build time in milliseconds (measured, not badged).
    pub ms: f64,
    /// Friendly one-line summary of what happened.
    pub message: String,
    /// Plain-language refusal — set iff nothing changed.
    pub reason: Option<String>,
}

impl ShapeReply {
    /// A refusal that changed nothing, explained in the author's language.
    #[must_use]
    pub fn refusal(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self {
            message: reason.clone(),
            reason: Some(reason),
            ..Self::default()
        }
    }
}

/// A finished, persistable shape build: the recipe, its content-addressed handle, the durable artifact
/// bytes and the display-ready asset.
#[derive(Clone, Debug)]
pub struct ShapeBuild {
    /// The editable source.
    pub recipe: ShapeRecipe,
    /// Content address of `artifact_bytes` — the store/document handle.
    pub handle: String,
    /// The durable blob (`MTKSHAP1` + form + payload).
    pub artifact_bytes: Vec<u8>,
    /// The display-ready mesh asset (satin per-kind material).
    pub asset: MeshAsset,
    /// Triangle count (the build fact the UI reports).
    pub triangles: usize,
}

/// Mesh-form artifact payload: a combine/meld result whose geometry is not re-derivable from
/// parameters alone. Bincode-encoded (bit-exact f32) behind the magic + form byte.
#[derive(Serialize, Deserialize)]
struct MeshFormPayload {
    recipe_json: String,
    positions: Vec<[f32; 3]>,
    indices: Vec<u32>,
    base_color: [f32; 4],
    metallic: f32,
    roughness: f32,
}

fn mesh_to_asset(mesh: &TriMesh, name: &str, material: Material) -> MeshAsset {
    let positions: Vec<[f32; 3]> = mesh
        .positions
        .iter()
        .map(|position| [position[0] as f32, position[1] as f32, position[2] as f32])
        .collect();
    let indices: Vec<u32> = mesh
        .triangles
        .iter()
        .flat_map(|t| t.iter().copied())
        .collect();
    MeshAsset {
        name: name.to_string(),
        primitives: vec![Primitive {
            positions,
            normals: Vec::new(),
            uvs: Vec::new(),
            tangents: Vec::new(),
            indices,
            material: 0,
            joints: Vec::new(),
            weights: Vec::new(),
        }],
        materials: vec![material],
        textures: Vec::new(),
        skeleton: None,
    }
}

/// Bake a parametric/drawn recipe into a persistable build. The artifact is the **recipe itself**
/// (form 1): a missing mesh blob rebuilds deterministically from it on load, and the handle is the
/// content address of the recipe bytes — same recipe ⇒ same handle, different colour/kind ⇒ different
/// handle (no cross-shape collision).
///
/// # Errors
/// Any [`ShapeError`] from [`build_shape`].
pub fn bake_shape(recipe: &ShapeRecipe) -> Result<ShapeBuild, ShapeError> {
    let mesh = build_shape(recipe)?;
    let mut artifact_bytes = Vec::with_capacity(64 + recipe.to_json().len());
    artifact_bytes.extend_from_slice(SHAPE_ARTIFACT_MAGIC);
    artifact_bytes.push(FORM_RECIPE);
    artifact_bytes.extend_from_slice(recipe.to_json().as_bytes());
    let handle = AssetId::of_bytes(&artifact_bytes).as_str().to_string();
    let asset = mesh_to_asset(
        &mesh,
        &format!("shape:{}", recipe.kind),
        shape_material(&recipe.kind),
    );
    let triangles = mesh.triangles.len();
    Ok(ShapeBuild {
        recipe: recipe.clone(),
        handle,
        artifact_bytes,
        asset,
        triangles,
    })
}

/// Bake a mesh-level result (combine/meld) into a persistable **mesh-form** build (form 2): the
/// geometry itself is the payload, since it is not re-derivable from the recipe.
#[must_use]
pub fn bake_mesh_result(recipe: &ShapeRecipe, mesh: &TriMesh, material: Material) -> ShapeBuild {
    let payload = MeshFormPayload {
        recipe_json: recipe.to_json(),
        positions: mesh
            .positions
            .iter()
            .map(|position| [position[0] as f32, position[1] as f32, position[2] as f32])
            .collect(),
        indices: mesh
            .triangles
            .iter()
            .flat_map(|t| t.iter().copied())
            .collect(),
        base_color: material.base_color,
        metallic: material.metallic,
        roughness: material.roughness,
    };
    let encoded = bincode::serialize(&payload).unwrap_or_default();
    let mut artifact_bytes = Vec::with_capacity(9 + encoded.len());
    artifact_bytes.extend_from_slice(SHAPE_ARTIFACT_MAGIC);
    artifact_bytes.push(FORM_MESH);
    artifact_bytes.extend_from_slice(&encoded);
    let handle = AssetId::of_bytes(&artifact_bytes).as_str().to_string();
    let asset = mesh_to_asset(mesh, &format!("shape:{}", recipe.kind), material);
    let triangles = mesh.triangles.len();
    ShapeBuild {
        recipe: recipe.clone(),
        handle,
        artifact_bytes,
        asset,
        triangles,
    }
}

/// Bake a meld field (the SDF smooth-union of two shape entities, in world space) into a mesh-form
/// build: compile by Marching Tetrahedra at `res`, validate, recentre, and return the build plus the
/// world centre the result entity lands at.
///
/// # Errors
/// [`ShapeError::Blocked`] when the compiled field is not a clean solid (never a silent crack).
pub fn bake_meld(
    recipe: &ShapeRecipe,
    field: &Sdf,
    res: usize,
    material: Material,
) -> Result<(ShapeBuild, [f64; 3], f64), ShapeError> {
    let grid = metrocalk_sdf::Grid::around(field, res, 0.06);
    let mesh = metrocalk_sdf::compile(field, &grid);
    if mesh.triangles.is_empty() {
        return Err(ShapeError::Blocked(
            "the result came out empty — move the shapes so they touch or overlap".to_string(),
        ));
    }
    let report = validate(&mesh);
    if !(report.watertight && report.manifold && report.oriented) {
        return Err(ShapeError::Blocked(format!(
            "the meld did not produce a clean solid: {}",
            report.explain()
        )));
    }
    let (local, centre) = recentre(&mesh);
    // The honest number (§4, never "lossless"): curved surfaces deviate from this bake by at most
    // roughly the grid's chord tolerance — returned so the reply can SAY it.
    Ok((
        bake_mesh_result(recipe, &local, material),
        centre,
        grid.chord_tolerance(),
    ))
}

/// Decode a persisted blob if it is a shape artifact. `Ok(None)` = not ours (the loader tries the next
/// format); `Err` = ours but corrupt (skipped loudly, never trusted).
///
/// # Errors
/// A plain-language reason when the artifact is recognisably ours but unusable.
pub fn decode_shape_artifact(bytes: &[u8]) -> Result<Option<(ShapeRecipe, MeshAsset)>, String> {
    if bytes.len() < SHAPE_ARTIFACT_MAGIC.len() + 1 || &bytes[..8] != SHAPE_ARTIFACT_MAGIC {
        return Ok(None);
    }
    if bytes.len() > MAX_ARTIFACT_BYTES {
        return Err("shape artifact is implausibly large".to_string());
    }
    let form = bytes[8];
    let payload = &bytes[9..];
    match form {
        FORM_RECIPE => {
            let json = std::str::from_utf8(payload)
                .map_err(|_| "shape recipe artifact is not UTF-8".to_string())?;
            let recipe = ShapeRecipe::from_json(json).map_err(|e| e.to_string())?;
            let mesh = build_shape(&recipe).map_err(|e| e.to_string())?;
            let asset = mesh_to_asset(
                &mesh,
                &format!("shape:{}", recipe.kind),
                shape_material(&recipe.kind),
            );
            Ok(Some((recipe, asset)))
        }
        FORM_MESH => {
            let decoded: MeshFormPayload = bincode::deserialize(payload)
                .map_err(|e| format!("shape mesh artifact did not decode: {e}"))?;
            if !decoded.indices.len().is_multiple_of(3)
                || decoded
                    .indices
                    .iter()
                    .any(|&i| i as usize >= decoded.positions.len())
            {
                return Err("shape mesh artifact indices are out of bounds".to_string());
            }
            if decoded
                .positions
                .iter()
                .any(|p| !p.iter().all(|c| c.is_finite()))
            {
                return Err("shape mesh artifact contains non-finite vertices".to_string());
            }
            let recipe = ShapeRecipe::from_json(&decoded.recipe_json).map_err(|e| e.to_string())?;
            let material = Material {
                base_color: decoded.base_color,
                metallic: decoded.metallic,
                roughness: decoded.roughness,
                ..Material::default()
            };
            let asset = MeshAsset {
                name: format!("shape:{}", recipe.kind),
                primitives: vec![Primitive {
                    positions: decoded.positions,
                    normals: Vec::new(),
                    uvs: Vec::new(),
                    tangents: Vec::new(),
                    indices: decoded.indices,
                    material: 0,
                    joints: Vec::new(),
                    weights: Vec::new(),
                }],
                materials: vec![material],
                textures: Vec::new(),
                skeleton: None,
            };
            Ok(Some((recipe, asset)))
        }
        other => Err(format!("unknown shape artifact form {other}")),
    }
}

// ============================================================================================
// World-space transforms — what Combine applies before the exact boolean
// ============================================================================================

/// Apply (in order) a display affine (`v·scale + translation` — how the renderer normalises fixture
/// assets), then a full TRS (uniform scale, quaternion rotate, translate) to a mesh, in `f64`. This is
/// exactly the transform the viewport applies, so a boolean of two entities operates on the shapes the
/// author is actually looking at.
#[must_use]
pub fn transform_mesh_world(
    mesh: &TriMesh,
    affine_scale: f64,
    affine_translation: [f64; 3],
    scale: f64,
    rotation: [f64; 4],
    translation: [f64; 3],
) -> TriMesh {
    let [qx, qy, qz, qw] = rotation;
    let rotate = |v: [f64; 3]| -> [f64; 3] {
        // q * v * q⁻¹ via the standard expansion: t = 2·(q.xyz × v); v' = v + qw·t + q.xyz × t.
        let (ux, uy, uz) = (qx, qy, qz);
        let tx = 2.0 * (uy * v[2] - uz * v[1]);
        let ty = 2.0 * (uz * v[0] - ux * v[2]);
        let tz = 2.0 * (ux * v[1] - uy * v[0]);
        [
            v[0] + qw * tx + (uy * tz - uz * ty),
            v[1] + qw * ty + (uz * tx - ux * tz),
            v[2] + qw * tz + (ux * ty - uy * tx),
        ]
    };
    let positions = mesh
        .positions
        .iter()
        .map(|position| {
            let local = [
                position[0] * affine_scale + affine_translation[0],
                position[1] * affine_scale + affine_translation[1],
                position[2] * affine_scale + affine_translation[2],
            ];
            let scaled = [local[0] * scale, local[1] * scale, local[2] * scale];
            let rotated = rotate(scaled);
            [
                rotated[0] + translation[0],
                rotated[1] + translation[1],
                rotated[2] + translation[2],
            ]
        })
        .collect();
    TriMesh {
        positions,
        triangles: mesh.triangles.clone(),
    }
}

/// Recentre a world-space result on its bounding-box centre; returns `(local mesh, world centre)` —
/// the landing position of the result entity.
#[must_use]
pub fn recentre(mesh: &TriMesh) -> (TriMesh, [f64; 3]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for position in &mesh.positions {
        for k in 0..3 {
            lo[k] = lo[k].min(position[k]);
            hi[k] = hi[k].max(position[k]);
        }
    }
    if !lo[0].is_finite() {
        return (mesh.clone(), [0.0; 3]);
    }
    let centre = [
        f64::midpoint(lo[0], hi[0]),
        f64::midpoint(lo[1], hi[1]),
        f64::midpoint(lo[2], hi[2]),
    ];
    let positions = mesh
        .positions
        .iter()
        .map(|position| {
            [
                position[0] - centre[0],
                position[1] - centre[1],
                position[2] - centre[2],
            ]
        })
        .collect();
    (
        TriMesh {
            positions,
            triangles: mesh.triangles.clone(),
        },
        centre,
    )
}

// ============================================================================================
// Combine — the exact-predicate boolean, behind the lib boundary (no csg type crosses to the app)
// ============================================================================================

/// Normalise a combine verb to its recipe kind, or `None` for an unknown verb.
#[must_use]
pub fn combine_kind(op_verb: &str) -> Option<&'static str> {
    match op_verb {
        "union" => Some("union"),
        "carve" | "difference" | "subtract" => Some("carve"),
        "intersect" | "intersection" => Some("intersect"),
        _ => None,
    }
}

/// The friendly name a combine result carries ("Union", "Carved shape", "Intersection").
#[must_use]
pub fn combine_label(kind: &str) -> &'static str {
    match kind {
        "union" => "Union",
        "carve" => "Carved shape",
        "intersect" => "Intersection",
        "meld" => "Melded shape",
        _ => "Combined shape",
    }
}

/// Run the exact-predicate boolean the Combine verbs use (weld tolerance matched to the f32 asset
/// store, the `csg_intent::carve` rationale). Validator-gated: a degenerate result is a
/// plain-language refusal, never a silent crack.
///
/// # Errors
/// [`ShapeError::BadParam`] for an unknown verb; [`ShapeError::Blocked`] when the boolean refuses.
pub fn combine_meshes(a: &TriMesh, b: &TriMesh, op_verb: &str) -> Result<TriMesh, ShapeError> {
    use metrocalk_csg::Csg;
    let kind = combine_kind(op_verb)
        .ok_or_else(|| ShapeError::BadParam(format!("there is no combine called \"{op_verb}\"")))?;
    let op = match kind {
        "union" => metrocalk_csg::BoolOp::Union,
        "carve" => metrocalk_csg::BoolOp::Difference,
        _ => metrocalk_csg::BoolOp::Intersection,
    };
    // The BSP boolean recurses per split plane, and on curved shapes (a default sphere is ~1,000
    // triangles) the tree goes deep enough to blow a default 1–2 MB thread stack — the first live
    // run of Shape Studio died exactly there (STATUS_STACK_OVERFLOW), which killed the whole editor
    // because a stack overflow aborts the process. So the boolean runs on a dedicated worker with a
    // deep stack RESERVATION (pages commit only as they are used), and a panic inside the csg crate
    // becomes a plain-language refusal instead of taking the engine thread down.
    let (ta, tb) = (a.clone(), b.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("shape-combine-bsp".into())
        .stack_size(256 * 1024 * 1024)
        .spawn(move || {
            let engine_csg = metrocalk_csg::ExactBspCsg { weld_rel: 1e-6 };
            let _ = tx.send(engine_csg.boolean(&ta, &tb, op));
        })
        .map_err(|e| ShapeError::Blocked(format!("the combine could not start: {e}")))?;
    // A bounded wait, not a join: past two minutes the answer is a truthful refusal and the scene is
    // NEVER mutated afterwards (the worker's late result is dropped with the channel — the "reason set
    // iff nothing changed" contract survives; the fragment budget bounds the stray worker's lifetime).
    let out = rx
        .recv_timeout(std::time::Duration::from_mins(2))
        .map_err(|_| {
            ShapeError::Blocked(
                "the combine took longer than two minutes — lower the shapes' smoothness and try again"
                    .to_string(),
            )
        })?
        .map_err(|e| ShapeError::Blocked(e.to_string()))?;
    // An empty boolean is Ok(empty) from the csg crate (a legitimately empty intersection) — but landing
    // it would silently DELETE both sources and place an invisible entity. Refuse in plain language,
    // exactly as the field path does.
    if out.triangles.is_empty() {
        return Err(ShapeError::Blocked(
            "the result came out empty — move the shapes so they touch or overlap".to_string(),
        ));
    }
    Ok(out)
}

/// The renderer's placeholder cube (what an entity with no real mesh displays), as geometry — so
/// Combine operates on exactly what the author is looking at, even for placeholder entities.
#[must_use]
pub fn placeholder_cube() -> TriMesh {
    metrocalk_csg::box_mesh([0.0, 0.0, 0.0], [0.5, 0.5, 0.5])
}

// ============================================================================================
// Meld — two shape recipes → one smooth SDF blob
// ============================================================================================

/// Build the SDF program for a meld-able shape at a world position, or explain why it can't meld.
/// Meld runs on the *field* representation, which exists for unrotated spheres, boxes and upright
/// cylinders/prisms — everything else has an exact-mesh Combine instead.
///
/// # Errors
/// A plain-language reason naming the entity when its kind or pose can't be expressed as a field.
pub fn meld_field(
    recipe: &ShapeRecipe,
    name: &str,
    translation: [f64; 3],
    rotation: [f64; 4],
    scale: f64,
) -> Result<Sdf, ShapeError> {
    let upright = rotation[0].abs() < 1e-6
        && rotation[1].abs() < 1e-6
        && rotation[2].abs() < 1e-6
        && (rotation[3].abs() - 1.0).abs() < 1e-6;
    let s = if scale > 0.0 { scale } else { 1.0 };
    // Builders rest shapes on y = 0, so the entity origin is the shape's FLOOR point — the centre sits
    // above it, and the entity's rotation swings that offset around the origin exactly as the renderer
    // does (world = translation + R·local).
    let rotate = |v: [f64; 3]| -> [f64; 3] {
        let [qx, qy, qz, qw] = rotation;
        let t = [
            2.0 * (qy * v[2] - qz * v[1]),
            2.0 * (qz * v[0] - qx * v[2]),
            2.0 * (qx * v[1] - qy * v[0]),
        ];
        [
            v[0] + qw * t[0] + (qy * t[2] - qz * t[1]),
            v[1] + qw * t[1] + (qz * t[0] - qx * t[2]),
            v[2] + qw * t[2] + (qx * t[1] - qy * t[0]),
        ]
    };
    match recipe.kind.as_str() {
        // A sphere is rotation-symmetric about its own centre, so ANY rotation melds exactly — but the
        // centre offset from the floor origin must ride the rotation, or the field sits up to a full
        // diameter away from the ball on screen.
        "sphere" => {
            let r = recipe.param("radius", 0.5) * s;
            let centre = rotate([0.0, r, 0.0]);
            Ok(Sdf::sphere(
                [
                    translation[0] + centre[0],
                    translation[1] + centre[1],
                    translation[2] + centre[2],
                ],
                r,
            ))
        }
        "box" if upright => {
            let half = [
                recipe.param("width", 1.0) * s / 2.0,
                recipe.param("height", 1.0) * s / 2.0,
                recipe.param("depth", 1.0) * s / 2.0,
            ];
            Ok(Sdf::cuboid(
                [translation[0], translation[1] + half[1], translation[2]],
                half,
            ))
        }
        // NOT "prism": a prism's flat sides are not a round cylinder — melding it as one would silently
        // swap the on-screen faceted solid for a circumscribed round column. Prisms combine exactly
        // through the mesh boolean instead.
        "cylinder" if upright => {
            let r = recipe.param("radius", 0.5) * s;
            let h = recipe.param("height", 1.0) * s;
            Ok(Sdf::cylinder(
                [translation[0], translation[1] + h / 2.0, translation[2]],
                r,
                h / 2.0,
                Axis::Y,
            ))
        }
        "box" | "cylinder" => Err(ShapeError::Blocked(format!(
            "Meld needs \"{name}\" unrotated — reset its rotation, or use Combine instead"
        ))),
        other => Err(ShapeError::Blocked(format!(
            "Meld works on spheres, boxes and cylinders for now — \"{name}\" is a {other}. Use Combine instead"
        ))),
    }
}

// ============================================================================================
// Landing — one undoable scene transaction (the ADR-036 discipline)
// ============================================================================================

/// What a landing commit needs to know about a build — plain values, so a replayed record (which has
/// no [`ShapeBuild`], only the recipe + handle) lands through the exact same path as a live bake.
#[derive(Clone, Debug)]
pub struct ShapeLanding {
    /// Content-addressed mesh handle.
    pub handle: String,
    /// The editable source.
    pub recipe: ShapeRecipe,
    /// Triangle count (the inspectable build fact).
    pub triangles: usize,
}

impl ShapeBuild {
    /// The landing values for this build.
    #[must_use]
    pub fn landing(&self) -> ShapeLanding {
        ShapeLanding {
            handle: self.handle.clone(),
            recipe: self.recipe.clone(),
            triangles: self.triangles,
        }
    }
}

/// The scene ops for a shape entity's component payload (shared by create and combine landings).
fn shape_field_ops(id: EntityId, landing: &ShapeLanding, name: &str) -> Vec<Op> {
    let mut ops = Vec::new();
    for (component, field, value) in [
        (INSTANCE_META, NAME_FIELD, FieldValue::Str(name.to_string())),
        (
            "MeshRenderer",
            MESH_FIELD,
            FieldValue::Str(landing.handle.clone()),
        ),
        ("MeshRenderer", "castShadows", FieldValue::Bool(true)),
        (
            SHAPE_COMPONENT,
            "source",
            FieldValue::Str(landing.recipe.to_json()),
        ),
        (
            SHAPE_COMPONENT,
            "version",
            FieldValue::Integer(i64::from(SHAPE_RECIPE_VERSION)),
        ),
        (
            SHAPE_COMPONENT,
            "kind",
            FieldValue::Str(landing.recipe.kind.clone()),
        ),
        (
            SHAPE_COMPONENT,
            "triangles",
            FieldValue::Integer(i64::try_from(landing.triangles).unwrap_or(i64::MAX)),
        ),
    ] {
        ops.push(Op::SetField {
            entity: id,
            component: component.into(),
            field: field.into(),
            value,
        });
    }
    ops
}

/// Land a persisted shape build as ONE undoable scene transaction: named renderable entity + the
/// editable recipe. The artifact must already be durable (persist-first, commit-last — ADR-089).
///
/// # Errors
/// The commit's [`PipelineError`] (the transaction is all-or-nothing).
pub fn land_shape_asset(
    engine: &mut Engine<metrocalk_ecs::FlecsWorld>,
    scene: &CapScene,
    landing: &ShapeLanding,
    name: &str,
    pos: [f32; 3],
) -> Result<EntityId, PipelineError> {
    let id = engine.alloc_entity_id();
    let mut ops = vec![Op::CreateEntity { id, parent: None }];
    for (field, value) in [("x", pos[0]), ("y", pos[1]), ("z", pos[2]), ("scale", 1.0)] {
        ops.push(Op::SetField {
            entity: id,
            component: "Transform".into(),
            field: field.into(),
            value: FieldValue::Number(f64::from(value)),
        });
    }
    ops.extend(shape_field_ops(id, landing, name));
    for cap in ["Renderable", "ProceduralAsset"] {
        if let Some(&target) = scene.caps.get(&canonical(cap)) {
            ops.push(Op::AddPair {
                entity: id,
                rel: scene.rels.provides,
                target,
            });
        }
    }
    engine.commit("create-shape", ops)?;
    Ok(id)
}

/// Swap an existing shape entity's cooked mesh + editable recipe in one undoable commit (a parameter
/// edit). Deliberately no Transform op — where the author put it stays put; undo restores the prior
/// recipe/handle pair atomically.
///
/// # Errors
/// The commit's [`PipelineError`].
pub fn replace_shape_asset(
    engine: &mut Engine<metrocalk_ecs::FlecsWorld>,
    id: EntityId,
    landing: &ShapeLanding,
) -> Result<(), PipelineError> {
    let ops = [
        (
            "MeshRenderer",
            MESH_FIELD,
            FieldValue::Str(landing.handle.clone()),
        ),
        (
            SHAPE_COMPONENT,
            "source",
            FieldValue::Str(landing.recipe.to_json()),
        ),
        (
            SHAPE_COMPONENT,
            "kind",
            FieldValue::Str(landing.recipe.kind.clone()),
        ),
        (
            SHAPE_COMPONENT,
            "triangles",
            FieldValue::Integer(i64::try_from(landing.triangles).unwrap_or(i64::MAX)),
        ),
    ]
    .into_iter()
    .map(|(component, field, value)| Op::SetField {
        entity: id,
        component: component.into(),
        field: field.into(),
        value,
    })
    .collect();
    engine.commit("edit-shape", ops)
}

/// Land a combine/meld result AND retire its two sources in ONE undoable transaction: create the
/// result entity, then `DeleteEntity` both inputs (full-state capture makes a single Ctrl-Z restore
/// them and remove the result — the whole operation is one honest undo step).
///
/// # Errors
/// The commit's [`PipelineError`] (all-or-nothing — a failed commit leaves both sources untouched).
pub fn land_combined_asset(
    engine: &mut Engine<metrocalk_ecs::FlecsWorld>,
    scene: &CapScene,
    landing: &ShapeLanding,
    name: &str,
    pos: [f32; 3],
    retire: [EntityId; 2],
) -> Result<EntityId, PipelineError> {
    let id = engine.alloc_entity_id();
    let mut ops = vec![Op::CreateEntity { id, parent: None }];
    for (field, value) in [("x", pos[0]), ("y", pos[1]), ("z", pos[2]), ("scale", 1.0)] {
        ops.push(Op::SetField {
            entity: id,
            component: "Transform".into(),
            field: field.into(),
            value: FieldValue::Number(f64::from(value)),
        });
    }
    ops.extend(shape_field_ops(id, landing, name));
    for cap in ["Renderable", "ProceduralAsset"] {
        if let Some(&target) = scene.caps.get(&canonical(cap)) {
            ops.push(Op::AddPair {
                entity: id,
                rel: scene.rels.provides,
                target,
            });
        }
    }
    ops.push(Op::DeleteEntity { id: retire[0] });
    ops.push(Op::DeleteEntity { id: retire[1] });
    engine.commit("combine-shapes", ops)?;
    Ok(id)
}

/// The deterministic spawn scatter: slot `k` on a golden-angle spiral, so a run of quick spawns lands
/// as a readable arc instead of a stack of coincident shapes.
#[must_use]
pub fn spawn_spot(k: u32) -> [f32; 3] {
    if k == 0 {
        return [0.0, 0.0, 0.0];
    }
    let kf = f64::from(k);
    let r = 1.1 * kf.sqrt();
    let theta = kf * 2.399_963_229_728_653; // the golden angle, radians
    [(r * theta.cos()) as f32, 0.0, (r * theta.sin()) as f32]
}

// ============================================================================================
// Tests
// ============================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Signed volume via the divergence theorem — positive iff consistently outward-oriented.
    fn signed_volume(mesh: &TriMesh) -> f64 {
        let mut six_v = 0.0;
        for t in &mesh.triangles {
            let a = mesh.positions[t[0] as usize];
            let b = mesh.positions[t[1] as usize];
            let c = mesh.positions[t[2] as usize];
            six_v += a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]);
        }
        six_v / 6.0
    }

    fn recipe(kind: &str) -> ShapeRecipe {
        ShapeRecipe::parametric(kind).expect("kind exists in the catalog")
    }

    #[test]
    fn every_catalog_shape_builds_clean_outward_and_grounded() {
        for spec in shape_specs() {
            let mesh = build_shape(&recipe(spec.kind))
                .unwrap_or_else(|e| panic!("{} builds with its own defaults: {e}", spec.kind));
            let report = validate(&mesh);
            assert!(
                report.watertight && report.manifold && report.oriented,
                "{} is a clean solid: {}",
                spec.kind,
                report.explain()
            );
            assert!(
                signed_volume(&mesh) > 0.0,
                "{} is outward-oriented (positive volume)",
                spec.kind
            );
            let min_y = mesh
                .positions
                .iter()
                .map(|position| position[1])
                .fold(f64::INFINITY, f64::min);
            assert!(
                min_y.abs() < 1e-9,
                "{} rests on the ground plane (min y = {min_y})",
                spec.kind
            );
            assert!(mesh.triangles.len() >= 8, "{} is a real surface", spec.kind);
        }
    }

    #[test]
    fn shape_volumes_match_the_analytic_solid() {
        // The box is exact; the round shapes converge from below (inscribed facets).
        let b = build_shape(&recipe("box")).unwrap();
        assert!(
            (signed_volume(&b) - 1.0).abs() < 1e-9,
            "unit box volume is 1"
        );

        let s = build_shape(&recipe("sphere")).unwrap();
        let exact = 4.0 / 3.0 * PI * 0.5f64.powi(3);
        let v = signed_volume(&s);
        assert!(
            v > exact * 0.97 && v < exact * 1.001,
            "sphere volume ≈ {exact}, got {v}"
        );

        let c = build_shape(&recipe("cylinder")).unwrap();
        let exact = PI * 0.5f64.powi(2);
        let v = signed_volume(&c);
        assert!(
            v > exact * 0.97 && v < exact * 1.001,
            "cylinder volume ≈ {exact}, got {v}"
        );

        let t = build_shape(&recipe("torus")).unwrap();
        let exact = TAU * 0.5 * PI * 0.18f64.powi(2); // 2πR · πr²
        let v = signed_volume(&t);
        assert!(
            v > exact * 0.93 && v < exact * 1.001,
            "torus volume ≈ {exact}, got {v}"
        );
    }

    #[test]
    fn a_tube_is_a_watertight_ring_solid_with_the_analytic_volume() {
        let mesh = build_shape(&recipe("tube")).unwrap();
        let report = validate(&mesh);
        assert!(
            report.watertight && report.manifold && report.oriented,
            "{}",
            report.explain()
        );
        assert_eq!(report.genus, Some(1), "a tube has one hole through it");
        // V = π·h·(R² − r²), faceted → converges from below (defaults: R=0.5, wall=0.12, h=1).
        let exact = PI * (0.5f64.powi(2) - 0.38f64.powi(2));
        let v = signed_volume(&mesh);
        assert!(
            v > exact * 0.97 && v < exact * 1.001,
            "tube volume ≈ {exact}, got {v}"
        );
    }

    #[test]
    fn a_tapered_extrude_is_a_frustum_with_the_prismatoid_volume() {
        let mut r = ShapeRecipe {
            v: SHAPE_RECIPE_VERSION,
            kind: "extrude".into(),
            params: [("height".to_string(), 1.0), ("taper".to_string(), 0.5)]
                .into_iter()
                .collect(),
            profile: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            sources: Vec::new(),
        };
        let mesh = build_shape(&r).expect("tapers");
        let report = validate(&mesh);
        assert!(report.watertight && report.oriented, "{}", report.explain());
        // Prismatoid: V = h/6·(A₁ + 4·Aₘ + A₂) = (1 + 4·0.5625 + 0.25)/6 = 0.583̅ for a unit square
        // tapered to half size.
        let v = signed_volume(&mesh);
        assert!((v - 3.5 / 6.0).abs() < 1e-9, "frustum volume, got {v}");

        // A concave outline (the L) stays simple under taper — uniform scaling preserves simplicity.
        r.profile = vec![
            [0.0, 0.0],
            [2.0, 0.0],
            [2.0, 1.0],
            [1.0, 1.0],
            [1.0, 2.0],
            [0.0, 2.0],
        ];
        let l_mesh = build_shape(&r).expect("the concave L tapers");
        assert!(validate(&l_mesh).watertight, "concave taper stays clean");

        // Out-of-range taper is refused by name.
        r.params.insert("taper".into(), 0.0);
        let msg = build_shape(&r).expect_err("refused").to_string();
        assert!(msg.contains("taper"), "{msg}");
    }

    #[test]
    fn a_recipe_is_bit_deterministic_and_content_addressed() {
        let r = recipe("torus");
        let m1 = build_shape(&r).unwrap();
        let m2 = build_shape(&r).unwrap();
        assert_eq!(
            m1.content_hash(),
            m2.content_hash(),
            "same recipe, same bits"
        );
        let b1 = bake_shape(&r).unwrap();
        let b2 = bake_shape(&r).unwrap();
        assert_eq!(b1.handle, b2.handle, "same recipe, same handle");
        // A different parameter is a different artifact.
        let mut r2 = r;
        r2.params.insert("radius".into(), 0.7);
        assert_ne!(bake_shape(&r2).unwrap().handle, b1.handle);
    }

    #[test]
    fn same_geometry_different_kind_keeps_distinct_handles() {
        // Identity is the RECIPE, not the geometry — two same-size shapes in different colours
        // (kinds) must not collide on one handle (the store would repaint one of them).
        let mut a = recipe("cylinder");
        a.params.insert("segments".into(), 6.0);
        let mut b = recipe("prism");
        b.params.insert("sides".into(), 6.0);
        let (ba, bb) = (bake_shape(&a).unwrap(), bake_shape(&b).unwrap());
        assert_ne!(ba.handle, bb.handle, "recipe identity keeps them apart");
    }

    #[test]
    fn the_artifact_round_trips_and_rebuilds_the_same_mesh() {
        let r = recipe("capsule");
        let build = bake_shape(&r).unwrap();
        let (decoded_recipe, decoded_asset) = decode_shape_artifact(&build.artifact_bytes)
            .expect("decodes")
            .expect("is a shape artifact");
        assert_eq!(decoded_recipe, r, "the recipe survives the round trip");
        assert_eq!(
            decoded_asset.primitives[0].positions, build.asset.primitives[0].positions,
            "the rebuilt mesh is identical (recipe-form artifacts derive, deterministically)"
        );
        // Foreign bytes are not ours; corrupt shape bytes are refused loudly.
        assert!(decode_shape_artifact(b"GLBxxxx").unwrap().is_none());
        let mut corrupt = build.artifact_bytes.clone();
        let end = corrupt.len() - 1;
        corrupt[end] = b'!';
        assert!(
            decode_shape_artifact(&corrupt).is_err(),
            "corrupt recipe is refused"
        );
    }

    #[test]
    fn mesh_form_artifact_round_trips_geometry_and_material() {
        let r = ShapeRecipe {
            v: SHAPE_RECIPE_VERSION,
            kind: "union".into(),
            params: BTreeMap::new(),
            profile: Vec::new(),
            sources: vec!["a".into(), "b".into()],
        };
        let mesh = metrocalk_csg::box_mesh([0.0, 0.5, 0.0], [0.5, 0.5, 0.5]);
        let build = bake_mesh_result(&r, &mesh, shape_material("union"));
        let (decoded_recipe, decoded_asset) = decode_shape_artifact(&build.artifact_bytes)
            .expect("decodes")
            .expect("is a shape artifact");
        assert_eq!(decoded_recipe.kind, "union");
        assert_eq!(
            decoded_recipe.sources,
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(
            decoded_asset.primitives[0].positions.len(),
            mesh.positions.len()
        );
        // Bit-exact material round-trip (compare bit patterns — clippy rightly refuses float ==).
        assert_eq!(
            decoded_asset.materials[0].base_color.map(f32::to_bits),
            shape_material("union").base_color.map(f32::to_bits)
        );
    }

    #[test]
    fn extrude_fills_a_concave_outline() {
        // An L-shape: concave, a real ear-clipping case.
        let r = ShapeRecipe {
            v: SHAPE_RECIPE_VERSION,
            kind: "extrude".into(),
            params: [("height".to_string(), 0.5)].into_iter().collect(),
            profile: vec![
                [0.0, 0.0],
                [2.0, 0.0],
                [2.0, 1.0],
                [1.0, 1.0],
                [1.0, 2.0],
                [0.0, 2.0],
            ],
            sources: Vec::new(),
        };
        let mesh = build_shape(&r).expect("the L extrudes");
        let report = validate(&mesh);
        assert!(report.watertight && report.oriented, "{}", report.explain());
        // L area = 3 m², height 0.5 ⇒ volume 1.5 m³ exactly (planar faces are exact).
        assert!((signed_volume(&mesh) - 1.5).abs() < 1e-9);
    }

    #[test]
    fn extrude_accepts_either_drawing_direction() {
        let ccw = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let cw: Vec<[f64; 2]> = ccw.iter().rev().copied().collect();
        for profile in [ccw, cw] {
            let r = ShapeRecipe {
                v: SHAPE_RECIPE_VERSION,
                kind: "extrude".into(),
                params: [("height".to_string(), 1.0)].into_iter().collect(),
                profile,
                sources: Vec::new(),
            };
            let mesh = build_shape(&r).expect("both directions extrude");
            assert!(
                (signed_volume(&mesh) - 1.0).abs() < 1e-9,
                "unit square → unit cube"
            );
        }
    }

    #[test]
    fn bad_outlines_are_refused_in_plain_language() {
        let base = |profile: Vec<[f64; 2]>| ShapeRecipe {
            v: SHAPE_RECIPE_VERSION,
            kind: "extrude".into(),
            params: BTreeMap::new(),
            profile,
            sources: Vec::new(),
        };
        let two = build_shape(&base(vec![[0.0, 0.0], [1.0, 0.0]]));
        assert!(matches!(two, Err(ShapeError::BadProfile(_))), "{two:?}");

        // A bow-tie crosses itself.
        let bowtie = build_shape(&base(vec![[0.0, 0.0], [1.0, 1.0], [1.0, 0.0], [0.0, 1.0]]));
        let msg = bowtie.expect_err("refused").to_string();
        assert!(msg.contains("crosses itself"), "explained: {msg}");

        // A revolve profile touching the axis is explained, not crashed.
        let mut rev = base(vec![[0.0, 0.0], [0.5, 0.0], [0.5, 0.5], [0.0, 0.5]]);
        rev.kind = "revolve".into();
        let msg = build_shape(&rev).expect_err("refused").to_string();
        assert!(msg.contains("centre line"), "explained: {msg}");
    }

    #[test]
    fn revolve_makes_a_watertight_ring_solid() {
        let r = ShapeRecipe {
            v: SHAPE_RECIPE_VERSION,
            kind: "revolve".into(),
            params: [("segments".to_string(), 48.0)].into_iter().collect(),
            profile: vec![[0.6, 0.0], [1.0, 0.0], [1.0, 0.4], [0.6, 0.4]],
            sources: Vec::new(),
        };
        let mesh = build_shape(&r).expect("the profile revolves");
        let report = validate(&mesh);
        assert!(
            report.watertight && report.manifold && report.oriented,
            "{}",
            report.explain()
        );
        assert_eq!(
            report.genus,
            Some(1),
            "a revolved off-axis profile is a ring"
        );
        // Pappus: V = 2π·centroid_r·A = 2π·0.8·(0.4·0.4). Faceted → converges from below.
        let exact = TAU * 0.8 * 0.16;
        let v = signed_volume(&mesh);
        assert!(
            v > exact * 0.97 && v < exact * 1.001,
            "ring volume ≈ {exact}, got {v}"
        );
    }

    #[test]
    fn out_of_range_parameters_are_refused_not_clamped_silently() {
        let mut r = recipe("sphere");
        r.params.insert("radius".into(), -1.0);
        let msg = build_shape(&r).expect_err("refused").to_string();
        assert!(msg.contains("radius"), "names the parameter: {msg}");

        let mut r = recipe("capsule");
        r.params.insert("height".into(), 0.3); // < 2r
        assert!(build_shape(&r).is_err(), "impossible capsule refused");

        let unknown = ShapeRecipe {
            v: SHAPE_RECIPE_VERSION,
            kind: "dodecahedron".into(),
            params: BTreeMap::new(),
            profile: Vec::new(),
            sources: Vec::new(),
        };
        let msg = build_shape(&unknown).expect_err("refused").to_string();
        assert!(msg.contains("dodecahedron"), "names the kind: {msg}");
    }

    #[test]
    fn world_transform_rotates_scales_and_translates() {
        let unit = metrocalk_csg::box_mesh([0.0, 0.0, 0.0], [0.5, 0.5, 0.5]);
        // 90° about Y: (x,z) → (z, -x) for q = (0, sin45, 0, cos45).
        let s = (0.5f64).sqrt();
        let out = transform_mesh_world(
            &unit,
            1.0,
            [0.0; 3],
            2.0,
            [0.0, s, 0.0, s],
            [10.0, 0.0, 0.0],
        );
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for position in &out.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(position[k]);
                hi[k] = hi[k].max(position[k]);
            }
        }
        assert!(
            (lo[0] - 9.0).abs() < 1e-9 && (hi[0] - 11.0).abs() < 1e-9,
            "scaled ×2 about x=10"
        );
        assert!((hi[1] - 1.0).abs() < 1e-9, "scaled ×2 in y");
        let (recentred, centre) = recentre(&out);
        assert!(
            (centre[0] - 10.0).abs() < 1e-9,
            "recentre finds the world centre"
        );
        let hi_x = recentred
            .positions
            .iter()
            .map(|position| position[0])
            .fold(f64::NEG_INFINITY, f64::max);
        assert!((hi_x - 1.0).abs() < 1e-9, "recentred to ±1");
    }

    #[test]
    fn a_rotated_sphere_melds_at_its_on_screen_centre() {
        // The entity origin is the shape's FLOOR point, so rotating the entity swings the ball's
        // centre around it. 180° about X puts the visible ball BELOW the origin — the field must
        // follow it there, or the meld acts a full diameter away from what is on screen.
        let sphere = ShapeRecipe::parametric("sphere").unwrap();
        let flipped = [1.0, 0.0, 0.0, 0.0]; // 180° about X
        let field = meld_field(&sphere, "Ball", [2.0, 3.0, 4.0], flipped, 1.0).expect("melds");
        assert!(
            field.eval([2.0, 2.5, 4.0]) < 0.0,
            "the field centre followed the rotation below the origin"
        );
        assert!(
            field.eval([2.0, 3.5, 4.0]) > 0.0,
            "the unrotated centre position is now OUTSIDE the ball"
        );
    }

    #[test]
    fn a_prism_refuses_to_meld_instead_of_becoming_a_round_column() {
        let prism = ShapeRecipe::parametric("prism").unwrap();
        let msg = meld_field(&prism, "Prism", [0.0; 3], [0.0, 0.0, 0.0, 1.0], 1.0)
            .expect_err("a prism's flat sides are not a cylinder")
            .to_string();
        assert!(msg.contains("Combine"), "points at the alternative: {msg}");
    }

    #[test]
    fn unknown_parameter_keys_are_refused_by_name() {
        assert!(validate_param_keys("sphere", ["radius", "segments"]).is_ok());
        let msg = validate_param_keys("box", ["radius"])
            .expect_err("boxes have no radius")
            .to_string();
        assert!(msg.contains("radius") && msg.contains("box"), "{msg}");
        assert!(validate_param_keys("extrude", ["height"]).is_ok());
        assert!(validate_param_keys("revolve", ["height"]).is_err());
    }

    #[test]
    fn a_disjoint_mesh_combine_refuses_instead_of_landing_nothing() {
        // Two planar solids far apart: the exact boolean legally returns an EMPTY intersection —
        // landing it would delete both sources for an invisible result. It must refuse.
        let a = metrocalk_csg::box_mesh([0.0, 0.0, 0.0], [0.5, 0.5, 0.5]);
        let b = metrocalk_csg::box_mesh([50.0, 0.0, 0.0], [0.5, 0.5, 0.5]);
        let msg = combine_meshes(&a, &b, "intersect")
            .expect_err("nothing overlaps")
            .to_string();
        assert!(msg.contains("touch or overlap"), "explained: {msg}");
    }

    #[test]
    fn meld_field_covers_the_supported_kinds_and_refuses_the_rest() {
        let sphere = recipe("sphere");
        let field = meld_field(
            &sphere,
            "Sphere",
            [1.0, 0.0, 2.0],
            [0.0, 0.0, 0.0, 1.0],
            1.0,
        )
        .expect("a sphere melds");
        // The builder rests the sphere on y=0, so its centre is at y=radius.
        assert!(field.eval([1.0, 0.5, 2.0]) < 0.0, "centre is inside");

        let torus = recipe("torus");
        let msg = meld_field(&torus, "Ring", [0.0; 3], [0.0, 0.0, 0.0, 1.0], 1.0)
            .expect_err("a torus can't meld")
            .to_string();
        assert!(msg.contains("Combine"), "points at the alternative: {msg}");

        let boxy = recipe("box");
        let rotated = [0.0, 0.383, 0.0, 0.924]; // 45° about Y
        let msg = meld_field(&boxy, "Box", [0.0; 3], rotated, 1.0)
            .expect_err("a rotated box can't meld")
            .to_string();
        assert!(msg.contains("unrotated"), "explains the pose: {msg}");
    }

    #[test]
    fn bake_meld_produces_one_clean_blob_from_two_spheres() {
        let recipe = ShapeRecipe {
            v: SHAPE_RECIPE_VERSION,
            kind: "meld".into(),
            params: [("k".to_string(), 0.25)].into_iter().collect(),
            profile: Vec::new(),
            sources: Vec::new(),
        };
        // Two grounded spheres 0.7 m apart (centres at y = 0.5), melded.
        let a = meld_field(
            &ShapeRecipe::parametric("sphere").unwrap(),
            "A",
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
            1.0,
        )
        .unwrap();
        let b = meld_field(
            &ShapeRecipe::parametric("sphere").unwrap(),
            "B",
            [0.7, 0.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
            1.0,
        )
        .unwrap();
        let field = a.smooth_union(b, 0.25);
        let (build, centre, tol) = bake_meld(&recipe, &field, 48, shape_material("meld")).unwrap();
        assert!(
            tol > 0.0 && tol < 0.1,
            "a real, honest chord tolerance: {tol}"
        );
        assert!(build.triangles > 100, "a real blob");
        assert!(
            (centre[0] - 0.35).abs() < 0.05,
            "centred between the spheres"
        );
        // The mesh-form artifact round-trips.
        let decoded = decode_shape_artifact(&build.artifact_bytes)
            .unwrap()
            .unwrap();
        assert_eq!(decoded.0.kind, "meld");
    }

    #[test]
    fn spawn_spots_scatter_without_stacking() {
        let spots: Vec<[f32; 3]> = (0..12).map(spawn_spot).collect();
        for i in 0..spots.len() {
            for j in i + 1..spots.len() {
                let dx = f64::from(spots[i][0] - spots[j][0]);
                let dz = f64::from(spots[i][2] - spots[j][2]);
                assert!(
                    (dx * dx + dz * dz).sqrt() > 0.6,
                    "spots {i} and {j} don't stack"
                );
            }
        }
    }
}
