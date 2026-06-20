//! Resolver-tier latencies (release is the metric:
//! `cargo test -p metrocalk-core --release --test tier_bench -- --nocapture`).
//! The marketplace query is the **second** tier — it needn't beat 16 ms, but it's recorded; the
//! **local** resolve must stay ~unchanged from ADR-012 (~85 µs). A debug run is not the benchmark.

#![allow(clippy::cast_precision_loss)]

use std::time::Instant;

use metrocalk_core::marketplace::{LocalCatalog, MarketplaceIndex};
use metrocalk_core::{resolve_local, stdlib};

fn pct(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (v[v.len() / 2], v[v.len() * 99 / 100])
}

#[test]
fn tier_latencies() {
    let lib = stdlib::standard_components();
    let cat = LocalCatalog::builtin();

    for _ in 0..300 {
        let _ = resolve_local(&lib, "health bar");
        let _ = cat.query("rusty medieval sword");
    }

    let mut lt = Vec::new();
    for _ in 0..3000 {
        let t = Instant::now();
        std::hint::black_box(resolve_local(&lib, "health bar"));
        lt.push(t.elapsed().as_secs_f64() * 1e6);
    }
    let (lp50, lp99) = pct(lt);

    let mut mt = Vec::new();
    for _ in 0..3000 {
        let t = Instant::now();
        std::hint::black_box(cat.query("rusty medieval sword"));
        mt.push(t.elapsed().as_secs_f64() * 1e6);
    }
    let (mp50, mp99) = pct(mt);

    eprintln!(
        "[M5-tier] resolve_local p50={lp50:.2}us p99={lp99:.2}us | marketplace query p50={mp50:.2}us p99={mp99:.2}us ({} catalog entries)",
        cat.len()
    );
    assert!(
        lp99 < 16_000.0,
        "local resolve stays well under the 16 ms budget (ADR-012)"
    );
    assert!(
        mp99 < 16_000.0,
        "marketplace query (2nd tier) measured + well under budget on the local catalog"
    );
}
