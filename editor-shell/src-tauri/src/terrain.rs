//! M19 (ADR-104) — the live terrain runtime: a worker pool, a residency ledger, and the per-frame draw
//! list the renderer consumes.
//!
//! ## What runs where
//!
//! * [`metrocalk_terrain::stream::Streamer`] **plans** — pure set arithmetic, no allocation of anything
//!   heavy. Called when the camera has moved meaningfully, not every frame.
//! * The **worker pool** builds. `build_chunk` is a pure function, so the workers need no locks, no ordering
//!   and no shared state beyond an `Arc<Terrain>` — two workers handed the same chunk would produce the same
//!   bytes. Each worker also packs the chunk's vertices, so the render thread receives upload-ready data.
//! * The **render thread** uploads a budgeted handful of finished chunks per frame and draws the visible
//!   set. It never blocks on a build and never touches the field.
//!
//! ## Why the upload carries geometry and textures separately
//!
//! A chunk's splat texture is half a megabyte and changes only when the chunk is re-baked at a new
//! resolution; its geometry changes whenever the LOD changes, which is often. Publishing them as one bundle
//! would copy the texture on every LOD switch. So [`crate::render::TerrainUpload`] carries `Option` textures:
//! a LOD switch sends vertices and indices only, and the render thread keeps the views it already has.
//!
//! Invariant 4 holds: nothing here crosses the JS boundary. The camera comes from the render loop's own
//! state and the draw list is consumed in the same lock.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use metrocalk_assets::gpu::MeshVertex;
use metrocalk_terrain::mesh::{ChunkMesh, ChunkSamples, MeshData};
use metrocalk_terrain::nav::{NavConfig, NavGrid, NavWorld, PathResult};
use metrocalk_terrain::profile::{BuildTimings, TerrainStats};
use metrocalk_terrain::scatter::{ScatterDraw, LOD_IMPOSTOR};
use metrocalk_terrain::stream::{build_chunk, ChunkBuild, ChunkRequest, Streamer, ViewParams};
use metrocalk_terrain::{ChunkCoord, Terrain};

use crate::render::{Instance, SceneState, TerrainDraw, TerrainUpload, IDENTITY_QUAT};

/// Chunks uploaded to the GPU per frame. Three 4 000-triangle chunks with a 256² texture is roughly a
/// megabyte of transfer — enough to fill a ring quickly, small enough not to stall a frame.
const UPLOADS_PER_FRAME: usize = 6;

/// How far the camera must move before the streamer re-plans. A quarter of a chunk keeps planning off the
/// per-frame path without ever letting the resident set fall behind the view.
const REPLAN_DISTANCE_FRACTION: f32 = 0.25;

/// Frames between plans while chunks are still arriving — the fill is paced by the plan, so this is what
/// decides how quickly a world appears.
const REPLAN_BUSY_FRAMES: u64 = 4;

/// Frames between forced re-plans even when the camera is still — so an in-flight build that finishes while
/// standing still still becomes visible promptly.
const REPLAN_FRAME_INTERVAL: u64 = 30;

/// Jobs allowed in flight at once. Bounded so a fast pan cannot queue thousands of stale builds.
const MAX_IN_FLIGHT: usize = 48;

struct Job {
    generation: u64,
    terrain: Arc<Terrain>,
    request: ChunkRequest,
    nav: NavConfig,
}

struct Built {
    generation: u64,
    build: Box<ChunkBuild>,
    vertices: Vec<MeshVertex>,
    indices: Vec<u32>,
    timings: BuildTimings,
}

/// What a worker sends back. A job either produces a chunk or it explains itself — there is no third
/// outcome in which nothing arrives.
///
/// Before this existed a panic inside `build_chunk` unwound the worker thread out of its loop, and three
/// things followed, all of them silent: the chunk's coord stayed in `in_flight` for ever, so that piece of
/// world could never be rebuilt; one of a bounded number of in-flight slots was gone for good; and the pool
/// was one thread smaller. After as many panics as there are workers the last `Receiver` dropped, every
/// subsequent `submit` failed, and the terrain simply stopped building while the editor carried on
/// reporting that work was pending.
enum Done {
    Built(Box<Built>),
    /// The build panicked. Carries the coord so the runtime can free the slot it was holding, and the
    /// panic's own message so the author is told which chunk and why.
    Failed {
        generation: u64,
        coord: ChunkCoord,
        why: String,
    },
}

/// The text a panic payload carries, for the two types `panic!` actually produces.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload.downcast_ref::<&str>().map_or_else(
        || {
            payload
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_else(|| "panicked".to_string())
        },
        |s| (*s).to_string(),
    )
}

/// The worker pool. Dropping it closes the channel, which ends every worker.
struct Pool {
    tx: Option<Sender<Job>>,
    rx: Receiver<Done>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl Pool {
    fn new(threads: usize) -> Self {
        let (job_tx, job_rx) = channel::<Job>();
        let (done_tx, done_rx) = channel::<Done>();
        let shared = Arc::new(Mutex::new(job_rx));
        let mut workers = Vec::with_capacity(threads);
        for i in 0..threads {
            let rx = Arc::clone(&shared);
            let tx = done_tx.clone();
            workers.push(
                std::thread::Builder::new()
                    .name(format!("terrain-{i}"))
                    .spawn(move || loop {
                        // Take one job, then release the lock: workers must not serialize on each other.
                        let job = {
                            let guard = match rx.lock() {
                                Ok(g) => g,
                                Err(_) => return,
                            };
                            guard.recv()
                        };
                        let Ok(job) = job else { return };
                        let coord = job.request.coord;
                        let generation = job.generation;
                        // Catch, rather than unwind out of the loop. A chunk build is a pure function over
                        // author-supplied numbers, so a bad recipe reaches it as an index or a NaN rather
                        // than as a validation error, and the honest response to that is to lose the chunk
                        // and say so — not to lose a worker.
                        let outcome =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                let t0 = Instant::now();
                                let build = build_chunk(&job.terrain, &job.request, &job.nav);
                                let field_us = elapsed_us(t0);
                                let t1 = Instant::now();
                                let lod = usize::from(
                                    job.request
                                        .lod
                                        .min(job.terrain.recipe().lod.levels.saturating_sub(1)),
                                );
                                let (vertices, indices) = build
                                    .meshes
                                    .get(lod)
                                    .map_or_else(|| (Vec::new(), Vec::new()), pack_chunk);
                                let mesh_us = elapsed_us(t1);
                                let timings = BuildTimings {
                                    // The field pass dominates a build and is not separately instrumented inside
                                    // the pure function; attributing the whole build to it and the packing to
                                    // `mesh` is the honest split rather than an invented breakdown.
                                    field_us,
                                    mesh_us,
                                    ..BuildTimings::default()
                                };
                                Built {
                                    generation,
                                    build: Box::new(build),
                                    vertices,
                                    indices,
                                    timings,
                                }
                            }));
                        let done = match outcome {
                            Ok(built) => Done::Built(Box::new(built)),
                            Err(payload) => Done::Failed {
                                generation,
                                coord,
                                why: panic_text(payload.as_ref()),
                            },
                        };
                        if tx.send(done).is_err() {
                            return;
                        }
                    })
                    .expect("terrain worker thread"),
            );
        }
        Self {
            tx: Some(job_tx),
            rx: done_rx,
            workers,
        }
    }

    fn submit(&self, job: Job) -> bool {
        self.tx.as_ref().is_some_and(|tx| tx.send(job).is_ok())
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        // Close the job channel first so every worker's `recv` returns and the joins cannot hang.
        self.tx = None;
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

fn elapsed_us(t: Instant) -> u32 {
    u32::try_from(t.elapsed().as_micros()).unwrap_or(u32::MAX)
}

/// Pack a chunk mesh into renderer vertices. Terrain colour comes from the baked splat texture, so the
/// vertex colour is white — tinting it here would multiply the bake a second time.
fn pack_chunk(mesh: &ChunkMesh) -> (Vec<MeshVertex>, Vec<u32>) {
    let d = &mesh.data;
    let verts = d
        .positions
        .iter()
        .enumerate()
        .map(|(i, p)| MeshVertex {
            position: *p,
            normal: d.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]),
            color: [1.0, 1.0, 1.0],
            metallic: 0.0,
            roughness: 0.9,
            uv: d.uvs.get(i).copied().unwrap_or([0.0, 0.0]),
            tangent: [0.0; 4],
        })
        .collect();
    (verts, d.indices.clone())
}

/// Pack a chunk's water surface, shading each corner by how deep the water is there.
///
/// The quad's corners sit exactly on the chunk edges, so a neighbouring chunk samples the same depth at the
/// same world point and gets the same colour — the shading is therefore continuous across the whole sea
/// without any chunk needing to know about its neighbours. Shallow water shows the sand through it; deep
/// water goes to open-ocean blue. It is an opaque surface on the existing PBR pipeline (low roughness, so it
/// takes a specular highlight); a refractive pass is the upgrade, not a prerequisite for having a sea.
fn pack_water(
    data: &MeshData,
    samples: &ChunkSamples,
    sea_level_m: f32,
) -> (Vec<MeshVertex>, Vec<u32>) {
    const SHALLOW: [f32; 3] = [0.32, 0.52, 0.50];
    const DEEP: [f32; 3] = [0.03, 0.10, 0.22];
    /// Depth, in metres, at which the water reads as fully deep.
    const FULL_DEPTH: f32 = 14.0;
    let last = samples.verts as i32 - 1;
    let verts = data
        .positions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            // The quad is emitted corner-by-corner in (-,-), (+,-), (-,+), (+,+) order.
            let (ix, iz) = (
                if i % 2 == 0 { 0 } else { last },
                if i < 2 { 0 } else { last },
            );
            let depth = (sea_level_m - samples.height_at(ix, iz)).max(0.0);
            let t = (depth / FULL_DEPTH).clamp(0.0, 1.0);
            let color = [
                SHALLOW[0] + (DEEP[0] - SHALLOW[0]) * t,
                SHALLOW[1] + (DEEP[1] - SHALLOW[1]) * t,
                SHALLOW[2] + (DEEP[2] - SHALLOW[2]) * t,
            ];
            MeshVertex {
                position: *p,
                normal: [0.0, 1.0, 0.0],
                color,
                metallic: 0.0,
                roughness: 0.06,
                uv: data.uvs.get(i).copied().unwrap_or([0.0, 0.0]),
                tangent: [0.0; 4],
            }
        })
        .collect();
    (verts, data.indices.clone())
}

/// A chunk the runtime is holding.
struct Resident {
    build: Box<ChunkBuild>,
    slot: u32,
    /// A second slot holding this chunk's water surface, when any of it is below the water line. Water is a
    /// separate slot rather than part of the terrain mesh because it is a different surface with a different
    /// material, and because a dry chunk must cost nothing at all.
    water_slot: Option<u32>,
    /// The LOD whose geometry is currently uploaded.
    uploaded_lod: u8,
    /// The LOD the streamer wants drawn.
    want_lod: u8,
}

/// Hand a chunk's render slots back. Every eviction path goes through here, so a new kind of slot cannot be
/// added and then leaked on one of the three paths that release chunks.
fn release(r: &Resident, drops: &mut Vec<u32>, free: &mut Vec<u32>) {
    for slot in [Some(r.slot), r.water_slot].into_iter().flatten() {
        drops.push(slot);
        free.push(slot);
    }
}

/// Everything the runtime knows about the live terrain.
pub struct TerrainRuntime {
    terrain: Option<Arc<Terrain>>,
    /// The COMMITTED terrain. `terrain` equals this unless a sculpt gesture is previewing on top of it, and
    /// keeping both is what lets the preview be dropped exactly rather than approximately reverted.
    base: Option<Arc<Terrain>>,
    /// The world rectangle the live preview has touched, so ending the gesture re-streams only that.
    preview_dirty: Option<([f32; 2], [f32; 2])>,
    streamer: Streamer,
    pool: Option<Pool>,
    resident: BTreeMap<ChunkCoord, Resident>,
    in_flight: BTreeSet<ChunkCoord>,
    /// Chunks whose build panicked, and why.
    ///
    /// The streamer keeps asking for them — it plans against the view and knows nothing about failure — so
    /// without this the runtime would resubmit a job that panics every time it replans. Cleared whenever the
    /// terrain changes, because an edit is exactly the event that might make the chunk buildable again, and
    /// is also the only moment an author would expect a retry.
    failed: BTreeMap<ChunkCoord, String>,
    free_slots: Vec<u32>,
    next_slot: u32,
    visible: Vec<(ChunkCoord, u8)>,
    stats: TerrainStats,
    nav_cfg: NavConfig,
    generation: u64,
    frame: u64,
    last_plan_eye: [f32; 3],
    last_plan_frame: u64,
    /// Reused across frames so the per-frame path allocates nothing.
    scatter_scratch: Vec<ScatterDraw>,
    /// The last plan's honest complaint, surfaced to the profiling panel.
    over_budget: bool,
    /// The last plan produced more work than the queue could take, so another plan is due immediately.
    pending_work: bool,
    /// Which artefacts the pending rebuild actually needs (ADR-106). `None` ⇒ all of them.
    ///
    /// Consumed by the chunk requests the next plan issues, so a vegetation-only edit re-scatters without
    /// re-baking a quarter-megabyte splat texture or re-running the navigation grid.
    rebuild_plan: Option<metrocalk_terrain::stage::RebuildPlan>,
}

impl Default for TerrainRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl TerrainRuntime {
    /// An empty runtime. No threads are started until a terrain is set.
    #[must_use]
    pub fn new() -> Self {
        Self {
            terrain: None,
            base: None,
            preview_dirty: None,
            streamer: Streamer::new(metrocalk_terrain::MemoryBudget::default()),
            pool: None,
            resident: BTreeMap::new(),
            in_flight: BTreeSet::new(),
            failed: BTreeMap::new(),
            free_slots: Vec::new(),
            next_slot: 0,
            visible: Vec::new(),
            stats: TerrainStats::default(),
            nav_cfg: NavConfig::default(),
            generation: 0,
            frame: 0,
            last_plan_eye: [f32::MAX; 3],
            last_plan_frame: 0,
            scatter_scratch: Vec::new(),
            over_budget: false,
            pending_work: false,
            rebuild_plan: None,
        }
    }

    /// Whether a terrain is live.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.terrain.is_some()
    }

    /// The compiled terrain, for height/nav queries from commands.
    #[must_use]
    pub fn terrain(&self) -> Option<&Arc<Terrain>> {
        self.terrain.as_ref()
    }

    /// Live counters.
    #[must_use]
    pub fn stats(&self) -> TerrainStats {
        self.stats
    }

    /// Whether the last plan could not fit the resident set inside the budget.
    #[must_use]
    pub fn over_budget(&self) -> bool {
        self.over_budget
    }

    /// Install (or replace) the terrain. Every resident chunk is dropped, because a recipe change means
    /// every derived artefact is stale — and results from the previous generation that are still in flight
    /// are discarded when they arrive rather than being drawn as a mix of two worlds.
    pub fn set_terrain(&mut self, terrain: Option<Terrain>, state: &mut SceneState) {
        self.base = None;
        self.preview_dirty = None;
        self.generation += 1;
        for (_, r) in std::mem::take(&mut self.resident) {
            release(&r, &mut state.terrain_drops, &mut self.free_slots);
        }
        self.in_flight.clear();
        // A new recipe is a new chance: whatever made a chunk panic may be gone.
        self.failed.clear();
        state.terrain_problem = None;
        self.visible.clear();
        state.terrain_draws.clear();
        state.terrain_uploads.clear();
        self.stats = TerrainStats::default();
        self.last_plan_eye = [f32::MAX; 3];
        match terrain {
            None => {
                self.terrain = None;
                self.streamer = Streamer::new(metrocalk_terrain::MemoryBudget::default());
                // Keep the pool: re-creating threads on every recompile is wasted work, and an idle pool
                // costs nothing.
            }
            Some(t) => {
                self.streamer = Streamer::new(t.recipe().budget);
                let shared = Arc::new(t);
                self.base = Some(Arc::clone(&shared));
                self.terrain = Some(shared);
                if self.pool.is_none() {
                    // Leave the main thread and the render thread room; at least one worker always.
                    let threads = std::thread::available_parallelism()
                        .map_or(2, |n| n.get().saturating_sub(2).clamp(1, 8));
                    self.pool = Some(Pool::new(threads));
                }
            }
        }
    }

    /// Advance the runtime one frame. Called from the render loop with the scene lock held.
    pub fn update(
        &mut self,
        state: &mut SceneState,
        eye: [f32; 3],
        view_proj: [f32; 16],
        focus: ChunkCoord,
    ) {
        self.frame += 1;
        let Some(terrain) = self.terrain.clone() else {
            return;
        };
        self.drain_finished(state, &terrain);
        if self.should_replan(&terrain, eye) {
            self.replan(state, &terrain, eye, view_proj, focus);
        }
        self.publish_draws(state, &terrain, eye);
    }

    fn should_replan(&self, terrain: &Terrain, eye: [f32; 3]) -> bool {
        let moved = {
            let d = [
                eye[0] - self.last_plan_eye[0],
                eye[1] - self.last_plan_eye[1],
                eye[2] - self.last_plan_eye[2],
            ];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        };
        // While work is still outstanding, plan often: the queue is bounded, so each plan hands over another
        // batch and the heartbeat is what paces the fill. Once nothing is in flight the slow heartbeat is
        // enough, and planning goes back to costing nothing.
        let heartbeat = if self.in_flight.is_empty() && !self.pending_work {
            REPLAN_FRAME_INTERVAL
        } else {
            REPLAN_BUSY_FRAMES
        };
        moved > terrain.recipe().chunk_size_m * REPLAN_DISTANCE_FRACTION
            || self.frame.saturating_sub(self.last_plan_frame) >= heartbeat
    }

    fn replan(
        &mut self,
        state: &mut SceneState,
        terrain: &Arc<Terrain>,
        eye: [f32; 3],
        view_proj: [f32; 16],
        focus: ChunkCoord,
    ) {
        self.last_plan_eye = eye;
        self.last_plan_frame = self.frame;
        let view = ViewParams {
            eye,
            view_proj,
            focus,
        };
        let plan = self.streamer.plan(terrain, &view);
        self.over_budget = plan.over_budget;
        self.stats.culled_frustum = u32::try_from(plan.culled_frustum).unwrap_or(u32::MAX);
        self.stats.culled_horizon = u32::try_from(plan.culled_horizon).unwrap_or(u32::MAX);

        for coord in &plan.unload {
            if let Some(r) = self.resident.remove(coord) {
                release(&r, &mut state.terrain_drops, &mut self.free_slots);
            }
        }
        for (coord, lod) in &plan.lod_changed {
            if let Some(r) = self.resident.get_mut(coord) {
                r.want_lod = *lod;
            }
        }
        self.visible = plan.visible;

        self.pending_work = false;
        let mut deferred: Vec<ChunkCoord> = Vec::new();
        if let Some(pool) = &self.pool {
            for req in &plan.load {
                if self.failed.contains_key(&req.coord) {
                    // Already tried, already panicked, already reported. Re-submitting it would burn a
                    // worker on the same panic every replan for as long as the camera can see it.
                    continue;
                }
                if self.in_flight.len() >= MAX_IN_FLIGHT {
                    // Hand it back rather than dropping it: the streamer has already marked it requested,
                    // and a request that is marked but never started is a chunk that never appears.
                    deferred.push(req.coord);
                    continue;
                }
                if !self.in_flight.insert(req.coord) {
                    continue;
                }
                let ok = pool.submit(Job {
                    generation: self.generation,
                    terrain: Arc::clone(terrain),
                    request: req.clone(),
                    nav: self.nav_cfg,
                });
                if !ok {
                    self.in_flight.remove(&req.coord);
                    deferred.push(req.coord);
                }
            }
        } else {
            deferred.extend(plan.load.iter().map(|r| r.coord));
        }
        self.pending_work = !deferred.is_empty();
        for c in deferred {
            self.streamer.defer(c);
        }
        self.stats.pending_builds = u32::try_from(self.in_flight.len()).unwrap_or(u32::MAX);
    }

    /// Record a chunk whose build did not survive, and say so.
    ///
    /// Three things have to happen together or the runtime degrades silently: the queue slot is freed (a
    /// leaked in-flight coord is permanent and the bound it counts against is small), the coord is
    /// remembered so the next plan does not resubmit the same panic, and the author is told which piece of
    /// world is missing. A hole someone can see and cannot explain is the worst of the available outcomes.
    fn note_failed(
        &mut self,
        state: &mut SceneState,
        generation: u64,
        coord: ChunkCoord,
        why: &str,
    ) {
        // Freed whatever generation it belonged to — a stale result still occupies a real slot.
        self.in_flight.remove(&coord);
        if generation != self.generation {
            return;
        }
        self.failed.insert(coord, why.to_string());
        state.terrain_problem = Some(format!(
            "a piece of the landscape at chunk ({}, {}) could not be built: {why}",
            coord.x, coord.z
        ));
        eprintln!("[terrain] chunk ({}, {}) panicked: {why}", coord.x, coord.z);
    }

    fn drain_finished(&mut self, state: &mut SceneState, terrain: &Terrain) {
        let sea_level = terrain.recipe().water.sea_level_m;
        let mut uploaded = 0usize;
        while uploaded < UPLOADS_PER_FRAME {
            let done = match self.pool.as_ref().map(|p| p.rx.try_recv()) {
                Some(Ok(b)) => b,
                Some(Err(TryRecvError::Empty)) | None => break,
                // Every worker is gone, which after `catch_unwind` can only mean the threads were torn
                // down — not that the queue is quiet. Treating it as "empty" made a dead pool and an idle
                // pool the same observation, so the one state the runtime can never recover from was also
                // the one it never mentioned.
                Some(Err(TryRecvError::Disconnected)) => {
                    if state.terrain_problem.is_none() {
                        state.terrain_problem = Some(
                            "the terrain builder stopped responding; reopen the project to restart it"
                                .to_string(),
                        );
                        eprintln!("[terrain] worker pool disconnected");
                    }
                    break;
                }
            };
            let built = match done {
                Done::Built(b) => *b,
                Done::Failed {
                    generation,
                    coord,
                    why,
                } => {
                    self.note_failed(state, generation, coord, &why);
                    continue;
                }
            };
            self.in_flight.remove(&built.build.coord);
            if built.generation != self.generation {
                // A result from a previous recipe: dropping it is what keeps the world from being a mix of
                // two generations.
                continue;
            }
            self.stats.completed_builds += 1;
            self.stats.timings.add(&built.timings);
            let coord = built.build.coord;
            let lod = built.build.texture_lod;
            // Take the previous resident OUT rather than reading through it: inserting over it would drop
            // the struct silently, and any slot it still owned would leak — an uploaded GPU buffer that is
            // never drawn and never released. A rebuild for an already-resident chunk is routine (a build
            // still in flight when its chunk is invalidated arrives after its replacement), so this is the
            // common path, not an edge case.
            let prev = self.resident.remove(&coord);
            let slot = prev.as_ref().map_or_else(|| self.alloc_slot(), |r| r.slot);
            let (albedo, normal) = built.build.textures.as_ref().map_or((None, None), |t| {
                (
                    Some(metrocalk_assets::mesh::Texture {
                        width: t.res,
                        height: t.res,
                        rgba8: t.albedo_rgba8.clone(),
                    }),
                    t.normal_rgba8
                        .as_ref()
                        .map(|n| metrocalk_assets::mesh::Texture {
                            width: t.res,
                            height: t.res,
                            rgba8: n.clone(),
                        }),
                )
            });
            state.terrain_uploads.push(TerrainUpload {
                slot,
                vertices: built.vertices,
                indices: built.indices,
                albedo,
                normal,
            });
            // The water surface, if this chunk has any. It carries no texture: the depth shading is in the
            // vertex colour, and the renderer's dummy albedo is white, so the colour arrives unmultiplied.
            let prev_water = prev.and_then(|r| r.water_slot);
            let water_slot = match (&built.build.water, prev_water) {
                (Some(w), reuse) => {
                    // Reuse the chunk's own water slot when it had one, so a rebuild re-uploads in place.
                    let s = reuse.unwrap_or_else(|| self.alloc_slot());
                    let (vertices, indices) = pack_water(w, &built.build.samples, sea_level);
                    state.terrain_uploads.push(TerrainUpload {
                        slot: s,
                        vertices,
                        indices,
                        albedo: None,
                        normal: None,
                    });
                    // Counted against the frame's upload budget like any other. A submerged chunk costs two
                    // uploads, and letting the second one ride free would silently double the transfer rate
                    // the budget exists to bound.
                    uploaded += 1;
                    Some(s)
                }
                (None, Some(stale)) => {
                    // The chunk dried out — a sculpt raised it above the water line, or the line dropped.
                    // Release the surface rather than leaving it hanging over dry land.
                    state.terrain_drops.push(stale);
                    self.free_slots.push(stale);
                    None
                }
                (None, None) => None,
            };
            self.streamer.insert(&built.build);
            self.resident.insert(
                coord,
                Resident {
                    build: built.build,
                    slot,
                    water_slot,
                    uploaded_lod: lod,
                    want_lod: lod,
                },
            );
            uploaded += 1;
        }

        // A LOD change needs no rebuild — every LOD's mesh is already in memory — but it does need a
        // re-upload of that chunk's geometry. Budgeted alongside the new chunks.
        if uploaded < UPLOADS_PER_FRAME {
            let stale: Vec<ChunkCoord> = self
                .resident
                .iter()
                .filter(|(_, r)| r.want_lod != r.uploaded_lod)
                .map(|(c, _)| *c)
                .take(UPLOADS_PER_FRAME - uploaded)
                .collect();
            for coord in stale {
                let Some(r) = self.resident.get_mut(&coord) else {
                    continue;
                };
                let lod = usize::from(r.want_lod);
                if let Some(mesh) = r.build.meshes.get(lod) {
                    let (vertices, indices) = pack_chunk(mesh);
                    state.terrain_uploads.push(TerrainUpload {
                        slot: r.slot,
                        vertices,
                        indices,
                        // The texture is unchanged, so it is not re-sent.
                        albedo: None,
                        normal: None,
                    });
                    r.uploaded_lod = r.want_lod;
                }
            }
        }
        self.refresh_memory_stats();
    }

    /// Take a render slot, recycling a released one before growing the table.
    fn alloc_slot(&mut self) -> u32 {
        self.free_slots.pop().unwrap_or_else(|| {
            let s = self.next_slot;
            self.next_slot += 1;
            s
        })
    }

    /// Every slot currently owned by a resident chunk. Debug-only bookkeeping check.
    #[cfg(test)]
    fn live_slots(&self) -> Vec<u32> {
        self.resident
            .values()
            .flat_map(|r| [Some(r.slot), r.water_slot])
            .flatten()
            .collect()
    }

    /// Panic if the slot ledger has gone wrong: a slot issued twice, or a slot both live and free.
    ///
    /// Slot bookkeeping is the one part of this runtime where a mistake is invisible in the numbers and
    /// fatal on the GPU — a leaked slot is an uploaded buffer nothing draws and nothing frees, and a
    /// double-issued one is two chunks writing the same geometry. Checking it is cheap; finding it from a
    /// crash is not.
    #[cfg(test)]
    fn assert_slots_sane(&self, where_: &str) {
        let live = self.live_slots();
        let live_set: BTreeSet<u32> = live.iter().copied().collect();
        assert_eq!(
            live.len(),
            live_set.len(),
            "{where_}: a slot is issued twice"
        );
        let free_set: BTreeSet<u32> = self.free_slots.iter().copied().collect();
        assert_eq!(
            self.free_slots.len(),
            free_set.len(),
            "{where_}: the free list has duplicates"
        );
        assert!(
            live_set.is_disjoint(&free_set),
            "{where_}: a live slot is also on the free list"
        );
        // Nothing may be lost: every slot ever handed out is either live or free.
        let accounted = live_set.len() + free_set.len();
        assert_eq!(
            accounted, self.next_slot as usize,
            "{where_}: {} slots issued but {accounted} accounted for — the rest leaked",
            self.next_slot
        );
    }

    fn refresh_memory_stats(&mut self) {
        let mut mesh = 0usize;
        let mut texture = 0usize;
        let mut scatter = 0usize;
        let mut collider = 0usize;
        let mut nav = 0usize;
        for r in self.resident.values() {
            mesh += r.build.samples.bytes()
                + r.build.meshes.iter().map(ChunkMesh::bytes).sum::<usize>()
                + r.build.water.as_ref().map_or(0, MeshData::bytes);
            texture += r.build.textures.as_ref().map_or(0, |t| t.bytes());
            scatter += r.build.scatter.bytes();
            collider += r.build.collider.as_ref().map_or(0, |c| c.bytes());
            nav += r.build.nav.as_ref().map_or(0, NavGrid::bytes);
        }
        self.stats.mesh_bytes = mesh;
        self.stats.texture_bytes = texture;
        self.stats.scatter_bytes = scatter;
        self.stats.collider_bytes = collider;
        self.stats.nav_bytes = nav;
        self.stats.resident_chunks = u32::try_from(self.resident.len()).unwrap_or(u32::MAX);
        self.stats.pending_builds = u32::try_from(self.in_flight.len()).unwrap_or(u32::MAX);
    }

    /// Rebuild the per-frame draw list. Cheap by construction: one push per visible chunk, plus the scatter
    /// selection, which writes into a buffer reused across frames.
    fn publish_draws(&mut self, state: &mut SceneState, terrain: &Terrain, eye: [f32; 3]) {
        state.terrain_draws.clear();
        let mut triangles = 0u64;
        for (coord, lod) in &self.visible {
            let Some(r) = self.resident.get(coord) else {
                continue;
            };
            // Draw whatever geometry is actually uploaded, which may briefly lag the wanted LOD. Drawing the
            // wanted LOD before it is uploaded would show the previous chunk's geometry — a visible flicker.
            let drawn = usize::from(r.uploaded_lod);
            let Some(mesh) = r.build.meshes.get(drawn) else {
                continue;
            };
            let _ = lod;
            triangles += mesh.data.tri_count() as u64;
            let instance = Instance {
                center: mesh.instance_center,
                scale: 1.0,
                color: [1.0, 1.0, 1.0],
                highlight: 0.0,
                rotation: IDENTITY_QUAT,
                material: [0.0; 4],
            };
            state.terrain_draws.push(TerrainDraw {
                slot: r.slot,
                instance,
            });
            // The water rides the same instance placement: it is chunk-local geometry around the same origin,
            // with its own absolute height, so it stays welded to the chunk through every LOD change.
            if let Some(slot) = r.water_slot {
                triangles += 2;
                state.terrain_draws.push(TerrainDraw { slot, instance });
            }
        }
        // Counted from the chunks, not the draw list: a submerged chunk contributes a second draw for its
        // water, and reporting that as another visible chunk would inflate the number the panel shows.
        self.stats.visible_chunks = u32::try_from(self.visible.len()).unwrap_or(u32::MAX);
        self.stats.drawn_triangles = triangles;

        // Scatter selection: instances are hash-generated, so this is a filter, not a lookup.
        self.scatter_scratch.clear();
        for (coord, _) in &self.visible {
            if let Some(r) = self.resident.get(coord) {
                r.build
                    .scatter
                    .select_into(terrain, eye, &mut self.scatter_scratch);
            }
        }
        self.stats.drawn_instances = u32::try_from(self.scatter_scratch.len()).unwrap_or(u32::MAX);
        self.stats.impostor_instances = u32::try_from(
            self.scatter_scratch
                .iter()
                .filter(|d| d.lod == LOD_IMPOSTOR)
                .count(),
        )
        .unwrap_or(u32::MAX);

        // Resolve each instance to a render slot. An unbound representation resolves to -1 and is skipped
        // rather than drawn as a placeholder cube — a forest of grey boxes is worse than an empty field.
        state.terrain_scatter.clear();
        for d in &self.scatter_scratch {
            let Some(row) = state.terrain_proto_slots.get(d.proto as usize) else {
                continue;
            };
            let which = if d.lod == LOD_IMPOSTOR {
                4
            } else {
                usize::from(d.lod).min(3)
            };
            let slot = row[which];
            if slot < 0 {
                continue;
            }
            state.terrain_scatter.push((
                slot,
                Instance {
                    center: d.position,
                    scale: d.scale,
                    color: [1.0, 1.0, 1.0],
                    highlight: 0.0,
                    rotation: d.rotation,
                    material: [0.0; 4],
                },
            ));
        }
    }

    /// Replace the terrain but keep every chunk the edit could not have changed.
    ///
    /// A sculpt stroke or a road changes the field inside a bounded rectangle and nowhere else, so
    /// re-streaming the whole world for one would turn a paint stroke into a second-long stall. Chunks
    /// outside `dirty` stay exactly as they are — which is correct, not merely fast: their geometry is a
    /// pure function of a field that did not change.
    pub fn replace_terrain_in(
        &mut self,
        terrain: Terrain,
        dirty: ([f32; 2], [f32; 2]),
        stages: Option<metrocalk_terrain::stage::StageMask>,
        state: &mut SceneState,
    ) {
        let recipe_changed_shape = self.base.as_ref().is_none_or(|b| {
            let a = b.recipe();
            let c = terrain.recipe();
            a.world_size_m != c.world_size_m
                || a.chunk_size_m != c.chunk_size_m
                || a.chunk_verts != c.chunk_verts
                || a.lod != c.lod
                || a.budget != c.budget
        });
        if recipe_changed_shape {
            // The chunk grid itself moved; nothing resident is addressable any more.
            self.set_terrain(Some(terrain), state);
            return;
        }
        self.preview_dirty = None;
        let shared = Arc::new(terrain);
        self.base = Some(Arc::clone(&shared));
        self.terrain = Some(shared);
        // What the dependency graph says must be rebuilt. `None` ⇒ everything, the honest default.
        //
        // A chunk holds its mesh, textures, scatter, collider and nav grid together, and dropping it drops
        // all five — so acting on a PARTIAL plan means patching a retained chunk in place, which is a change
        // to the residency model rather than a flag. What is honest and complete today is the other end of
        // the scale: an edit that reaches no chunk artefact at all costs nothing, instead of re-streaming a
        // region because something was renamed.
        let plan = stages.map(metrocalk_terrain::stage::RebuildPlan::from_mask);
        self.rebuild_plan = plan;
        if plan.is_some_and(metrocalk_terrain::stage::RebuildPlan::is_noop) {
            return;
        }
        self.invalidate(state, dirty.0, dirty.1);
    }

    /// Show a sculpt gesture before it is committed.
    ///
    /// Compiles the committed recipe plus the in-flight strokes — reusing the erosion bakes, so this is a
    /// millisecond, not a second — and re-streams only the chunks under the brush. The document is not
    /// touched: nothing here is undoable because nothing here has happened yet.
    pub fn set_preview_strokes(
        &mut self,
        strokes: &[metrocalk_terrain::Stroke],
        state: &mut SceneState,
    ) {
        let Some(base) = self.base.clone() else {
            return;
        };
        if strokes.is_empty() {
            self.clear_preview(state);
            return;
        }
        let mut recipe = base.recipe().clone();
        recipe.strokes.extend_from_slice(strokes);
        let Ok(preview) = Terrain::compile_with_bakes(recipe, BTreeMap::new(), base.bakes()) else {
            return;
        };
        let (min, max) = stroke_bounds(strokes);
        // Union with whatever the preview already dirtied, so retracting a stroke restores what it covered.
        let dirty = match self.preview_dirty {
            None => (min, max),
            Some((pmin, pmax)) => (
                [pmin[0].min(min[0]), pmin[1].min(min[1])],
                [pmax[0].max(max[0]), pmax[1].max(max[1])],
            ),
        };
        self.terrain = Some(Arc::new(preview));
        self.preview_dirty = Some(dirty);
        self.invalidate(state, min, max);
    }

    /// Drop the preview and go back to the committed terrain.
    pub fn clear_preview(&mut self, state: &mut SceneState) {
        let Some(base) = self.base.clone() else {
            return;
        };
        let dirty = self.preview_dirty.take();
        self.terrain = Some(base);
        if let Some((min, max)) = dirty {
            self.invalidate(state, min, max);
        }
    }

    /// Drop every resident chunk overlapping a world rectangle so the next plan rebuilds it.
    ///
    /// Bumps the generation and clears `in_flight`. Both are load-bearing and both were missing:
    ///
    /// * Without the bump, a build that STARTED before this edit finishes after it and is applied on top —
    ///   silently overwriting the edit with pre-edit terrain, permanently, because nothing ever asks for
    ///   that chunk again.
    /// * Without clearing `in_flight`, the replacement build for a chunk that already had one outstanding is
    ///   rejected by the `in_flight.insert` guard and simply never happens, leaving the old terrain resident.
    ///
    /// Together they are the difference between "local editing works" and "local editing usually works".
    fn invalidate(&mut self, state: &mut SceneState, min: [f32; 2], max: [f32; 2]) {
        let Some(t) = self.terrain.clone() else {
            return;
        };
        // Anything already in flight was computed against the PREVIOUS terrain: retire it.
        self.generation += 1;
        self.in_flight.clear();
        // An edit is the one event that can make a chunk buildable again, and the one moment an author
        // would expect the engine to try. Clearing the whole set (not just the edited region) keeps the
        // rule simple: you changed the world, so every hole gets another attempt.
        if !self.failed.is_empty() {
            self.failed.clear();
            state.terrain_problem = None;
        }
        for coord in self.streamer.invalidate_region(t.recipe(), min, max) {
            if let Some(r) = self.resident.remove(&coord) {
                release(&r, &mut state.terrain_drops, &mut self.free_slots);
            }
        }
        // Force the next update to re-plan even if the camera has not moved.
        self.last_plan_eye = [f32::MAX; 3];
        self.pending_work = true;
    }

    /// The chunk the physics/nav focus sits in. The camera stands in for the player until Play mode has one
    /// — and stating that here is better than silently defaulting the focus to the world origin, which would
    /// put the colliders in the wrong place the moment the camera moved.
    #[must_use]
    pub fn focus_chunk(&self, eye: [f32; 3]) -> ChunkCoord {
        self.terrain
            .as_ref()
            .map_or_else(ChunkCoord::default, |t| t.recipe().chunk_at(eye[0], eye[2]))
    }

    /// Terrain height at a world position, for placement and snapping. `None` when no terrain is live.
    #[must_use]
    pub fn height_at(&self, x: f32, z: f32) -> Option<f32> {
        self.terrain.as_ref().map(|t| t.height(x, z))
    }

    /// Find a walkable path across the resident navigation grids.
    #[must_use]
    pub fn find_path(&self, from: [f32; 3], to: [f32; 3]) -> Option<PathResult> {
        let grids: BTreeMap<ChunkCoord, NavGrid> = self
            .resident
            .iter()
            .filter_map(|(c, r)| r.build.nav.as_ref().map(|n| (*c, n.clone())))
            .collect();
        let world = NavWorld::new(&grids)?;
        Some(metrocalk_terrain::nav::find_path(
            &world,
            from,
            to,
            &self.nav_cfg,
            200_000,
        ))
    }
}

/// The world rectangle a set of strokes can affect, padded by each stroke's own radius.
fn stroke_bounds(strokes: &[metrocalk_terrain::Stroke]) -> ([f32; 2], [f32; 2]) {
    let mut min = [f32::MAX; 2];
    let mut max = [f32::MIN; 2];
    for s in strokes {
        let r = s.radius_m.max(0.01);
        min[0] = min[0].min(s.x - r);
        min[1] = min[1].min(s.z - r);
        max[0] = max[0].max(s.x + r);
        max[1] = max[1].max(s.z + r);
    }
    if min[0] > max[0] {
        return ([0.0, 0.0], [0.0, 0.0]);
    }
    (min, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrocalk_terrain::preset;
    use std::collections::BTreeMap as Map;

    fn scene() -> SceneState {
        SceneState::default()
    }

    fn small_terrain() -> Terrain {
        let mut r = preset::by_id("rolling-hills").expect("preset");
        r.world_size_m = 512.0;
        r.chunk_verts = 33;
        r.lod.max_view_distance_m = 256.0;
        Terrain::compile(r, Map::new()).expect("compile")
    }

    /// Pump the runtime until the pending queue drains, or the frame budget runs out.
    ///
    /// Bounded by a settled *state* rather than a fixed frame count: these tests each spin up a worker pool,
    /// and `cargo test` runs them concurrently, so a fixed count is really a bet on how much CPU this test
    /// happened to get. Stopping when nothing is in flight makes the run both faster and not a coin toss.
    fn settle(rt: &mut TerrainRuntime, state: &mut SceneState, frames: usize) {
        let mut quiet = 0;
        for _ in 0..frames {
            rt.update(
                state,
                [256.0, 40.0, 256.0],
                [0.0; 16],
                ChunkCoord::new(4, 4),
            );
            // "Quiet" means nothing outstanding for several consecutive frames: the queue is bounded, so it
            // empties briefly between batches and a single idle frame proves nothing.
            let s = rt.stats();
            quiet = if s.pending_builds == 0 && s.resident_chunks > 0 {
                quiet + 1
            } else {
                0
            };
            if quiet >= 8 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn a_terrain_streams_in_and_publishes_draws() {
        let mut state = scene();
        let mut rt = TerrainRuntime::new();
        assert!(!rt.is_active());
        rt.set_terrain(Some(small_terrain()), &mut state);
        assert!(rt.is_active());
        settle(&mut rt, &mut state, 400);
        assert!(
            rt.stats().resident_chunks > 0,
            "nothing became resident: {:?}",
            rt.stats()
        );
        assert!(
            !state.terrain_draws.is_empty(),
            "nothing was published to draw"
        );
        assert!(rt.stats().drawn_triangles > 0);
        // Every draw points at a slot that was uploaded.
        let uploaded: BTreeSet<u32> = state.terrain_uploads.iter().map(|u| u.slot).collect();
        for d in &state.terrain_draws {
            assert!(
                uploaded.contains(&d.slot),
                "draw references an unuploaded slot"
            );
        }
    }

    #[test]
    fn replacing_the_terrain_drops_every_slot() {
        let mut state = scene();
        let mut rt = TerrainRuntime::new();
        rt.set_terrain(Some(small_terrain()), &mut state);
        settle(&mut rt, &mut state, 400);
        assert!(rt.stats().resident_chunks > 0);
        // Count the SLOTS, not the chunks: a submerged chunk also holds a water slot, and leaking those was
        // the failure mode this test exists to catch.
        let held: usize = rt
            .resident
            .values()
            .map(|r| 1 + usize::from(r.water_slot.is_some()))
            .sum();
        state.terrain_drops.clear();
        rt.set_terrain(Some(small_terrain()), &mut state);
        assert_eq!(
            state.terrain_drops.len(),
            held,
            "every resident slot must be released on a recipe change"
        );
        assert_eq!(rt.stats().resident_chunks, 0);
        assert!(state.terrain_draws.is_empty());
    }

    /// The sequence that killed the packaged app: stream a world with water, invalidate a region the way a
    /// sculpt does, let it re-stream, then replace the terrain the way an undo does — and do it repeatedly.
    ///
    /// A rebuild landing on an already-resident chunk used to reuse its ground slot but allocate a *fresh*
    /// water slot, dropping the old `Resident` and with it the only record of the previous one. Each pass
    /// leaked GPU buffers until the device gave up. The numbers all looked healthy while it happened, which
    /// is exactly why the invariant is asserted rather than eyeballed.
    #[test]
    fn sculpting_and_undoing_over_water_never_leaks_a_render_slot() {
        let flooded = || {
            let mut r = preset::by_id("rolling-hills").expect("preset");
            r.world_size_m = 512.0;
            r.chunk_verts = 33;
            r.lod.max_view_distance_m = 192.0;
            r.water.enabled = true;
            // Above some of the terrain and below the rest, so chunks flip between wet and dry as the
            // ground is sculpted — the case where a chunk must GIVE BACK a water slot.
            r.water.sea_level_m = 6.0;
            Terrain::compile(r, Map::new()).expect("compile")
        };
        let mut state = scene();
        let mut rt = TerrainRuntime::new();
        rt.set_terrain(Some(flooded()), &mut state);
        settle(&mut rt, &mut state, 400);
        rt.assert_slots_sane("after the first stream");
        assert!(rt.stats().resident_chunks > 0);

        for pass in 0..3 {
            // A sculpt: drop everything under the brush and let it come back.
            rt.invalidate(&mut state, [180.0, 180.0], [330.0, 330.0]);
            rt.assert_slots_sane(&format!("pass {pass}: after invalidating"));
            settle(&mut rt, &mut state, 400);
            rt.assert_slots_sane(&format!("pass {pass}: after re-streaming"));

            // An undo: the recipe changes, so every derived artefact is stale.
            rt.set_terrain(Some(flooded()), &mut state);
            rt.assert_slots_sane(&format!("pass {pass}: after replacing"));
            assert!(
                rt.live_slots().is_empty(),
                "pass {pass}: nothing may stay resident"
            );
            settle(&mut rt, &mut state, 400);
            rt.assert_slots_sane(&format!("pass {pass}: after re-streaming the replacement"));
        }

        // The whole point: three sculpt-and-undo cycles must not have grown the slot table beyond what one
        // resident set needs. A leak shows up here as a table several times too large.
        let live = rt.live_slots().len();
        assert!(
            (rt.next_slot as usize) <= live * 2 + 8,
            "slot table grew to {} for {live} live slots — slots are leaking",
            rt.next_slot
        );
    }

    #[test]
    fn a_flooded_world_grows_a_water_surface_and_a_dry_one_does_not() {
        // Rolling hills at a sea level well above the terrain: every chunk is submerged, so every chunk must
        // carry a water surface, and every water surface must be drawn.
        let flooded = {
            let mut r = preset::by_id("rolling-hills").expect("preset");
            r.world_size_m = 512.0;
            r.chunk_verts = 33;
            r.lod.max_view_distance_m = 256.0;
            r.water.enabled = true;
            r.water.sea_level_m = 500.0;
            Terrain::compile(r, Map::new()).expect("compile")
        };
        let mut state = scene();
        let mut rt = TerrainRuntime::new();
        rt.set_terrain(Some(flooded), &mut state);
        settle(&mut rt, &mut state, 400);
        assert!(rt.stats().resident_chunks > 0);
        assert!(
            rt.resident.values().all(|r| r.water_slot.is_some()),
            "a wholly submerged world must have water everywhere"
        );
        // Two draws per visible chunk — the ground and the water riding on it.
        assert_eq!(
            state.terrain_draws.len(),
            rt.visible.len() * 2,
            "every visible submerged chunk owes a ground draw and a water draw"
        );
        // And the water is above the ground it covers, which is the only thing that makes it visible.
        let uploaded: BTreeSet<u32> = state.terrain_uploads.iter().map(|u| u.slot).collect();
        for r in rt.resident.values() {
            let slot = r.water_slot.expect("water");
            assert!(uploaded.contains(&slot), "water slot was never uploaded");
        }

        // The same world with the water switched off costs nothing at all.
        let dry = {
            let mut r = preset::by_id("rolling-hills").expect("preset");
            r.world_size_m = 512.0;
            r.chunk_verts = 33;
            r.lod.max_view_distance_m = 256.0;
            r.water.enabled = false;
            Terrain::compile(r, Map::new()).expect("compile")
        };
        let mut state = scene();
        let mut rt = TerrainRuntime::new();
        rt.set_terrain(Some(dry), &mut state);
        settle(&mut rt, &mut state, 400);
        assert!(rt.stats().resident_chunks > 0);
        assert!(
            rt.resident.values().all(|r| r.water_slot.is_none()),
            "water is off, so no chunk may hold a water slot"
        );
        assert_eq!(state.terrain_draws.len(), rt.visible.len());
    }

    #[test]
    fn clearing_the_terrain_leaves_nothing_behind() {
        let mut state = scene();
        let mut rt = TerrainRuntime::new();
        rt.set_terrain(Some(small_terrain()), &mut state);
        settle(&mut rt, &mut state, 400);
        rt.set_terrain(None, &mut state);
        assert!(!rt.is_active());
        rt.update(
            &mut state,
            [0.0, 10.0, 0.0],
            [0.0; 16],
            ChunkCoord::new(0, 0),
        );
        assert!(state.terrain_draws.is_empty());
        assert_eq!(rt.stats().resident_chunks, 0);
    }

    #[test]
    fn height_and_paths_are_answerable_from_the_runtime() {
        let mut state = scene();
        let mut rt = TerrainRuntime::new();
        assert_eq!(rt.height_at(10.0, 10.0), None);
        rt.set_terrain(Some(small_terrain()), &mut state);
        let h = rt.height_at(256.0, 256.0).expect("live terrain answers");
        assert!(h.is_finite());
        settle(&mut rt, &mut state, 400);
        // With nav grids resident, a short path across walkable ground resolves.
        if let Some(path) = rt.find_path([250.0, h, 250.0], [262.0, h, 262.0]) {
            assert!(
                path.found || path.reason.is_some(),
                "a failure must explain itself"
            );
        }
    }

    #[test]
    fn memory_accounting_reflects_what_is_held() {
        let mut state = scene();
        let mut rt = TerrainRuntime::new();
        rt.set_terrain(Some(small_terrain()), &mut state);
        settle(&mut rt, &mut state, 400);
        let s = rt.stats();
        assert!(s.mesh_bytes > 0, "meshes are held but not accounted");
        assert!(s.texture_bytes > 0, "textures are held but not accounted");
        assert!(s.total_bytes() >= s.mesh_bytes + s.texture_bytes);
    }
    #[test]
    fn a_chunk_that_fails_to_build_is_reported_once_and_not_retried_for_ever() {
        // SCOPE, stated honestly: this drives the runtime's half of the failure path directly. It does NOT
        // induce a real panic inside a worker — forcing `build_chunk` to panic would need a rigged recipe
        // or a test-only branch in the hot loop, and neither is worth the cost. What the worker does with a
        // panic is `catch_unwind` returning `Err`, which is std's contract; what the RUNTIME does with the
        // result is the part that was silently wrong, and that is what this pins.
        let mut state = scene();
        let mut rt = TerrainRuntime::new();
        rt.set_terrain(Some(small_terrain()), &mut state);
        settle(&mut rt, &mut state, 400);

        let bad = ChunkCoord::new(1, 1);
        rt.in_flight.insert(bad);
        let gen = rt.generation;
        rt.note_failed(&mut state, gen, bad, "index out of bounds");

        // The queue slot is handed back — leaking it is permanent, and there are only MAX_IN_FLIGHT.
        assert!(
            !rt.in_flight.contains(&bad),
            "the in-flight slot must be freed"
        );
        // The author is told which piece of world is missing, and why.
        let problem = state
            .terrain_problem
            .clone()
            .expect("a problem is reported");
        assert!(problem.contains("(1, 1)"), "{problem}");
        assert!(problem.contains("index out of bounds"), "{problem}");

        // And it is not resubmitted. The streamer plans against the view and knows nothing about failure,
        // so without the guard this chunk would burn a worker on the same panic every single replan.
        rt.last_plan_eye = [f32::MAX; 3];
        for _ in 0..40 {
            rt.update(
                &mut state,
                [256.0, 40.0, 256.0],
                [0.0; 16],
                ChunkCoord::new(4, 4),
            );
        }
        assert!(
            !rt.in_flight.contains(&bad),
            "a failed chunk must not be re-queued"
        );
        assert!(rt.failed.contains_key(&bad), "and it stays remembered");

        // Editing the world is the one event that can make it buildable again, and the only moment an
        // author would expect another attempt. So an edit clears the record and the complaint with it.
        rt.invalidate(&mut state, [0.0, 0.0], [512.0, 512.0]);
        assert!(
            rt.failed.is_empty(),
            "an edit must give every hole another try"
        );
        assert!(
            state.terrain_problem.is_none(),
            "and must not leave a stale complaint"
        );
    }

    #[test]
    fn a_stale_generation_failure_frees_its_slot_without_alarming_anyone() {
        // A result from a retired recipe is not news: the world it belonged to is gone. It must still hand
        // back the queue slot it was holding, or every edit would leak one.
        let mut state = scene();
        let mut rt = TerrainRuntime::new();
        rt.set_terrain(Some(small_terrain()), &mut state);
        let old = ChunkCoord::new(2, 2);
        rt.in_flight.insert(old);
        let stale = rt.generation.wrapping_sub(1);
        rt.note_failed(&mut state, stale, old, "gone");
        assert!(!rt.in_flight.contains(&old), "the slot is still freed");
        assert!(
            rt.failed.is_empty(),
            "but a dead generation is not a live problem"
        );
        assert!(
            state.terrain_problem.is_none(),
            "and the author is not told about it"
        );
    }

    #[test]
    fn a_panic_payload_is_read_back_as_text_whichever_form_it_took() {
        let s: String = std::panic::catch_unwind(|| panic!("borrowed message"))
            .map_or_else(|e| panic_text(e.as_ref()), |()| String::new());
        assert_eq!(s, "borrowed message");
        let owned: String = std::panic::catch_unwind(|| panic!("{}", format!("owned {}", 1)))
            .map_or_else(|e| panic_text(e.as_ref()), |()| String::new());
        assert_eq!(owned, "owned 1");
    }
}
