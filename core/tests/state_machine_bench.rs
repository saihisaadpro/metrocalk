//! M12.2 (ADR-046) state-machine authoring/validation latency (**release**): the state-graph builder
//! validates the whole machine on every edit (the typo-proof + no-dangling + reachability gate) and each
//! edit is one `SetStateMachine` commit — both ride the interactive editor, so they must hold the <16 ms
//! budget. Measured at **graph scale** (N states + N transitions), since the validator walks every
//! transition (reusing `validate_rule`) and computes reachability. Release-only (benchmark discipline:
//! `--release` for timing; CI runs `cargo test` in debug, where it is `#[ignore]`d — it still COMPILES +
//! is collected by the gated core test job, so it is not a dark test).
//!
//! NOTE (benchmark-discipline, product-principle 3): this box is an i9-13900H (high-end), so these are the
//! UPPER-BOUND numbers this hardware gives — the entry-level / min-spec profile is **owed** (no min-spec rig
//! available; the budget holds here by orders of magnitude, so the margin is large). The editor-side
//! graph-RENDER cost is the React Flow layer (M2.5, virtualized/selective — measured in the editor), not
//! this core bench, which covers the authoring + validation half.

#![allow(clippy::cast_precision_loss)]

use std::time::Instant;

use metrocalk_core::rules::{Action, CompareOp, Condition, RuleData};
use metrocalk_core::state_machine::{validate_state_machine, StateMachine, Transition};
use metrocalk_core::stdlib::{standard_actions, standard_components, standard_events};
use metrocalk_core::{Engine, FieldValue, Op, Registry};
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

/// A transition `from` -> `to` on entity `q`: a registry-fed Rule (a KillCounter guard + the canonical
/// "enter `to`" set-state action) — the shape the builder produces.
fn transition(id: String, from: &str, to: &str, q: &str, event: &str) -> Transition {
    Transition {
        id,
        from: from.into(),
        to: to.into(),
        rule: RuleData {
            name: format!("{from} -> {to}"),
            enabled: true,
            event: event.into(),
            conditions: vec![Condition {
                entity: q.into(),
                component: "KillCounter".into(),
                field: "count".into(),
                op: CompareOp::Ge,
                value: FieldValue::Integer(4),
            }],
            actions: vec![Action {
                action: "SetField".into(),
                entity: q.into(),
                component: "QuestState".into(),
                field: "state".into(),
                value: FieldValue::Str(to.into()),
            }],
        },
    }
}

/// A machine with `m` states (S0..S{m-1}) + a reachable chain of m-1 transitions + `extra` deterministic
/// cross edges (index-seeded — no RNG, so the scene is reproducible). Every state stays reachable.
fn machine(q: &str, m: usize, extra: usize) -> StateMachine {
    let states: Vec<String> = (0..m).map(|i| format!("S{i}")).collect();
    let mut sm = StateMachine {
        name: "quest".into(),
        entity: q.into(),
        component: "QuestState".into(),
        field: "state".into(),
        states: states.clone(),
        initial: "S0".into(),
        transitions: vec![],
    };
    let mut transitions = Vec::with_capacity(m + extra);
    let mut tid = 0usize;
    for i in 0..m.saturating_sub(1) {
        transitions.push(transition(
            format!("t{tid}"),
            &states[i],
            &states[i + 1],
            q,
            "EnemyDied",
        ));
        tid += 1;
    }
    for k in 0..extra {
        let from = k % m;
        let to = (k * 7 + 3) % m;
        transitions.push(transition(
            format!("t{tid}"),
            &states[from],
            &states[to],
            q,
            "ZoneEntered",
        ));
        tid += 1;
    }
    sm.transitions = transitions;
    sm
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release-only timing measurement")]
fn state_machine_validation_and_authoring_hold_the_budget() {
    const STATES: usize = 64; // graph scale: states (nodes)
    const EXTRA: usize = 64; // + cross edges → ~127 transitions (edges)
    const N: usize = 1000; // machines authored, to measure cost as the `state_machines` map grows

    let reg = registry();
    let mut e = Engine::new(FlecsWorld::new(), 1);
    let mut q = String::new();
    for _ in 0..16 {
        let id = e.alloc_entity_id();
        e.commit("spawn", vec![Op::CreateEntity { id, parent: None }])
            .unwrap();
        q = id.to_loro_key();
    }
    let sm = machine(&q, STATES, EXTRA);
    let edges = sm.transitions.len();
    // Sanity: the fixture is valid (every transition a real Rule, no dangling, all reachable).
    validate_state_machine(&reg, &sm, |x| e.entity_exists(x)).expect("the bench machine is valid");

    // Warm up (the validation gate the builder runs on every edit).
    for _ in 0..200 {
        let _ = validate_state_machine(&reg, &sm, |x| e.entity_exists(x));
    }

    let mut val = Vec::with_capacity(5000);
    for _ in 0..5000 {
        let t0 = Instant::now();
        let ok = validate_state_machine(&reg, &sm, |x| e.entity_exists(x));
        val.push(t0.elapsed().as_secs_f64() * 1e6);
        assert!(ok.is_ok());
    }

    // Authoring cost as the `state_machines` map grows (each edit is one undoable SetStateMachine commit).
    let mut auth = Vec::with_capacity(N);
    for _ in 0..N {
        let id = e.alloc_state_machine_id();
        let t0 = Instant::now();
        e.commit(
            "author state machine",
            vec![Op::SetStateMachine { id, sm: sm.clone() }],
        )
        .unwrap();
        auth.push(t0.elapsed().as_secs_f64() * 1e6);
    }
    assert_eq!(e.state_machines().len(), N, "all {N} machines authored");

    let (vp50, vp99) = percentiles(val);
    let (ap50, ap99) = percentiles(auth);
    eprintln!(
        "[M12.2] validate_state_machine ({STATES} states, {edges} transitions): p50={vp50:.2}us p99={vp99:.2}us"
    );
    eprintln!(
        "[M12.2] author state machine (SetStateMachine commit, up to {N} machines): p50={ap50:.2}us p99={ap99:.2}us"
    );
    assert!(vp99 < 16_000.0, "validate p99={vp99:.1}us must be << 16ms");
    assert!(ap99 < 16_000.0, "author p99={ap99:.1}us must be << 16ms");
}
