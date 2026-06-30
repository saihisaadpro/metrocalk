//! Release-only timing budget for the robust CSG tier (M13.2). A boolean is a discrete, off-hot-path
//! authoring op (inv. 4 untouched), so the budget is generous (≪ a frame is not required — but we assert
//! it holds well under interactive expectations). Marked `#[cfg_attr(debug_assertions, ignore)]` so it is
//! SKIPPED in the debug `cargo test` (ci.yml) and RUN by `cargo test --workspace --release`
//! (release-budgets.yml) — NOT a dark test.
//!
//! Min-spec is OWED: this measures the dev box, not a true low-core rig (the honest ceiling, per
//! `<benchmark_discipline>` — never fabricate a min-spec number).

use metrocalk_csg::{box_mesh, validate, Csg, ExactBspCsg};
use std::time::Instant;

fn percentiles(mut us: Vec<u128>) -> (u128, u128) {
    us.sort_unstable();
    let p50 = us[us.len() / 2];
    let p99 = us[(us.len() * 99 / 100).min(us.len() - 1)];
    (p50, p99)
}

/// A representative single carve (the destructible-wall op) + a heavier accumulated carve, timed in
/// release. Asserts a generous budget and prints the numbers (surfaced in the release-budgets log).
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release-only timing budget (run by release-budgets.yml)"
)]
fn csg_boolean_holds_an_interactive_budget() {
    const N: usize = 200;
    let csg = ExactBspCsg::new();
    let wall = box_mesh([0.0, 0.0, 0.0], [3.0, 1.5, 0.5]);
    let carve = box_mesh([0.0, 0.5, 0.0], [0.5, 0.5, 1.0]);

    // Warm up.
    for _ in 0..5 {
        let _ = csg.difference(&wall, &carve).unwrap();
    }

    let mut single = Vec::with_capacity(N);
    for _ in 0..N {
        let t = Instant::now();
        let out = csg.difference(&wall, &carve).unwrap();
        single.push(t.elapsed().as_micros());
        assert!(validate(&out).is_clean());
    }
    let (s50, s99) = percentiles(single);

    // A heavier case: the destructible wall progressively carved by 5 varied boxes (pockets + a
    // through-hole + an end-shave) — the accumulating workload, kept to the cases the buildable tier
    // handles cleanly (deep coplanar accumulation is the exact-arrangement tail, named in ADR-051).
    let carves = [
        box_mesh([-2.2, 0.5, 0.0], [0.4, 0.4, 1.0]),
        box_mesh([-0.9, -0.4, 0.0], [0.3, 0.5, 1.0]),
        box_mesh([0.3, 0.2, 0.0], [0.3, 0.3, 2.0]),
        box_mesh([1.6, 0.4, 0.0], [0.4, 0.4, 1.0]),
        box_mesh([3.0, 0.0, 0.0], [0.6, 1.5, 1.0]),
    ];
    let mut accum = Vec::with_capacity(50);
    for _ in 0..50 {
        let t = Instant::now();
        let mut w = wall.clone();
        for c in &carves {
            w = csg.difference(&w, c).unwrap();
        }
        accum.push(t.elapsed().as_micros());
        assert!(validate(&w).watertight);
    }
    let (a50, a99) = percentiles(accum);

    println!("::notice::csg-single-carve-p50-us={s50}");
    println!("::notice::csg-single-carve-p99-us={s99}");
    println!("::notice::csg-five-carve-chain-p50-us={a50}");
    println!("::notice::csg-five-carve-chain-p99-us={a99}");

    // Generous budgets (a discrete authoring op): a single carve well under a frame, a five-carve chain
    // under a few frames. Min-spec is owed (this is the dev box).
    assert!(s99 < 16_000, "single-carve p99 {s99}us exceeded 16ms");
    assert!(a99 < 64_000, "five-carve-chain p99 {a99}us exceeded 64ms");
}
