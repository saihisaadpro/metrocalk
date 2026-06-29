//! M12.5 (ADR-049) Rules-in-Play tick + truth-state projection latency (**release**). The Rules tick runs
//! **every Play frame** (alongside the M8 sim), so @N rules it must hold the <16 ms frame budget; the
//! truth-state read is the click-time **debug projection** (off the per-frame path) and the decision-history
//! scrub is the time-travel cost — all measured here so a regression can't ship silently. Release-only
//! (benchmark discipline: `--release` for timing; CI's DEBUG `cargo test` ignores it, the release-budgets
//! job runs it).
//!
//! NOTE (benchmark-discipline, product-principle 3): this box is an i9-13900H (high-end), so these are the
//! UPPER-BOUND numbers this hardware gives — the entry-level / **min-spec** profile is **owed** (no min-spec
//! rig available; the budget holds here by orders of magnitude, so the margin is large).

#![allow(clippy::cast_precision_loss)]

use std::time::Instant;

use metrocalk_core::rule_runtime::{RuleRecording, RuleReplay, RuntimeState};
use metrocalk_core::rules::{Action, CompareOp, Condition, RuleData, RuleId};
use metrocalk_core::FieldValue;

fn percentiles(mut us: Vec<f64>) -> (f64, f64) {
    us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (us[us.len() / 2], us[us.len() * 99 / 100])
}

const SWORD: &str = "1_0";

/// A test-#5-shaped rule (count threshold + zone gate -> ignite), referencing the sword.
fn ignite_rule(i: usize) -> (RuleId, RuleData) {
    (
        RuleId::new(format!("r_{i:06}")),
        RuleData {
            name: format!("rule {i}"),
            enabled: true,
            event: "EnemyDied".into(),
            conditions: vec![
                Condition {
                    entity: SWORD.into(),
                    component: "KillCounter".into(),
                    field: "count".into(),
                    op: CompareOp::Ge,
                    value: FieldValue::Integer(4),
                },
                Condition {
                    entity: SWORD.into(),
                    component: "Zone".into(),
                    field: "current".into(),
                    op: CompareOp::Eq,
                    value: FieldValue::Str("BossArena".into()),
                },
            ],
            actions: vec![Action {
                action: "AdjustCounter".into(),
                entity: SWORD.into(),
                component: "KillCounter".into(),
                field: "count".into(),
                value: FieldValue::Integer(1),
            }],
        },
    )
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release-only timing measurement")]
fn the_rules_tick_and_truth_state_hold_the_frame_budget() {
    const N: usize = 2000; // authored rules evaluated every tick
    const FRAMES: u64 = 600; // ~10 s of Play at 60 Hz

    let mut initial = RuntimeState::new();
    initial.set(SWORD, "KillCounter", "count", FieldValue::Integer(0));
    initial.set(
        SWORD,
        "Zone",
        "current",
        FieldValue::Str("BossArena".into()),
    );
    let rules: Vec<_> = (0..N).map(ignite_rule).collect();
    let mut rec = RuleRecording::new(initial, rules, vec![]);
    for f in 0..FRAMES {
        rec.add_event(f, "EnemyDied", None);
    }

    let mut cur = RuleReplay::new(rec.clone());

    // Warm up.
    for _ in 0..30 {
        cur.advance();
    }

    // Per-tick cost @N rules (the per-frame Play budget — every rule's When/If evaluated each tick).
    cur = RuleReplay::new(rec.clone());
    let mut tick = Vec::new();
    for _ in 0..FRAMES {
        let t0 = Instant::now();
        cur.advance();
        tick.push(t0.elapsed().as_secs_f64() * 1e6);
    }

    // Truth-state cost (the click-time debug projection — off the per-frame path).
    let mut debug = Vec::with_capacity(2000);
    for _ in 0..2000 {
        let t0 = Instant::now();
        let _ = cur.truth_state(SWORD);
        debug.push(t0.elapsed().as_secs_f64() * 1e6);
    }

    // A full decision-history scrub (rewind-rebuild + replay-forward over the whole timeline).
    let t0 = Instant::now();
    cur.seek(0);
    cur.seek(FRAMES);
    let scrub_ms = t0.elapsed().as_secs_f64() * 1e3;

    let (tp50, tp99) = percentiles(tick);
    let (dp50, dp99) = percentiles(debug);
    eprintln!("[M12.5] rules tick @{N} rules: p50={tp50:.2}us p99={tp99:.2}us (budget 16ms)");
    eprintln!("[M12.5] truth_state projection: p50={dp50:.2}us p99={dp99:.2}us");
    eprintln!("[M12.5] full {FRAMES}-frame decision-history scrub: {scrub_ms:.2}ms");

    assert!(
        tp99 < 16_000.0,
        "tick p99={tp99:.1}us must be < 16ms frame budget @{N} rules"
    );
    assert!(
        dp99 < 16_000.0,
        "truth_state p99={dp99:.1}us must be ≪ 16ms (off the hot path)"
    );
}
