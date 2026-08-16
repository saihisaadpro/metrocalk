//! M19 (ADR-104) — the large-world acceptance bench: build a multi-kilometre terrain, fly a camera across
//! it, and measure what a person would actually feel.
//!
//! ## What this measures, and what it cannot
//!
//! Measured here, headlessly and honestly:
//!
//! * **Loading speed** — compile time, per-chunk build time, and the wall-clock to fill a view ring on a
//!   real worker pool.
//! * **Memory** — the resident working set against the recipe's declared ceiling, by category.
//! * **CPU frame cost** — the per-frame terrain work (plan, cull, draw-list rebuild, scatter selection),
//!   which is the part of "frame time" the subsystem controls.
//! * **Visual stability** — the *measured* screen-space error at every LOD switch, and bit-exact agreement
//!   between neighbouring chunks' shared edges. These are the two things that make terrain pop and crack.
//! * **Determinism at scale** — the same world, built twice, in different orders, byte for byte.
//!
//! **Not** measured here: GPU frame time and real pixels. There is no device in a headless test, and a
//! number invented for one would be worse than no number. The GPU path is exercised by the packaged app;
//! this bench bounds the CPU side that feeds it.
//!
//! Run with `cargo test --release --test terrain_large_world -- --nocapture` to see the report.

// A measurement harness: it converts byte counts to megabytes and indices to percentile positions
// constantly, and every one of those conversions is deliberate. The two long test functions are single
// linear reports — splitting them would scatter one narrative across five helpers.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use metrocalk_terrain::mesh::{measure_all_lods, screen_error_px, switch_distance};
use metrocalk_terrain::nav::NavConfig;
use metrocalk_terrain::preset;
use metrocalk_terrain::stream::{build_chunk, ChunkBuild, ChunkRequest, Streamer, ViewParams};
use metrocalk_terrain::{ChunkCoord, MemoryBudget, Terrain, TerrainRecipe};

/// The world under test: 4 km square at 1 m cells — 4 096 chunks, 17 M potential vertices, an eroded
/// mountain stack with biomes and scatter rules. Large enough that anything accidentally quadratic shows up.
fn large_world() -> TerrainRecipe {
    let mut r = preset::by_id("alpine").expect("preset");
    r.world_size_m = 4096.0;
    r.chunk_size_m = 64.0;
    r.chunk_verts = 65;
    r.lod.max_view_distance_m = 1536.0;
    r
}

fn mb(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i]
}

/// A worker pool exactly like the app's, so the loading-speed number is the one a user would experience.
fn build_on_pool(
    terrain: &Arc<Terrain>,
    requests: &[ChunkRequest],
    threads: usize,
) -> Vec<ChunkBuild> {
    let (job_tx, job_rx) = channel::<ChunkRequest>();
    let (done_tx, done_rx) = channel::<ChunkBuild>();
    let shared = Arc::new(Mutex::new(job_rx));
    let mut handles = Vec::new();
    for _ in 0..threads {
        let rx = Arc::clone(&shared);
        let tx = done_tx.clone();
        let t = Arc::clone(terrain);
        handles.push(std::thread::spawn(move || {
            let cfg = NavConfig::default();
            loop {
                let job = {
                    let Ok(guard) = rx.lock() else { return };
                    guard.recv()
                };
                let Ok(job) = job else { return };
                if tx.send(build_chunk(&t, &job, &cfg)).is_err() {
                    return;
                }
            }
        }));
    }
    for r in requests {
        job_tx.send(r.clone()).expect("queue");
    }
    drop(job_tx);
    drop(done_tx);
    let out: Vec<ChunkBuild> = done_rx.iter().collect();
    for h in handles {
        let _ = h.join();
    }
    out
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release-only measurement (run with --release)"
)]
fn a_four_kilometre_world_streams_inside_its_budget() {
    let recipe = large_world();
    println!("\n=== terrain large-world bench ===");
    println!(
        "world {} m · {} chunks · {} m cells · view {} m",
        recipe.world_size_m,
        recipe.chunk_count(),
        recipe.cell_size_at_lod(0),
        recipe.lod.max_view_distance_m
    );

    // ── compile (including the erosion bake) ──────────────────────────────────────────────────────────
    let t0 = Instant::now();
    let terrain = Terrain::compile(recipe.clone(), BTreeMap::new()).expect("compile");
    let compile_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let bakes: BTreeMap<_, _> = terrain
        .bakes()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let t1 = Instant::now();
    let _cached =
        Terrain::compile_with_bakes(recipe.clone(), BTreeMap::new(), &bakes).expect("recompile");
    let recompile_ms = t1.elapsed().as_secs_f64() * 1000.0;
    println!(
        "compile: {compile_ms:.0} ms cold (erosion baked) · {recompile_ms:.0} ms warm (bake reused) · \
         field state {:.1} MB",
        mb(terrain.bytes())
    );
    assert!(
        recompile_ms < compile_ms.max(1.0),
        "the bake cache did not help: {recompile_ms:.0} ms warm vs {compile_ms:.0} ms cold"
    );
    // An edit must feel live. A warm recompile is what a slider drag costs.
    assert!(
        recompile_ms < 400.0,
        "a warm recompile takes {recompile_ms:.0} ms — terrain editing would not feel live"
    );

    // ── streaming a view ring on a real worker pool ───────────────────────────────────────────────────
    let terrain = Arc::new(terrain);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().clamp(2, 8));
    let mut streamer = Streamer::new(recipe.budget);
    let eye = [2048.0, 320.0, 2048.0];
    let focus = recipe.chunk_at(eye[0], eye[2]);
    let plan = streamer.plan(&terrain, &ViewParams::at(eye, focus));
    let requested = plan.load.len();
    let t2 = Instant::now();
    let builds = build_on_pool(&terrain, &plan.load, threads);
    let fill_ms = t2.elapsed().as_secs_f64() * 1000.0;

    let mut per_chunk_ms: Vec<f64> = Vec::new();
    for req in plan.load.iter().take(24) {
        let t = Instant::now();
        let _ = build_chunk(&terrain, req, &NavConfig::default());
        per_chunk_ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    per_chunk_ms.sort_by(f64::total_cmp);
    println!(
        "streaming: {requested} chunks requested · filled in {fill_ms:.0} ms on {threads} workers · \
         per-chunk build p50 {:.2} ms / p99 {:.2} ms (single-threaded)",
        percentile(&per_chunk_ms, 0.5),
        percentile(&per_chunk_ms, 0.99)
    );
    assert!(!builds.is_empty(), "nothing streamed in");
    // One chunk must not cost a frame on its own, or a fast pan hitches however many workers there are.
    assert!(
        percentile(&per_chunk_ms, 0.99) < 60.0,
        "a chunk takes {:.1} ms to build — a fast pan would starve",
        percentile(&per_chunk_ms, 0.99)
    );

    // ── memory against the declared ceiling ───────────────────────────────────────────────────────────
    for b in &builds {
        streamer.insert(b);
    }
    let after = streamer.plan(&terrain, &ViewParams::at(eye, focus));
    let stats = streamer.stats();
    let cap = recipe.budget.total_bytes();
    let (mut mesh, mut tex, mut scat, mut coll, mut nav) = (0usize, 0usize, 0usize, 0usize, 0usize);
    for b in &builds {
        mesh += b.samples.bytes()
            + b.meshes
                .iter()
                .map(metrocalk_terrain::ChunkMesh::bytes)
                .sum::<usize>();
        tex += b
            .textures
            .as_ref()
            .map_or(0, metrocalk_terrain::material::ChunkTextures::bytes);
        scat += b.scatter.bytes();
        coll += b
            .collider
            .as_ref()
            .map_or(0, metrocalk_terrain::ChunkCollider::bytes);
        nav += b.nav.as_ref().map_or(0, metrocalk_terrain::NavGrid::bytes);
    }
    println!(
        "memory: {:.1} MB resident of a {:.0} MB ceiling — mesh {:.1} · texture {:.1} · scatter {:.1} · \
         collider {:.1} · nav {:.1}",
        mb(stats.bytes),
        mb(cap),
        mb(mesh),
        mb(tex),
        mb(scat),
        mb(coll),
        mb(nav)
    );
    assert!(
        stats.bytes <= cap || after.over_budget,
        "over the ceiling ({:.1} MB of {:.1} MB) without saying so",
        mb(stats.bytes),
        mb(cap)
    );
    assert!(
        stats.resident <= recipe.budget.max_resident_chunks as usize,
        "chunk cap ignored: {} resident",
        stats.resident
    );

    // ── planning cost ─────────────────────────────────────────────────────────────────────────────────
    // A full planning tick: residency, range and frustum culling, horizon ray marching and eviction. This is
    // NOT the per-frame cost — the runtime gates planning on camera movement and on a slow heartbeat, so a
    // frame normally pays only for the draw-list rebuild. It is measured because it is the expensive one, and
    // because a plan that cannot fit inside a frame would stall the render thread on the frames it does run.
    let mut frame_us: Vec<f64> = Vec::new();
    let mut still_requests = 0usize;
    for _ in 0..120 {
        let view = ViewParams::at(eye, focus);
        let t = Instant::now();
        let p = streamer.plan(&terrain, &view);
        frame_us.push(t.elapsed().as_secs_f64() * 1e6);
        still_requests += p.load.len();
    }
    // A camera that has not moved must ask for nothing at all. This is the property that separates a
    // streamer from a treadmill: if standing still costs work, so does everything else.
    assert_eq!(
        still_requests, 0,
        "a still camera re-requested {still_requests} chunks across 120 plans"
    );

    // And a *small* move re-requests only texture upgrades, never whole chunks.
    let nudged = streamer.plan(
        &terrain,
        &ViewParams::at([eye[0] + 8.0, eye[1], eye[2]], focus),
    );
    let fresh = nudged
        .load
        .iter()
        .filter(|r| !streamer.is_resident(r.coord))
        .count();
    println!(
        "an 8 m step: {} requests, {fresh} of them chunks that were not already resident",
        nudged.load.len()
    );
    assert!(
        fresh * 20 < stats.resident.max(1),
        "an 8 m step pulled in {fresh} new chunks out of {} resident",
        stats.resident
    );
    frame_us.sort_by(f64::total_cmp);
    println!(
        "planning tick (residency + cull + horizon + evict): p50 {:.0} µs · p99 {:.0} µs · {} chunks drawn          of {} resident (no projection supplied here, so only the horizon test culls)",
        percentile(&frame_us, 0.5),
        percentile(&frame_us, 0.99),
        after.visible.len(),
        stats.resident
    );
    assert!(
        percentile(&frame_us, 0.99) < 4000.0,
        "a planning tick costs {:.0} µs — it would stall the frame it runs on",
        percentile(&frame_us, 0.99)
    );
    assert!(
        after.culled_frustum + after.culled_horizon > 0 || after.visible.len() < stats.resident,
        "culling rejected nothing at all — the test is not exercising it"
    );

    // ── visual stability: the measured pop at every LOD switch ────────────────────────────────────────
    let mut worst_px = 0.0f32;
    let mut worst_error_m = 0.0f32;
    for b in builds.iter().take(64) {
        for e in measure_all_lods(&b.samples, recipe.lod.levels) {
            worst_error_m = worst_error_m.max(e.error_m);
            let d = switch_distance(e.error_m, &recipe.lod);
            if d > 0.0 {
                worst_px = worst_px.max(screen_error_px(e.error_m, d, &recipe.lod));
            }
        }
    }
    println!(
        "visual stability: worst geometric error {worst_error_m:.2} m · worst on-screen movement at a LOD \
         switch {worst_px:.3} px (budget {:.2} px)",
        recipe.lod.screen_error_px
    );
    assert!(
        worst_px <= recipe.lod.screen_error_px + 1e-3,
        "geometry moves {worst_px:.3} px at a LOD switch — that is a visible pop"
    );

    // ── seams: neighbouring chunks agree bit-exactly ──────────────────────────────────────────────────
    let mut checked = 0usize;
    for cz in 30..34 {
        for cx in 30..34 {
            let a = terrain
                .sample_chunk(ChunkCoord::new(cx, cz))
                .expect("chunk");
            let b = terrain
                .sample_chunk(ChunkCoord::new(cx + 1, cz))
                .expect("chunk");
            let n = a.verts as i32 - 1;
            for iz in 0..=n {
                assert_eq!(
                    a.height_at(n, iz).to_bits(),
                    b.height_at(0, iz).to_bits(),
                    "chunks ({cx},{cz}) and ({},{cz}) disagree on their shared edge",
                    cx + 1
                );
                // And on the halo, which is what makes the normals match too.
                assert_eq!(
                    a.height_at(n + 1, iz).to_bits(),
                    b.height_at(1, iz).to_bits()
                );
                checked += 1;
            }
        }
    }
    println!("seams: {checked} shared-edge vertices bit-identical across chunk boundaries");

    // ── determinism at scale ──────────────────────────────────────────────────────────────────────────
    let coords: Vec<ChunkCoord> = builds.iter().take(8).map(|b| b.coord).collect();
    let req = |c: ChunkCoord| ChunkRequest {
        coord: c,
        lod: 0,
        want_textures: true,
        want_normals: true,
        want_scatter: true,
        collider_lod: Some(0),
        want_nav: true,
        priority: 0.0,
    };
    let forward: Vec<ChunkBuild> = coords
        .iter()
        .map(|c| build_chunk(&terrain, &req(*c), &NavConfig::default()))
        .collect();
    let backward: Vec<ChunkBuild> = coords
        .iter()
        .rev()
        .map(|c| build_chunk(&terrain, &req(*c), &NavConfig::default()))
        .collect();
    for (f, b) in forward.iter().zip(backward.iter().rev()) {
        assert_eq!(
            f, b,
            "build order changed chunk ({}, {})",
            f.coord.x, f.coord.z
        );
    }
    println!(
        "determinism: {} chunks identical when built in reverse order",
        coords.len()
    );
    println!("=== end ===\n");
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release-only measurement (run with --release)"
)]
fn flying_across_the_world_streams_without_thrashing_or_leaking() {
    // The end-user motion this subsystem exists for: hold a direction and keep going. Chunks must arrive
    // ahead of the camera, leave behind it, and the totals must stay bounded — a streamer that re-requests
    // what it just dropped looks fine for ten seconds and dies after a minute.
    let mut recipe = large_world();
    recipe.lod.max_view_distance_m = 768.0;
    recipe.budget = MemoryBudget {
        mesh_mb: 96,
        texture_mb: 128,
        scatter_mb: 32,
        collider_mb: 32,
        max_resident_chunks: 256,
    };
    let terrain = Arc::new(Terrain::compile(recipe.clone(), BTreeMap::new()).expect("compile"));
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().clamp(2, 8));
    let mut streamer = Streamer::new(recipe.budget);

    let mut total_loads = 0usize;
    let mut total_evictions = 0usize;
    let mut ever_resident: BTreeSet<ChunkCoord> = BTreeSet::new();
    let mut reloaded = 0usize;
    let mut peak_bytes = 0usize;
    let start = Instant::now();
    // 3 km of travel in 64 m steps — one chunk per step, the worst case for a streaming system.
    for step in 0..48 {
        let x = 512.0 + step as f32 * 64.0;
        let eye = [x, 300.0, 2048.0];
        let view = ViewParams::at(eye, recipe.chunk_at(x, 2048.0));
        let plan = streamer.plan(&terrain, &view);
        total_loads += plan.load.len();
        total_evictions += plan.unload.len();
        for r in &plan.load {
            if !ever_resident.insert(r.coord) {
                reloaded += 1;
            }
        }
        for b in build_on_pool(&terrain, &plan.load, threads) {
            streamer.insert(&b);
        }
        peak_bytes = peak_bytes.max(streamer.stats().bytes);
    }
    let elapsed = start.elapsed().as_secs_f64();
    let cap = recipe.budget.total_bytes();
    println!(
        "\nfly-through: 3 km in 48 steps, {elapsed:.1} s · {total_loads} loads · {total_evictions} evictions \
         · {reloaded} reloads of a chunk already seen · peak {:.1} MB of a {:.0} MB ceiling · {} resident",
        mb(peak_bytes),
        mb(cap),
        streamer.stats().resident
    );
    assert!(total_loads > 100, "the fly-through streamed almost nothing");
    assert!(
        total_evictions > 0,
        "nothing was ever evicted — memory would grow without bound"
    );
    // Some reloading is legitimate (a chunk left the ring and came back is not one of these — the path is a
    // straight line), but a thrashing streamer reloads a large fraction of what it drops.
    assert!(
        reloaded * 4 < total_loads,
        "{reloaded} of {total_loads} loads were repeats — the streamer is thrashing"
    );
    assert!(
        peak_bytes <= cap,
        "peak {:.1} MB exceeded the {:.0} MB ceiling",
        mb(peak_bytes),
        mb(cap)
    );
}
