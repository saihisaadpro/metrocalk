//! M12.3 (ADR-047) — WASM plugin call overhead (**release**). A plugin runs on the engine/worker thread,
//! OFF the per-frame JS hot path (invariant 4) — it's a discrete action (like authoring a rule), never a
//! frame callback — so its cost rides the interactive budget, not the 16 ms frame. We measure the one-shot
//! **load** (instantiate the sandbox) + the steady-state **call** so a regression is caught. Release-only
//! (benchmark discipline: `--release` for timing; in CI's DEBUG `cargo test --workspace` it is `#[ignore]`d,
//! but still COMPILES + is collected — not a dark test — and runs in the `release-budgets` job).
//!
//! NOTE (benchmark-discipline, product-principle 3): this box is an i9-13900H (high-end), so these are the
//! UPPER-BOUND numbers; the entry-level / min-spec profile is **owed** (no min-spec rig). wasmtime JITs the
//! module, so `load` (compile + instantiate) dominates; a steady `call` reuses the instance.

use std::time::Instant;

use metrocalk_plugins::{ExtismHost, PluginHost, PluginInstance, Sandbox};

const ARRANGE_WASM: &[u8] = include_bytes!("fixtures/arrange.wasm");

fn percentiles(mut us: Vec<f64>) -> (f64, f64) {
    us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (us[us.len() / 2], us[us.len() * 99 / 100])
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release-only timing measurement")]
#[allow(clippy::cast_precision_loss)]
fn plugin_load_and_call_overhead() {
    let host = ExtismHost::new();
    let sandbox = Sandbox::restrictive();
    let input =
        br#"{"ids":["1_0","1_1","1_2","1_3","1_4","1_5","1_6","1_7"],"seed":3,"spacing":2.0}"#;

    // warm up (JIT caches, allocator).
    for _ in 0..20 {
        let mut p = host.load(ARRANGE_WASM, &sandbox).unwrap();
        let _ = p.call("arrange", input).unwrap();
    }

    // One-shot LOAD: compile + instantiate the sandboxed module (the heavy, JIT-bound step).
    let mut load = Vec::with_capacity(200);
    for _ in 0..200 {
        let t0 = Instant::now();
        let p = host.load(ARRANGE_WASM, &sandbox).unwrap();
        load.push(t0.elapsed().as_secs_f64() * 1e6);
        drop(p);
    }

    // Steady CALL: reuse one instance, call repeatedly (the per-invocation cost in a hot loop).
    let mut p = host.load(ARRANGE_WASM, &sandbox).unwrap();
    let mut call = Vec::with_capacity(2000);
    for _ in 0..2000 {
        let t0 = Instant::now();
        let out = p.call("arrange", input).unwrap();
        call.push(t0.elapsed().as_secs_f64() * 1e6);
        assert!(!out.is_empty());
    }

    let (lp50, lp99) = percentiles(load);
    let (cp50, cp99) = percentiles(call);
    eprintln!(
        "[M12.3] plugin load (compile + instantiate sandbox): p50={lp50:.1}us p99={lp99:.1}us"
    );
    eprintln!("[M12.3] plugin call (steady, 8-entity arrange): p50={cp50:.2}us p99={cp99:.2}us");
    // A plugin is a discrete action off the per-frame path; a steady call must stay well under the 16 ms
    // interaction budget (load is one-shot when a plugin is first used).
    assert!(
        cp99 < 16_000.0,
        "steady plugin call p99={cp99:.1}us must be << 16ms"
    );
    assert!(
        lp99 < 100_000.0,
        "plugin load p99={lp99:.1}us stays bounded (one-shot)"
    );
}
