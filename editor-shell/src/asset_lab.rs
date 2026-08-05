//! Deterministic, non-destructive mesh audit, cleanup, UV preparation, and static-mesh optimization.
//!
//! `Asset Lab` operates on the project-owned [`MeshAsset`] and never mutates its input. Every operation
//! returns a new asset plus measured before/after reports. The implementation deliberately distinguishes
//! facts stored in `MeshAsset` from capabilities owned by another subsystem: a mesh can be *eligible* for
//! the existing collision planner, for example, but this type does not contain a collider or embedded LOD.
//!
//! The UV generator is an honest box-bounds planar projection onto the two largest local-space axes. It is
//! deterministic and useful for flat/simple props, but it is **not** a chart unwrap: folded surfaces can
//! overlap and texel density is not equalized. The audit reruns its bounded positive-area UV overlap check
//! after generation so callers can present that limitation instead of claiming a production unwrap.
//!
//! Optimization delegates to the native conditioning compiler's attribute-, silhouette-, seam- and
//! rig-aware QEM boundary. Materials, decoded textures, UVs, normals, joints and weights remain attached;
//! semantic discontinuities are locked and MikkTSpace tangents are regenerated after topology changes.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::many_single_char_names,
    clippy::too_many_lines
)]

use metrocalk_assets::{MeshAsset, Primitive};
use metrocalk_conditioning::{MeshSimplifier, MeshoptQemSimplifier, SimplifyConfig};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

/// Default absolute position tolerance used to reason about topology across deliberately duplicated
/// render vertices (UV seams and hard-normal splits). It is expressed in the asset's local units.
pub const DEFAULT_TOPOLOGY_WELD_THRESHOLD: f32 = 1.0e-6;
/// Default cap for exact UV-triangle pair tests. Crossing it produces `Inconclusive`, never a false clear.
pub const DEFAULT_UV_PAIR_BUDGET: usize = 250_000;

/// Tunable, bounded audit controls.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditOptions {
    /// Absolute local-space tolerance used only to identify coincident topology vertices.
    pub topology_weld_threshold: f32,
    /// Twice-area threshold below which a 3D triangle is reported degenerate.
    pub degenerate_area_epsilon: f32,
    /// Maximum number of UV triangle pairs tested for positive-area intersection.
    pub uv_pair_budget: usize,
}

impl Default for AuditOptions {
    fn default() -> Self {
        Self {
            topology_weld_threshold: DEFAULT_TOPOLOGY_WELD_THRESHOLD,
            degenerate_area_epsilon: 1.0e-12,
            uv_pair_budget: DEFAULT_UV_PAIR_BUDGET,
        }
    }
}

/// A serialized bounds value independent of the asset crate's renderer-facing type.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundsReport {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub dimensions: [f32; 3],
}

/// Material-slot integrity and usage.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialAudit {
    pub slots: usize,
    pub used_slots: usize,
    pub unused_slots: usize,
    pub invalid_primitive_assignments: usize,
    pub invalid_texture_references: usize,
}

/// One decoded texture's measurable payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextureDescriptor {
    pub index: usize,
    pub width: u32,
    pub height: u32,
    pub rgba8_bytes: usize,
    pub layout_valid: bool,
    pub referenced: bool,
}

/// Texture inventory and validity.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextureAudit {
    pub count: usize,
    pub referenced: usize,
    pub total_texels: u64,
    pub decoded_bytes: usize,
    pub invalid_layouts: usize,
    pub descriptors: Vec<TextureDescriptor>,
}

/// Per-vertex normal health. `missing_vertices` includes absent and partial normal streams.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalAudit {
    pub complete_primitives: usize,
    pub missing_vertices: usize,
    pub invalid_vectors: usize,
    pub zero_length_vectors: usize,
    pub non_unit_vectors: usize,
}

/// Result of the bounded UV overlap signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UvOverlapState {
    /// All candidate pairs were tested and none overlap with positive area.
    Clear,
    /// At least one positive-area overlap was measured.
    Detected,
    /// The configured pair budget was exhausted before a clear result was possible.
    Inconclusive,
    /// There are not enough complete, non-degenerate UV triangles to test.
    NotApplicable,
}

/// Defensible overlap signal: exact convex-triangle clipping, bounded by a caller-visible pair budget.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UvOverlapSignal {
    pub state: UvOverlapState,
    pub triangles_considered: usize,
    pub pairs_tested: usize,
    pub overlapping_pairs: usize,
    pub pair_budget: usize,
    pub method: String,
}

/// UV stream, range, degeneracy, and overlap health.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UvAudit {
    pub complete_primitives: usize,
    pub missing_primitives: usize,
    pub partial_primitives: usize,
    pub missing_vertices: usize,
    pub invalid_coordinates: usize,
    pub out_of_unit_range_vertices: usize,
    pub degenerate_uv_triangles: usize,
    pub overlap: UvOverlapSignal,
}

/// Estimated resident source payload, excluding allocator capacity and external GPU/codec overhead.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ByteEstimate {
    pub positions: usize,
    pub normals: usize,
    pub uvs: usize,
    pub indices: usize,
    pub skin: usize,
    pub textures: usize,
    pub material_factors: usize,
    pub total_payload: usize,
}

/// Honest capability state for this concrete asset and this module's data model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AvailabilityState {
    Available,
    Missing,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Availability {
    pub state: AvailabilityState,
    pub reason: String,
}

/// Availability facts. `*_data` fields mean data actually present in `MeshAsset`, not aspirational UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityAudit {
    pub cleanup: Availability,
    pub planar_uv_generation: Availability,
    pub hole_filling: Availability,
    pub hidden_geometry_removal: Availability,
    pub remeshing: Availability,
    pub lod_generation: Availability,
    pub lod_data: Availability,
    pub collision_generation: Availability,
    pub collision_data: Availability,
    pub texture_bake_input: Availability,
    pub high_to_low_texture_baking: Availability,
}

/// Comprehensive measurable report over one immutable `MeshAsset`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetAuditReport {
    pub name: String,
    /// `MeshAsset` represents one logical asset when non-empty; scene instance count is unavailable here.
    pub logical_objects: usize,
    pub connected_components: usize,
    pub primitives: usize,
    pub non_empty_primitives: usize,
    pub vertices: usize,
    pub unique_position_vertices: usize,
    pub duplicate_position_vertices: usize,
    pub isolated_vertices: usize,
    pub indices: usize,
    pub trailing_indices: usize,
    pub invalid_indices: usize,
    pub triangles: usize,
    pub valid_triangles: usize,
    pub degenerate_triangles: usize,
    pub duplicate_triangles: usize,
    pub edges: usize,
    pub boundary_edges: usize,
    pub manifold_edges: usize,
    pub non_manifold_edges: usize,
    pub invalid_positions: usize,
    pub draw_calls: usize,
    pub materials: MaterialAudit,
    pub textures: TextureAudit,
    pub normals: NormalAudit,
    pub uvs: UvAudit,
    pub bounds: Option<BoundsReport>,
    pub estimated_bytes: ByteEstimate,
    pub capabilities: CapabilityAudit,
    pub warnings: Vec<String>,
}

/// How cleanup should handle the normal stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NormalRepairMode {
    Preserve,
    #[default]
    MissingOrInvalid,
    RecomputeAreaWeightedSmooth,
}

/// UV generation policy. Planar projection is described in this module's documentation and result warning.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UvGenerationMode {
    #[default]
    None,
    PlanarWhenAbsent,
    ReplaceIncompleteWithPlanar,
}

/// Non-destructive cleanup configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)] // independent user-facing cleanup switches, not state flags
pub struct CleanupConfig {
    pub weld_threshold: f32,
    pub degenerate_area_epsilon: f32,
    pub preserve_attribute_seams: bool,
    pub remove_invalid_triangles: bool,
    pub remove_degenerate_triangles: bool,
    pub remove_duplicate_triangles: bool,
    pub remove_isolated_vertices: bool,
    /// Disabled at zero. A positive value removes disconnected triangle components smaller than this count.
    pub remove_components_smaller_than_triangles: usize,
    pub repair_winding: bool,
    pub normal_repair: NormalRepairMode,
    pub uv_generation: UvGenerationMode,
}

impl Default for CleanupConfig {
    fn default() -> Self {
        Self {
            weld_threshold: DEFAULT_TOPOLOGY_WELD_THRESHOLD,
            degenerate_area_epsilon: 1.0e-12,
            preserve_attribute_seams: true,
            remove_invalid_triangles: true,
            remove_degenerate_triangles: true,
            remove_duplicate_triangles: true,
            remove_isolated_vertices: true,
            remove_components_smaller_than_triangles: 0,
            repair_winding: true,
            normal_repair: NormalRepairMode::MissingOrInvalid,
            uv_generation: UvGenerationMode::None,
        }
    }
}

/// Exact measured edits made by cleanup.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupChanges {
    pub vertices_welded: usize,
    pub isolated_vertices_removed: usize,
    pub invalid_triangles_removed: usize,
    pub degenerate_triangles_removed: usize,
    pub duplicate_triangles_removed: usize,
    pub small_component_triangles_removed: usize,
    pub triangles_flipped: usize,
    pub normal_vertices_recomputed: usize,
    pub uv_vertices_generated: usize,
    pub empty_primitives_removed: usize,
}

/// Before/after evidence shared by cleanup and optimization.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetChangeReport {
    pub before: AssetAuditReport,
    pub after: AssetAuditReport,
    pub changes: Vec<String>,
    pub warnings: Vec<String>,
}

/// A successfully cleaned copy. The caller decides whether/when it replaces the source asset.
#[derive(Clone, Debug, PartialEq)]
pub struct CleanupResult {
    pub asset: MeshAsset,
    pub measured: CleanupChanges,
    pub report: AssetChangeReport,
}

/// Meaningful optimization target presets. Each maps to a real triangle-ratio target.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OptimizationPreset {
    Draft,
    #[default]
    Balanced,
    HighQuality,
    Mobile,
    Web,
    Desktop,
    Cinematic,
}

impl OptimizationPreset {
    #[must_use]
    pub const fn target_ratio(self) -> f32 {
        match self {
            Self::Draft => 0.15,
            Self::Balanced => 0.50,
            Self::HighQuality => 0.72,
            Self::Mobile => 0.20,
            Self::Web => 0.35,
            Self::Desktop => 0.60,
            Self::Cinematic => 0.85,
        }
    }
}

/// Attribute-aware QEM optimization controls. The legacy candidate fields remain serialized so older
/// editor builds can send their saved settings; the native reducer now publishes one measured result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptimizationConfig {
    pub preset: OptimizationPreset,
    /// Overrides the preset; must be finite and in `(0, 1)`.
    pub target_ratio: Option<f32>,
    /// Legacy compatibility field; ignored by the QEM reducer.
    pub candidate_levels: u8,
    /// Legacy compatibility field; ignored by the QEM reducer.
    pub base_fraction: f32,
}

impl Default for OptimizationConfig {
    fn default() -> Self {
        Self {
            preset: OptimizationPreset::Balanced,
            target_ratio: None,
            candidate_levels: 10,
            base_fraction: 0.003,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LodCandidateReport {
    pub level: u8,
    pub vertices: usize,
    pub triangles: usize,
    pub ratio: f32,
}

/// A selected optimized mesh and its measured QEM evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct OptimizationResult {
    pub asset: MeshAsset,
    pub requested_ratio: f32,
    pub achieved_ratio: f32,
    pub chosen_level: u8,
    pub candidates: Vec<LodCandidateReport>,
    pub report: AssetChangeReport,
}

/// Fail-fast error. Unsupported means the engine refused a lossy or dishonest operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetLabError {
    InvalidConfig(String),
    InvalidInput(String),
    Unsupported(String),
}

impl fmt::Display for AssetLabError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, detail) = match self {
            Self::InvalidConfig(v) => ("invalid Asset Lab configuration", v),
            Self::InvalidInput(v) => ("invalid mesh input", v),
            Self::Unsupported(v) => ("unsupported Asset Lab operation", v),
        };
        write!(f, "{kind}: {detail}")
    }
}

impl std::error::Error for AssetLabError {}

#[derive(Default)]
struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<u8>,
}

impl UnionFind {
    fn push(&mut self) -> usize {
        let id = self.parent.len();
        self.parent.push(id);
        self.rank.push(0);
        id
    }

    fn find(&mut self, x: usize) -> usize {
        if self.parent[x] != x {
            self.parent[x] = self.find(self.parent[x]);
        }
        self.parent[x]
    }

    fn union(&mut self, a: usize, b: usize) {
        let (mut ra, mut rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        if self.rank[ra] < self.rank[rb] {
            std::mem::swap(&mut ra, &mut rb);
        }
        self.parent[rb] = ra;
        if self.rank[ra] == self.rank[rb] {
            self.rank[ra] += 1;
        }
    }
}

/// Deterministic spatial welder used for audit topology. It picks the lowest previously-created matching
/// representative across neighboring cells, avoiding hash seed or map iteration nondeterminism.
struct PositionWelder {
    threshold: f32,
    exact: BTreeMap<[u32; 3], usize>,
    cells: BTreeMap<[i64; 3], Vec<usize>>,
    positions: Vec<[f32; 3]>,
}

impl PositionWelder {
    fn new(threshold: f32) -> Self {
        Self {
            threshold,
            exact: BTreeMap::new(),
            cells: BTreeMap::new(),
            positions: Vec::new(),
        }
    }

    fn insert(&mut self, p: [f32; 3]) -> Option<usize> {
        if !p.iter().all(|v| v.is_finite()) {
            return None;
        }
        if self.threshold == 0.0 {
            let key = [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
            if let Some(&id) = self.exact.get(&key) {
                return Some(id);
            }
            let id = self.positions.len();
            self.positions.push(p);
            self.exact.insert(key, id);
            return Some(id);
        }
        let cell = position_cell(p, self.threshold);
        let limit2 = self.threshold * self.threshold;
        let mut found = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let key = [cell[0] + dx, cell[1] + dy, cell[2] + dz];
                    if let Some(ids) = self.cells.get(&key) {
                        for &id in ids {
                            if distance_squared(self.positions[id], p) <= limit2 {
                                found = Some(found.map_or(id, |old: usize| old.min(id)));
                            }
                        }
                    }
                }
            }
        }
        if let Some(id) = found {
            return Some(id);
        }
        let id = self.positions.len();
        self.positions.push(p);
        self.cells.entry(cell).or_default().push(id);
        Some(id)
    }
}

fn position_cell(p: [f32; 3], threshold: f32) -> [i64; 3] {
    let t = f64::from(threshold);
    [
        (f64::from(p[0]) / t).floor() as i64,
        (f64::from(p[1]) / t).floor() as i64,
        (f64::from(p[2]) / t).floor() as i64,
    ]
}

fn distance_squared(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0].mul_add(d[0], d[1].mul_add(d[1], d[2] * d[2]))
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1].mul_add(b[2], -a[2] * b[1]),
        a[2].mul_add(b[0], -a[0] * b[2]),
        a[0].mul_add(b[1], -a[1] * b[0]),
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0].mul_add(b[0], a[1].mul_add(b[1], a[2] * b[2]))
}

fn triangle_twice_area(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f32 {
    let n = cross(sub(b, a), sub(c, a));
    dot(n, n).sqrt()
}

fn canonical_triangle(mut tri: [usize; 3]) -> [usize; 3] {
    tri.sort_unstable();
    tri
}

#[derive(Clone, Copy)]
struct UvTriangle {
    points: [[f64; 2]; 3],
}

#[derive(Clone, Copy)]
struct UvSweepEntry {
    triangle: usize,
    min: [f64; 2],
    max: [f64; 2],
}

/// Audit an immutable mesh. Invalid configuration is reported; malformed mesh content is measured rather
/// than panicking, so the caller can offer cleanup.
pub fn audit_asset(
    asset: &MeshAsset,
    options: &AuditOptions,
) -> Result<AssetAuditReport, AssetLabError> {
    validate_audit_options(options)?;

    let vertices = asset.vertex_count();
    let indices = asset.index_count();
    let triangles = asset.primitives.iter().map(|p| p.indices.len() / 3).sum();
    let trailing_indices = asset.primitives.iter().map(|p| p.indices.len() % 3).sum();
    let mut position_welder = PositionWelder::new(options.topology_weld_threshold);
    let mut welded_ids: Vec<Vec<Option<usize>>> = Vec::with_capacity(asset.primitives.len());
    let mut invalid_positions = 0;
    let mut bounds_min = [f32::INFINITY; 3];
    let mut bounds_max = [f32::NEG_INFINITY; 3];
    for prim in &asset.primitives {
        let mut ids = Vec::with_capacity(prim.positions.len());
        for &p in &prim.positions {
            if p.iter().all(|v| v.is_finite()) {
                for axis in 0..3 {
                    bounds_min[axis] = bounds_min[axis].min(p[axis]);
                    bounds_max[axis] = bounds_max[axis].max(p[axis]);
                }
            } else {
                invalid_positions += 1;
            }
            ids.push(position_welder.insert(p));
        }
        welded_ids.push(ids);
    }

    let mut invalid_indices = 0;
    let mut degenerate_triangles = 0;
    let mut duplicate_triangles = 0;
    let mut valid_triangles = 0;
    let mut referenced: Vec<Vec<bool>> = asset
        .primitives
        .iter()
        .map(|p| vec![false; p.positions.len()])
        .collect();
    let mut edge_counts: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    let mut seen_triangles: BTreeSet<[usize; 3]> = BTreeSet::new();
    let mut topology = UnionFind::default();
    for _ in 0..position_welder.positions.len() {
        topology.push();
    }
    let mut used_topology_vertices = BTreeSet::new();
    let mut uv_triangles = Vec::new();
    let mut degenerate_uv_triangles = 0;

    for (pi, prim) in asset.primitives.iter().enumerate() {
        for tri in prim.indices.chunks_exact(3) {
            let raw = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
            if raw.iter().any(|&i| i >= prim.positions.len()) {
                invalid_indices += raw.iter().filter(|&&i| i >= prim.positions.len()).count();
                continue;
            }
            for &i in &raw {
                referenced[pi][i] = true;
            }
            let Some(wtri) = welded_triangle(&welded_ids[pi], raw) else {
                degenerate_triangles += 1;
                continue;
            };
            let positions = [
                prim.positions[raw[0]],
                prim.positions[raw[1]],
                prim.positions[raw[2]],
            ];
            if wtri[0] == wtri[1]
                || wtri[1] == wtri[2]
                || wtri[0] == wtri[2]
                || triangle_twice_area(positions[0], positions[1], positions[2])
                    <= options.degenerate_area_epsilon
            {
                degenerate_triangles += 1;
                continue;
            }
            valid_triangles += 1;
            let canonical = canonical_triangle(wtri);
            if !seen_triangles.insert(canonical) {
                duplicate_triangles += 1;
            }
            for &(a, b) in &[(wtri[0], wtri[1]), (wtri[1], wtri[2]), (wtri[2], wtri[0])] {
                let edge = if a < b { (a, b) } else { (b, a) };
                *edge_counts.entry(edge).or_default() += 1;
            }
            topology.union(wtri[0], wtri[1]);
            topology.union(wtri[1], wtri[2]);
            used_topology_vertices.extend(wtri);

            if prim.uvs.len() == prim.positions.len() {
                let uv = [prim.uvs[raw[0]], prim.uvs[raw[1]], prim.uvs[raw[2]]];
                if uv.iter().flatten().all(|v| v.is_finite()) {
                    let uv64 = [
                        [f64::from(uv[0][0]), f64::from(uv[0][1])],
                        [f64::from(uv[1][0]), f64::from(uv[1][1])],
                        [f64::from(uv[2][0]), f64::from(uv[2][1])],
                    ];
                    if triangle_area_2d(uv64).abs() <= 1.0e-12 {
                        degenerate_uv_triangles += 1;
                    } else {
                        uv_triangles.push(UvTriangle { points: uv64 });
                    }
                }
            }
        }
    }

    let connected_components = {
        let mut roots = BTreeSet::new();
        for id in used_topology_vertices {
            roots.insert(topology.find(id));
        }
        roots.len()
    };
    let boundary_edges = edge_counts.values().filter(|&&count| count == 1).count();
    let manifold_edges = edge_counts.values().filter(|&&count| count == 2).count();
    let non_manifold_edges = edge_counts.values().filter(|&&count| count > 2).count();
    let isolated_vertices = referenced
        .iter()
        .map(|used| used.iter().filter(|&&v| !v).count())
        .sum();

    let materials = audit_materials(asset);
    let textures = audit_textures(asset);
    let normals = audit_normals(asset);
    let mut uvs = audit_uv_streams(asset, options, &uv_triangles);
    uvs.degenerate_uv_triangles = degenerate_uv_triangles;
    let bounds = if position_welder.positions.is_empty() {
        None
    } else {
        Some(BoundsReport {
            min: bounds_min,
            max: bounds_max,
            dimensions: [
                bounds_max[0] - bounds_min[0],
                bounds_max[1] - bounds_min[1],
                bounds_max[2] - bounds_min[2],
            ],
        })
    };
    let estimated_bytes = estimate_bytes(asset);
    let draw_calls = asset
        .primitives
        .iter()
        .filter(|p| {
            p.indices
                .chunks_exact(3)
                .any(|tri| tri.iter().all(|&i| (i as usize) < p.positions.len()))
        })
        .count();
    let capabilities = capability_audit(asset, valid_triangles, invalid_indices, &uvs);
    let mut warnings = Vec::new();
    if invalid_positions > 0 {
        warnings.push(format!(
            "{invalid_positions} non-finite position vertices cannot be rendered safely"
        ));
    }
    if invalid_indices > 0 || trailing_indices > 0 {
        warnings.push("index data is malformed; run cleanup before optimization or export".into());
    }
    if non_manifold_edges > 0 {
        warnings.push(format!(
            "{non_manifold_edges} welded edges have more than two incident faces"
        ));
    }
    if boundary_edges > 0 {
        warnings.push(format!(
            "{boundary_edges} welded boundary edges indicate open surfaces or holes"
        ));
    }
    if uvs.overlap.state == UvOverlapState::Inconclusive {
        warnings.push(
            "UV overlap audit hit its pair budget; the result is inconclusive, not clear".into(),
        );
    }
    if duplicate_triangles > 0 {
        warnings.push(format!(
            "{duplicate_triangles} duplicate triangles overlap exactly after topology welding"
        ));
    }

    Ok(AssetAuditReport {
        name: asset.name.clone(),
        logical_objects: usize::from(!asset.primitives.is_empty()),
        connected_components,
        primitives: asset.primitives.len(),
        non_empty_primitives: asset
            .primitives
            .iter()
            .filter(|p| !p.indices.is_empty())
            .count(),
        vertices,
        unique_position_vertices: position_welder.positions.len(),
        duplicate_position_vertices: vertices
            .saturating_sub(invalid_positions + position_welder.positions.len()),
        isolated_vertices,
        indices,
        trailing_indices,
        invalid_indices,
        triangles,
        valid_triangles,
        degenerate_triangles,
        duplicate_triangles,
        edges: edge_counts.len(),
        boundary_edges,
        manifold_edges,
        non_manifold_edges,
        invalid_positions,
        draw_calls,
        materials,
        textures,
        normals,
        uvs,
        bounds,
        estimated_bytes,
        capabilities,
        warnings,
    })
}

fn validate_audit_options(options: &AuditOptions) -> Result<(), AssetLabError> {
    if !options.topology_weld_threshold.is_finite() || options.topology_weld_threshold < 0.0 {
        return Err(AssetLabError::InvalidConfig(
            "topology_weld_threshold must be finite and non-negative".into(),
        ));
    }
    if !options.degenerate_area_epsilon.is_finite() || options.degenerate_area_epsilon < 0.0 {
        return Err(AssetLabError::InvalidConfig(
            "degenerate_area_epsilon must be finite and non-negative".into(),
        ));
    }
    Ok(())
}

fn welded_triangle(ids: &[Option<usize>], raw: [usize; 3]) -> Option<[usize; 3]> {
    Some([ids[raw[0]]?, ids[raw[1]]?, ids[raw[2]]?])
}

fn audit_materials(asset: &MeshAsset) -> MaterialAudit {
    let mut used = BTreeSet::new();
    let mut invalid_primitive_assignments = 0;
    for prim in &asset.primitives {
        if prim.material < asset.materials.len() {
            used.insert(prim.material);
        } else {
            invalid_primitive_assignments += 1;
        }
    }
    let mut invalid_texture_references = 0;
    for material in &asset.materials {
        for slot in [
            material.base_color_texture,
            material.metallic_roughness_texture,
            material.normal_texture,
            material.occlusion_texture,
            material.curvature_texture,
        ] {
            if slot.is_some_and(|index| index >= asset.textures.len()) {
                invalid_texture_references += 1;
            }
        }
    }
    MaterialAudit {
        slots: asset.materials.len(),
        used_slots: used.len(),
        unused_slots: asset.materials.len().saturating_sub(used.len()),
        invalid_primitive_assignments,
        invalid_texture_references,
    }
}

fn audit_textures(asset: &MeshAsset) -> TextureAudit {
    let referenced_indices: BTreeSet<usize> = asset
        .materials
        .iter()
        .flat_map(|m| {
            [
                m.base_color_texture,
                m.metallic_roughness_texture,
                m.normal_texture,
                m.occlusion_texture,
                m.curvature_texture,
            ]
        })
        .flatten()
        .filter(|&i| i < asset.textures.len())
        .collect();
    let mut total_texels = 0_u64;
    let mut decoded_bytes = 0_usize;
    let mut invalid_layouts = 0;
    let descriptors = asset
        .textures
        .iter()
        .enumerate()
        .map(|(index, texture)| {
            let texels = u64::from(texture.width) * u64::from(texture.height);
            total_texels = total_texels.saturating_add(texels);
            decoded_bytes = decoded_bytes.saturating_add(texture.rgba8.len());
            let expected = texels.checked_mul(4).and_then(|n| usize::try_from(n).ok());
            let layout_valid = texture.width > 0
                && texture.height > 0
                && expected.is_some_and(|n| n == texture.rgba8.len());
            if !layout_valid {
                invalid_layouts += 1;
            }
            TextureDescriptor {
                index,
                width: texture.width,
                height: texture.height,
                rgba8_bytes: texture.rgba8.len(),
                layout_valid,
                referenced: referenced_indices.contains(&index),
            }
        })
        .collect();
    TextureAudit {
        count: asset.textures.len(),
        referenced: referenced_indices.len(),
        total_texels,
        decoded_bytes,
        invalid_layouts,
        descriptors,
    }
}

fn audit_normals(asset: &MeshAsset) -> NormalAudit {
    let mut out = NormalAudit::default();
    for prim in &asset.primitives {
        if prim.normals.len() == prim.positions.len() {
            out.complete_primitives += 1;
        } else {
            out.missing_vertices += prim.positions.len().saturating_sub(prim.normals.len());
            if prim.normals.len() > prim.positions.len() {
                out.invalid_vectors += prim.normals.len() - prim.positions.len();
            }
        }
        for n in prim.normals.iter().take(prim.positions.len()) {
            if !n.iter().all(|v| v.is_finite()) {
                out.invalid_vectors += 1;
                continue;
            }
            let len2 = dot(*n, *n);
            if len2 <= 1.0e-12 {
                out.zero_length_vectors += 1;
            } else if (len2.sqrt() - 1.0).abs() > 1.0e-3 {
                out.non_unit_vectors += 1;
            }
        }
    }
    out
}

fn audit_uv_streams(
    asset: &MeshAsset,
    options: &AuditOptions,
    triangles: &[UvTriangle],
) -> UvAudit {
    let mut complete_primitives = 0;
    let mut missing_primitives = 0;
    let mut partial_primitives = 0;
    let mut missing_vertices = 0;
    let mut invalid_coordinates = 0;
    let mut out_of_unit_range_vertices = 0;
    for prim in &asset.primitives {
        if prim.uvs.is_empty() {
            missing_primitives += 1;
            missing_vertices += prim.positions.len();
        } else if prim.uvs.len() == prim.positions.len() {
            complete_primitives += 1;
        } else {
            partial_primitives += 1;
            missing_vertices += prim.positions.len().saturating_sub(prim.uvs.len());
            invalid_coordinates += prim.uvs.len().saturating_sub(prim.positions.len()) * 2;
        }
        for uv in prim.uvs.iter().take(prim.positions.len()) {
            if !uv.iter().all(|v| v.is_finite()) {
                invalid_coordinates += uv.iter().filter(|v| !v.is_finite()).count();
            } else if uv.iter().any(|&v| !(0.0..=1.0).contains(&v)) {
                out_of_unit_range_vertices += 1;
            }
        }
    }
    UvAudit {
        complete_primitives,
        missing_primitives,
        partial_primitives,
        missing_vertices,
        invalid_coordinates,
        out_of_unit_range_vertices,
        degenerate_uv_triangles: 0,
        overlap: detect_uv_overlap(triangles, options.uv_pair_budget),
    }
}

fn detect_uv_overlap(triangles: &[UvTriangle], budget: usize) -> UvOverlapSignal {
    let method = "deterministic UV AABB sweep-and-prune followed by positive-area convex triangle clipping; shared edges/points are not overlaps; detection stops after the first proven overlap".into();
    if triangles.len() < 2 {
        return UvOverlapSignal {
            state: UvOverlapState::NotApplicable,
            triangles_considered: triangles.len(),
            pairs_tested: 0,
            overlapping_pairs: 0,
            pair_budget: budget,
            method,
        };
    }
    let mut sweep: Vec<UvSweepEntry> = triangles
        .iter()
        .enumerate()
        .map(|(triangle, candidate)| {
            let mut min = [f64::INFINITY; 2];
            let mut max = [f64::NEG_INFINITY; 2];
            for point in candidate.points {
                min[0] = min[0].min(point[0]);
                min[1] = min[1].min(point[1]);
                max[0] = max[0].max(point[0]);
                max[1] = max[1].max(point[1]);
            }
            UvSweepEntry { triangle, min, max }
        })
        .collect();
    sweep.sort_by(|a, b| {
        a.min[0]
            .total_cmp(&b.min[0])
            .then_with(|| a.min[1].total_cmp(&b.min[1]))
            .then_with(|| a.triangle.cmp(&b.triangle))
    });

    let mut pairs_tested = 0;
    let bounds_epsilon = 1.0e-12;
    for i in 0..sweep.len() - 1 {
        let a = sweep[i];
        for &b in &sweep[i + 1..] {
            // Sorted minimum U means every later entry is also disjoint once this one starts beyond A.
            if b.min[0] > a.max[0] + bounds_epsilon {
                break;
            }
            if b.max[1] < a.min[1] - bounds_epsilon || b.min[1] > a.max[1] + bounds_epsilon {
                continue;
            }
            if pairs_tested >= budget {
                return UvOverlapSignal {
                    state: UvOverlapState::Inconclusive,
                    triangles_considered: triangles.len(),
                    pairs_tested,
                    overlapping_pairs: 0,
                    pair_budget: budget,
                    method,
                };
            }
            pairs_tested += 1;
            if triangle_intersection_area(
                triangles[a.triangle].points,
                triangles[b.triangle].points,
            ) > 1.0e-10
            {
                return UvOverlapSignal {
                    state: UvOverlapState::Detected,
                    triangles_considered: triangles.len(),
                    pairs_tested,
                    overlapping_pairs: 1,
                    pair_budget: budget,
                    method,
                };
            }
        }
    }
    UvOverlapSignal {
        state: UvOverlapState::Clear,
        triangles_considered: triangles.len(),
        pairs_tested,
        overlapping_pairs: 0,
        pair_budget: budget,
        method,
    }
}

fn triangle_area_2d(t: [[f64; 2]; 3]) -> f64 {
    ((t[1][0] - t[0][0]) * (t[2][1] - t[0][1]) - (t[1][1] - t[0][1]) * (t[2][0] - t[0][0])) * 0.5
}

fn triangle_intersection_area(mut subject: [[f64; 2]; 3], mut clip: [[f64; 2]; 3]) -> f64 {
    if triangle_area_2d(subject) < 0.0 {
        subject.swap(1, 2);
    }
    if triangle_area_2d(clip) < 0.0 {
        clip.swap(1, 2);
    }
    let mut polygon = subject.to_vec();
    for edge in 0..3 {
        if polygon.is_empty() {
            return 0.0;
        }
        let a = clip[edge];
        let b = clip[(edge + 1) % 3];
        let input = std::mem::take(&mut polygon);
        let mut previous = *input.last().expect("non-empty clipping polygon");
        let mut previous_inside = side_2d(a, b, previous) >= -1.0e-12;
        for current in input {
            let current_inside = side_2d(a, b, current) >= -1.0e-12;
            if current_inside != previous_inside {
                polygon.push(line_intersection(previous, current, a, b));
            }
            if current_inside {
                polygon.push(current);
            }
            previous = current;
            previous_inside = current_inside;
        }
    }
    polygon_area(&polygon).abs()
}

fn side_2d(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}

fn line_intersection(p: [f64; 2], q: [f64; 2], a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let r = [q[0] - p[0], q[1] - p[1]];
    let s = [b[0] - a[0], b[1] - a[1]];
    let denom = r[0] * s[1] - r[1] * s[0];
    if denom.abs() <= 1.0e-18 {
        return p;
    }
    let ap = [a[0] - p[0], a[1] - p[1]];
    let t = (ap[0] * s[1] - ap[1] * s[0]) / denom;
    [p[0] + t * r[0], p[1] + t * r[1]]
}

fn polygon_area(polygon: &[[f64; 2]]) -> f64 {
    if polygon.len() < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        sum += a[0] * b[1] - a[1] * b[0];
    }
    sum * 0.5
}

fn estimate_bytes(asset: &MeshAsset) -> ByteEstimate {
    let mut out = ByteEstimate::default();
    for p in &asset.primitives {
        out.positions = out
            .positions
            .saturating_add(p.positions.len() * 3 * size_of::<f32>());
        out.normals = out
            .normals
            .saturating_add(p.normals.len() * 3 * size_of::<f32>());
        out.uvs = out.uvs.saturating_add(p.uvs.len() * 2 * size_of::<f32>());
        out.indices = out
            .indices
            .saturating_add(p.indices.len() * size_of::<u32>());
        out.skin = out.skin.saturating_add(
            p.joints.len() * 4 * size_of::<u16>() + p.weights.len() * 4 * size_of::<f32>(),
        );
    }
    out.textures = asset.textures.iter().map(|t| t.rgba8.len()).sum();
    // Four color floats, two factors, and five optional texture indices (payload estimate, not Rust layout).
    out.material_factors = asset.materials.len() * (6 * size_of::<f32>() + 5 * size_of::<usize>());
    out.total_payload = out
        .positions
        .saturating_add(out.normals)
        .saturating_add(out.uvs)
        .saturating_add(out.indices)
        .saturating_add(out.skin)
        .saturating_add(out.textures)
        .saturating_add(out.material_factors);
    out
}

fn availability(state: AvailabilityState, reason: impl Into<String>) -> Availability {
    Availability {
        state,
        reason: reason.into(),
    }
}

fn capability_audit(
    asset: &MeshAsset,
    valid_triangles: usize,
    invalid_indices: usize,
    uv: &UvAudit,
) -> CapabilityAudit {
    let texture_refs_valid = audit_materials(asset).invalid_texture_references == 0
        && audit_textures(asset).invalid_layouts == 0;
    let valid_geometry = valid_triangles > 0 && invalid_indices == 0;
    let lod_generation = if valid_geometry {
        availability(
            AvailabilityState::Available,
            "seam-, silhouette-, material- and rig-aware native QEM preserves parallel vertex payloads",
        )
    } else {
        availability(
            AvailabilityState::Missing,
            "valid triangle geometry is required",
        )
    };
    let uv_layout_usable = !matches!(
        uv.overlap.state,
        UvOverlapState::Detected | UvOverlapState::Inconclusive
    ) && uv.out_of_unit_range_vertices == 0
        && uv.degenerate_uv_triangles == 0;
    let texture_bake_input = if valid_geometry
        && uv.missing_vertices == 0
        && uv.invalid_coordinates == 0
        && uv_layout_usable
        && texture_refs_valid
    {
        availability(
            AvailabilityState::Available,
            "geometry, complete UV0 streams, and referenced decoded textures are structurally valid",
        )
    } else {
        availability(
            AvailabilityState::Missing,
            "complete valid UV0, valid triangles, and valid texture references are required",
        )
    };
    let bake_ready = texture_bake_input.state == AvailabilityState::Available;
    CapabilityAudit {
        cleanup: if asset.primitives.is_empty() {
            availability(AvailabilityState::Missing, "the asset contains no primitives")
        } else {
            availability(AvailabilityState::Available, "non-destructive cleanup returns a new measured asset")
        },
        planar_uv_generation: if valid_triangles > 0 {
            availability(
                AvailabilityState::Available,
                "native xatlas chart unwrap, square-atlas packing, texel-density evidence and MikkTSpace tangents are available",
            )
        } else {
            availability(AvailabilityState::Missing, "valid triangles are required")
        },
        hole_filling: availability(
            AvailabilityState::Unsupported,
            "boundary edges and likely holes are measured, but this module has no constrained surface-fill solver",
        ),
        hidden_geometry_removal: availability(
            AvailabilityState::Unsupported,
            "reliable occlusion removal needs scene context and visibility sampling unavailable to MeshAsset",
        ),
        remeshing: availability(
            AvailabilityState::Available,
            "native QEM topology reduction is available with explicit seam and skinning locks",
        ),
        lod_generation,
        lod_data: availability(
            AvailabilityState::Missing,
            "MeshAsset has no embedded LOD field; generated levels are returned separately",
        ),
        collision_generation: if valid_geometry {
            availability(
                AvailabilityState::Available,
                "valid geometry can be handed to the existing physics collider planner",
            )
        } else {
            availability(AvailabilityState::Missing, "valid triangle geometry is required")
        },
        collision_data: availability(
            AvailabilityState::Missing,
            "MeshAsset does not contain collider data; inspect the scene physics component instead",
        ),
        texture_bake_input,
        high_to_low_texture_baking: if bake_ready {
            availability(
                AvailabilityState::Available,
                "the low target is ready for arbitrary multi-source normal, AO and signed-curvature projection",
            )
        } else {
            availability(
                AvailabilityState::Missing,
                "prepare a complete non-overlapping UV0 layout before choosing high-poly sources",
            )
        },
    }
}

/// Clean and repair an immutable mesh, returning a new asset and measured before/after evidence.
pub fn cleanup_asset(
    asset: &MeshAsset,
    config: &CleanupConfig,
) -> Result<CleanupResult, AssetLabError> {
    validate_cleanup_config(config)?;
    let audit_options = AuditOptions {
        topology_weld_threshold: config.weld_threshold,
        degenerate_area_epsilon: config.degenerate_area_epsilon,
        ..AuditOptions::default()
    };
    let before = audit_asset(asset, &audit_options)?;
    if asset.primitives.iter().any(|p| {
        (!p.joints.is_empty() && p.joints.len() != p.positions.len())
            || (!p.weights.is_empty() && p.weights.len() != p.positions.len())
    }) {
        return Err(AssetLabError::Unsupported(
            "partial skin streams cannot be remapped safely; repair or reimport the rig first"
                .into(),
        ));
    }

    let mut measured = CleanupChanges::default();
    let mut warnings = Vec::new();
    let mut primitives = Vec::with_capacity(asset.primitives.len());
    for prim in &asset.primitives {
        let mut cleaned = weld_primitive(prim, config, &mut measured, &mut warnings)?;
        filter_triangles(&mut cleaned, config, &mut measured);
        if config.remove_components_smaller_than_triangles > 0 {
            remove_small_components(
                &mut cleaned,
                config.remove_components_smaller_than_triangles,
                &mut measured,
            );
        }
        let flips_before = measured.triangles_flipped;
        if config.repair_winding {
            repair_winding(&mut cleaned, &mut measured, &mut warnings);
        }
        if config.remove_isolated_vertices {
            compact_vertices(&mut cleaned, &mut measured);
        }
        let winding_changed = measured.triangles_flipped > flips_before;
        let normal_mode =
            if winding_changed && config.normal_repair == NormalRepairMode::MissingOrInvalid {
                NormalRepairMode::RecomputeAreaWeightedSmooth
            } else {
                config.normal_repair
            };
        if winding_changed && config.normal_repair == NormalRepairMode::Preserve {
            warnings.push(
                "triangle winding changed while authored normals were explicitly preserved; verify face/normal agreement"
                    .into(),
            );
        }
        repair_normals(&mut cleaned, normal_mode, &mut measured, &mut warnings);
        repair_uvs(
            &mut cleaned,
            config.uv_generation,
            &mut measured,
            &mut warnings,
        );
        if cleaned.indices.is_empty() {
            measured.empty_primitives_removed += 1;
        } else {
            primitives.push(cleaned);
        }
    }
    let output = MeshAsset {
        name: asset.name.clone(),
        primitives,
        materials: asset.materials.clone(),
        textures: asset.textures.clone(),
        skeleton: asset.skeleton.clone(),
    };
    let after = audit_asset(&output, &audit_options)?;
    let changes = cleanup_change_lines(&measured);
    Ok(CleanupResult {
        asset: output,
        measured,
        report: AssetChangeReport {
            before,
            after,
            changes,
            warnings,
        },
    })
}

fn validate_cleanup_config(config: &CleanupConfig) -> Result<(), AssetLabError> {
    if !config.weld_threshold.is_finite() || config.weld_threshold < 0.0 {
        return Err(AssetLabError::InvalidConfig(
            "weld_threshold must be finite and non-negative".into(),
        ));
    }
    if !config.degenerate_area_epsilon.is_finite() || config.degenerate_area_epsilon < 0.0 {
        return Err(AssetLabError::InvalidConfig(
            "degenerate_area_epsilon must be finite and non-negative".into(),
        ));
    }
    if !config.remove_invalid_triangles {
        return Err(AssetLabError::Unsupported(
            "retaining out-of-range triangle indices cannot produce a safe MeshAsset".into(),
        ));
    }
    Ok(())
}

fn weld_primitive(
    prim: &Primitive,
    config: &CleanupConfig,
    measured: &mut CleanupChanges,
    warnings: &mut Vec<String>,
) -> Result<Primitive, AssetLabError> {
    let n = prim.positions.len();
    let normals_complete = prim.normals.len() == n;
    let uvs_complete = prim.uvs.len() == n;
    let joints_complete = prim.joints.len() == n;
    let weights_complete = prim.weights.len() == n;
    let complete_streams = CompleteStreams {
        normals: normals_complete,
        uvs: uvs_complete,
        joints: joints_complete,
        weights: weights_complete,
    };
    if !prim.normals.is_empty() && !normals_complete {
        warnings.push(
            "partial normal stream was discarded; normal repair can regenerate a complete stream"
                .into(),
        );
    }
    if !prim.uvs.is_empty() && !uvs_complete {
        warnings
            .push("partial UV stream was discarded because it cannot be mapped reliably".into());
    }
    if (!prim.joints.is_empty() && !joints_complete)
        || (!prim.weights.is_empty() && !weights_complete)
    {
        return Err(AssetLabError::Unsupported(
            "partial skin streams cannot be welded safely".into(),
        ));
    }

    let mut exact: BTreeMap<[u32; 3], Vec<u32>> = BTreeMap::new();
    let mut cells: BTreeMap<[i64; 3], Vec<u32>> = BTreeMap::new();
    let mut representative_source = Vec::<usize>::new();
    let mut out = Primitive {
        material: prim.material,
        ..Primitive::default()
    };
    let mut remap = Vec::with_capacity(n);
    for (source, &position) in prim.positions.iter().enumerate() {
        if !position.iter().all(|v| v.is_finite()) {
            // Keep it for now so every old index has a remap. Triangle filtering removes all faces using it.
            let id = u32::try_from(out.positions.len()).map_err(|_| {
                AssetLabError::InvalidInput("vertex count exceeds u32 index capacity".into())
            })?;
            push_vertex(&mut out, prim, source, complete_streams);
            representative_source.push(source);
            remap.push(id);
            continue;
        }
        let mut candidate = None;
        if config.weld_threshold == 0.0 {
            let key = [
                position[0].to_bits(),
                position[1].to_bits(),
                position[2].to_bits(),
            ];
            if let Some(ids) = exact.get(&key) {
                candidate = compatible_candidate(
                    ids,
                    prim,
                    source,
                    &representative_source,
                    config.preserve_attribute_seams,
                );
            }
        } else {
            let cell = position_cell(position, config.weld_threshold);
            let limit2 = config.weld_threshold * config.weld_threshold;
            let mut ids = Vec::new();
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        if let Some(found) = cells.get(&[cell[0] + dx, cell[1] + dy, cell[2] + dz])
                        {
                            ids.extend(found.iter().copied());
                        }
                    }
                }
            }
            ids.sort_unstable();
            candidate = ids.into_iter().find(|&id| {
                distance_squared(out.positions[id as usize], position) <= limit2
                    && attributes_compatible(
                        prim,
                        source,
                        representative_source[id as usize],
                        config.preserve_attribute_seams,
                    )
            });
        }
        let id = if let Some(id) = candidate {
            measured.vertices_welded += 1;
            id
        } else {
            let id = u32::try_from(out.positions.len()).map_err(|_| {
                AssetLabError::InvalidInput("vertex count exceeds u32 index capacity".into())
            })?;
            push_vertex(&mut out, prim, source, complete_streams);
            representative_source.push(source);
            if config.weld_threshold == 0.0 {
                exact
                    .entry([
                        position[0].to_bits(),
                        position[1].to_bits(),
                        position[2].to_bits(),
                    ])
                    .or_default()
                    .push(id);
            } else {
                cells
                    .entry(position_cell(position, config.weld_threshold))
                    .or_default()
                    .push(id);
            }
            id
        };
        remap.push(id);
    }
    out.indices = prim
        .indices
        .iter()
        .map(|&i| remap.get(i as usize).copied().unwrap_or(u32::MAX))
        .collect();
    Ok(out)
}

fn compatible_candidate(
    ids: &[u32],
    prim: &Primitive,
    source: usize,
    representatives: &[usize],
    preserve: bool,
) -> Option<u32> {
    ids.iter()
        .copied()
        .find(|&id| attributes_compatible(prim, source, representatives[id as usize], preserve))
}

fn attributes_compatible(prim: &Primitive, a: usize, b: usize, preserve: bool) -> bool {
    if !preserve {
        return true;
    }
    (prim.normals.len() != prim.positions.len()
        || float3_bits_equal(prim.normals[a], prim.normals[b]))
        && (prim.uvs.len() != prim.positions.len() || float2_bits_equal(prim.uvs[a], prim.uvs[b]))
        && (prim.joints.len() != prim.positions.len() || prim.joints[a] == prim.joints[b])
        && (prim.weights.len() != prim.positions.len()
            || float4_bits_equal(prim.weights[a], prim.weights[b]))
}

fn float2_bits_equal(a: [f32; 2], b: [f32; 2]) -> bool {
    a.into_iter()
        .zip(b)
        .all(|(left, right)| left.to_bits() == right.to_bits())
}

fn float3_bits_equal(a: [f32; 3], b: [f32; 3]) -> bool {
    a.into_iter()
        .zip(b)
        .all(|(left, right)| left.to_bits() == right.to_bits())
}

fn float4_bits_equal(a: [f32; 4], b: [f32; 4]) -> bool {
    a.into_iter()
        .zip(b)
        .all(|(left, right)| left.to_bits() == right.to_bits())
}

#[derive(Clone, Copy)]
#[allow(clippy::struct_excessive_bools)] // compact internal snapshot of four independent stream lengths
struct CompleteStreams {
    normals: bool,
    uvs: bool,
    joints: bool,
    weights: bool,
}

fn push_vertex(out: &mut Primitive, source: &Primitive, index: usize, streams: CompleteStreams) {
    out.positions.push(source.positions[index]);
    if streams.normals {
        out.normals.push(source.normals[index]);
    }
    if streams.uvs {
        out.uvs.push(source.uvs[index]);
    }
    if streams.joints {
        out.joints.push(source.joints[index]);
    }
    if streams.weights {
        out.weights.push(source.weights[index]);
    }
}

fn filter_triangles(prim: &mut Primitive, config: &CleanupConfig, measured: &mut CleanupChanges) {
    let mut clean = Vec::with_capacity(prim.indices.len());
    // Match the audit's topology definition exactly: render vertices may stay split for normals/UVs,
    // while face validity and duplicate detection reason over position-welded topology. Without this,
    // a seam-split micro face could survive Repair and then be reported as still degenerate by Inspect.
    let mut welder = PositionWelder::new(config.weld_threshold);
    let topology_ids: Vec<Option<usize>> = prim
        .positions
        .iter()
        .map(|&position| welder.insert(position))
        .collect();
    let mut seen = BTreeSet::<[usize; 3]>::new();
    for tri in prim.indices.chunks_exact(3) {
        let raw = [tri[0], tri[1], tri[2]];
        if raw.iter().any(|&i| (i as usize) >= prim.positions.len()) {
            measured.invalid_triangles_removed += 1;
            continue;
        }
        let Some(topology) = welded_triangle(
            &topology_ids,
            [raw[0] as usize, raw[1] as usize, raw[2] as usize],
        ) else {
            // A face referencing a non-finite position can never be emitted safely, independent of the
            // optional geometric-degenerate policy.
            measured.invalid_triangles_removed += 1;
            continue;
        };
        let p = [
            prim.positions[raw[0] as usize],
            prim.positions[raw[1] as usize],
            prim.positions[raw[2] as usize],
        ];
        let degenerate = topology[0] == topology[1]
            || topology[1] == topology[2]
            || topology[0] == topology[2]
            || triangle_twice_area(p[0], p[1], p[2]) <= config.degenerate_area_epsilon;
        if degenerate && config.remove_degenerate_triangles {
            measured.degenerate_triangles_removed += 1;
            continue;
        }
        let canonical = canonical_triangle(topology);
        if config.remove_duplicate_triangles && !seen.insert(canonical) {
            measured.duplicate_triangles_removed += 1;
            continue;
        }
        clean.extend_from_slice(&raw);
    }
    // A trailing 1-2 indices cannot form geometry and is always discarded.
    prim.indices = clean;
}

fn remove_small_components(prim: &mut Primitive, minimum: usize, measured: &mut CleanupChanges) {
    let faces: Vec<[u32; 3]> = prim
        .indices
        .chunks_exact(3)
        .map(|t| [t[0], t[1], t[2]])
        .collect();
    if faces.is_empty() {
        return;
    }
    let mut vertex_faces: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (fi, face) in faces.iter().enumerate() {
        for &v in face {
            vertex_faces.entry(v).or_default().push(fi);
        }
    }
    let mut component = vec![usize::MAX; faces.len()];
    let mut sizes = Vec::new();
    for start in 0..faces.len() {
        if component[start] != usize::MAX {
            continue;
        }
        let id = sizes.len();
        let mut queue = VecDeque::from([start]);
        component[start] = id;
        let mut size = 0;
        while let Some(face) = queue.pop_front() {
            size += 1;
            for &vertex in &faces[face] {
                if let Some(neighbors) = vertex_faces.get(&vertex) {
                    for &next in neighbors {
                        if component[next] == usize::MAX {
                            component[next] = id;
                            queue.push_back(next);
                        }
                    }
                }
            }
        }
        sizes.push(size);
    }
    let mut indices = Vec::new();
    for (fi, face) in faces.iter().enumerate() {
        if sizes[component[fi]] < minimum {
            measured.small_component_triangles_removed += 1;
        } else {
            indices.extend_from_slice(face);
        }
    }
    prim.indices = indices;
}

#[derive(Clone, Copy)]
struct DirectedEdge {
    face: usize,
    forward: bool,
}

fn repair_winding(prim: &mut Primitive, measured: &mut CleanupChanges, warnings: &mut Vec<String>) {
    let mut faces: Vec<[u32; 3]> = prim
        .indices
        .chunks_exact(3)
        .map(|t| [t[0], t[1], t[2]])
        .collect();
    if faces.is_empty() {
        return;
    }
    let mut edges: BTreeMap<(u32, u32), Vec<DirectedEdge>> = BTreeMap::new();
    for (face, tri) in faces.iter().enumerate() {
        for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            let key = if a < b { (a, b) } else { (b, a) };
            edges.entry(key).or_default().push(DirectedEdge {
                face,
                forward: a < b,
            });
        }
    }
    let mut adjacency = vec![Vec::<(usize, bool)>::new(); faces.len()];
    let mut ambiguous_faces = BTreeSet::new();
    for incidents in edges.values() {
        if incidents.len() == 2 {
            let a = incidents[0];
            let b = incidents[1];
            let relation = a.forward == b.forward;
            adjacency[a.face].push((b.face, relation));
            adjacency[b.face].push((a.face, relation));
        } else if incidents.len() > 2 {
            ambiguous_faces.extend(incidents.iter().map(|e| e.face));
        }
    }
    let mut parity = vec![None; faces.len()];
    for start in 0..faces.len() {
        if parity[start].is_some() {
            continue;
        }
        let mut members = Vec::new();
        let mut queue = VecDeque::from([start]);
        parity[start] = Some(false);
        let mut conflict = false;
        while let Some(face) = queue.pop_front() {
            members.push(face);
            let current = parity[face].unwrap_or(false);
            for &(next, differs) in &adjacency[face] {
                let desired = current ^ differs;
                if let Some(existing) = parity[next] {
                    if existing != desired {
                        conflict = true;
                    }
                } else {
                    parity[next] = Some(desired);
                    queue.push_back(next);
                }
            }
        }
        if conflict || members.iter().any(|f| ambiguous_faces.contains(f)) {
            for member in members {
                parity[member] = Some(false);
            }
            warnings.push(
                "winding repair skipped a non-manifold or contradictory component because orientation was ambiguous"
                    .into(),
            );
        }
    }
    for (face, flip) in faces.iter_mut().zip(parity.iter()) {
        if flip == &Some(true) {
            face.swap(1, 2);
            measured.triangles_flipped += 1;
        }
    }

    // For each now-consistent closed component, signed volume reliably distinguishes inward from outward.
    let mut face_components = vec![usize::MAX; faces.len()];
    let mut components = Vec::<Vec<usize>>::new();
    for start in 0..faces.len() {
        if face_components[start] != usize::MAX {
            continue;
        }
        let id = components.len();
        let mut queue = VecDeque::from([start]);
        face_components[start] = id;
        let mut members = Vec::new();
        while let Some(face) = queue.pop_front() {
            members.push(face);
            for &(next, _) in &adjacency[face] {
                if face_components[next] == usize::MAX {
                    face_components[next] = id;
                    queue.push_back(next);
                }
            }
        }
        components.push(members);
    }
    for members in components {
        let member_set: BTreeSet<usize> = members.iter().copied().collect();
        let closed = edges.values().all(|incidents| {
            let count = incidents
                .iter()
                .filter(|incident| member_set.contains(&incident.face))
                .count();
            count == 0 || count == 2
        });
        if !closed {
            continue;
        }
        let volume6: f64 = members
            .iter()
            .map(|&face| {
                let tri = faces[face];
                let a = prim.positions[tri[0] as usize];
                let b = prim.positions[tri[1] as usize];
                let c = prim.positions[tri[2] as usize];
                f64::from(dot(a, cross(b, c)))
            })
            .sum();
        if volume6 < -1.0e-12 {
            for face in members {
                faces[face].swap(1, 2);
                measured.triangles_flipped += 1;
            }
        } else if volume6.abs() <= 1.0e-12 {
            warnings.push(
                "closed component has near-zero signed volume; outward winding is indeterminate"
                    .into(),
            );
        }
    }
    prim.indices = faces.into_iter().flatten().collect();
}

fn compact_vertices(prim: &mut Primitive, measured: &mut CleanupChanges) {
    let mut used = vec![false; prim.positions.len()];
    for &index in &prim.indices {
        if let Some(value) = used.get_mut(index as usize) {
            *value = true;
        }
    }
    let removed = used.iter().filter(|&&value| !value).count();
    if removed == 0 {
        return;
    }
    measured.isolated_vertices_removed += removed;
    let mut remap = vec![u32::MAX; prim.positions.len()];
    let complete_normals = prim.normals.len() == prim.positions.len();
    let complete_uvs = prim.uvs.len() == prim.positions.len();
    let complete_joints = prim.joints.len() == prim.positions.len();
    let complete_weights = prim.weights.len() == prim.positions.len();
    let mut positions = Vec::with_capacity(prim.positions.len() - removed);
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut joints = Vec::new();
    let mut weights = Vec::new();
    for (old, &keep) in used.iter().enumerate() {
        if !keep {
            continue;
        }
        remap[old] = positions.len() as u32;
        positions.push(prim.positions[old]);
        if complete_normals {
            normals.push(prim.normals[old]);
        }
        if complete_uvs {
            uvs.push(prim.uvs[old]);
        }
        if complete_joints {
            joints.push(prim.joints[old]);
        }
        if complete_weights {
            weights.push(prim.weights[old]);
        }
    }
    for index in &mut prim.indices {
        *index = remap[*index as usize];
    }
    prim.positions = positions;
    prim.normals = normals;
    prim.uvs = uvs;
    prim.joints = joints;
    prim.weights = weights;
}

fn repair_normals(
    prim: &mut Primitive,
    mode: NormalRepairMode,
    measured: &mut CleanupChanges,
    warnings: &mut Vec<String>,
) {
    let complete = prim.normals.len() == prim.positions.len();
    let valid = complete
        && prim.normals.iter().all(|n| {
            if !n.iter().all(|v| v.is_finite()) {
                return false;
            }
            let length = dot(*n, *n).sqrt();
            length > 1.0e-12 && (length - 1.0).abs() <= 1.0e-3
        });
    let recompute = match mode {
        NormalRepairMode::Preserve => false,
        NormalRepairMode::MissingOrInvalid => !valid,
        NormalRepairMode::RecomputeAreaWeightedSmooth => true,
    };
    if !recompute {
        return;
    }
    let mut normals = vec![[0.0_f32; 3]; prim.positions.len()];
    for tri in prim.indices.chunks_exact(3) {
        let ids = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        let face = cross(
            sub(prim.positions[ids[1]], prim.positions[ids[0]]),
            sub(prim.positions[ids[2]], prim.positions[ids[0]]),
        );
        for id in ids {
            normals[id][0] += face[0];
            normals[id][1] += face[1];
            normals[id][2] += face[2];
        }
    }
    let mut zero = 0;
    for n in &mut normals {
        let length = dot(*n, *n).sqrt();
        if length > 1.0e-12 {
            n[0] /= length;
            n[1] /= length;
            n[2] /= length;
        } else {
            *n = [0.0, 1.0, 0.0];
            zero += 1;
        }
    }
    measured.normal_vertices_recomputed += normals.len();
    prim.normals = normals;
    warnings.push(
        "normals were regenerated with area-weighted vertex smoothing; explicit hard-edge groups are not present in MeshAsset"
            .into(),
    );
    if zero > 0 {
        warnings.push(format!(
            "{zero} vertices had no incident surface normal and received +Y fallback normals"
        ));
    }
}

fn repair_uvs(
    prim: &mut Primitive,
    mode: UvGenerationMode,
    measured: &mut CleanupChanges,
    warnings: &mut Vec<String>,
) {
    let generate = match mode {
        UvGenerationMode::None => false,
        UvGenerationMode::PlanarWhenAbsent => prim.uvs.is_empty(),
        UvGenerationMode::ReplaceIncompleteWithPlanar => prim.uvs.len() != prim.positions.len(),
    };
    if !generate || prim.positions.is_empty() {
        return;
    }
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in &prim.positions {
        for axis in 0..3 {
            min[axis] = min[axis].min(p[axis]);
            max[axis] = max[axis].max(p[axis]);
        }
    }
    let dimensions = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
    let mut axes = [0_usize, 1, 2];
    axes.sort_by(|&a, &b| dimensions[b].total_cmp(&dimensions[a]).then(a.cmp(&b)));
    let (u_axis, v_axis) = (axes[0], axes[1]);
    prim.uvs = prim
        .positions
        .iter()
        .map(|p| {
            let project = |axis: usize| {
                let extent = dimensions[axis];
                if extent > 0.0 {
                    (p[axis] - min[axis]) / extent
                } else {
                    0.5
                }
            };
            [project(u_axis), project(v_axis)]
        })
        .collect();
    measured.uv_vertices_generated += prim.uvs.len();
    warnings.push(format!(
        "generated UV0 by bounds-normalized planar projection on local axes {u_axis}/{v_axis}; folded surfaces may overlap and no chart packing or texel-density equalization was performed"
    ));
}

fn cleanup_change_lines(changes: &CleanupChanges) -> Vec<String> {
    let mut out = Vec::new();
    for (count, label) in [
        (changes.vertices_welded, "vertices welded"),
        (
            changes.isolated_vertices_removed,
            "isolated vertices removed",
        ),
        (
            changes.invalid_triangles_removed,
            "invalid triangles removed",
        ),
        (
            changes.degenerate_triangles_removed,
            "degenerate triangles removed",
        ),
        (
            changes.duplicate_triangles_removed,
            "duplicate triangles removed",
        ),
        (
            changes.small_component_triangles_removed,
            "small disconnected-component triangles removed",
        ),
        (changes.triangles_flipped, "triangle windings flipped"),
        (
            changes.normal_vertices_recomputed,
            "vertex normals recomputed",
        ),
        (changes.uv_vertices_generated, "UV vertices generated"),
        (changes.empty_primitives_removed, "empty primitives removed"),
    ] {
        if count > 0 {
            out.push(format!("{count} {label}"));
        }
    }
    if out.is_empty() {
        out.push("no geometry changes were required".into());
    }
    out
}

/// Produce a material-, seam-, silhouette- and rig-aware QEM derivative.
pub fn optimize_asset(
    asset: &MeshAsset,
    config: &OptimizationConfig,
) -> Result<OptimizationResult, AssetLabError> {
    let requested_ratio = config
        .target_ratio
        .unwrap_or_else(|| config.preset.target_ratio());
    if !requested_ratio.is_finite() || !(0.0..1.0).contains(&requested_ratio) {
        return Err(AssetLabError::InvalidConfig(
            "target_ratio must be finite and strictly between 0 and 1".into(),
        ));
    }
    let before = audit_asset(asset, &AuditOptions::default())?;
    if before.valid_triangles == 0 {
        return Err(AssetLabError::InvalidInput(
            "asset contains no valid triangles".into(),
        ));
    }
    if before.invalid_indices > 0
        || before.trailing_indices > 0
        || before.invalid_positions > 0
        || before.degenerate_triangles > 0
        || before.duplicate_triangles > 0
    {
        return Err(AssetLabError::InvalidInput(
            "run cleanup before optimization; malformed, duplicate, or degenerate geometry remains"
                .into(),
        ));
    }
    if before.materials.invalid_primitive_assignments > 0
        || before.materials.invalid_texture_references > 0
        || before.textures.invalid_layouts > 0
    {
        return Err(AssetLabError::InvalidInput(
            "repair invalid material assignments and texture payloads before optimization".into(),
        ));
    }
    // meshoptimizer's error includes the weighted normal/UV/skin attribute
    // dimensions below, not geometry alone. Sub-percent values effectively
    // forbid every collapse on ordinary textured grids. These preset bounds
    // remain conservative while producing a useful derivative; semantic
    // borders, seams, silhouettes and joint-support changes are still exact
    // locks and the measured result error is reported to the caller.
    let target_error = match config.preset {
        OptimizationPreset::Cinematic | OptimizationPreset::HighQuality => 0.02,
        OptimizationPreset::Desktop => 0.05,
        OptimizationPreset::Balanced | OptimizationPreset::Web => 0.10,
        OptimizationPreset::Mobile => 0.20,
        OptimizationPreset::Draft => 0.30,
    };
    let simplified = MeshoptQemSimplifier
        .simplify(
            asset,
            &SimplifyConfig {
                target_ratio: requested_ratio,
                target_error,
                normal_weight: 1.0,
                uv_weight: 2.0,
                skin_weight: 4.0,
                lock_borders: true,
                preserve_attribute_seams: true,
                preserve_skinning: true,
                hard_edge_angle_degrees: 60.0,
                regularize_rigged: true,
            },
        )
        .map_err(|error| AssetLabError::Unsupported(error.to_string()))?;
    if simplified.report.output_triangles >= simplified.report.source_triangles {
        return Err(AssetLabError::Unsupported(
            "semantic seam and silhouette locks leave no invariant-safe triangle reduction at this target"
                .into(),
        ));
    }
    let mut output = simplified.asset;
    output.name = format!("{} (QEM optimized)", asset.name);
    let after = audit_asset(&output, &AuditOptions::default())?;
    let chosen_report = LodCandidateReport {
        level: 1,
        vertices: output.vertex_count(),
        triangles: simplified.report.output_triangles,
        ratio: simplified.report.achieved_ratio,
    };
    let mut warnings = Vec::new();
    if (simplified.report.achieved_ratio - requested_ratio).abs() > 0.10 {
        warnings.push(format!(
            "semantic locks achieved {:.1}% rather than the requested {:.1}% triangle ratio",
            simplified.report.achieved_ratio * 100.0,
            requested_ratio * 100.0
        ));
    }
    warnings.push(simplified.report.determinism.clone());
    let changes = vec![format!(
        "QEM reduced {} to {} triangles ({:.1}% of source), locked {} semantic vertices, max relative error {:.6}",
        simplified.report.source_triangles,
        simplified.report.output_triangles,
        simplified.report.achieved_ratio * 100.0,
        simplified.report.locked_vertices,
        simplified.report.max_result_error,
    )];
    Ok(OptimizationResult {
        asset: output,
        requested_ratio,
        achieved_ratio: simplified.report.achieved_ratio,
        chosen_level: 1,
        candidates: vec![chosen_report],
        report: AssetChangeReport {
            before,
            after,
            changes,
            warnings,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrocalk_assets::{GltfImporter, Material, MeshSource, Texture};
    use metrocalk_skeleton::Skeleton;

    fn defective_asset() -> MeshAsset {
        MeshAsset {
            name: "defective".into(),
            primitives: vec![Primitive {
                positions: vec![
                    [0.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    [0.0, 0.0, 0.0], // duplicate vertex
                    [8.0, 8.0, 8.0], // isolated vertex
                ],
                normals: vec![[0.0, 0.0, 0.0]; 5],
                uvs: Vec::new(),
                tangents: Vec::new(),
                indices: vec![
                    0, 1, 2, // good
                    3, 1, 2, // duplicate after weld
                    0, 0, 1, // degenerate
                    0, 1, 99, // invalid
                    0,  // trailing
                ],
                material: 0,
                joints: Vec::new(),
                weights: Vec::new(),
            }],
            materials: vec![Material::default()],
            textures: Vec::new(),
            skeleton: None,
        }
    }

    fn grid_primitive(n: usize, x_offset: f32, material: usize) -> Primitive {
        let mut positions = Vec::with_capacity(n * n);
        for y in 0..n {
            for x in 0..n {
                positions.push([
                    x_offset + x as f32 / (n - 1) as f32,
                    y as f32 / (n - 1) as f32,
                    0.0,
                ]);
            }
        }
        let mut indices = Vec::new();
        for y in 0..n - 1 {
            for x in 0..n - 1 {
                let a = (y * n + x) as u32;
                let b = a + 1;
                let c = a + n as u32;
                let d = c + 1;
                indices.extend_from_slice(&[a, b, c, b, d, c]);
            }
        }
        Primitive {
            positions,
            normals: Vec::new(),
            uvs: Vec::new(),
            tangents: Vec::new(),
            indices,
            material,
            joints: Vec::new(),
            weights: Vec::new(),
        }
    }

    #[test]
    fn audit_measures_malformed_geometry_without_panicking() {
        let report = audit_asset(&defective_asset(), &AuditOptions::default()).expect("audit");
        assert_eq!(report.logical_objects, 1);
        assert_eq!(report.primitives, 1);
        assert_eq!(report.vertices, 5);
        assert_eq!(report.duplicate_position_vertices, 1);
        assert_eq!(report.isolated_vertices, 1);
        assert_eq!(report.trailing_indices, 1);
        assert_eq!(report.invalid_indices, 1);
        assert_eq!(report.degenerate_triangles, 1);
        assert_eq!(report.duplicate_triangles, 1);
        assert_eq!(report.normals.zero_length_vectors, 5);
        assert_eq!(report.uvs.missing_vertices, 5);
        assert!(report.estimated_bytes.total_payload > 0);
        assert_eq!(
            report.capabilities.high_to_low_texture_baking.state,
            AvailabilityState::Missing
        );
    }

    #[test]
    fn cleanup_is_immutable_measured_and_repairs_the_defective_fixture() {
        let source = defective_asset();
        let untouched = source.clone();
        let result = cleanup_asset(
            &source,
            &CleanupConfig {
                uv_generation: UvGenerationMode::PlanarWhenAbsent,
                ..CleanupConfig::default()
            },
        )
        .expect("cleanup");
        assert_eq!(source, untouched, "input remains immutable");
        assert_eq!(result.asset.primitives.len(), 1);
        assert_eq!(result.asset.primitives[0].positions.len(), 3);
        assert_eq!(result.asset.primitives[0].indices.len(), 3);
        assert_eq!(result.measured.vertices_welded, 1);
        assert_eq!(result.measured.invalid_triangles_removed, 1);
        assert_eq!(result.measured.degenerate_triangles_removed, 1);
        assert_eq!(result.measured.duplicate_triangles_removed, 1);
        assert_eq!(result.measured.isolated_vertices_removed, 1);
        assert_eq!(result.measured.normal_vertices_recomputed, 3);
        assert_eq!(result.measured.uv_vertices_generated, 3);
        assert_eq!(result.report.after.invalid_indices, 0);
        assert_eq!(result.report.after.degenerate_triangles, 0);
        assert_eq!(result.report.after.duplicate_triangles, 0);
        assert_eq!(result.report.after.isolated_vertices, 0);
        assert_eq!(result.report.after.uvs.complete_primitives, 1);
    }

    #[test]
    fn topology_reports_non_manifold_edges_and_connected_components() {
        let asset = MeshAsset {
            name: "non-manifold".into(),
            primitives: vec![Primitive {
                positions: vec![
                    [0.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    [0.0, -1.0, 0.0],
                    [0.0, 0.0, 1.0],
                    [10.0, 0.0, 0.0],
                    [11.0, 0.0, 0.0],
                    [10.0, 1.0, 0.0],
                ],
                indices: vec![0, 1, 2, 1, 0, 3, 0, 1, 4, 5, 6, 7],
                material: 0,
                ..Primitive::default()
            }],
            materials: vec![Material::default()],
            ..MeshAsset::default()
        };
        let report = audit_asset(&asset, &AuditOptions::default()).expect("audit");
        assert_eq!(report.non_manifold_edges, 1);
        assert_eq!(report.connected_components, 2);
        assert!(report.boundary_edges > 0);
    }

    #[test]
    fn uv_overlap_signal_detects_positive_area_but_not_shared_edges() {
        let mut overlap = Primitive {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 1.0],
                [0.0, 1.0, 1.0],
            ],
            uvs: vec![
                [0.0, 0.0],
                [1.0, 0.0],
                [0.0, 1.0],
                [0.0, 0.0],
                [1.0, 0.0],
                [0.0, 1.0],
            ],
            indices: vec![0, 1, 2, 3, 4, 5],
            material: 0,
            ..Primitive::default()
        };
        let asset = MeshAsset {
            name: "uv-overlap".into(),
            primitives: vec![overlap.clone()],
            materials: vec![Material::default()],
            ..MeshAsset::default()
        };
        assert_eq!(
            audit_asset(&asset, &AuditOptions::default())
                .expect("audit")
                .uvs
                .overlap
                .state,
            UvOverlapState::Detected
        );

        overlap.positions.truncate(4);
        overlap.uvs = vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]];
        overlap.indices = vec![0, 1, 2, 1, 3, 2];
        let shared_edge = MeshAsset {
            primitives: vec![overlap],
            materials: vec![Material::default()],
            ..MeshAsset::default()
        };
        assert_eq!(
            audit_asset(&shared_edge, &AuditOptions::default())
                .expect("audit")
                .uvs
                .overlap
                .state,
            UvOverlapState::Clear
        );
    }

    #[test]
    fn uv_budget_reports_inconclusive_instead_of_false_clear() {
        let primitive = grid_primitive(8, 0.0, 0);
        let mut with_uv = primitive;
        with_uv.uvs = with_uv.positions.iter().map(|p| [p[0], p[1]]).collect();
        let asset = MeshAsset {
            primitives: vec![with_uv],
            materials: vec![Material::default()],
            ..MeshAsset::default()
        };
        let report = audit_asset(
            &asset,
            &AuditOptions {
                uv_pair_budget: 3,
                ..AuditOptions::default()
            },
        )
        .expect("audit");
        assert_eq!(report.uvs.overlap.state, UvOverlapState::Inconclusive);
        assert_eq!(report.uvs.overlap.pairs_tested, 3);
    }

    #[test]
    fn uv_broad_phase_proves_disjoint_layout_without_exact_pair_budget() {
        let triangles: Vec<UvTriangle> = (0..4_096)
            .map(|index| {
                let u = f64::from(index) * 2.0;
                UvTriangle {
                    points: [[u, 0.0], [u + 0.5, 0.0], [u, 0.5]],
                }
            })
            .collect();
        let signal = detect_uv_overlap(&triangles, 0);
        assert_eq!(signal.state, UvOverlapState::Clear);
        assert_eq!(signal.pairs_tested, 0);
        assert!(signal.method.contains("sweep-and-prune"));
    }

    #[test]
    fn optimization_is_deterministic_and_preserves_material_partitions() {
        let asset = MeshAsset {
            name: "two-material-grid".into(),
            primitives: vec![grid_primitive(65, 0.0, 0), grid_primitive(65, 2.0, 1)],
            materials: vec![Material::default(), Material::default()],
            ..MeshAsset::default()
        };
        let config = OptimizationConfig {
            preset: OptimizationPreset::Web,
            ..OptimizationConfig::default()
        };
        let a = optimize_asset(&asset, &config).expect("optimize");
        let b = optimize_asset(&asset, &config).expect("repeat");
        assert_eq!(
            a.asset, b.asset,
            "same input and controls are bit-deterministic"
        );
        assert_eq!(a.chosen_level, b.chosen_level);
        assert!(a.report.after.triangles < a.report.before.triangles);
        let materials: BTreeSet<usize> = a.asset.primitives.iter().map(|p| p.material).collect();
        assert_eq!(materials, BTreeSet::from([0, 1]));
        assert_eq!(
            a.candidates.len(),
            1,
            "the native QEM result is measured directly"
        );
    }

    #[test]
    fn packaged_dense_sphere_repairs_then_optimizes() {
        let source = GltfImporter::new()
            .import(include_bytes!("../assets/dense_sphere.glb"))
            .expect("packaged dense sphere fixture imports");
        let source_audit = audit_asset(&source, &AuditOptions::default()).expect("source audit");
        assert_eq!(source_audit.triangles, 9_216);
        assert_eq!(source_audit.valid_triangles, 9_024);
        assert_eq!(source_audit.degenerate_triangles, 192);
        assert_eq!(source_audit.uvs.complete_primitives, 0);

        let repaired = cleanup_asset(&source, &CleanupConfig::default()).expect("repair fixture");
        let optimized = optimize_asset(
            &repaired.asset,
            &OptimizationConfig {
                preset: OptimizationPreset::Web,
                target_ratio: Some(OptimizationPreset::Web.target_ratio()),
                ..OptimizationConfig::default()
            },
        )
        .expect("the repaired viewport fixture remains eligible for optimization");

        assert!(optimized.report.after.valid_triangles > 0);
        assert!(optimized.report.after.valid_triangles < repaired.report.after.valid_triangles);
        let uv_prepared = cleanup_asset(
            &optimized.asset,
            &CleanupConfig {
                uv_generation: UvGenerationMode::PlanarWhenAbsent,
                ..CleanupConfig::default()
            },
        )
        .expect("planar UV preparation and bounded overlap audit complete on the viewport fixture");
        assert_eq!(
            uv_prepared.report.after.uvs.complete_primitives,
            uv_prepared.asset.primitives.len()
        );
        assert_eq!(
            uv_prepared.report.after.uvs.overlap.state,
            UvOverlapState::Detected,
            "the folded sphere projection is honestly reported as overlapping"
        );
        assert_eq!(
            source_audit.valid_triangles, 9_024,
            "the source remains immutable"
        );
    }

    #[test]
    fn textured_and_rigged_optimization_preserves_every_parallel_payload() {
        let mut primitive = grid_primitive(17, 0.0, 0);
        primitive.uvs = primitive.positions.iter().map(|p| [p[0], p[1]]).collect();
        primitive.normals = vec![[0.0, 0.0, 1.0]; primitive.positions.len()];
        primitive.tangents = vec![[1.0, 0.0, 0.0, 1.0]; primitive.positions.len()];
        primitive.joints = vec![[0, 0, 0, 0]; primitive.positions.len()];
        primitive.weights = vec![[1.0, 0.0, 0.0, 0.0]; primitive.positions.len()];
        let asset = MeshAsset {
            primitives: vec![primitive],
            materials: vec![Material {
                base_color_texture: Some(0),
                ..Material::default()
            }],
            textures: vec![Texture {
                width: 1,
                height: 1,
                rgba8: vec![255, 255, 255, 255],
            }],
            skeleton: Some(Skeleton::default()),
            ..MeshAsset::default()
        };
        let optimized = optimize_asset(&asset, &OptimizationConfig::default())
            .expect("attribute-aware rigged QEM");
        assert!(optimized.asset.triangle_count() < asset.triangle_count());
        assert_eq!(optimized.asset.materials, asset.materials);
        assert_eq!(optimized.asset.textures, asset.textures);
        assert_eq!(optimized.asset.skeleton, asset.skeleton);
        let output = &optimized.asset.primitives[0];
        assert_eq!(output.positions.len(), output.normals.len());
        assert_eq!(output.positions.len(), output.uvs.len());
        assert_eq!(output.positions.len(), output.tangents.len());
        assert_eq!(output.positions.len(), output.joints.len());
        assert_eq!(output.positions.len(), output.weights.len());
        let report = audit_asset(&asset, &AuditOptions::default()).expect("audit");
        assert_eq!(
            report.capabilities.lod_generation.state,
            AvailabilityState::Available
        );
        assert_eq!(
            report.capabilities.texture_bake_input.state,
            AvailabilityState::Available
        );
        assert_eq!(
            report.capabilities.high_to_low_texture_baking.state,
            AvailabilityState::Available
        );
    }

    #[test]
    fn texture_material_and_byte_audit_catches_bad_references_and_layouts() {
        let asset = MeshAsset {
            primitives: vec![grid_primitive(2, 0.0, 3)],
            materials: vec![Material {
                base_color_texture: Some(1),
                ..Material::default()
            }],
            textures: vec![Texture {
                width: 2,
                height: 2,
                rgba8: vec![0; 7],
            }],
            ..MeshAsset::default()
        };
        let report = audit_asset(&asset, &AuditOptions::default()).expect("audit");
        assert_eq!(report.materials.invalid_primitive_assignments, 1);
        assert_eq!(report.materials.invalid_texture_references, 1);
        assert_eq!(report.textures.invalid_layouts, 1);
        assert_eq!(report.textures.decoded_bytes, 7);
        assert!(report.estimated_bytes.total_payload >= 7);
    }
}
