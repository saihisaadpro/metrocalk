//! Criterion cross-check for Bench 1 (cached compatibility query at 5k entities), to confirm the
//! hand-rolled timing loop in main.rs isn't measuring noise. Same seed, same scene, same query.

use criterion::{criterion_group, criterion_main, Criterion};
use flecs_ecs::prelude::*;
use flecs_spike::{build_scene, compat_query, SEED};
use std::hint::black_box;

fn bench_compat_cached_5k(c: &mut Criterion) {
    let scene = build_scene(SEED, 5_000, 2_000);
    let q = compat_query(&scene, true);
    // warm the cache once
    let mut warm = 0usize;
    q.each(|_| warm += 1);
    black_box(warm);

    c.bench_function("compat_cached_5k", |b| {
        b.iter(|| {
            let mut count = 0usize;
            q.each(|_| count += 1);
            black_box(count);
        })
    });
}

criterion_group!(benches, bench_compat_cached_5k);
criterion_main!(benches);
