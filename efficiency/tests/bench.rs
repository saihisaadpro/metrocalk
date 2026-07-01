//! Release-only overhead budget for the min-spec ADMISSION CHECK (M13.6). The per-commit schedulability
//! check must be cheap enough to run on every commit + potentially per-frame, so this measures its own
//! cost. `#[cfg_attr(debug_assertions, ignore)]` → SKIPPED in the debug `cargo test`, RUN by
//! `cargo test --workspace --release` (release-budgets.yml). NOT a dark test.
//!
//! NOTE: this measures the ADMISSION-CHECK overhead only — it is NOT an energy measurement. True
//! frames-per-joule / SCI needs a real power source (RAPL / platform counter / meter), which this box
//! lacks — that number is OWED, never invented (see ADR-055).

use metrocalk_efficiency::{admit, Budget, Criticality, Task, MIN_SPEC_60FPS_US};
use std::time::Instant;

fn percentiles(mut us: Vec<u128>) -> (u128, u128) {
    us.sort_unstable();
    (
        us[us.len() / 2],
        us[(us.len() * 99 / 100).min(us.len() - 1)],
    )
}

#[allow(clippy::cast_possible_truncation)] // small loop indices fit u32 on the bench target
fn realistic_frame(n_level3: usize) -> Vec<Task> {
    let mut tasks = vec![
        Task::new("input", Criticality::Level1, 200),
        Task::new("physics", Criticality::Level1, 3_000),
        Task::new("audio", Criticality::Level1, 800),
        Task::new("gameplay", Criticality::Level2, 2_000),
    ];
    for i in 0..n_level3 {
        tasks.push(Task::new(
            format!("effect{i}"),
            Criticality::Level3,
            500 + (i as u32 % 7) * 300,
        ));
    }
    tasks
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release-only admission-check overhead budget (run by release-budgets.yml)"
)]
fn admission_check_holds_a_negligible_overhead() {
    const RUNS: usize = 1000;
    let budget = Budget {
        frame_us: MIN_SPEC_60FPS_US,
    };
    // A heavy frame: 64 Level-3 effects contending for the budget.
    let tasks = realistic_frame(64);

    // Warm up + confirm it degrades under overload while staying schedulable (Level-1 fits).
    let sched = admit(&tasks, budget);
    assert!(sched.schedulable);

    let mut us = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let t = Instant::now();
        let _ = admit(&tasks, budget);
        us.push(t.elapsed().as_micros());
    }
    let (p50, p99) = percentiles(us);
    println!("::notice::minspec-admission-check-p50-us={p50}");
    println!("::notice::minspec-admission-check-p99-us={p99}");

    // A per-commit / per-frame gate must be sub-millisecond over a heavy frame.
    assert!(
        p99 < 1_000,
        "admission-check p99 {p99}us exceeded 1ms (it must be negligible)"
    );
}
