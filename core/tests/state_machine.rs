//! M12.2 (ADR-046) — state machines as data, transitions are M12.1 Rules.
//!
//! Headless coverage of the DoD + the adversarial guards: a registry-fed machine validates (the canonical
//! `QuestState` Hunting -> ReadyForBoss -> FacingBoss); **a transition IS an M12.1 Rule** (the reuse is
//! asserted, not a parallel model); a **dangling** transition is **Blocked + explained** (ADR-016,
//! test-first); a transition whose effect doesn't enter `to` is rejected; **reachability** is a warning
//! (explained, not rejected); **simultaneous transitions are deterministically ordered**; authoring/editing
//! a machine is **one undoable transaction** that round-trips export->reload; the **current-state read is
//! the M12.5 seam** (defaults to `initial`); and **two concurrent machine edits merge** without clobber
//! (invariant 1). Running the machine is M12.5 (not exercised here).

use metrocalk_core::pipeline::Op;
use metrocalk_core::rules::{Action, CompareOp, Condition, RuleData};
use metrocalk_core::state_machine::{
    validate_state_machine, StateMachine, StateMachineError, Transition,
};
use metrocalk_core::stdlib::{standard_actions, standard_components, standard_events};
use metrocalk_core::{validate_rule, Engine, FieldValue, Registry};
use metrocalk_ecs::FlecsWorld;

// ── fixtures ────────────────────────────────────────────────────────────────

fn registry() -> Registry<FlecsWorld> {
    let mut reg = Registry::new(FlecsWorld::new());
    for meta in standard_components() {
        reg.register(meta).expect("stdlib component registers");
    }
    for ev in standard_events() {
        reg.register_event(ev);
    }
    for ac in standard_actions() {
        reg.register_action(ac);
    }
    reg
}

fn engine() -> Engine<FlecsWorld> {
    Engine::new(FlecsWorld::new(), 1)
}

fn spawn(e: &mut Engine<FlecsWorld>) -> String {
    let id = e.alloc_entity_id();
    e.commit("spawn", vec![Op::CreateEntity { id, parent: None }])
        .expect("create");
    id.to_loro_key()
}

/// Build a transition whose rule is a real M12.1 Rule: When `event` / If `conditions` / Then the canonical
/// "enter `to`" set-state action (constructed from the machine, so it can never typo the state field).
fn transition(
    sm: &StateMachine,
    id: &str,
    from: &str,
    to: &str,
    event: &str,
    conditions: Vec<Condition>,
) -> Transition {
    Transition {
        id: id.into(),
        from: from.into(),
        to: to.into(),
        rule: RuleData {
            name: format!("{from} -> {to}"),
            enabled: true,
            event: event.into(),
            conditions,
            actions: vec![sm.enter_action(to)],
        },
    }
}

/// The canonical test-#5 machine on entity `q`: Hunting -> ReadyForBoss (after 4 kills) -> FacingBoss
/// (on reaching the boss arena). `q` carries the `QuestState` state field; conditions read `KillCounter`
/// and `Zone` (referenced on `q` for the fixture — `validate_rule` checks the registry + entity existence,
/// not component presence, exactly as the M12.1 rules tests do).
fn quest_machine(q: &str) -> StateMachine {
    let mut sm = StateMachine {
        name: "QuestState".into(),
        entity: q.into(),
        component: "QuestState".into(),
        field: "state".into(),
        states: vec!["Hunting".into(), "ReadyForBoss".into(), "FacingBoss".into()],
        initial: "Hunting".into(),
        transitions: vec![],
    };
    let transitions = vec![
        transition(
            &sm,
            "t1",
            "Hunting",
            "ReadyForBoss",
            "EnemyDied",
            vec![Condition {
                entity: q.into(),
                component: "KillCounter".into(),
                field: "count".into(),
                op: CompareOp::Ge,
                value: FieldValue::Integer(4),
            }],
        ),
        transition(
            &sm,
            "t2",
            "ReadyForBoss",
            "FacingBoss",
            "ZoneEntered",
            vec![Condition {
                entity: q.into(),
                component: "Zone".into(),
                field: "current".into(),
                op: CompareOp::Eq,
                value: FieldValue::Str("BossArena".into()),
            }],
        ),
    ];
    sm.transitions = transitions;
    sm
}

// ── validation: registry-fed, no-dangling, Blocked + explained (ADR-016) ─────

#[test]
fn the_canonical_quest_machine_validates() {
    let reg = registry();
    let mut e = engine();
    let q = spawn(&mut e);
    let sm = quest_machine(&q);
    let report = validate_state_machine(&reg, &sm, |id| e.entity_exists(id))
        .expect("the Hunting->ReadyForBoss->FacingBoss machine is valid");
    assert!(
        report.unreachable.is_empty(),
        "every state is reachable from initial"
    );
}

#[test]
fn a_transition_is_an_m12_1_rule() {
    // The reuse guard: each transition's guard+effect IS a `RuleData`, validated by the SAME
    // `validate_rule` the Rules layer uses — not a forked logic model.
    let reg = registry();
    let mut e = engine();
    let q = spawn(&mut e);
    let sm = quest_machine(&q);
    for t in &sm.transitions {
        let _: &RuleData = &t.rule; // type-level: a transition carries an M12.1 RuleData
        validate_rule(&reg, &t.rule, |id| e.entity_exists(id))
            .expect("each transition validates through the Rules validator (reuse, not a fork)");
    }
}

#[test]
fn a_dangling_transition_is_blocked_and_explained() {
    // test-first: a transition that targets a state the machine doesn't declare is rejected with a reason.
    let reg = registry();
    let mut e = engine();
    let q = spawn(&mut e);

    let mut bad_to = quest_machine(&q);
    bad_to.transitions[0].to = "Nowhere".into();
    let err = validate_state_machine(&reg, &bad_to, |id| e.entity_exists(id)).unwrap_err();
    assert!(matches!(err, StateMachineError::DanglingTo { .. }));
    assert!(
        err.to_string()
            .contains("isn't one of this machine's states"),
        "the rejection explains itself: {err}"
    );

    let mut bad_from = quest_machine(&q);
    bad_from.transitions[1].from = "Limbo".into();
    assert!(matches!(
        validate_state_machine(&reg, &bad_from, |id| e.entity_exists(id)),
        Err(StateMachineError::DanglingFrom { .. })
    ));
}

#[test]
fn an_invalid_transition_rule_is_blocked_through_the_rules_validator() {
    // A typo in a transition's When/If/Then surfaces as the SAME Rules-layer explanation (reuse).
    let reg = registry();
    let mut e = engine();
    let q = spawn(&mut e);
    let mut sm = quest_machine(&q);
    sm.transitions[0].rule.event = "EnemyExploded".into(); // not in the registry vocabulary
    let err = validate_state_machine(&reg, &sm, |id| e.entity_exists(id)).unwrap_err();
    assert!(matches!(
        err,
        StateMachineError::TransitionRule {
            source: metrocalk_core::rules::RuleError::UnknownEvent(_),
            ..
        }
    ));
    assert!(
        err.to_string().contains("isn't an event the engine knows"),
        "the underlying Rule reason carries through: {err}"
    );
}

#[test]
fn a_transition_that_does_not_change_the_state_is_blocked() {
    // A Rule that's valid but whose effect doesn't ENTER `to` isn't a real transition — refused, so the
    // graph can never wire an edge that doesn't move the state.
    let reg = registry();
    let mut e = engine();
    let q = spawn(&mut e);
    let mut sm = quest_machine(&q);
    // Replace the set-state action with a (valid) action that sets a different field.
    sm.transitions[0].rule.actions = vec![Action {
        action: "SetField".into(),
        entity: q.clone(),
        component: "Flammable".into(),
        field: "lit".into(),
        value: FieldValue::Bool(true),
    }];
    assert!(matches!(
        validate_state_machine(&reg, &sm, |id| e.entity_exists(id)),
        Err(StateMachineError::NotAStateChange { .. })
    ));
}

#[test]
fn structural_problems_are_each_blocked_and_explained() {
    let reg = registry();
    let mut e = engine();
    let q = spawn(&mut e);

    let mut nameless = quest_machine(&q);
    nameless.name = "  ".into();
    assert_eq!(
        validate_state_machine(&reg, &nameless, |id| e.entity_exists(id)),
        Err(StateMachineError::EmptyName)
    );

    let mut no_states = quest_machine(&q);
    no_states.states.clear();
    assert_eq!(
        validate_state_machine(&reg, &no_states, |id| e.entity_exists(id)),
        Err(StateMachineError::NoStates)
    );

    let mut dup = quest_machine(&q);
    dup.states.push("Hunting".into());
    assert!(matches!(
        validate_state_machine(&reg, &dup, |id| e.entity_exists(id)),
        Err(StateMachineError::DuplicateState(_))
    ));

    let mut bad_initial = quest_machine(&q);
    bad_initial.initial = "Sleeping".into();
    assert!(matches!(
        validate_state_machine(&reg, &bad_initial, |id| e.entity_exists(id)),
        Err(StateMachineError::InitialNotAState(_))
    ));

    let mut dup_tid = quest_machine(&q);
    dup_tid.transitions[1].id = "t1".into();
    assert!(matches!(
        validate_state_machine(&reg, &dup_tid, |id| e.entity_exists(id)),
        Err(StateMachineError::DuplicateTransitionId(_))
    ));
}

#[test]
fn an_invalid_or_non_string_state_target_is_blocked() {
    // Even a zero-transition machine is typo-proof: its (entity, component, field) must be a real registry
    // String field on a live entity ("states are an ordinary registry-fed slice of an entity").
    let reg = registry();
    let mut e = engine();
    let q = spawn(&mut e);

    let bare = |entity: &str, component: &str, field: &str| StateMachine {
        name: "M".into(),
        entity: entity.into(),
        component: component.into(),
        field: field.into(),
        states: vec!["A".into()],
        initial: "A".into(),
        transitions: vec![],
    };

    // Dangling entity.
    assert!(matches!(
        validate_state_machine(&reg, &bare("ff_ff", "QuestState", "state"), |id| e
            .entity_exists(id)),
        Err(StateMachineError::UnknownEntity(_))
    ));
    // Unknown component.
    assert!(matches!(
        validate_state_machine(&reg, &bare(&q, "Frobnicator", "state"), |id| e
            .entity_exists(id)),
        Err(StateMachineError::UnknownComponent(_))
    ));
    // Known component, unknown field.
    assert!(matches!(
        validate_state_machine(&reg, &bare(&q, "QuestState", "phase"), |id| e
            .entity_exists(id)),
        Err(StateMachineError::UnknownField { .. })
    ));
    // A non-string field can't hold a state name (KillCounter.count is Integer).
    assert!(matches!(
        validate_state_machine(&reg, &bare(&q, "KillCounter", "count"), |id| e
            .entity_exists(id)),
        Err(StateMachineError::StateFieldNotString { .. })
    ));
}

#[test]
fn an_unreachable_state_is_warned_not_rejected() {
    // An island state (no incoming transition from initial) is a WARNING, explained — not a hard reject.
    let reg = registry();
    let mut e = engine();
    let q = spawn(&mut e);
    let mut sm = quest_machine(&q);
    sm.states.push("Orphan".into()); // declared, but nothing transitions into it
    let report = validate_state_machine(&reg, &sm, |id| e.entity_exists(id))
        .expect("an unreachable state is a warning, the machine still validates");
    assert_eq!(report.unreachable, vec!["Orphan".to_string()]);
}

// ── deterministic ordering of simultaneous transitions ───────────────────────

#[test]
fn simultaneous_transitions_have_a_deterministic_order() {
    // Two transitions out of the SAME state on the SAME event are competing "simultaneous" candidates; the
    // tie-break (from, event, to, id) makes their order reproducible — defined now so M12.5's firing is
    // deterministic. Inserted in reverse tie-break order; `ordered_transitions` sorts them stably.
    let mut e = engine();
    let q = spawn(&mut e);
    let mut sm = StateMachine {
        name: "race".into(),
        entity: q.clone(),
        component: "QuestState".into(),
        field: "state".into(),
        states: vec!["Start".into(), "Alpha".into(), "Beta".into()],
        initial: "Start".into(),
        transitions: vec![],
    };
    // Insert deliberately out of order: to=Beta before to=Alpha (Alpha < Beta), id z9 before a1.
    let to_beta = transition(&sm, "z9", "Start", "Beta", "EnemyDied", vec![]);
    let to_alpha = transition(&sm, "a1", "Start", "Alpha", "EnemyDied", vec![]);
    sm.transitions = vec![to_beta, to_alpha];

    let ordered: Vec<&str> = sm
        .ordered_transitions()
        .iter()
        .map(|t| t.to.as_str())
        .collect();
    assert_eq!(
        ordered,
        vec!["Alpha", "Beta"],
        "same-from/same-event transitions order by `to` (then id) — deterministic"
    );
    // Idempotent / reproducible across calls.
    let again: Vec<&str> = sm
        .ordered_transitions()
        .iter()
        .map(|t| t.to.as_str())
        .collect();
    assert_eq!(ordered, again);
}

// ── authoring: one undoable transaction on the Loro doc, survives reload ─────

#[test]
fn authoring_a_machine_is_one_undoable_transaction() {
    let mut e = engine();
    let q = spawn(&mut e);
    let id = e.alloc_state_machine_id();
    let sm = quest_machine(&q);

    e.commit(
        "author machine",
        vec![Op::SetStateMachine {
            id: id.clone(),
            sm: sm.clone(),
        }],
    )
    .expect("author");
    let stored = e.state_machine(&id).expect("machine present");
    assert_eq!(
        stored, sm,
        "every state + transition round-trips through the document"
    );
    assert_eq!(e.state_machines().len(), 1);

    // ONE undo removes the whole machine (states + all transitions), not one edge.
    assert!(e.undo());
    assert!(
        e.state_machine(&id).is_none(),
        "undo removes the whole machine in one step"
    );
    assert!(e.state_machines().is_empty());

    // Redo restores it exactly.
    assert!(e.redo());
    assert_eq!(e.state_machine(&id), Some(sm));
}

#[test]
fn editing_a_machine_is_undoable_and_replaces_in_place() {
    // Drawing a new transition (the graph edit) is a SetStateMachine with the updated whole machine — one
    // undoable tx that replaces in place, never appends a second machine.
    let mut e = engine();
    let q = spawn(&mut e);
    let id = e.alloc_state_machine_id();
    let v1 = quest_machine(&q);
    e.commit(
        "author",
        vec![Op::SetStateMachine {
            id: id.clone(),
            sm: v1.clone(),
        }],
    )
    .unwrap();

    let mut v2 = v1.clone();
    v2.states.push("Won".into());
    let win = transition(&v2, "t3", "FacingBoss", "Won", "EnemyDied", vec![]);
    v2.transitions.push(win);
    e.commit(
        "draw transition",
        vec![Op::SetStateMachine {
            id: id.clone(),
            sm: v2.clone(),
        }],
    )
    .unwrap();
    assert_eq!(e.state_machine(&id), Some(v2));
    assert_eq!(
        e.state_machines().len(),
        1,
        "edit replaced in place, not appended"
    );

    // Undo the edit → back to v1 (the new transition + state gone, the machine intact).
    assert!(e.undo());
    assert_eq!(e.state_machine(&id), Some(v1));
}

#[test]
fn the_current_state_read_is_the_m12_5_seam_defaulting_to_initial() {
    // M12.2 defines + reads the `current` slot; until M12.5 ticks it, it reads as `initial`.
    let mut e = engine();
    let q = spawn(&mut e);
    let id = e.alloc_state_machine_id();
    let sm = quest_machine(&q);
    e.commit("author", vec![Op::SetStateMachine { id: id.clone(), sm }])
        .unwrap();
    assert_eq!(
        e.state_machine_current(&id),
        Some("Hunting".into()),
        "current defaults to the initial state (M12.5 will advance it)"
    );
    let absent = e.alloc_state_machine_id();
    assert_eq!(e.state_machine_current(&absent), None);
}

#[test]
fn machines_survive_export_reload() {
    let mut e = engine();
    let q = spawn(&mut e);
    let id = e.alloc_state_machine_id();
    let sm = quest_machine(&q);
    e.commit(
        "author",
        vec![Op::SetStateMachine {
            id: id.clone(),
            sm: sm.clone(),
        }],
    )
    .unwrap();

    // Reopen: a fresh engine merges the snapshot (the `.mtk` reload path).
    let snapshot = e.snapshot();
    let mut reopened = engine();
    reopened.merge(&snapshot).expect("reload");
    assert_eq!(
        reopened.state_machine(&id),
        Some(sm),
        "the machine (all states + transitions) survives reload"
    );
}

// ── merge: two concurrent machine edits converge without clobber (invariant 1) ─

#[test]
fn concurrent_machine_authoring_merges_without_clobber() {
    let reg = registry();
    let mut base = engine();
    let q = spawn(&mut base);
    let snapshot = base.snapshot();

    let mut peer_a = Engine::new(FlecsWorld::new(), 0xA);
    peer_a.merge(&snapshot).unwrap();
    let mut peer_b = Engine::new(FlecsWorld::new(), 0xB);
    peer_b.merge(&snapshot).unwrap();

    let id_a = peer_a.alloc_state_machine_id();
    let mut sm_a = quest_machine(&q);
    sm_a.name = "machine A".into();
    peer_a
        .commit(
            "A",
            vec![Op::SetStateMachine {
                id: id_a.clone(),
                sm: sm_a.clone(),
            }],
        )
        .unwrap();

    let id_b = peer_b.alloc_state_machine_id();
    let mut sm_b = quest_machine(&q);
    sm_b.name = "machine B".into();
    peer_b
        .commit(
            "B",
            vec![Op::SetStateMachine {
                id: id_b.clone(),
                sm: sm_b.clone(),
            }],
        )
        .unwrap();

    // Cross-merge both ways → both peers converge to the SAME two machines (no clobber).
    peer_a.merge(&peer_b.export_updates()).unwrap();
    peer_b.merge(&peer_a.export_updates()).unwrap();

    for peer in [&peer_a, &peer_b] {
        assert_eq!(
            peer.state_machines().len(),
            2,
            "both machines survive the merge"
        );
        assert_eq!(
            peer.state_machine(&id_a).map(|m| m.name),
            Some("machine A".into())
        );
        assert_eq!(
            peer.state_machine(&id_b).map(|m| m.name),
            Some("machine B".into())
        );
        // Merge-validation (invariant 3): every merged machine is still registry-valid.
        for (_, sm) in peer.state_machines() {
            validate_state_machine(&reg, &sm, |id| peer.entity_exists(id))
                .expect("a merged machine re-validates against the registry");
        }
    }
}
