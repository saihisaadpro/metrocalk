//! M15.0 (ADR-070, Leg B) — the SDF-compile-to-mesh benchmark. Marching-Tetrahedra surface extraction is a
//! **discrete authoring op** (a commit-time bake), NOT per-frame work — it never touches the <16 ms render
//! hot path (inv. 4). This measures the one-shot compile cost of the canonical box−cylinder op at a coarse
//! "min-spec authoring" grid and a finer grid, so the ADR can quote a real number (not an estimate).
//!
//! Release-only (`--release`; run in CI by `release-budgets.yml`), warm-up + median + p99, ≥2-run
//! determinism re-checked under the bench too (`<benchmark_discipline>`).

#![cfg(not(debug_assertions))]

use metrocalk_sdf::{compile, validate, Axis, Grid, Sdf};
use std::time::Instant;

fn box_minus_cylinder() -> Sdf {
    Sdf::cuboid([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]).difference(Sdf::cylinder(
        [0.0, 0.0, 0.0],
        0.5,
        2.0,
        Axis::Y,
    ))
}

fn bench_grid(label: &str, res: usize, iters: usize) -> (f64, f64, usize) {
    let sdf = box_minus_cylinder();
    let grid = Grid::around(&sdf, res, 0.06);
    // warm up
    for _ in 0..3 {
        let _ = compile(&sdf, &grid);
    }
    let mut samples = Vec::with_capacity(iters);
    let mut tri = 0;
    for _ in 0..iters {
        let t = Instant::now();
        let mesh = compile(&sdf, &grid);
        samples.push(t.elapsed().as_secs_f64() * 1e3);
        tri = mesh.triangle_count();
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = samples[samples.len() / 2];
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    println!(
        "[sdf-compile] {label}: res={res} tris={tri} p50={p50:.3}ms p99={p99:.3}ms (n={iters})"
    );
    (p50, p99, tri)
}

#[test]
fn sdf_compile_to_mesh_is_a_fast_discrete_authoring_op() {
    println!("env: rustc-1.92.0 x86_64-pc-windows-msvc release");
    // A coarse "min-spec authoring" grid and a finer grid.
    let (_p50c, p99c, tric) = bench_grid("min-spec", 32, 60);
    let (_p50f, p99f, trif) = bench_grid("fine", 64, 30);

    assert!(
        tric > 100 && trif > tric,
        "both grids produce a real surface"
    );
    // Discrete authoring op (not per-frame): a coarse compile stays well under an interactive-author budget.
    // The bound is generous (it is off the hot path) but has teeth — a 10x regression fails CI.
    assert!(
        p99c < 250.0,
        "coarse SDF compile stays interactive for authoring: p99={p99c:.3}ms"
    );
    assert!(
        p99f < 2000.0,
        "fine SDF compile is a bounded bake: p99={p99f:.3}ms"
    );

    // Correct + deterministic under the bench too.
    let sdf = box_minus_cylinder();
    let grid = Grid::around(&sdf, 48, 0.06);
    let a = compile(&sdf, &grid);
    let b = compile(&sdf, &grid);
    assert!(validate(&a).watertight, "bench output is watertight");
    assert_eq!(
        a.content_hash(),
        b.content_hash(),
        "deterministic under the release bench"
    );
}
