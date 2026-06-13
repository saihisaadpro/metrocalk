//! M0 spike ②: Flecs v4.1 via flecs_ecs 0.2.2 — compatibility-query performance at editor scale
//! (ADR-001 gate). One command runs the whole suite serially and prints a Markdown report:
//!   cargo run --release                        (safety locks ON  — flecs_safety_locks)
//!   cargo run --release --no-default-features  (safety locks OFF)
//! Quality bar: trustworthy measurements, not production.

use flecs_ecs::prelude::*;
use flecs_spike::rng::Rng;
use flecs_spike::{build_scene, compat_query, BindsTo, Provides, HEALTH_CAP, SEED};
use std::time::Instant;

fn main() {
    let safety = cfg!(feature = "safety");
    println!("# Flecs (flecs_ecs 0.2.2 / flecs C v4.1.2) spike results\n");
    println!(
        "seed = 0x{SEED:016X} · safety locks (`flecs_safety_locks`): **{}**\n",
        if safety { "ON" } else { "OFF" }
    );

    bench1_compat(5_000, 2_000);
    bench2_under_mutation(5_000, 2_000);
    bench3_wildcard(5_000, 2_000);
    bench4_churn(5_000, 2_000);
    bench1_compat(20_000, 8_000); // benchmark 5: scale
    bench5_memory(20_000, 8_000);

    println!("\npeak RSS over whole process: {:.1} MB", peak_rss_mb());
}

// ---------- bench 1: the compatibility question, cold (uncached) vs cached ----------

fn bench1_compat(n: usize, edges: usize) {
    println!("\n## Bench 1 — compatibility query at {n} entities ((Provides,Health) without (BindsTo,*))\n");
    let scene = build_scene(SEED, n, edges);

    let qc = compat_query(&scene, true);
    let qu = compat_query(&scene, false);

    // correctness + result-set size (same for both queries)
    let mut ids: Vec<Entity> = Vec::new();
    qc.each_entity(|e, _| ids.push(e.id()));
    let matched = ids.len();

    // cold cached: a freshly built cached query's first iteration populates its cache.
    let qcold = compat_query(&scene, true);
    let t = Instant::now();
    let mut c0 = 0usize;
    qcold.each(|_| c0 += 1);
    let cold_cached = t.elapsed().as_secs_f64() * 1e6;
    assert_eq!(c0, matched);

    let warm_cached = measure(2_000, 200, || {
        let mut c = 0usize;
        qc.each(|_| c += 1);
        std::hint::black_box(c);
    });
    let uncached = measure(2_000, 200, || {
        let mut c = 0usize;
        qu.each(|_| c += 1);
        std::hint::black_box(c);
    });

    println!("matched entities: {matched} (of {n}); first cached eval (cold): {cold_cached:.1} µs\n");
    println!("| query | median | p99 | max |");
    println!("|---|---|---|---|");
    print_row("cached (warm, steady state)", &warm_cached);
    print_row("uncached (re-evaluated each call)", &uncached);
}

// ---------- bench 2: query latency under mutation (cache invalidation) ----------

fn bench2_under_mutation(n: usize, edges: usize) {
    println!("\n## Bench 2 — cached query latency under mutation (100 pair add/removes between iterations)\n");
    let scene = build_scene(SEED, n, edges);
    let q = compat_query(&scene, true);
    let mut rng = Rng::new(SEED ^ 0x2222);

    // churn: add/remove random (Provides, cap) and (BindsTo, target) pairs, then re-query.
    let churn = |rng: &mut Rng| {
        for _ in 0..100 {
            let e = scene.world.entity_from_id(scene.entities[rng.below(scene.entities.len())]);
            let cap = scene.caps[rng.below(scene.caps.len())];
            let tgt = scene.entities[rng.below(scene.entities.len())];
            match rng.below(4) {
                0 => {
                    e.add((Provides, cap));
                }
                1 => {
                    e.remove((Provides, cap));
                }
                2 => {
                    e.add((BindsTo, tgt));
                }
                _ => {
                    e.remove((BindsTo, tgt));
                }
            }
        }
    };

    // warm up
    for _ in 0..20 {
        churn(&mut rng);
        let mut c = 0usize;
        q.each(|_| c += 1);
        std::hint::black_box(c);
    }
    let mut samples = Vec::with_capacity(300);
    for _ in 0..300 {
        churn(&mut rng);
        let t = Instant::now();
        let mut c = 0usize;
        q.each(|_| c += 1);
        std::hint::black_box(c);
        samples.push(t.elapsed().as_secs_f64() * 1e6);
    }
    let st = stats(&mut samples);
    println!("| query after 100 mutations | median | p99 | max |");
    println!("|---|---|---|---|");
    print_row("cached re-query", &st);
}

// ---------- bench 3: wildcard traversal of every binding edge ----------

fn bench3_wildcard(n: usize, edges: usize) {
    println!("\n## Bench 3 — wildcard traversal: every (BindsTo, *) edge (relationship visualizer)\n");
    let scene = build_scene(SEED, n, edges);
    let mut b = scene.world.query::<()>();
    b.with((BindsTo, id::<flecs::Wildcard>()));
    b.set_cached();
    let q = b.build();

    // collect (source, target) for each edge; count to verify
    let mut pairs: Vec<(Entity, Entity)> = Vec::new();
    q.each_iter(|it, i, _| {
        let src = it.entity(i).id();
        let tgt = it.pair(0).second_id().id();
        pairs.push((src, tgt));
    });
    let edge_count = pairs.len();

    let st = measure(2_000, 200, || {
        let mut c = 0usize;
        q.each_iter(|it, i, _| {
            let _ = it.entity(i).id();
            let _t = it.pair(0).second_id().id();
            c += 1;
        });
        std::hint::black_box(c);
    });
    println!("edges traversed: {edge_count}\n");
    println!("| traversal | median | p99 | max |");
    println!("|---|---|---|---|");
    print_row("every BindsTo edge (cached)", &st);
}

// ---------- bench 4: churn correctness (zero stale results) ----------

fn bench4_churn(n: usize, edges: usize) {
    println!("\n## Bench 4 — churn correctness: 1,000 entities created then destroyed\n");
    let scene = build_scene(SEED, n, edges);
    let q = compat_query(&scene, true);
    let health = scene.caps[HEALTH_CAP];

    let count = || {
        let mut c = 0usize;
        q.each(|_| c += 1);
        c
    };
    let baseline = count();

    // create 1,000 fresh Health-providing entities with NO binding → all should match
    let mut created = Vec::with_capacity(1_000);
    for _ in 0..1_000 {
        created.push(scene.world.entity().add((Provides, health)).id());
    }
    let after_create = count();

    // destroy them
    let t = Instant::now();
    for e in &created {
        scene.world.entity_from_id(*e).destruct();
    }
    let destroy_us = t.elapsed().as_secs_f64() * 1e6;
    let after_destroy = count();

    let ok = after_create == baseline + 1_000 && after_destroy == baseline;
    println!("baseline {baseline} → +1000 created → {after_create} → destroyed → {after_destroy}");
    println!("destruct 1000 entities: {destroy_us:.0} µs");
    println!(
        "\n**zero stale results: {}** (expected {} == {} after destroy)",
        if ok { "PASS" } else { "FAIL" },
        after_destroy,
        baseline
    );
    assert!(ok, "stale results after churn");
}

// ---------- bench 5: memory per entity ----------

fn bench5_memory(n: usize, edges: usize) {
    println!("\n## Bench 5 — memory at {n} entities\n");
    let before = cur_rss_mb();
    let scene = build_scene(SEED, n, edges);
    let after = cur_rss_mb();
    let live = scene.entities.len();
    std::hint::black_box(&scene);
    println!("RSS before scene: {before:.1} MB · after: {after:.1} MB · entities: {live}");
    println!(
        "approx bytes/entity (delta): {:.0}",
        (after - before) * 1024.0 * 1024.0 / live as f64
    );
}

// ---------- timing helpers ----------

struct Stats {
    median: f64,
    p99: f64,
    max: f64,
}

fn measure(iters: usize, warmup: usize, mut f: impl FnMut()) -> Stats {
    for _ in 0..warmup {
        f();
    }
    let mut samples = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t = Instant::now();
        f();
        samples.push(t.elapsed().as_secs_f64() * 1e6);
    }
    stats(&mut samples)
}

fn stats(xs: &mut [f64]) -> Stats {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = xs.len();
    Stats {
        median: xs[n / 2],
        p99: xs[((n as f64 * 0.99).ceil() as usize).min(n) - 1],
        max: xs[n - 1],
    }
}

fn print_row(label: &str, s: &Stats) {
    let fmt = |v: f64| {
        if v >= 1000.0 {
            format!("{:.2} ms", v / 1000.0)
        } else {
            format!("{v:.1} µs")
        }
    };
    println!("| {label} | {} | {} | {} |", fmt(s.median), fmt(s.p99), fmt(s.max));
}

// ---------- RSS (Windows) ----------

#[cfg(windows)]
fn mem_counters() -> windows_sys::Win32::System::ProcessStatus::PROCESS_MEMORY_COUNTERS {
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let mut pmc: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        K32GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb);
        pmc
    }
}

#[cfg(windows)]
fn peak_rss_mb() -> f64 {
    mem_counters().PeakWorkingSetSize as f64 / (1024.0 * 1024.0)
}
#[cfg(windows)]
fn cur_rss_mb() -> f64 {
    mem_counters().WorkingSetSize as f64 / (1024.0 * 1024.0)
}
#[cfg(not(windows))]
fn peak_rss_mb() -> f64 {
    0.0
}
#[cfg(not(windows))]
fn cur_rss_mb() -> f64 {
    0.0
}
