//! M6 — the placeholder→commit latency (the *interactive* part of generation: the instant grey
//! placeholder the user sees before the provider round-trip). Release is the metric
//! (`cargo test -p metrocalk-editor-shell --release --test generate_bench -- --nocapture`). The
//! generation round-trip itself is provider-latency-bound (network) — NOT measured here, by design.

#![allow(clippy::cast_precision_loss)]

use std::time::Instant;

use metrocalk_core::Engine;
use metrocalk_ecs::FlecsWorld;

use metrocalk_editor_shell::capscene::{self, CapScene};

const N: usize = 5000;

#[test]
fn placeholder_commit_is_instant() {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    capscene::seed(&mut engine, &scene, N).expect("seed");
    engine.clear_history();

    for _ in 0..50 {
        let _ = capscene::place_generation_placeholder(&mut engine, &scene, [0.0; 3]);
    }
    let mut t = Vec::new();
    for _ in 0..500 {
        let t0 = Instant::now();
        let id = capscene::place_generation_placeholder(&mut engine, &scene, [0.0; 3]).unwrap();
        t.push(t0.elapsed().as_secs_f64() * 1e6); // µs
        std::hint::black_box(id);
    }
    t.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let (p50, p99) = (t[t.len() / 2], t[t.len() * 99 / 100]);
    eprintln!("[M6] place_generation_placeholder on 5k scene: p50={p50:.2}us p99={p99:.2}us");
    // The interactive placeholder must be effectively instant — well under the 16 ms frame budget.
    assert!(
        p99 < 16_000.0,
        "placeholder commit must be instant: p99={p99:.2}us"
    );
}
