//! M12.4 (ADR-048) AI-compose authoring/validation latency (**release**): a composition is
//! validate-then-applied as ONE undoable transaction on the interactive editor path (the user reviews a
//! proposal, then Applies) — so both the proposal-time `validate_composition` gate and the `apply_composition`
//! commit must hold the <16 ms interactive budget. The composition here is the flagship demo (a `SetField`
//! seed + an `AuthorRule`), reusing the M12.1 rule + M12.2 machine validators (no parallel path). Guards the
//! cost as the project's rule count grows. Release-only (benchmark discipline: `--release` for timing; CI
//! runs `cargo test` in debug, where it is ignored).
//!
//! NOTE (benchmark-discipline, product-principle 3): this box is an i9-13900H (high-end), so these are the
//! UPPER-BOUND numbers this hardware gives — the entry-level / min-spec profile is **owed** (no min-spec rig
//! available; the budget holds here by ~3 orders of magnitude, so the margin is large).

#![allow(clippy::cast_precision_loss)]

use std::time::Instant;

use metrocalk_core::compose::{ComposeOp, Composition};
use metrocalk_core::stdlib::{standard_actions, standard_components, standard_events};
use metrocalk_core::{
    apply_composition, validate_composition, Action, CompareOp, Condition, Engine, FieldValue, Op,
    Registry, RuleData,
};
use metrocalk_ecs::FlecsWorld;

fn percentiles(mut us: Vec<f64>) -> (f64, f64) {
    us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (us[us.len() / 2], us[us.len() * 99 / 100])
}

fn registry() -> Registry<FlecsWorld> {
    let mut reg = Registry::new(FlecsWorld::new());
    for c in standard_components() {
        reg.register(c).expect("register");
    }
    for e in standard_events() {
        reg.register_event(e);
    }
    for a in standard_actions() {
        reg.register_action(a);
    }
    reg
}

/// The flagship demo rule (When EnemyDied / If KillCounter.count ≥ 4 / Then set Flammable.lit).
fn ignite_rule(entity: &str) -> RuleData {
    RuleData {
        name: "ignite on kills".into(),
        enabled: true,
        event: "EnemyDied".into(),
        conditions: vec![Condition {
            entity: entity.into(),
            component: "KillCounter".into(),
            field: "count".into(),
            op: CompareOp::Ge,
            value: FieldValue::Integer(4),
        }],
        actions: vec![Action {
            action: "SetField".into(),
            entity: entity.into(),
            component: "Flammable".into(),
            field: "lit".into(),
            value: FieldValue::Bool(true),
        }],
    }
}

/// The demo composition: seed the counter the rule reads + author the rule, with rule id `rule_id`.
fn composition(entity: &str, rule_id: &str) -> Composition {
    Composition {
        ops: vec![
            ComposeOp::SetField {
                entity: entity.into(),
                component: "KillCounter".into(),
                field: "count".into(),
                value: FieldValue::Integer(0),
            },
            ComposeOp::AuthorRule {
                id: rule_id.into(),
                rule: ignite_rule(entity),
            },
        ],
    }
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release-only timing measurement")]
fn compose_validation_and_apply_hold_the_budget() {
    const N: usize = 2000; // compositions applied, to measure the cost as the `rules` map grows
    let reg = registry();
    let mut e = Engine::new(FlecsWorld::new(), 1);
    let mut entity = String::new();
    for _ in 0..16 {
        let id = e.alloc_entity_id();
        e.commit("spawn", vec![Op::CreateEntity { id, parent: None }])
            .unwrap();
        entity = id.to_loro_key();
    }
    let probe = composition(&entity, "r_probe");

    // Warm up the proposal-time validation gate (the engine runs it before the user can Apply).
    for _ in 0..200 {
        let _ = validate_composition(&reg, &probe, |x| e.entity_exists(x));
    }

    let mut val = Vec::with_capacity(5000);
    for _ in 0..5000 {
        let t0 = Instant::now();
        let ok = validate_composition(&reg, &probe, |x| e.entity_exists(x));
        val.push(t0.elapsed().as_secs_f64() * 1e6);
        assert!(ok.is_ok());
    }

    // Apply cost as the `rules` map grows (each apply is validate + one undoable transaction).
    let mut apply = Vec::with_capacity(N);
    for i in 0..N {
        let comp = composition(&entity, &format!("r_ai_{i}"));
        let t0 = Instant::now();
        apply_composition(&mut e, &reg, &comp).unwrap();
        apply.push(t0.elapsed().as_secs_f64() * 1e6);
    }
    assert_eq!(e.rules().len(), N, "all {N} compositions authored a rule");

    let (vp50, vp99) = percentiles(val);
    let (ap50, ap99) = percentiles(apply);
    eprintln!(
        "[M12.4] validate_composition (SetField+AuthorRule): p50={vp50:.2}us p99={vp99:.2}us"
    );
    eprintln!(
        "[M12.4] apply_composition (validate + one undoable tx, up to {N} rules): p50={ap50:.2}us p99={ap99:.2}us"
    );
    assert!(vp99 < 16_000.0, "validate p99={vp99:.1}us must be ≪ 16ms");
    assert!(ap99 < 16_000.0, "apply p99={ap99:.1}us must be ≪ 16ms");
}
