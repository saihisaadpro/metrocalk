//! Wrapper-vs-raw zero-cost check (deliverable 4): the cached compat query, run through the
//! `World` trait, must stay within run-to-run noise of the raw-Flecs spike (8.7 µs median /
//! 12.2 µs p99 @5k). Run: `cargo run --release -p metrocalk-ecs --bin bench`.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)] // timing math + conventional w/s/q bench locals

use metrocalk_ecs::scene::build_compat_scene;
use metrocalk_ecs::{Clause, FlecsWorld, Target, Term, World};
use std::time::Instant;

fn main() {
    let mut w = FlecsWorld::new();
    let s = build_compat_scene(&mut w, 5000, 2000);
    let clauses = [
        Clause::with(Term::Pair {
            rel: s.provides,
            target: Target::Exact(s.health),
        }),
        Clause::without(Term::Pair {
            rel: s.binds_to,
            target: Target::Any,
        }),
    ];
    let q = w.build_query(&clauses);

    // warm up (cache populates on first eval)
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
    let n = us.len();
    let median = us[n / 2];
    let p99 = us[((n as f64 * 0.99).ceil() as usize).min(n) - 1];
    let matched = w.matches(&q).len();

    println!("wrapper compat query @5k (safety locks ON):");
    println!("  matched = {matched} (expected 211)");
    println!(
        "  median = {median:.2} µs · p99 = {p99:.2} µs · max = {:.2} µs",
        us[n - 1]
    );
    println!("  raw-Flecs spike ② baseline: 8.7 µs median / 12.2 µs p99 @5k");
}
