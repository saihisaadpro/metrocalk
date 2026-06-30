//! Adversarial property test for the robust-CSG tier (M13.2). Hammers the boolean with many
//! deterministically-randomised box configurations and asserts the load-bearing invariants:
//!   1. **It never panics** (no index-out-of-bounds, no unwrap, no infinite recursion/overflow) — a
//!      degenerate input is `Ok(clean)` or `Err(explained)`, never a crash.
//!   2. **`Ok` ⟹ watertight + oriented + clean** — the always-on validator gates every output (the
//!      crack-free guarantee is enforced, not hoped).
//!   3. **Deterministic** — the same configuration produces the bit-identical content hash on a re-run.
//!
//! Randomness is a seeded integer LCG (no `rand`, no wall clock) so the suite is reproducible across
//! machines and CI (`<benchmark_discipline>` determinism rule).

use metrocalk_csg::{box_mesh, validate, BoolOp, Csg, ExactBspCsg};

/// A tiny deterministic LCG (Numerical Recipes constants) → reproducible pseudo-random configs.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }
    /// A float in `[lo, hi)`, quantised to a coarse grid so configs hit coplanar / shared-edge / tangent
    /// cases often (the crack-makers) rather than always-generic positions.
    #[allow(clippy::cast_precision_loss)] // a 0..=16 grid index; precision loss is irrelevant
    fn coord(&mut self, lo: f64, hi: f64) -> f64 {
        let steps = 16u64;
        let k = self.next() % (steps + 1);
        lo + (hi - lo) * (k as f64) / (steps as f64)
    }
}

#[test]
fn fuzz_no_panic_ok_implies_clean_and_deterministic() {
    const N: usize = 400;
    let csg = ExactBspCsg::new();
    let wall = box_mesh([0.0, 0.0, 0.0], [2.0, 1.0, 0.5]);
    let ops = [
        BoolOp::Difference,
        BoolOp::Union,
        BoolOp::Intersection,
        BoolOp::Xor,
    ];

    let mut rng = Lcg(0x5125_4341_4C43_4347); // "MTKCALCG"-ish, fixed seed
    let mut ok = 0usize;
    let mut blocked = 0usize;

    for i in 0..N {
        // A box whose coordinates land on a coarse grid shared with the wall → frequent coplanar faces,
        // shared edges, tangencies, and full-miss / full-contain degeneracies.
        let center = [
            rng.coord(-2.5, 2.5),
            rng.coord(-1.5, 1.5),
            rng.coord(-1.0, 1.0),
        ];
        let half = [
            rng.coord(0.25, 1.5),
            rng.coord(0.25, 1.5),
            rng.coord(0.25, 1.5),
        ];
        let b = box_mesh(center, half);
        let op = ops[i % ops.len()];

        // (1) Never panics — the call returns either way.
        match csg.boolean(&wall, &b, op) {
            Ok(out) => {
                ok += 1;
                // (2) Ok ⟹ the validator says clean (watertight + oriented + no NaN/sliver).
                let r = validate(&out);
                assert!(
                    r.is_clean(),
                    "config {i} ({op:?}) returned Ok but is NOT clean: {}",
                    r.explain()
                );
                // (3) Deterministic — same config, same content hash.
                let again = csg.boolean(&wall, &b, op).expect("re-run also Ok");
                assert_eq!(
                    out.content_hash(),
                    again.content_hash(),
                    "config {i} ({op:?}) non-deterministic"
                );
            }
            Err(_) => blocked += 1, // a degenerate case, explained — acceptable
        }
    }

    // A sanity floor: the grid is chosen so MOST configs are clean (a real boolean, not all-blocked). If the
    // robustness regressed to mostly-blocked this would catch it.
    assert!(
        ok * 2 >= N,
        "only {ok}/{N} configs produced a clean solid (too many blocked: {blocked})"
    );
    println!("::notice::csg-fuzz-clean={ok}/{N} (blocked={blocked})");
}
