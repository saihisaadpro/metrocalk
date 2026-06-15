//! M2 go/no-go — the integrated editor's end-to-end frame-budget components on the real 5k scene,
//! plus the "Windows snapshot-load cliff" residual. Release-only is the metric (`cargo test
//! --release`); a debug run is not the benchmark. Render-submit (p50 0.74 ms, M2.6) and live reveal
//! (p99 1.523 ms, `reveal_live_cost`) are measured elsewhere; this covers commit + delta + load.

#![allow(clippy::cast_precision_loss)]

use std::time::Instant;

use metrocalk_core::{Engine, FieldValue, Op};
use metrocalk_ecs::FlecsWorld;
use metrocalk_editor_shell::{capscene, project_full, CapScene};

fn pct(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (v[v.len() / 2], v[v.len() * 99 / 100])
}

#[test]
fn m2_end_to_end_budget_on_5k() {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    let index = capscene::seed(&mut engine, &scene, 5000).expect("seed 5k");
    let bar = index.health_bars[0];

    // 1) commit: a field edit through the single pipeline (the user-edit hot path).
    for i in 0..20 {
        engine
            .commit("warm", vec![set_x(bar, f64::from(i))])
            .unwrap();
    }
    let mut commit_t = Vec::new();
    for i in 0..200 {
        let t0 = Instant::now();
        engine
            .commit("edit", vec![set_x(bar, f64::from(i))])
            .unwrap();
        commit_t.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let (c50, c99) = pct(commit_t);

    // 2) project_full: the whole-scene delta (the connect/undo path — the heaviest delta build).
    let mut proj_t = Vec::new();
    for _ in 0..50 {
        let t0 = Instant::now();
        let d = project_full(&engine);
        proj_t.push(t0.elapsed().as_secs_f64() * 1000.0);
        assert!(!d.ops.is_empty());
    }
    let (p50, p99) = pct(proj_t);

    // 3) snapshot-load cliff: export the whole doc, import (merge) into a fresh engine — the Windows
    //    "open a saved 5k project" path.
    let snapshot = engine.export_updates();
    let mut load_t = Vec::new();
    for _ in 0..20 {
        let t0 = Instant::now();
        let mut fresh = Engine::new(FlecsWorld::new(), 1);
        fresh.merge(&snapshot).expect("merge snapshot");
        load_t.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let (l50, l99) = pct(load_t);

    eprintln!("[M2-budget] 5k scene (release):");
    eprintln!("  commit (field edit):    p50={c50:.3}ms p99={c99:.3}ms");
    eprintln!("  project_full (delta):   p50={p50:.3}ms p99={p99:.3}ms");
    eprintln!(
        "  snapshot-load (merge):  p50={l50:.3}ms p99={l99:.3}ms  (snapshot {} bytes)",
        snapshot.len()
    );

    // A single committed edit + its echo must fit a frame; project_full is not per-frame (connect/undo).
    assert!(
        c99 < 16.0,
        "commit must fit the frame budget: p99={c99:.3}ms"
    );
}

fn set_x(entity: metrocalk_core::EntityId, x: f64) -> Op {
    Op::SetField {
        entity,
        component: "Transform".into(),
        field: "x".into(),
        value: FieldValue::Number(x),
    }
}
