//! M12.1 (ADR-045) rules authoring/validation latency (**release**): the registry-fed builder validates
//! every rule on Create (the typo-proof gate) and authoring is one commit — both ride the interactive
//! editor, so they must hold the <16 ms budget. Guards the cost as the rule count grows. Release-only
//! (benchmark discipline: `--release` for timing; CI runs `cargo test` in debug, where it is ignored).
//!
//! NOTE (benchmark-discipline, product-principle 3): this box is an i9-13900H (high-end), so these are the
//! UPPER-BOUND numbers this hardware gives — the entry-level / min-spec profile is **owed** (no min-spec
//! rig available; the budget holds here by ~3 orders of magnitude, so the margin is large).

#![allow(clippy::cast_precision_loss)]

use std::time::Instant;

use metrocalk_core::stdlib::{standard_actions, standard_components, standard_events};
use metrocalk_core::{
    validate_rule, Action, CompareOp, Condition, Engine, FieldValue, Op, Registry, RuleData,
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

/// The canonical test-#5 conditional referencing `entity` (3 typed parts — what the builder produces).
fn rule(entity: &str) -> RuleData {
    RuleData {
        name: "rusty sword ignites".into(),
        enabled: true,
        event: "EnemyDied".into(),
        conditions: vec![
            Condition {
                entity: entity.into(),
                component: "KillCounter".into(),
                field: "count".into(),
                op: CompareOp::Ge,
                value: FieldValue::Integer(4),
            },
            Condition {
                entity: entity.into(),
                component: "Zone".into(),
                field: "current".into(),
                op: CompareOp::Eq,
                value: FieldValue::Str("BossArena".into()),
            },
        ],
        actions: vec![Action {
            action: "SetField".into(),
            entity: entity.into(),
            component: "Flammable".into(),
            field: "lit".into(),
            value: FieldValue::Bool(true),
        }],
    }
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release-only timing measurement")]
fn rule_validation_and_authoring_hold_the_budget() {
    let reg = registry();
    let mut e = Engine::new(FlecsWorld::new(), 1);
    let mut entity = String::new();
    for _ in 0..16 {
        let id = e.alloc_entity_id();
        e.commit("spawn", vec![Op::CreateEntity { id, parent: None }])
            .unwrap();
        entity = id.to_loro_key();
    }
    let r = rule(&entity);

    // Warm up (the registry-fed validation gate the builder runs on every Create).
    for _ in 0..200 {
        let _ = validate_rule(&reg, &r, |x| e.entity_exists(x));
    }

    let mut val = Vec::with_capacity(5000);
    for _ in 0..5000 {
        let t0 = Instant::now();
        let ok = validate_rule(&reg, &r, |x| e.entity_exists(x));
        val.push(t0.elapsed().as_secs_f64() * 1e6);
        assert!(ok.is_ok());
    }

    // Authoring cost as the `rules` map grows (each Create is one undoable SetRule commit).
    const N: usize = 2000;
    let mut auth = Vec::with_capacity(N);
    for _ in 0..N {
        let id = e.alloc_rule_id();
        let t0 = Instant::now();
        e.commit(
            "author rule",
            vec![Op::SetRule {
                id,
                rule: r.clone(),
            }],
        )
        .unwrap();
        auth.push(t0.elapsed().as_secs_f64() * 1e6);
    }
    assert_eq!(e.rules().len(), N, "all {N} rules authored");

    let (vp50, vp99) = percentiles(val);
    let (ap50, ap99) = percentiles(auth);
    eprintln!("[M12.1] validate_rule (3-part conditional): p50={vp50:.2}us p99={vp99:.2}us");
    eprintln!("[M12.1] author rule (SetRule commit, up to {N} rules): p50={ap50:.2}us p99={ap99:.2}us");
    assert!(vp99 < 16_000.0, "validate p99={vp99:.1}us must be ≪ 16ms");
    assert!(ap99 < 16_000.0, "author p99={ap99:.1}us must be ≪ 16ms");
}
