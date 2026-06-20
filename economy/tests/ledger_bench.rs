//! Ledger-op latency (M7, **release**): a charge/reserve is a deterministic fold over the append-only
//! log, so it rides the interactive paid actions (buy/generate/edit). Measured on a realistic-size
//! ledger — a long session of prior entries — to confirm the fold stays far under a frame budget.
//!
//! Run with `cargo test -p metrocalk-economy --release --test ledger_bench -- --nocapture` (≥2 runs).
//! No clock/RNG inside the crate; the timing harness lives here, in the test.

#![allow(clippy::cast_precision_loss)]

use metrocalk_economy::{AccountId, Action, Ledger, Mtk, Reason};

/// A long session: one big grant + `N` edit transactions (each a debit + a platform accrual), so the
/// log is realistically large before we measure a fresh charge.
fn realistic_ledger(n: usize) -> Ledger {
    let mut l = Ledger::new();
    l.grant(
        AccountId::User,
        Mtk::from_tokens(10_000_000),
        Reason::FreeTier,
    );
    for i in 0..n {
        let _ = l.charge(&AccountId::User, &Action::Edit, &format!("seed-{i}"));
    }
    l
}

fn percentiles(mut us: Vec<f64>) -> (f64, f64) {
    us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (us[us.len() / 2], us[us.len() * 99 / 100])
}

#[test]
fn charge_latency_on_a_realistic_ledger() {
    let mut l = realistic_ledger(5_000); // ~10k entries (debit + accrue each)
    let start_len = l.len();

    // Warm up.
    for _ in 0..50 {
        let _ = l.available(&AccountId::User);
    }

    let mut times = Vec::with_capacity(200);
    for i in 0..200 {
        let t0 = std::time::Instant::now();
        let _ = l.charge(&AccountId::User, &Action::Edit, &format!("bench-{i}"));
        times.push(t0.elapsed().as_secs_f64() * 1e6);
    }
    let (p50, p99) = percentiles(times);
    eprintln!(
        "[M7] ledger CHARGE (debit+accrue, affordability fold) on a {start_len}-entry ledger: \
         p50={p50:.1}us p99={p99:.1}us"
    );
    assert!(
        p99 < 16_000.0,
        "a ledger charge (p99={p99:.1}us) must be far under the 16ms frame budget"
    );
}

#[test]
fn reserve_then_settle_latency_on_a_realistic_ledger() {
    let mut l = realistic_ledger(5_000);
    let start_len = l.len();
    for _ in 0..50 {
        let _ = l.available(&AccountId::User);
    }

    let mut times = Vec::with_capacity(200);
    for i in 0..200 {
        let t0 = std::time::Instant::now();
        let hold = l
            .reserve(&AccountId::User, &Action::Generate, &format!("g{i}"))
            .unwrap();
        l.settle(hold, &format!("g{i}"));
        times.push(t0.elapsed().as_secs_f64() * 1e6);
    }
    let (p50, p99) = percentiles(times);
    eprintln!(
        "[M7] ledger RESERVE+SETTLE (generation round) on a {start_len}-entry ledger: \
         p50={p50:.1}us p99={p99:.1}us"
    );
    assert!(
        p99 < 16_000.0,
        "reserve+settle (p99={p99:.1}us) must be far under the 16ms frame budget"
    );
}
