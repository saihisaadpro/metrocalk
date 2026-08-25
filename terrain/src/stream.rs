//! Streaming, culling and budgets — deciding what exists, at what detail, and what to throw away.
//!
//! ## Division of labour
//!
//! This module **plans**; it never builds and never blocks. [`Streamer::plan`] answers "which chunks should
//! be resident, at which LOD, in which order, and which should be evicted", and [`build_chunk`] is a *pure
//! function* the host runs wherever it likes — one worker thread or sixteen. Because a build is pure and
//! deterministic, the host needs no locking, no work stealing discipline and no ordering guarantees: two
//! threads building the same chunk produce the same bytes.
//!
//! `plan` is deliberately **not** on the per-frame path. It is a few hundred microseconds of set arithmetic
//! plus, optionally, horizon ray marching; the host calls it when the camera has moved meaningfully. What
//! *is* per-frame is frustum testing resident chunks, which is a handful of dot products each.
//!
//! ## Every LOD is built at once
//!
//! A chunk build produces the mesh for **every** LOD, not just the requested one. The extra cost is about a
//! third of the LOD-0 mesh, and it buys something worth much more than that: an LOD change becomes a
//! different draw call on data already in memory, so it is instant and cannot pop in late. Requesting a
//! coarse mesh, then discovering the camera has approached and rebuilding, is how terrain gets its
//! reputation for blurry surfaces that sharpen a second too late.
//!
//! Textures are the exception — they dominate memory, so exactly one resolution is baked, matching the LOD
//! the chunk was requested at, and a closer approach re-requests the bake.
//!
//! ## Culling
//!
//! Frustum culling is the ordinary six-plane AABB test, with the planes extracted from the view-projection
//! matrix (Gribb–Hartmann). Occlusion culling is a **horizon test**: terrain occludes terrain, so marching
//! the height field along the ray from the eye to a chunk's top corners answers "is this entire chunk behind
//! a hill?" exactly, on the CPU, with no depth buffer readback, no frame of latency, and no GPU round trip.
//! It is the one occlusion technique that is both cheap and *right* for this geometry; a hierarchical Z
//! buffer would generalize to arbitrary occluders and is the named upgrade if props ever need to occlude.

use std::collections::BTreeMap;

use crate::collision::{self, ChunkCollider};
use crate::field::Terrain;
use crate::material::{self, ChunkTextures};
use crate::mesh::{self, ChunkMesh, ChunkSamples, LodError};
use crate::nav::{self, NavConfig, NavGrid};
use crate::recipe::{ChunkCoord, MemoryBudget};
use crate::scatter::{self, ScatterOutput};

/// Where the camera is and what it can see.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewParams {
    /// World-space eye position.
    pub eye: [f32; 3],
    /// Column-major view-projection matrix (as `glam::Mat4::to_cols_array` produces).
    pub view_proj: [f32; 16],
    /// The chunk the physics/nav focus sits in — usually the player, not the camera.
    pub focus: ChunkCoord,
}

impl ViewParams {
    /// A view with an identity projection — for headless planning where only the eye matters. Frustum
    /// culling is skipped in this case rather than being wrong.
    #[must_use]
    pub fn at(eye: [f32; 3], focus: ChunkCoord) -> Self {
        Self {
            eye,
            view_proj: [0.0; 16],
            focus,
        }
    }

    /// Whether a real projection was supplied.
    #[must_use]
    pub fn has_projection(&self) -> bool {
        self.view_proj.iter().any(|v| *v != 0.0)
    }
}

/// Six normalized frustum planes, inward-facing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frustum {
    /// `(a, b, c, d)` per plane; a point is inside when `a·x + b·y + c·z + d >= 0` for all six.
    pub planes: [[f32; 4]; 6],
}

impl Frustum {
    /// Extract the planes from a column-major view-projection matrix.
    #[must_use]
    pub fn from_view_proj(m: &[f32; 16]) -> Self {
        // Row `i` of a column-major matrix is at indices i, 4+i, 8+i, 12+i.
        let row = |i: usize| [m[i], m[4 + i], m[8 + i], m[12 + i]];
        let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
        let add = |a: [f32; 4], b: [f32; 4]| [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]];
        let sub = |a: [f32; 4], b: [f32; 4]| [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]];
        // wgpu clip space keeps z in [0, w], so near is `z >= 0` rather than `z >= -w`.
        let raw = [
            add(r3, r0),
            sub(r3, r0),
            add(r3, r1),
            sub(r3, r1),
            r2,
            sub(r3, r2),
        ];
        let mut planes = [[0.0f32; 4]; 6];
        for (out, p) in planes.iter_mut().zip(raw) {
            let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            let inv = if len > 1e-12 { 1.0 / len } else { 0.0 };
            *out = [p[0] * inv, p[1] * inv, p[2] * inv, p[3] * inv];
        }
        Self { planes }
    }

    /// Whether an axis-aligned box is at least partly inside.
    ///
    /// Uses the "positive vertex" test: for each plane, the box corner furthest along the plane normal. If
    /// even that corner is outside, no part of the box can be inside.
    #[must_use]
    pub fn intersects_aabb(&self, min: [f32; 3], max: [f32; 3]) -> bool {
        for p in &self.planes {
            let vx = if p[0] >= 0.0 { max[0] } else { min[0] };
            let vy = if p[1] >= 0.0 { max[1] } else { min[1] };
            let vz = if p[2] >= 0.0 { max[2] } else { min[2] };
            if p[0] * vx + p[1] * vy + p[2] * vz + p[3] < 0.0 {
                return false;
            }
        }
        true
    }
}

/// What the host should build.
///
/// Note that the three resolutions are chosen by three *different* criteria, because they answer three
/// different questions:
///
/// * **mesh LOD** — measured geometric error against a pixel budget ([`mesh::choose_lod`]). Not in this
///   struct at all: every LOD's mesh is built, so the choice is made per frame on data already resident.
/// * **texture LOD** ([`Self::lod`]) — how many pixels the chunk covers on screen, which is what decides
///   whether a 256² bake is being magnified or thrown away. Geometric error is irrelevant to it: a flat
///   chunk right under the camera still wants a sharp texture.
/// * **collider LOD** ([`Self::collider_lod`]) — integer chunk rings, never a float distance, because
///   collider resolution changes what the simulation computes and must be bit-identical everywhere.
#[derive(Clone, Debug, PartialEq)]
pub struct ChunkRequest {
    /// Which chunk.
    pub coord: ChunkCoord,
    /// Texture LOD: the bake resolution, chosen from the chunk's on-screen pixel coverage.
    pub lod: u8,
    /// Bake the splat albedo.
    pub want_textures: bool,
    /// Also bake the detail normal map (only worth it near the camera).
    pub want_normals: bool,
    /// Place scattered instances.
    pub want_scatter: bool,
    /// Collider LOD, or `None` for no collider. Chosen by integer ring distance from the physics focus.
    pub collider_lod: Option<u8>,
    /// Build a navigation grid.
    pub want_nav: bool,
    /// Lower is more urgent — the host sorts by this and takes as many as its frame budget allows.
    pub priority: f32,
}

/// Everything a chunk build produced.
#[derive(Clone, Debug, PartialEq)]
pub struct ChunkBuild {
    /// Which chunk.
    pub coord: ChunkCoord,
    /// The one field pass every artefact came from, retained so a LOD change can re-derive the collider or
    /// nav grid without touching the field again.
    pub samples: ChunkSamples,
    /// One mesh per LOD, index = LOD.
    pub meshes: Vec<ChunkMesh>,
    /// Measured geometric error per LOD.
    pub errors: Vec<LodError>,
    /// Baked textures, and the LOD their resolution matches.
    pub textures: Option<ChunkTextures>,
    /// The LOD `textures` was baked for.
    pub texture_lod: u8,
    /// Scattered instances.
    pub scatter: ScatterOutput,
    /// Height-field collider.
    pub collider: Option<ChunkCollider>,
    /// Navigation grid.
    pub nav: Option<NavGrid>,
    /// Water surface for the chunk, when any of it is submerged.
    pub water: Option<mesh::MeshData>,
}

impl ChunkBuild {
    /// Total heap bytes.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.samples.bytes()
            + self.meshes.iter().map(ChunkMesh::bytes).sum::<usize>()
            + self.textures.as_ref().map_or(0, ChunkTextures::bytes)
            + self.scatter.bytes()
            + self.collider.as_ref().map_or(0, ChunkCollider::bytes)
            + self.nav.as_ref().map_or(0, NavGrid::bytes)
            + self.water.as_ref().map_or(0, mesh::MeshData::bytes)
    }

    /// Triangles across every LOD (a diagnostic, not a draw cost — only one LOD is drawn).
    #[must_use]
    pub fn total_triangles(&self) -> usize {
        self.meshes.iter().map(|m| m.data.tri_count()).sum()
    }
}

/// Build one chunk. Pure, deterministic, thread-safe — the host may call this from any worker.
#[must_use]
pub fn build_chunk(terrain: &Terrain, req: &ChunkRequest, nav_cfg: &NavConfig) -> ChunkBuild {
    let recipe = terrain.recipe();
    let samples = terrain.sample_chunk_unchecked(req.coord);
    let levels = recipe.lod.levels.max(1);
    let errors = mesh::measure_all_lods(&samples, levels);
    let meshes: Vec<ChunkMesh> = (0..levels)
        .map(|lod| mesh::build_chunk_mesh_with_errors(&samples, lod, &recipe.lod, &errors))
        .collect();
    let textures = req.want_textures.then(|| {
        material::bake_chunk_textures(
            terrain,
            &samples,
            recipe.texture_res_at_lod(req.lod),
            req.want_normals,
        )
    });
    let scatter = if req.want_scatter {
        scatter::scatter_chunk(terrain, &samples)
    } else {
        ScatterOutput {
            coord: req.coord,
            ..ScatterOutput::default()
        }
    };
    let collider = req
        .collider_lod
        .map(|lod| collision::chunk_collider(&samples, lod));
    let nav = req
        .want_nav
        .then(|| nav::build_nav_grid(terrain, &samples, nav_cfg));
    let water = if recipe.water.enabled {
        mesh::build_water_mesh(&samples, recipe.water.sea_level_m)
    } else {
        None
    };
    ChunkBuild {
        coord: req.coord,
        samples,
        meshes,
        errors,
        textures,
        texture_lod: req.lod,
        scatter,
        collider,
        nav,
        water,
    }
}

/// A resident chunk's bookkeeping.
#[derive(Clone, Debug, PartialEq)]
pub struct Resident {
    /// The LOD currently drawn.
    pub lod: u8,
    /// The LOD the textures were baked for.
    pub texture_lod: u8,
    /// Measured errors per LOD.
    pub errors: Vec<LodError>,
    /// World AABB (LOD 0's, which includes the deepest skirt).
    pub bounds_min: [f32; 3],
    /// World AABB maximum.
    pub bounds_max: [f32; 3],
    /// Heap bytes this chunk holds.
    pub bytes: usize,
    /// Plan tick when it was last inside the view distance — the LRU key.
    pub last_seen: u64,
    /// Whether the last plan found it visible.
    pub visible: bool,
}

/// The plan for this tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreamPlan {
    /// Chunks to build, most urgent first.
    pub load: Vec<ChunkRequest>,
    /// Chunks to drop.
    pub unload: Vec<ChunkCoord>,
    /// Resident chunks that survived culling, with the LOD to draw.
    pub visible: Vec<(ChunkCoord, u8)>,
    /// Resident chunks whose drawn LOD changed this tick.
    pub lod_changed: Vec<(ChunkCoord, u8)>,
    /// Chunks rejected by the frustum test.
    pub culled_frustum: usize,
    /// Chunks rejected by the horizon test.
    pub culled_horizon: usize,
    /// Bytes freed by eviction.
    pub evicted_bytes: usize,
    /// True when eviction could not get under the ceiling, or the view distance had to be cut to fit it —
    /// the honest signal that the budget is smaller than the world that was asked for.
    pub over_budget: bool,
    /// The view distance actually used, which is the requested one unless the budget could not afford it.
    pub effective_view_distance_m: f32,
}

/// Live counters for the profiler.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StreamStats {
    /// Resident chunk count.
    pub resident: usize,
    /// Total resident bytes.
    pub bytes: usize,
    /// Chunks drawn after culling.
    pub visible: usize,
    /// Cumulative loads requested.
    pub loads: u64,
    /// Cumulative evictions.
    pub evictions: u64,
    /// Cumulative LOD changes.
    pub lod_changes: u64,
}

/// The residency planner.
#[derive(Clone, Debug)]
pub struct Streamer {
    resident: BTreeMap<ChunkCoord, Resident>,
    pending: BTreeMap<ChunkCoord, u8>,
    budget: MemoryBudget,
    tick: u64,
    stats: StreamStats,
    horizon_cursor: usize,
}

/// Chunks the horizon test may examine per plan, so occlusion never becomes the expensive part.
///
/// Each test marches five rays of twelve field samples, so this is the dominant cost of a plan: at 48 it
/// measured 1.4 ms of the 2.2 ms p99. Sixteen keeps a plan comfortably under a millisecond and still sweeps
/// the whole resident set within a second of plans, which is far faster than terrain occlusion changes.
const HORIZON_TESTS_PER_PLAN: usize = 16;
/// Samples along a horizon ray.
const HORIZON_SAMPLES: usize = 12;
/// Chunks nearer than this are never horizon-tested — the payoff is not worth the rays.
const HORIZON_MIN_DISTANCE_M: f32 = 96.0;

impl Streamer {
    /// A planner with a memory ceiling.
    #[must_use]
    pub fn new(budget: MemoryBudget) -> Self {
        Self {
            resident: BTreeMap::new(),
            pending: BTreeMap::new(),
            budget,
            tick: 0,
            stats: StreamStats::default(),
            horizon_cursor: 0,
        }
    }

    /// Current counters.
    #[must_use]
    pub fn stats(&self) -> StreamStats {
        self.stats
    }

    /// The resident set.
    #[must_use]
    pub fn resident(&self) -> &BTreeMap<ChunkCoord, Resident> {
        &self.resident
    }

    /// Whether a chunk is resident.
    #[must_use]
    pub fn is_resident(&self, c: ChunkCoord) -> bool {
        self.resident.contains_key(&c)
    }

    /// Record a completed build. Call this when a worker's result arrives.
    pub fn insert(&mut self, build: &ChunkBuild) {
        let bounds = build
            .meshes
            .first()
            .map_or(([0.0, 0.0, 0.0], [0.0, 0.0, 0.0]), |m| {
                (m.bounds_min, m.bounds_max)
            });
        self.pending.remove(&build.coord);
        self.resident.insert(
            build.coord,
            Resident {
                lod: build.texture_lod,
                texture_lod: build.texture_lod,
                errors: build.errors.clone(),
                bounds_min: bounds.0,
                bounds_max: bounds.1,
                bytes: build.bytes(),
                last_seen: self.tick,
                visible: false,
            },
        );
        self.refresh_bytes();
    }

    /// Take back a request the host could not start.
    ///
    /// `plan` marks what it asked for so the next plan does not ask twice — but a host with a bounded work
    /// queue will not start everything it is handed. Without this the unstarted requests stay marked for
    /// ever and are never asked for again: on the live app that pinned a two-kilometre world at the first
    /// thirty-two chunks and left the rest of the landscape missing.
    pub fn defer(&mut self, c: ChunkCoord) {
        self.pending.remove(&c);
    }

    /// Forget a chunk (after the host has released its GPU resources).
    pub fn remove(&mut self, c: ChunkCoord) {
        self.resident.remove(&c);
        self.pending.remove(&c);
        self.refresh_bytes();
    }

    /// Forget every chunk overlapping a world rectangle, returning what was dropped.
    ///
    /// This is what makes a sculpt gesture feel live. An edit changes the field only inside the brush, so
    /// re-streaming the whole world for it would turn a paint stroke into a second-long stall; dropping the
    /// handful of chunks the brush actually touched rebuilds them on the next plan and leaves the rest of
    /// the landscape exactly where it is.
    pub fn invalidate_region(
        &mut self,
        recipe: &crate::recipe::TerrainRecipe,
        min_xz: [f32; 2],
        max_xz: [f32; 2],
    ) -> Vec<ChunkCoord> {
        let lo = recipe.chunk_at(min_xz[0], min_xz[1]);
        let hi = recipe.chunk_at(max_xz[0], max_xz[1]);
        let hit: Vec<ChunkCoord> = self
            .resident
            .keys()
            .copied()
            .filter(|c| c.x >= lo.x && c.x <= hi.x && c.z >= lo.z && c.z <= hi.z)
            .collect();
        for c in &hit {
            self.resident.remove(c);
            self.pending.remove(c);
        }
        self.refresh_bytes();
        hit
    }

    fn refresh_bytes(&mut self) {
        self.stats.resident = self.resident.len();
        self.stats.bytes = self.resident.values().map(|r| r.bytes).sum();
    }

    /// Plan this tick.
    ///
    /// Not a per-frame call — see the module docs. `terrain` is needed for the horizon test; pass a view
    /// without a projection to skip frustum culling (headless planning).
    #[allow(clippy::too_many_lines)] // one linear pass with clearly separated phases; splitting it would hide the order
    pub fn plan(&mut self, terrain: &Terrain, view: &ViewParams) -> StreamPlan {
        self.tick += 1;
        let recipe = terrain.recipe();
        let mut plan = StreamPlan::default();
        let requested = recipe.lod.max_view_distance_m.max(recipe.chunk_size_m);
        let view_dist = self.affordable_view_distance(recipe, requested);
        plan.effective_view_distance_m = view_dist;
        let per_axis = recipe.chunks_per_axis() as i32;
        let frustum = view
            .has_projection()
            .then(|| Frustum::from_view_proj(&view.view_proj));

        // Candidate window: the chunks whose centres could be within the view distance.
        let center = recipe.chunk_at(view.eye[0], view.eye[2]);
        let reach = (view_dist / recipe.chunk_size_m).ceil() as i32 + 1;
        let mut wanted: BTreeMap<ChunkCoord, (u8, u8, f32)> = BTreeMap::new();
        for cz in (center.z - reach).max(0)..=(center.z + reach).min(per_axis - 1) {
            for cx in (center.x - reach).max(0)..=(center.x + reach).min(per_axis - 1) {
                let c = ChunkCoord::new(cx, cz);
                // Residency is decided on the HORIZONTAL distance, which does not depend on whether the
                // chunk happens to be loaded. The 3D distance uses a chunk's real vertical bounds once it is
                // resident and the recipe's clamp before that, so a chunk near the ring edge would cross the
                // boundary the moment it loaded, be evicted, look near again on the next plan, and load for
                // ever. Measured: seventeen chunks oscillating under a camera that had not moved.
                let d = Self::chunk_distance_xz(recipe, c, view.eye);
                if d > view_dist {
                    continue;
                }
                let lod_d = self.chunk_distance(recipe, c, view.eye);
                wanted.insert(
                    c,
                    (
                        self.desired_lod(recipe, c, lod_d),
                        texture_lod(recipe, lod_d),
                        d,
                    ),
                );
            }
        }

        // Phase 1: residency. Request what is missing; note LOD changes on what is present.
        for (&c, &(lod, tex_lod, d)) in &wanted {
            if let Some(r) = self.resident.get_mut(&c) {
                r.last_seen = self.tick;
                if r.lod != lod {
                    r.lod = lod;
                    plan.lod_changed.push((c, lod));
                    self.stats.lod_changes += 1;
                }
                // A closer approach deserves a sharper bake; a retreat does not deserve a blurrier one
                // (throwing away a texture we already hold buys nothing). Because the texture LOD is a pure
                // function of distance, a stationary camera re-requests nothing at all.
                if tex_lod < r.texture_lod {
                    plan.load
                        .push(request(c, tex_lod, d, view_dist, recipe, view.focus, true));
                    r.texture_lod = tex_lod;
                }
            } else if self.pending.get(&c).copied() != Some(tex_lod) {
                self.pending.insert(c, tex_lod);
                plan.load
                    .push(request(c, tex_lod, d, view_dist, recipe, view.focus, false));
                self.stats.loads += 1;
            }
        }
        // Nearest first: the chunk under the camera matters more than the one at the horizon.
        plan.load.sort_by(|a, b| {
            a.priority
                .total_cmp(&b.priority)
                .then(a.coord.cmp(&b.coord))
        });

        // Phase 2: culling of the resident set. Distance and frustum first (cheap, every chunk), then the
        // horizon test on a rotating window so no single plan pays for all of them.
        let coords: Vec<ChunkCoord> = self.resident.keys().copied().collect();
        let mut horizon_queue: Vec<ChunkCoord> = Vec::new();
        for c in coords {
            let d = Self::chunk_distance_xz(recipe, c, view.eye);
            let mut visible = d <= view_dist;
            let lod = self.resident[&c].lod;
            if visible {
                if let Some(f) = &frustum {
                    let r = &self.resident[&c];
                    if !f.intersects_aabb(r.bounds_min, r.bounds_max) {
                        visible = false;
                        plan.culled_frustum += 1;
                    }
                }
            }
            if visible && recipe.lod.horizon_culling && d > HORIZON_MIN_DISTANCE_M {
                horizon_queue.push(c);
            }
            if let Some(r) = self.resident.get_mut(&c) {
                r.visible = visible;
            }
            if visible {
                plan.visible.push((c, lod));
            }
        }
        if !horizon_queue.is_empty() {
            let n = horizon_queue.len();
            let start = self.horizon_cursor % n;
            for k in 0..n.min(HORIZON_TESTS_PER_PLAN) {
                let c = horizon_queue[(start + k) % n];
                let r = &self.resident[&c];
                if horizon_occluded(terrain, view.eye, r.bounds_min, r.bounds_max) {
                    plan.culled_horizon += 1;
                    plan.visible.retain(|(vc, _)| *vc != c);
                    if let Some(r) = self.resident.get_mut(&c) {
                        r.visible = false;
                    }
                }
            }
            self.horizon_cursor = self
                .horizon_cursor
                .wrapping_add(n.min(HORIZON_TESTS_PER_PLAN));
        }

        // Phase 3: eviction. Out-of-range first, then farthest-first while over the ceiling.
        let cap = self.budget.total_bytes();
        let max_chunks = self.budget.max_resident_chunks as usize;
        let mut candidates: Vec<(ChunkCoord, f32, usize)> = self
            .resident
            .iter()
            .map(|(c, r)| (*c, Self::chunk_distance_xz(recipe, *c, view.eye), r.bytes))
            .collect();
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut total: usize = candidates.iter().map(|c| c.2).sum();
        let mut count = candidates.len();
        for (c, d, bytes) in candidates {
            let out_of_range = d > view_dist;
            let over = total > cap || count > max_chunks;
            if !out_of_range && !over {
                break;
            }
            // Never evict the chunk the camera is standing in, whatever the budget says — a hole under the
            // player is worse than any overrun, and the honest response to a too-small budget is to report
            // it, which `over_budget` does.
            if !out_of_range && c == center {
                continue;
            }
            plan.unload.push(c);
            plan.evicted_bytes += bytes;
            total = total.saturating_sub(bytes);
            count -= 1;
            self.stats.evictions += 1;
        }
        for c in &plan.unload {
            self.resident.remove(c);
        }
        self.refresh_bytes();
        // Honest reporting: either the bytes still do not fit, or the view distance had to be cut to make
        // them fit. Both are "the budget is smaller than the world you asked for", and both must be said out
        // loud rather than silently absorbed.
        plan.over_budget = self.stats.bytes > cap || view_dist < requested - 0.5;
        plan.visible.retain(|(c, _)| self.resident.contains_key(c));
        self.stats.visible = plan.visible.len();
        plan
    }

    /// The view distance this budget can actually hold.
    ///
    /// **The fix for streaming thrash.** The obvious design — want everything inside the authored view
    /// distance, then evict whatever does not fit — puts the two halves of the streamer in direct conflict:
    /// eviction drops a chunk the very next plan wants back, so the system spends its whole time rebuilding
    /// what it just threw away. Measured on a 4 km world it re-requested three quarters of everything it
    /// loaded.
    ///
    /// Deriving the distance from the budget instead makes the system converge: fewer chunks are *wanted*,
    /// so nothing wanted is evicted, so nothing is re-requested. The cost is an honest one — you see less far
    /// than you asked for — and [`StreamPlan::over_budget`] and
    /// [`StreamPlan::effective_view_distance_m`] both say so.
    fn affordable_view_distance(
        &self,
        recipe: &crate::recipe::TerrainRecipe,
        requested: f32,
    ) -> f32 {
        let cap = self.budget.total_bytes();
        // Deliberately the *estimate*, never the measured resident average. Measuring would make the radius
        // depend on what is currently loaded, which depends on the radius: the loop oscillates by a ring and
        // the boundary chunks flicker in and out for ever. A pure function of the recipe cannot oscillate,
        // and if the estimate is optimistic, ordinary eviction still protects the ceiling.
        let per_chunk = crate::profile::estimate_chunk_bytes(recipe).max(1);
        let by_bytes = cap / per_chunk;
        let affordable = by_bytes.min(self.budget.max_resident_chunks as usize);
        if affordable == 0 {
            return recipe.chunk_size_m;
        }
        // A square ring of side `s` holds `s²` chunks, so a budget for `n` chunks affords a radius of
        // `(√n − 1) / 2`. Leave the centre chunk always affordable.
        let side = (affordable as f32).sqrt();
        let radius_chunks = ((side - 1.0) * 0.5).floor().max(0.0);
        (radius_chunks * recipe.chunk_size_m)
            .max(recipe.chunk_size_m)
            .min(requested)
    }

    /// Horizontal distance from the eye to a chunk's square (0 inside it).
    ///
    /// Residency and range culling use this rather than the 3D distance because it is a pure function of the
    /// recipe and the camera — it cannot change when a chunk loads, so it cannot oscillate.
    fn chunk_distance_xz(
        recipe: &crate::recipe::TerrainRecipe,
        c: ChunkCoord,
        eye: [f32; 3],
    ) -> f32 {
        let o = recipe.chunk_origin(c);
        let s = recipe.chunk_size_m;
        let dx = (o[0] - eye[0]).max(eye[0] - (o[0] + s)).max(0.0);
        let dz = (o[1] - eye[2]).max(eye[2] - (o[1] + s)).max(0.0);
        (dx * dx + dz * dz).sqrt()
    }

    /// Distance from the eye to a chunk's box (0 inside it).
    fn chunk_distance(
        &self,
        recipe: &crate::recipe::TerrainRecipe,
        c: ChunkCoord,
        eye: [f32; 3],
    ) -> f32 {
        let o = recipe.chunk_origin(c);
        let s = recipe.chunk_size_m;
        let (lo_y, hi_y) = match self.resident.get(&c) {
            Some(r) => (r.bounds_min[1], r.bounds_max[1]),
            // Not built yet: use the recipe's clamp, which is guaranteed to contain the real range.
            None => (recipe.height_clamp[0], recipe.height_clamp[1]),
        };
        let dx = (o[0] - eye[0]).max(eye[0] - (o[0] + s)).max(0.0);
        let dz = (o[1] - eye[2]).max(eye[2] - (o[1] + s)).max(0.0);
        let dy = (lo_y - eye[1]).max(eye[1] - hi_y).max(0.0);
        (dx * dx + dy * dy + dz * dz).sqrt()
    }

    /// The LOD a chunk should draw at this distance.
    fn desired_lod(&self, recipe: &crate::recipe::TerrainRecipe, c: ChunkCoord, d: f32) -> u8 {
        if let Some(r) = self.resident.get(&c) {
            // Measured errors: the real criterion.
            mesh::choose_lod(&r.errors, d, &recipe.lod, Some(r.lod))
        } else {
            // Not built yet, so no measurement exists. Fall back to distance rings, which only decides the
            // *initial* texture/collider resolution — the mesh LOD is corrected on the very next plan, from
            // measurements, without a rebuild (every LOD's mesh is built).
            let rings = (d / recipe.chunk_size_m.max(1.0)) as i32;
            let lod = (rings / 3).clamp(0, i32::from(recipe.lod.levels.saturating_sub(1)));
            lod as u8
        }
    }
}

/// The texture bake resolution for a chunk at a distance: the coarsest level that still has at least one
/// texel per screen pixel the chunk covers.
///
/// This is the right criterion for a *texture*, and it is a different one from the mesh's. A dead-flat chunk
/// under the camera has zero geometric error and draws at its coarsest mesh LOD, but it still fills the
/// screen and still wants a 256² bake; conversely a jagged chunk on the horizon needs fine geometry to keep
/// its silhouette and a 32² texture is more than enough for it.
#[must_use]
pub fn texture_lod(recipe: &crate::recipe::TerrainRecipe, distance_m: f32) -> u8 {
    let px = recipe.chunk_size_m * mesh::pixels_per_metre(distance_m, &recipe.lod);
    let mut chosen = 0u8;
    for lod in 0..recipe.lod.levels.max(1) {
        if f32::from(u16::try_from(recipe.texture_res_at_lod(lod)).unwrap_or(u16::MAX)) >= px {
            chosen = lod;
        } else {
            // Resolution only decreases, so once it is too coarse every deeper level is too.
            break;
        }
    }
    chosen
}

fn request(
    c: ChunkCoord,
    lod: u8,
    distance: f32,
    view_dist: f32,
    recipe: &crate::recipe::TerrainRecipe,
    focus: ChunkCoord,
    textures_only: bool,
) -> ChunkRequest {
    let collider_lod = collision::physics_lod(c, focus, &recipe.lod);
    ChunkRequest {
        coord: c,
        lod,
        want_textures: true,
        // Detail normals only matter where they can be seen; they double a chunk's texture cost.
        want_normals: lod == 0,
        want_scatter: !textures_only && distance <= view_dist,
        collider_lod: if textures_only { None } else { collider_lod },
        want_nav: !textures_only && collider_lod.is_some(),
        priority: distance,
    }
}

/// Is every top corner of this box behind terrain, seen from `eye`?
///
/// Conservative by construction: it tests the box's *upper* corners, so a chunk is only culled when even its
/// highest visible extent is blocked. Marching the real height field means the answer is exact for
/// terrain-occludes-terrain, which in an open world is the overwhelming majority of occlusion.
#[must_use]
pub fn horizon_occluded(terrain: &Terrain, eye: [f32; 3], min: [f32; 3], max: [f32; 3]) -> bool {
    let mid_x = f32::midpoint(min[0], max[0]);
    let mid_z = f32::midpoint(min[2], max[2]);
    let corners = [
        [min[0], max[1], min[2]],
        [max[0], max[1], min[2]],
        [min[0], max[1], max[2]],
        [max[0], max[1], max[2]],
        [mid_x, max[1], mid_z],
    ];
    corners
        .iter()
        .all(|c| ray_blocked(terrain, eye, *c, HORIZON_SAMPLES))
}

fn ray_blocked(terrain: &Terrain, from: [f32; 3], to: [f32; 3], samples: usize) -> bool {
    for i in 1..samples {
        let t = i as f32 / samples as f32;
        let x = from[0] + (to[0] - from[0]) * t;
        let y = from[1] + (to[1] - from[1]) * t;
        let z = from[2] + (to[2] - from[2]) * t;
        // A small margin keeps a ray that grazes its own start chunk from reporting a false block.
        if terrain.height(x, z) > y + 0.25 {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::{Layer, LayerKind, LodPolicy, Stroke, StrokeKind, TerrainRecipe};
    use std::collections::BTreeMap as Map;

    fn world(size: f32) -> Terrain {
        let mut r = TerrainRecipe {
            world_size_m: size,
            chunk_size_m: 64.0,
            chunk_verts: 33,
            ..TerrainRecipe::default()
        };
        r.layers.push(Layer::new(
            "Hills",
            LayerKind::Fbm {
                amplitude: 30.0,
                wavelength_m: 200.0,
                octaves: 5,
                lacunarity: 2.0,
                gain: 0.5,
                warp_m: 10.0,
                warp_wavelength_m: 300.0,
            },
        ));
        r.lod.max_view_distance_m = 400.0;
        Terrain::compile(r, Map::new()).expect("compile")
    }

    fn build_all(t: &Terrain, s: &mut Streamer, plan: &StreamPlan) {
        let cfg = NavConfig::default();
        for req in &plan.load {
            let b = build_chunk(t, req, &cfg);
            s.insert(&b);
        }
    }

    #[test]
    fn planning_loads_what_is_near_and_nothing_beyond_the_view_distance() {
        let t = world(1024.0);
        let mut s = Streamer::new(MemoryBudget::default());
        let view = ViewParams::at([512.0, 60.0, 512.0], ChunkCoord::new(8, 8));
        let plan = s.plan(&t, &view);
        assert!(!plan.load.is_empty());
        let recipe = t.recipe();
        for req in &plan.load {
            let c = recipe.chunk_center(req.coord);
            let d = ((c[0] - 512.0).powi(2) + (c[1] - 512.0).powi(2)).sqrt();
            assert!(d < 700.0, "requested a chunk {d} m away");
        }
        // Nearest first.
        for w in plan.load.windows(2) {
            assert!(w[0].priority <= w[1].priority);
        }
    }

    #[test]
    fn a_second_plan_requests_nothing_new() {
        let t = world(512.0);
        let mut s = Streamer::new(MemoryBudget::default());
        let view = ViewParams::at([256.0, 60.0, 256.0], ChunkCoord::new(4, 4));
        let first = s.plan(&t, &view);
        build_all(&t, &mut s, &first);
        let second = s.plan(&t, &view);
        // Not one request, of any kind: the texture LOD is a pure function of distance, so a stationary
        // camera asks for nothing the first plan did not already cover.
        assert!(
            second.load.is_empty(),
            "a stationary camera re-requested {} chunks: {:?}",
            second.load.len(),
            second
                .load
                .iter()
                .map(|r| (r.coord, r.lod))
                .collect::<Vec<_>>()
        );
        assert!(!second.visible.is_empty());
    }

    #[test]
    fn moving_the_camera_streams_in_and_out_without_thrashing() {
        let t = world(2048.0);
        let mut s = Streamer::new(MemoryBudget::default());
        let mut loads = 0u64;
        let mut evictions = 0u64;
        for step in 0..12 {
            let x = 200.0 + step as f32 * 64.0;
            let view = ViewParams::at([x, 60.0, 200.0], t.recipe().chunk_at(x, 200.0));
            let plan = s.plan(&t, &view);
            build_all(&t, &mut s, &plan);
            loads += plan.load.len() as u64;
            evictions += plan.unload.len() as u64;
        }
        assert!(
            loads > 0 && evictions > 0,
            "nothing streamed: {loads}/{evictions}"
        );
        // A steady pan must not reload chunks it just dropped: total loads stay proportional to ground
        // covered, not to frames.
        assert!(
            loads < 400,
            "streaming thrashed: {loads} loads for a 12-step pan"
        );
    }

    #[test]
    fn the_memory_ceiling_is_enforced_and_reported() {
        let t = world(2048.0);
        // A deliberately tiny budget: eviction must keep it under, and say when it cannot.
        let budget = MemoryBudget {
            mesh_mb: 1,
            texture_mb: 1,
            scatter_mb: 1,
            collider_mb: 1,
            max_resident_chunks: 12,
        };
        let mut s = Streamer::new(budget);
        let mut last_view = ViewParams::at([300.0, 60.0, 300.0], ChunkCoord::new(4, 4));
        for step in 0..6 {
            let x = 300.0 + step as f32 * 64.0;
            last_view = ViewParams::at([x, 60.0, 300.0], t.recipe().chunk_at(x, 300.0));
            let plan = s.plan(&t, &last_view);
            build_all(&t, &mut s, &plan);
        }
        // Completed builds land between plans, so the resident set legitimately overshoots by the number of
        // builds in flight; the ceiling is a property of the planned state, so plan once more before asking.
        s.plan(&t, &last_view);
        let stats = s.stats();
        assert!(
            stats.resident <= 13,
            "chunk cap ignored: {} resident",
            stats.resident
        );
        assert!(
            stats.evictions > 0,
            "nothing was evicted under a 4 MB ceiling"
        );
    }

    #[test]
    fn the_chunk_under_the_camera_is_never_evicted() {
        let t = world(1024.0);
        // A zero ceiling is the impossible case: nothing can fit, so the planner must both refuse to leave a
        // hole under the camera and say out loud that it is over budget.
        let budget = MemoryBudget {
            mesh_mb: 0,
            texture_mb: 0,
            scatter_mb: 0,
            collider_mb: 0,
            max_resident_chunks: 1,
        };
        let mut s = Streamer::new(budget);
        let eye = [300.0, 60.0, 300.0];
        let here = t.recipe().chunk_at(eye[0], eye[2]);
        let view = ViewParams::at(eye, here);
        let plan = s.plan(&t, &view);
        build_all(&t, &mut s, &plan);
        let plan2 = s.plan(&t, &view);
        assert!(
            !plan2.unload.contains(&here),
            "evicted the ground under the camera"
        );
        assert!(s.is_resident(here));
        assert!(plan2.over_budget, "an impossible budget must be reported");
    }

    #[test]
    fn frustum_planes_accept_what_is_in_front_and_reject_what_is_behind() {
        // A simple look-down-the--Z-axis projection built by hand (column-major, wgpu depth range).
        let n = 0.1f32;
        let f = 1000.0f32;
        let t = 1.0f32; // tan(45°)
        let proj = [
            1.0 / t,
            0.0,
            0.0,
            0.0, //
            0.0,
            1.0 / t,
            0.0,
            0.0, //
            0.0,
            0.0,
            f / (n - f),
            -1.0, //
            0.0,
            0.0,
            f * n / (n - f),
            0.0,
        ];
        let fr = Frustum::from_view_proj(&proj);
        // 10 m in front of the eye, at the origin, looking down -Z.
        assert!(
            fr.intersects_aabb([-1.0, -1.0, -11.0], [1.0, 1.0, -9.0]),
            "in front"
        );
        assert!(
            !fr.intersects_aabb([-1.0, -1.0, 9.0], [1.0, 1.0, 11.0]),
            "behind"
        );
        assert!(
            !fr.intersects_aabb([200.0, -1.0, -11.0], [202.0, 1.0, -9.0]),
            "far off to the side"
        );
        // Straddling the eye plane still counts as visible.
        assert!(fr.intersects_aabb([-1.0, -1.0, -5.0], [1.0, 1.0, 5.0]));
    }

    #[test]
    fn a_chunk_behind_a_mountain_is_horizon_culled_and_one_in_the_open_is_not() {
        // A tall ridge between the eye and the target.
        let mut r = TerrainRecipe {
            world_size_m: 512.0,
            chunk_size_m: 64.0,
            chunk_verts: 33,
            ..TerrainRecipe::default()
        };
        r.strokes.push(Stroke {
            kind: StrokeKind::Raise,
            x: 200.0,
            z: 100.0,
            radius_m: 60.0,
            strength: 220.0,
            hardness: 0.2,
            target_m: 0.0,
        });
        let t = Terrain::compile(r, Map::new()).expect("compile");
        let eye = [100.0, 6.0, 100.0];
        // Behind the ridge.
        let hidden_min = [300.0, 0.0, 80.0];
        let hidden_max = [340.0, 4.0, 120.0];
        assert!(
            horizon_occluded(&t, eye, hidden_min, hidden_max),
            "the ridge should hide this"
        );
        // Off to the side, in the open.
        let open_min = [300.0, 0.0, 400.0];
        let open_max = [340.0, 4.0, 440.0];
        assert!(
            !horizon_occluded(&t, eye, open_min, open_max),
            "nothing is in the way here"
        );
    }

    #[test]
    fn every_lod_mesh_is_built_so_a_switch_needs_no_rebuild() {
        let t = world(256.0);
        let cfg = NavConfig::default();
        let req = ChunkRequest {
            coord: ChunkCoord::new(1, 1),
            lod: 2,
            want_textures: true,
            want_normals: false,
            want_scatter: false,
            collider_lod: Some(2),
            want_nav: true,
            priority: 0.0,
        };
        let b = build_chunk(&t, &req, &cfg);
        assert_eq!(b.meshes.len(), usize::from(t.recipe().lod.levels));
        for (i, m) in b.meshes.iter().enumerate() {
            assert_eq!(m.lod as usize, i);
            assert!(!m.data.is_empty());
        }
        assert_eq!(b.errors.len(), usize::from(t.recipe().lod.levels));
        // The texture matches the requested LOD's resolution, not LOD 0's.
        assert_eq!(
            b.textures.as_ref().unwrap().res,
            t.recipe().texture_res_at_lod(2)
        );
        assert!(b.collider.is_some() && b.nav.is_some());
        assert!(b.bytes() > 0);
    }

    #[test]
    fn builds_are_deterministic_regardless_of_order() {
        let t = world(256.0);
        let cfg = NavConfig::default();
        let reqs: Vec<ChunkRequest> = [(0, 0), (1, 0), (0, 1), (1, 1)]
            .iter()
            .map(|(x, z)| ChunkRequest {
                coord: ChunkCoord::new(*x, *z),
                lod: 0,
                want_textures: true,
                want_normals: true,
                want_scatter: true,
                collider_lod: Some(0),
                want_nav: true,
                priority: 0.0,
            })
            .collect();
        let forward: Vec<ChunkBuild> = reqs.iter().map(|r| build_chunk(&t, r, &cfg)).collect();
        let backward: Vec<ChunkBuild> = reqs
            .iter()
            .rev()
            .map(|r| build_chunk(&t, r, &cfg))
            .collect();
        for (f, b) in forward.iter().zip(backward.iter().rev()) {
            assert_eq!(f, b, "build order changed the result");
        }
    }

    #[test]
    fn a_flat_world_draws_at_its_coarsest_lod_everywhere() {
        // Because error-driven selection reports zero error, not because of a distance heuristic.
        let r = TerrainRecipe {
            world_size_m: 512.0,
            chunk_size_m: 64.0,
            chunk_verts: 33,
            lod: LodPolicy {
                max_view_distance_m: 400.0,
                ..LodPolicy::default()
            },
            ..TerrainRecipe::default()
        };
        let t = Terrain::compile(r, Map::new()).expect("compile");
        let mut s = Streamer::new(MemoryBudget::default());
        let view = ViewParams::at([256.0, 20.0, 256.0], ChunkCoord::new(4, 4));
        let plan = s.plan(&t, &view);
        build_all(&t, &mut s, &plan);
        let plan2 = s.plan(&t, &view);
        let coarsest = t.recipe().lod.levels - 1;
        assert!(
            plan2.visible.iter().all(|(_, lod)| *lod == coarsest),
            "flat terrain should need no detail anywhere: {:?}",
            plan2.visible.iter().map(|(_, l)| *l).collect::<Vec<_>>()
        );
    }
}
