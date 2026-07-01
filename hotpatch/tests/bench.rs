//! Release-only loop-time budget for the instant-iteration tier (M13.3). The edit→hot-patch→
//! state-restored loop is a discrete DEV/iteration op (off the per-frame hot path, inv. 4), so this
//! measures the daily-felt wedge as a repeatable number. Marked `#[cfg_attr(debug_assertions, ignore)]`
//! so it is SKIPPED in the debug `cargo test` (ci.yml) and RUN by `cargo test --workspace --release`
//! (release-budgets.yml) — NOT a dark test.
//!
//! WHAT THIS MEASURES (honest boundary): the loop we OWN — the hot-patch swap + the op-log replay that
//! reconstructs running state across a schema change. The `subsecond` binary recompile-jump (~130 ms,
//! tip-crate only) is the production dev backing behind the `HotPatch` trait; it needs the `dx` harness
//! + a running process, so its number is a documented LOCAL gate, never fabricated here.
//!
//! Min-spec is OWED: this measures the dev box, not a true low-core rig (the honest ceiling per
//! `<benchmark_discipline>` — never fabricate a min-spec number).

use metrocalk_core::{ComponentMeta, FieldSpec, FieldType, FieldValue, RuntimeState};
use metrocalk_hotpatch::{restore, IterationLoop, Migration, Schema, SwapHotPatch, SystemFn};
use std::time::Instant;

fn percentiles(mut us: Vec<u128>) -> (u128, u128) {
    us.sort_unstable();
    let p50 = us[us.len() / 2];
    let p99 = us[(us.len() * 99 / 100).min(us.len() - 1)];
    (p50, p99)
}

fn field(name: &str, ty: FieldType, required: bool) -> FieldSpec {
    FieldSpec {
        name: name.to_string(),
        ty,
        required,
        format: None,
    }
}

fn health(fields: Vec<FieldSpec>) -> ComponentMeta {
    ComponentMeta {
        name: "Health".to_string(),
        fields,
        ..Default::default()
    }
}

/// A representative scene: `Health.hp` on N entities (the recorded op-log the loop replays).
fn scene(n: usize) -> metrocalk_hotpatch::OpLog {
    let mut log = metrocalk_hotpatch::OpLog::new();
    for i in 0..n {
        #[allow(clippy::cast_possible_wrap)]
        log.set(
            format!("e{i}"),
            "Health",
            "hp",
            FieldValue::Integer((i % 100) as i64),
        );
    }
    log
}

fn system_v2(state: &RuntimeState, entities: &[String]) -> String {
    let mut total = 0i64;
    for e in entities {
        if let Some(FieldValue::Integer(cur)) = state.get(e, "Health", "current") {
            total += *cur;
        }
    }
    format!("sum={total}")
}

/// The whole edit→hot-patch→state-restored loop, timed in release over a representative scene. Asserts
/// a generous sub-second budget (it is instant — the dominant real-world cost is the `subsecond`
/// recompile, the dev-harness number) and prints the measured numbers (surfaced in release-budgets).
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release-only timing budget (run by release-budgets.yml)"
)]
fn hot_iterate_loop_holds_a_sub_second_budget() {
    const N: usize = 500;
    const RUNS: usize = 200;

    let v1_schema = Schema::new().with(health(vec![field("hp", FieldType::Integer, true)]));
    let v2_schema = Schema::new().with(health(vec![
        field("current", FieldType::Integer, true),
        field("max", FieldType::Integer, true),
    ]));
    let migration = Migration::new(1, 2)
        .rename_field("Health", "hp", "current")
        .add_field(
            "Health",
            "max",
            FieldType::Integer,
            FieldValue::Integer(100),
        );
    let v2: SystemFn = system_v2;

    // Warm up.
    for _ in 0..5 {
        let mut l = IterationLoop::new(scene(N), v1_schema.clone(), SwapHotPatch::new(system_v2));
        let _ = l.hot_iterate(v2, &migration, v2_schema.clone()).unwrap();
    }

    // The full loop: rebuild the fresh scene + hot-patch + replay-across-schema-change.
    let mut loop_us = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let t = Instant::now();
        let mut l = IterationLoop::new(scene(N), v1_schema.clone(), SwapHotPatch::new(system_v2));
        let result = l.hot_iterate(v2, &migration, v2_schema.clone()).unwrap();
        loop_us.push(t.elapsed().as_micros());
        assert_eq!(result.output, "sum=24750"); // sum of (i%100) for i in 0..500 = 5*4950
    }
    let (l50, l99) = percentiles(loop_us);

    // Just the op-replay-across-schema-change (the substrate advantage, isolated).
    let mut replay_us = Vec::with_capacity(RUNS);
    let scene = scene(N);
    for _ in 0..RUNS {
        let t = Instant::now();
        let _ = restore(&scene, &migration, &v2_schema).unwrap();
        replay_us.push(t.elapsed().as_micros());
    }
    let (r50, r99) = percentiles(replay_us);

    println!("::notice::hotpatch-loop-p50-us={l50}");
    println!("::notice::hotpatch-loop-p99-us={l99}");
    println!("::notice::hotpatch-opreplay-p50-us={r50}");
    println!("::notice::hotpatch-opreplay-p99-us={r99}");

    // Generous, order-of-magnitude budgets: the owned loop is comfortably sub-second (instant) over a
    // 500-entity scene. Min-spec is owed (this is the dev box).
    assert!(l99 < 100_000, "hot-iterate loop p99 {l99}us exceeded 100ms");
    assert!(r99 < 50_000, "op-replay p99 {r99}us exceeded 50ms");
}
