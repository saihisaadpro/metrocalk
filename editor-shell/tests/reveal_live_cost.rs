//! The *live* per-click reveal cost on the real 5k capability scene — what the Tauri `reveal_targets`
//! command actually does on each selection: the reveal engine's indexed query + the bounded
//! nearest-first "every-no-explained" greyed scan, with `positions` precomputed (the live shell caches
//! it in the viewport rebuild, so it is NOT on the per-click hot path). This is the M2/M3 frame-budget
//! number for binding-by-intent; it must hold well under 16 ms.
//!
//! Release-only is the metric (`cargo test --release`); a debug run is not the benchmark.

#![allow(clippy::cast_precision_loss)]

use std::collections::HashMap;

use metrocalk_core::{Engine, EntityId};
use metrocalk_ecs::{Entity, FlecsWorld};

use metrocalk_editor_shell::capscene::{self, CapScene};
use metrocalk_editor_shell::reveal::{reveal, why_not, Context};

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Mirror the live `compute_reveal`: the indexed reveal + the bounded greyed scan + the bound list.
/// Returns (compatible_count, greyed_count) so the work isn't optimized away.
fn compute_reveal_cost(
    engine: &Engine<FlecsWorld>,
    scene: &CapScene,
    positions: &HashMap<Entity, [f32; 3]>,
    eid: EntityId,
) -> (usize, usize) {
    let sel_ecs = engine.ecs_entity(eid).unwrap();
    let recency = HashMap::new();
    let ctx = Context {
        cap_name: &scene.cap_name,
        position: positions,
        recency: &recency,
    };
    let r = reveal(engine.world(), sel_ecs, scene.rels, &ctx);

    let sel_pos = positions.get(&sel_ecs).copied().unwrap_or([0.0; 3]);
    let mut others: Vec<(Entity, f32)> = engine
        .entity_ids()
        .into_iter()
        .filter(|&id| id != eid)
        .filter_map(|id| {
            let e = engine.ecs_entity(id)?;
            Some((
                e,
                dist(sel_pos, positions.get(&e).copied().unwrap_or([0.0; 3])),
            ))
        })
        .collect();
    others.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut greyed = 0usize;
    for (e, _) in others {
        if greyed >= 60 {
            break;
        }
        if why_not(engine.world(), sel_ecs, scene.rels, e, &scene.cap_name).is_some() {
            greyed += 1;
        }
    }
    (r.compatible.len(), greyed)
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release-only timing measurement (discipline: --release for timing)"
)]
fn live_reveal_cost_under_budget_on_5k_capability_scene() {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    let index = capscene::seed(&mut engine, &scene, 5000).expect("seed 5k");
    let positions = capscene::positions(&engine);
    let bar = index.health_bars[0];

    // warm
    for _ in 0..5 {
        let _ = compute_reveal_cost(&engine, &scene, &positions, bar);
    }
    let mut times = Vec::new();
    let (mut compat, mut greyed) = (0, 0);
    for _ in 0..100 {
        let t0 = std::time::Instant::now();
        let (c, g) = compute_reveal_cost(&engine, &scene, &positions, bar);
        times.push(t0.elapsed().as_secs_f64() * 1000.0);
        compat = c;
        greyed = g;
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = times[times.len() / 2];
    let p99 = times[times.len() * 99 / 100];
    eprintln!(
        "[M3.1-live] reveal_targets on 5k scene: p50={p50:.3}ms p99={p99:.3}ms (compatible={compat}, greyed={greyed})"
    );
    assert!(compat > 0 && greyed > 0, "the scene exercises both paths");
    assert!(
        p99 < 16.0,
        "live reveal must hold the frame budget: p99={p99:.3}ms"
    );
}
