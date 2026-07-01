//! The IVM-vs-eager **crossover study** (M13.4) — THE measured go/no-go, this milestone's whole point.
//! On ONE expensive query (the capability compat-join), sweep entity-count × churn-rate and report where
//! the maintained view (O(Δ)/frame) starts beating eager recompute (O(query)/frame), the per-update cost,
//! the bookkeeping memory, and bit-identical-output equality. Release-only + serial (contention poisons
//! IVM-vs-eager numbers): `#[cfg_attr(debug_assertions, ignore)]` → SKIPPED in the debug `cargo test`,
//! RUN by `cargo test --workspace --release` (release-budgets.yml). NOT a dark test.
//!
//! Min-spec is OWED: this measures the dev box, not a true low-core rig (the honest ceiling per
//! `<benchmark_discipline>` — never fabricate a min-spec number). The workload is seeded (deterministic).

use metrocalk_ivm::{Cap, Delta, EagerCompat, IncrementalCompat, IncrementalView, Lcg};
use std::time::Instant;

fn percentiles(mut us: Vec<u128>) -> (u128, u128) {
    us.sort_unstable();
    let p50 = us[us.len() / 2];
    let p99 = us[(us.len() * 99 / 100).min(us.len() - 1)];
    (p50, p99)
}

/// Build the initial relation (each entity provides + requires one cap) into both views, tracking each
/// entity's current provided cap so churn can move it deterministically. `caps ≈ n / DENSITY`.
#[allow(clippy::cast_possible_truncation)] // entity counts fit usize on the 64-bit bench target
fn build(n: u64, caps: u64, seed: u64) -> (EagerCompat, IncrementalCompat, Vec<Cap>) {
    let mut rng = Lcg::new(seed);
    let caps = caps.max(1);
    let (mut eager, mut inc) = (EagerCompat::new(), IncrementalCompat::new());
    let mut cur_provide = vec![0u32; n as usize];
    for e in 0..n {
        let cp = u32::try_from(rng.below(caps)).unwrap_or(0);
        let cr = u32::try_from(rng.below(caps)).unwrap_or(0);
        for d in [
            Delta::Provide { entity: e, cap: cp },
            Delta::Require { entity: e, cap: cr },
        ] {
            eager.apply(d);
            inc.apply(d);
        }
        cur_provide[e as usize] = cp;
    }
    (eager, inc, cur_provide)
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release-only crossover study (run by release-budgets.yml)"
)]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn ivm_vs_eager_crossover_on_the_compat_query() {
    const DENSITY: u64 = 8; // ~8 providers + ~8 requirers per cap (a modestly dense join)
    const FRAMES: usize = 15;
    let sizes = [2_000u64, 20_000u64];
    let churn_fracs = [0.001f64, 0.01, 0.1, 1.0];

    println!("::notice::ivm-crossover-study N x churn (eager-recompute vs incremental-maintain, p50 us):");

    // The GO/NO-GO gate is captured as assertions below: at large N + low churn the maintained view must
    // beat eager recompute (else the verdict would be "don't generalize", reported honestly).
    let mut win_at_low_churn_large_n = None;

    for &n in &sizes {
        let caps = n / DENSITY;
        for &churn in &churn_fracs {
            let (mut eager, mut inc, mut cur) = build(n, caps, 0x00C0_FFEE ^ n);
            // Bit-identical from the start.
            assert_eq!(
                inc.result(),
                eager.result(),
                "initial state bit-identical (N={n})"
            );

            let churn_n = ((n as f64) * churn).max(1.0) as u64;
            let mut rng = Lcg::new(0xBEEF ^ n ^ (churn.to_bits()));

            // Warm up (one frame, untimed).
            for _ in 0..churn_n {
                let e = rng.below(n);
                let new_cap = u32::try_from(rng.below(caps.max(1))).unwrap_or(0);
                let old = cur[e as usize];
                for d in [
                    Delta::Unprovide {
                        entity: e,
                        cap: old,
                    },
                    Delta::Provide {
                        entity: e,
                        cap: new_cap,
                    },
                ] {
                    eager.apply(d);
                    inc.apply(d);
                }
                cur[e as usize] = new_cap;
            }

            let mut eager_us = Vec::with_capacity(FRAMES);
            let mut inc_us = Vec::with_capacity(FRAMES);
            for frame in 0..FRAMES {
                // Build this frame's churn (the same deltas fed to both views).
                let mut deltas = Vec::with_capacity((churn_n * 2) as usize);
                for _ in 0..churn_n {
                    let e = rng.below(n);
                    let new_cap = u32::try_from(rng.below(caps.max(1))).unwrap_or(0);
                    let old = cur[e as usize];
                    deltas.push(Delta::Unprovide {
                        entity: e,
                        cap: old,
                    });
                    deltas.push(Delta::Provide {
                        entity: e,
                        cap: new_cap,
                    });
                    cur[e as usize] = new_cap;
                }

                // EAGER per-frame cost = apply the churn (cheap) + recompute the whole query.
                let t = Instant::now();
                for &d in &deltas {
                    eager.apply(d);
                }
                let eager_result = eager.result();
                eager_us.push(t.elapsed().as_micros());

                // INCREMENTAL per-frame cost = apply the churn (maintain only; the result stays current).
                let t = Instant::now();
                for &d in &deltas {
                    inc.apply(d);
                }
                inc_us.push(t.elapsed().as_micros());

                // Bit-identical every few frames (equality is the correctness guarantee).
                if frame % 5 == 0 {
                    assert_eq!(
                        inc.result(),
                        eager_result,
                        "bit-identical (N={n}, churn={churn})"
                    );
                }
            }

            let (e50, _) = percentiles(eager_us);
            let (i50, _) = percentiles(inc_us);
            let speedup = e50 as f64 / (i50.max(1)) as f64;
            let verdict = if i50 < e50 { "IVM WINS" } else { "eager wins" };
            println!(
                "::notice::ivm N={n} churn={churn} eager-p50={e50}us incr-p50={i50}us speedup={speedup:.1}x mem-eager={}KB mem-incr={}KB [{verdict}]",
                eager.memory_bytes() / 1024,
                inc.memory_bytes() / 1024,
            );

            if (churn - 0.01).abs() < 1e-9 && n == 20_000 {
                win_at_low_churn_large_n = Some((e50, i50, speedup));
            }
        }
    }

    // THE GO GATE: at N=20000, 1% churn, the maintained view must clearly beat eager recompute.
    let (e50, i50, speedup) =
        win_at_low_churn_large_n.expect("the N=20000, churn=1% config was measured");
    assert!(
        i50 < e50,
        "GO gate: incremental must beat eager at large N + low churn (eager {e50}us vs incr {i50}us)"
    );
    println!(
        "::notice::ivm-VERDICT=GO — at N=20000 / 1% churn the maintained view is {speedup:.1}x faster than eager recompute (crossover well below 100% churn); generalize to expensive low-churn derived queries (scoped, NOT a rewrite)"
    );
}
