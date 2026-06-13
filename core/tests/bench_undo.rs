//! Undo latency benchmark: latest-op undo must be < 5 ms p99 (north-star: interactive Ctrl-Z).
//! Runs twice per orchestrator discipline (two independent measurements).
//!
//! Sample size is deliberately large (`SAMPLES` = 500). `cargo test` runs test binaries in
//! parallel, so an undo bench shares the box with the rest of the suite; with a small n the "p99"
//! index collapses onto the single worst sample and a lone OS hiccup flakes the gate. At n=500 the
//! p99 index (≈495) excludes the top ~5 outliers, so the assertion tracks the true tail — a real
//! regression moves the *median* well before it could move a robust p99, and is caught regardless.

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

const SAMPLES: usize = 500;

fn engine() -> Engine<FlecsWorld> {
    Engine::new(FlecsWorld::new(), 1)
}

/// (median, p99) in milliseconds from a latency sample (sorts in place).
fn percentiles(latencies: &mut [f64]) -> (f64, f64) {
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = latencies[latencies.len() / 2];
    let p99 = latencies[(latencies.len() as f64 * 0.99) as usize];
    (median, p99)
}

fn measure_setfield_undo(label: &str) -> (f64, f64) {
    let mut e = engine();
    let mut entities = Vec::new();

    // Build a scene of SAMPLES entities, each with 3 component fields.
    for _ in 0..SAMPLES {
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

    // One additional SetField commit per entity — these are what we undo.
    for (i, id) in entities.iter().enumerate() {
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

    // Warm up: undo + redo once.
    e.undo();
    e.redo();

    let mut latencies = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        assert!(e.undo());
        latencies.push(start.elapsed().as_secs_f64() * 1000.0); // ms
    }

    let (median, p99) = percentiles(&mut latencies);
    eprintln!("[{label}] SetField undo: median={median:.3}ms  p99={p99:.3}ms  (n={SAMPLES})");
    (median, p99)
}

#[test]
fn undo_latency_run_1() {
    let (median, p99) = measure_setfield_undo("run-1");
    assert!(p99 < 5.0, "run-1: undo p99 = {p99:.3}ms exceeds 5ms target");
    assert!(
        median < 2.0,
        "run-1: undo median = {median:.3}ms unexpectedly high"
    );
}

#[test]
fn undo_latency_run_2() {
    let (median, p99) = measure_setfield_undo("run-2");
    assert!(p99 < 5.0, "run-2: undo p99 = {p99:.3}ms exceeds 5ms target");
    assert!(
        median < 2.0,
        "run-2: undo median = {median:.3}ms unexpectedly high"
    );
}

#[test]
fn undo_delete_entity_latency() {
    let mut e = engine();

    // Create SAMPLES entities with components, then delete each in its own commit.
    let mut ids = Vec::new();
    for _ in 0..SAMPLES {
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
    for id in &ids {
        e.commit("delete", vec![Op::DeleteEntity { id: *id }])
            .unwrap();
    }

    // Warm up, then measure undo of delete (entity resurrection — the heaviest undo).
    e.undo();
    e.redo();

    let mut latencies = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        assert!(e.undo());
        latencies.push(start.elapsed().as_secs_f64() * 1000.0);
    }

    let (median, p99) = percentiles(&mut latencies);
    eprintln!("[resurrection] undo: median={median:.3}ms  p99={p99:.3}ms  (n={SAMPLES})");
    assert!(
        p99 < 5.0,
        "resurrection undo p99 = {p99:.3}ms exceeds 5ms target"
    );
    assert!(
        median < 2.0,
        "resurrection undo median = {median:.3}ms unexpectedly high"
    );
}
