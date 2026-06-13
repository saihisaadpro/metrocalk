//! F1 measurement (spike ②): does marking capability relationships `DontFragment` (sparse) collapse
//! the ~14.8 KB/entity archetype-fragmentation overhead without hurting the compatibility-query
//! latency? Measures ONE (variant, size) per process so RSS isn't contaminated across variants.
//!
//! Usage: `scene-bench <dense|sparse> <5000|20000>`  (defaults: dense 20000)

use memory_stats::memory_stats;
use metrocalk_ecs::scene::{build_scene, compat_clauses, SceneParams};
use metrocalk_ecs::{FlecsWorld, World};
use std::time::Instant;

fn rss_bytes() -> u64 {
    memory_stats().map_or(0, |m| m.physical_mem as u64)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let variant = args.get(1).map_or("dense", String::as_str);
    let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20_000);
    let sparse = match variant {
        "sparse" => true,
        "dense" => false,
        other => {
            eprintln!("unknown variant {other:?}; use dense|sparse");
            std::process::exit(2);
        }
    };
    let params = if n >= 20_000 {
        SceneParams::preset_20k()
    } else {
        SceneParams::preset_5k()
    };

    let before = rss_bytes();
    let mut w = FlecsWorld::new();
    let scene = build_scene(&mut w, &params, sparse);
    let after = rss_bytes();
    let peak = memory_stats()
        .map_or(0, |m| m.physical_mem as u64)
        .max(after);

    let bytes_per_entity = (after.saturating_sub(before)) as f64 / params.entities as f64;

    // latency: cached compat query, median + p99 over 2000 samples after warm-up
    let q = w.build_query(&compat_clauses(&scene));
    for _ in 0..200 {
        let mut c = 0usize;
        w.for_each_match(&q, &mut |_| c += 1);
        std::hint::black_box(c);
    }
    let mut us = Vec::with_capacity(2000);
    for _ in 0..2000 {
        let t = Instant::now();
        let mut c = 0usize;
        w.for_each_match(&q, &mut |_| c += 1);
        std::hint::black_box(c);
        us.push(t.elapsed().as_secs_f64() * 1e6);
    }
    us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let len = us.len();
    let median = us[len / 2];
    let p99 = us[((len as f64 * 0.99).ceil() as usize).min(len) - 1];
    let matched = w.matches(&q).len();

    println!("variant={variant} n={n} digest={:#018x}", scene.digest);
    println!(
        "  memory: RSS before {:.1} MB → after {:.1} MB  ⇒  {:.0} bytes/entity (delta){}",
        before as f64 / 1_048_576.0,
        after as f64 / 1_048_576.0,
        bytes_per_entity,
        if sparse {
            ""
        } else {
            "   (baseline target: 14.8 KB/entity @20k)"
        },
    );
    println!("  compat query: matched={matched} median={median:.2} µs p99={p99:.2} µs");
    let _ = peak;
}
