//! Catalog/search latency (M3.4, **release**): the browse query (`grouped`) + the search
//! (resolver-backed `search`) ride the interactive "+ Add" palette, so they must hold the <16 ms budget.
//! The catalog is small today; this guards the query shape as supply grows. Release-only (benchmark
//! discipline: always `--release` for timing; CI runs `cargo test` in debug, where it is ignored).

#![allow(clippy::cast_precision_loss)]

use std::time::Instant;

use metrocalk_core::catalog::{grouped, search};
use metrocalk_core::marketplace::LocalCatalog;
use metrocalk_core::stdlib::standard_components;

fn percentiles(mut us: Vec<f64>) -> (f64, f64) {
    us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (us[us.len() / 2], us[us.len() * 99 / 100])
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release-only timing measurement")]
fn catalog_browse_and_search_latency_under_budget() {
    let metas = standard_components();
    let cat = LocalCatalog::builtin();

    for _ in 0..100 {
        let _ = grouped(&metas, &cat);
        let _ = search(&metas, &cat, "health bar");
    }

    let mut browse = Vec::with_capacity(2000);
    for _ in 0..2000 {
        let t0 = Instant::now();
        let g = grouped(&metas, &cat);
        browse.push(t0.elapsed().as_secs_f64() * 1e6);
        assert!(!g.is_empty());
    }
    let mut srch = Vec::with_capacity(2000);
    for _ in 0..2000 {
        let t0 = Instant::now();
        let r = search(&metas, &cat, "health bar");
        srch.push(t0.elapsed().as_secs_f64() * 1e6);
        assert!(!r.items.is_empty());
    }

    let (bp50, bp99) = percentiles(browse);
    let (sp50, sp99) = percentiles(srch);
    eprintln!("[M3.4] catalog grouped(): p50={bp50:.1}us p99={bp99:.1}us");
    eprintln!("[M3.4] catalog search() (resolver-backed): p50={sp50:.1}us p99={sp99:.1}us");
    assert!(bp99 < 16_000.0, "browse p99={bp99:.1}us must be ≪ 16ms");
    assert!(sp99 < 16_000.0, "search p99={sp99:.1}us must be ≪ 16ms");
}
