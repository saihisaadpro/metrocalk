//! M1.5 — the compatibility-query performance gate (north-star test #1: an edit must re-answer
//! "what's compatible now?" within one 60 Hz frame, <16 ms). Runs the M1.2 wrapper's *cached*
//! compat query on M1.4's shared 5k preset and exits non-zero if p99 blows the budget, so an
//! order-of-magnitude regression (an accidental uncached path, an archetype-fragmentation blowup)
//! fails CI — the same tripwire discipline spike ③ gave the wasm build.
//!
//! Reuses M1.4's fixture (`scene::build_scene` / `preset_5k`) — not a bespoke scene — through the
//! [`World`] trait only (no raw Flecs). Release-only (debug timings are meaningless). The runner
//! calibration + margin rationale lives in `tools/query-gate/README.md`.
//!
//! Env knobs (CI normally sets none):
//!   `METROCALK_GATE_BUDGET_US`      — override the 16 ms budget, in µs (calibration / tightening)
//!   `METROCALK_GATE_INJECT_SLOW_US` — per-sample busy-spin in µs: the deliberate-regression hook
//!                                     used to prove the gate goes red (see the README recipe)

use metrocalk_ecs::scene::{build_scene, compat_clauses, SceneParams};
use metrocalk_ecs::{FlecsWorld, World};
use std::time::{Duration, Instant};

/// One 60 Hz frame — north-star test #1, the product's core interactivity promise.
const BUDGET_US: f64 = 16_000.0;
const WARMUP: usize = 200;
const SAMPLES: usize = 2000;

fn env_f64(key: &str) -> Option<f64> {
    std::env::var(key).ok().and_then(|s| s.trim().parse().ok())
}

/// Busy-spin for `d` — the deliberate-regression hook (a `sleep` would yield the core and measure
/// scheduling, not work; spinning charges wall-clock to this sample like a real slow query would).
fn spin(d: Duration) {
    let t = Instant::now();
    while t.elapsed() < d {
        std::hint::spin_loop();
    }
}

fn main() {
    let budget = env_f64("METROCALK_GATE_BUDGET_US").unwrap_or(BUDGET_US);
    let inject = env_f64("METROCALK_GATE_INJECT_SLOW_US")
        .unwrap_or(0.0)
        .max(0.0);
    let inject_dur = Duration::from_micros(inject as u64);

    // M1.4's shared fixture, built through the wrapper (not raw Flecs).
    let mut w = FlecsWorld::new();
    let scene = build_scene(&mut w, &SceneParams::preset_5k(), false);
    let q = w.build_query(&compat_clauses(&scene));

    // Integrity: the cached query must still return the spike's ground truth, else we're timing
    // nonsense — a gate that passes on a broken query is worse than no gate.
    let matched = w.matches(&q).len();
    assert_eq!(
        matched, scene.expected_compat,
        "gate fixture integrity: compat query returned {matched}, expected {}",
        scene.expected_compat
    );

    for _ in 0..WARMUP {
        let mut c = 0usize;
        w.for_each_match(&q, &mut |_| c += 1);
        std::hint::black_box(c);
    }

    let mut us = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let t = Instant::now();
        let mut c = 0usize;
        w.for_each_match(&q, &mut |_| c += 1);
        std::hint::black_box(c);
        if inject > 0.0 {
            spin(inject_dur); // deliberate-regression hook (red proof)
        }
        us.push(t.elapsed().as_secs_f64() * 1e6);
    }
    us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = us[SAMPLES / 2];
    let p99 = us[((SAMPLES as f64 * 0.99).ceil() as usize).min(SAMPLES) - 1];
    let max = us[SAMPLES - 1];

    println!("compat-query perf gate — 5k preset, cached query through the World wrapper");
    println!("  samples   = {SAMPLES} (after {WARMUP} warm-up)");
    println!(
        "  matched   = {matched} (expected {})",
        scene.expected_compat
    );
    println!("  median    = {median:.2} µs");
    println!("  p99       = {p99:.2} µs");
    println!("  max       = {max:.2} µs");
    println!("  budget    = {budget:.0} µs  (north-star test #1: one 60 Hz frame)");
    println!("  headroom  = {:.0}x under budget at p99", budget / p99);
    if inject > 0.0 {
        println!(
            "  NOTE: METROCALK_GATE_INJECT_SLOW_US={inject:.0} — deliberate regression injected"
        );
    }

    if p99 > budget {
        eprintln!(
            "::error::compat-query p99 {p99:.2} us EXCEEDS the {budget:.0} us budget — north-star test #1 (<16 ms) regressed"
        );
        std::process::exit(1);
    }
    println!("GATE PASS — p99 {p99:.2} us within the {budget:.0} us budget");
}
