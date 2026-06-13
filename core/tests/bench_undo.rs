//! Undo latency benchmark: must be < 5 ms p99.
//! Runs twice per orchestrator discipline (two independent measurements).

// Benchmark stats: loop-counter and percentile-index casts where precision/wrap is irrelevant.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

use metrocalk_core::{Engine, FieldValue, Op};
use metrocalk_ecs::FlecsWorld;
use std::time::Instant;

fn engine() -> Engine<FlecsWorld> {
    Engine::new(FlecsWorld::new(), 1)
}

fn measure_undo_latencies(label: &str) -> (f64, f64) {
    let mut e = engine();
    let mut entities = Vec::new();

    // Build up a scene with 200 entities, each with 3 component fields
    for _ in 0..200 {
        let id = e.alloc_entity_id();
        e.commit(
            "create",
            vec![
                Op::CreateEntity { id, parent: None },
                Op::SetField {
                    entity: id,
                    component: "Health".into(),
                    field: "hp".into(),
                    value: FieldValue::Integer(100),
                },
                Op::SetField {
                    entity: id,
                    component: "Transform".into(),
                    field: "px".into(),
                    value: FieldValue::Number(1.0),
                },
                Op::SetField {
                    entity: id,
                    component: "Tag".into(),
                    field: "name".into(),
                    value: FieldValue::Str("entity".into()),
                },
            ],
        )
        .unwrap();
        entities.push(id);
    }

    // Now do 100 additional SetField commits (one per commit)
    for (i, id) in entities.iter().take(100).enumerate() {
        e.commit(
            "update",
            vec![Op::SetField {
                entity: *id,
                component: "Health".into(),
                field: "hp".into(),
                value: FieldValue::Integer(i as i64 * 10),
            }],
        )
        .unwrap();
    }

    // Warm up: undo + redo once
    e.undo();
    e.redo();

    // Measure: undo the 100 SetField commits
    let mut latencies = Vec::with_capacity(100);
    for _ in 0..100 {
        let start = Instant::now();
        assert!(e.undo());
        let elapsed = start.elapsed();
        latencies.push(elapsed.as_secs_f64() * 1000.0); // ms
    }

    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = latencies[latencies.len() / 2];
    let p99 = latencies[(latencies.len() as f64 * 0.99) as usize];

    eprintln!("[{label}] undo latency: median={median:.3}ms  p99={p99:.3}ms  (n=100)");
    (median, p99)
}

#[test]
fn undo_latency_run_1() {
    let (median, p99) = measure_undo_latencies("run-1");
    assert!(
        p99 < 5.0,
        "run-1: undo p99 = {p99:.3}ms exceeds 5ms target"
    );
    assert!(
        median < 2.0,
        "run-1: undo median = {median:.3}ms unexpectedly high"
    );
}

#[test]
fn undo_latency_run_2() {
    let (median, p99) = measure_undo_latencies("run-2");
    assert!(
        p99 < 5.0,
        "run-2: undo p99 = {p99:.3}ms exceeds 5ms target"
    );
    assert!(
        median < 2.0,
        "run-2: undo median = {median:.3}ms unexpectedly high"
    );
}

#[test]
fn undo_delete_entity_latency() {
    let mut e = engine();

    // Create 50 entities with components
    let mut ids = Vec::new();
    for _ in 0..50 {
        let id = e.alloc_entity_id();
        e.commit(
            "create",
            vec![
                Op::CreateEntity { id, parent: None },
                Op::SetField {
                    entity: id,
                    component: "Health".into(),
                    field: "hp".into(),
                    value: FieldValue::Integer(100),
                },
            ],
        )
        .unwrap();
        ids.push(id);
    }

    // Delete them one by one (each a separate commit)
    for id in &ids {
        e.commit("delete", vec![Op::DeleteEntity { id: *id }])
            .unwrap();
    }

    // Measure undo of delete (entity resurrection)
    let mut latencies = Vec::with_capacity(50);
    for _ in 0..50 {
        let start = Instant::now();
        assert!(e.undo());
        let elapsed = start.elapsed();
        latencies.push(elapsed.as_secs_f64() * 1000.0);
    }

    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = latencies[latencies.len() / 2];
    let p99 = latencies[(latencies.len() as f64 * 0.99) as usize];

    eprintln!("[resurrection] undo latency: median={median:.3}ms  p99={p99:.3}ms  (n=50)");
    assert!(
        p99 < 5.0,
        "resurrection undo p99 = {p99:.3}ms exceeds 5ms target"
    );
}
