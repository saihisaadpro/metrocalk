//! Run the M8.1 physics determinism spike + print the report; the active precision is the build feature.
//!   native f64:  cargo run --release
//!   native f32:  cargo run --release --no-default-features --features f32
//!   wasm32:      cargo build --release --target wasm32-wasip1 [--features ...] ; run under wasmtime/node
//! Prints the provenance envelope + the P1 final hash + the P2 replay + the P3 snapshot results to
//! stdout, so a CI job (Linux) / wasmtime (wasm) / native run all surface the same quoted hashes.

use metrocalk_physics_spike::harness::{
    precision_sizes, replay_check, run, snapshot_check, BroadPhaseMode,
};
use metrocalk_physics_spike::PRECISION;

fn main() {
    let (real_bytes, vec_bytes) = precision_sizes();
    eprintln!(
        "[M8.1] physics determinism spike — precision={PRECISION}, enhanced-determinism ON, simd/parallel OFF"
    );
    println!("PRECISION_PROBE[{PRECISION}]: size_of(Real)={real_bytes} size_of(Vector)={vec_bytes} (f64→8/24, f32→4/12)");

    // P1 — the determinism run + the bake/replay provenance envelope (deliverable 6).
    let prov = run(BroadPhaseMode::Default);
    println!("=== PROVENANCE ENVELOPE ===");
    println!(
        "{}",
        serde_json::to_string_pretty(&prov).expect("serialize provenance")
    );
    println!("FINAL_WORLD_HASH[{PRECISION}] = {}", prov.final_world_hash);
    if let (Some(p50), Some(p99)) = (prov.step_us_p50, prov.step_us_p99) {
        println!(
            "STEP_TIME[{PRECISION}]: p50={p50:.2}us p99={p99:.2}us over {} steps ({} bodies)",
            prov.steps, prov.body_count
        );
    }

    // P2 — input-replay end-hash equality (reset + re-feed + re-step == original).
    let (a, b) = replay_check();
    println!("REPLAY[{PRECISION}]: original={a} replayed={b} equal={}", a == b);

    // P3 — snapshot/restore in BOTH broad-phase modes (default expected to diverge per #910; `None` is
    // the determinism-preserving mitigation).
    let (ref_def, restored_def) = snapshot_check(BroadPhaseMode::Default);
    println!(
        "SNAPSHOT[{PRECISION}/default]: ref={ref_def} restored={restored_def} equal={}",
        ref_def == restored_def
    );
    let (ref_none, restored_none) = snapshot_check(BroadPhaseMode::NoneStrategy);
    println!(
        "SNAPSHOT[{PRECISION}/None]: ref={ref_none} restored={restored_none} equal={}",
        ref_none == restored_none
    );
}
