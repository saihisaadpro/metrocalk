//! M3.3 action-model latency — the per-right-click `actions_for` query on the real 5k capability scene
//! (the menu-populate cost). Release is the metric
//! (`cargo test -p metrocalk-editor-shell --release --test action_bench -- --nocapture`); a debug run is
//! not the benchmark. It must hold well under the 16 ms frame budget — it's deterministic, O(1)/action
//! plus a bounded bindings scan, and runs once per right-click, never per frame.

#![allow(clippy::cast_precision_loss)]

use std::time::Instant;

use metrocalk_ecs::FlecsWorld;

use metrocalk_core::Engine;
use metrocalk_editor_shell::actions::actions_for;
use metrocalk_editor_shell::capscene::{self, CapScene};

const N: usize = 5000; // the shell's real scene size

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release-only timing measurement (discipline: --release for timing)"
)]
fn action_model_latency_under_budget_on_5k() {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    let index = capscene::seed(&mut engine, &scene, N).expect("seed");
    engine.clear_history();
    let bar = index.health_bars[0];

    for _ in 0..200 {
        std::hint::black_box(actions_for(&engine, &scene, bar));
    }
    let mut t = Vec::new();
    for _ in 0..2000 {
        let t0 = Instant::now();
        std::hint::black_box(actions_for(&engine, &scene, bar));
        t.push(t0.elapsed().as_secs_f64() * 1e6); // µs
    }
    t.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let (p50, p99) = (t[t.len() / 2], t[t.len() * 99 / 100]);
    eprintln!("[M3.3] actions_for on 5k scene: p50={p50:.2}us p99={p99:.2}us (5 actions)");
    assert!(
        p99 < 16_000.0,
        "the action model holds the frame budget: p99={p99:.2}us"
    );
}
