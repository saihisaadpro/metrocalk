//! M12.1 (ADR-045) — the Rules layer: When/If/Then as registry-fed, typo-proof, undoable data.
//!
//! Headless coverage of the DoD: a rule validates **against the registry** (only known events/components/
//! fields/actions/value-types accepted; an unknown one is **Blocked + explained** — the ADR-016 guard,
//! test-first); authoring a rule is **one undoable transaction** that round-trips export→reload; the
//! **mirror-rule** offer fires on an add-on-enter rule; a `KillCounter` reads/mutates through the typed
//! vocabulary; and **two concurrent rule edits merge** without clobber (invariant 1). Running a rule is
//! M12.5 (not exercised here).

use metrocalk_core::pipeline::Op;
use metrocalk_core::rules::{Action, CompareOp, Condition, RuleData, RuleError};
use metrocalk_core::stdlib::{standard_actions, standard_components, standard_events};
use metrocalk_core::{propose_mirror, validate_rule, Engine, FieldValue, Registry};
use metrocalk_ecs::FlecsWorld;

// ── fixtures ────────────────────────────────────────────────────────────────

/// A registry carrying the full M12.1 vocabulary: components (incl. the rule-target primitives), events,
/// and the closed action set — exactly what a registry-fed builder offers.
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

/// Create a scene entity and return its editor-id-space (Loro-key) string — the form a Condition/Action
/// references an entity by (matching the React builder's id space).
fn spawn(e: &mut Engine<FlecsWorld>) -> String {
    let id = e.alloc_entity_id();
    e.commit("spawn", vec![Op::CreateEntity { id, parent: None }])
        .expect("create");
    id.to_loro_key()
}

/// The canonical test-#5 rule, parameterized by the entities it references: "when an enemy dies, if
/// KillCounter ≥ 4 and zone = BossArena, then ignite the sword."
fn test5_rule(counter: &str, zone: &str, sword: &str) -> RuleData {
    RuleData {
        name: "rusty sword ignites".into(),
        enabled: true,
        event: "EnemyDied".into(),
        conditions: vec![
            Condition {
                entity: counter.into(),
                component: "KillCounter".into(),
                field: "count".into(),
                op: CompareOp::Ge,
                value: FieldValue::Integer(4),
            },
            Condition {
                entity: zone.into(),
                component: "Zone".into(),
                field: "current".into(),
                op: CompareOp::Eq,
                value: FieldValue::Str("BossArena".into()),
            },
        ],
        actions: vec![Action {
            action: "SetField".into(),
            entity: sword.into(),
            component: "Flammable".into(),
            field: "lit".into(),
            value: FieldValue::Bool(true),
        }],
    }
}

// ── validation: registry-fed, typo-proof, Blocked + explained (ADR-016) ──────

#[test]
fn a_registry_fed_conditional_validates() {
    let reg = registry();
    let mut e = engine();
    let (counter, zone, sword) = (spawn(&mut e), spawn(&mut e), spawn(&mut e));
    let rule = test5_rule(&counter, &zone, &sword);
    validate_rule(&reg, &rule, |id| e.entity_exists(id))
        .expect("the canonical 3-part rule is valid");
}

#[test]
fn an_unknown_event_is_blocked_and_explained() {
    let reg = registry();
    let mut e = engine();
    let sword = spawn(&mut e);
    let mut rule = test5_rule(&sword, &sword, &sword);
    rule.event = "EnemyExploded".into(); // not in the registry vocabulary
    let err = validate_rule(&reg, &rule, |id| e.entity_exists(id)).unwrap_err();
    assert_eq!(err, RuleError::UnknownEvent("EnemyExploded".into()));
    assert!(
        err.to_string().contains("isn't an event the engine knows"),
        "the rejection explains itself: {err}"
    );
}

#[test]
fn an_unknown_component_or_field_is_blocked() {
    let reg = registry();
    let mut e = engine();
    let sword = spawn(&mut e);

    // Unknown component.
    let mut bad_comp = test5_rule(&sword, &sword, &sword);
    bad_comp.conditions[0].component = "FrobCounter".into();
    assert!(matches!(
        validate_rule(&reg, &bad_comp, |id| e.entity_exists(id)),
        Err(RuleError::UnknownComponent { .. })
    ));

    // Known component, unknown field.
    let mut bad_field = test5_rule(&sword, &sword, &sword);
    bad_field.conditions[0].field = "kills".into(); // KillCounter has `count`, not `kills`
    assert!(matches!(
        validate_rule(&reg, &bad_field, |id| e.entity_exists(id)),
        Err(RuleError::UnknownField { .. })
    ));
}

#[test]
fn an_unknown_action_is_blocked() {
    let reg = registry();
    let mut e = engine();
    let sword = spawn(&mut e);
    let mut rule = test5_rule(&sword, &sword, &sword);
    rule.actions[0].action = "Explode".into(); // not in the closed action vocabulary
    assert!(matches!(
        validate_rule(&reg, &rule, |id| e.entity_exists(id)),
        Err(RuleError::UnknownAction(_))
    ));
}

#[test]
fn a_value_type_mismatch_is_blocked() {
    let reg = registry();
    let mut e = engine();
    let sword = spawn(&mut e);
    let mut rule = test5_rule(&sword, &sword, &sword);
    // KillCounter.count is an integer; compare it to a string → mismatch.
    rule.conditions[0].value = FieldValue::Str("four".into());
    assert!(matches!(
        validate_rule(&reg, &rule, |id| e.entity_exists(id)),
        Err(RuleError::FieldTypeMismatch { .. })
    ));
}

#[test]
fn a_dangling_entity_reference_is_blocked() {
    let reg = registry();
    let mut e = engine();
    let sword = spawn(&mut e);
    let mut rule = test5_rule(&sword, &sword, &sword);
    rule.actions[0].entity = "ff_ff".into(); // a well-formed key for an entity that doesn't exist
    assert!(matches!(
        validate_rule(&reg, &rule, |id| e.entity_exists(id)),
        Err(RuleError::UnknownEntity(_))
    ));
}

#[test]
fn an_empty_name_or_no_actions_is_blocked() {
    let reg = registry();
    let mut e = engine();
    let sword = spawn(&mut e);

    let mut nameless = test5_rule(&sword, &sword, &sword);
    nameless.name = "   ".into();
    assert_eq!(
        validate_rule(&reg, &nameless, |id| e.entity_exists(id)),
        Err(RuleError::EmptyName)
    );

    let mut inert = test5_rule(&sword, &sword, &sword);
    inert.actions.clear();
    assert_eq!(
        validate_rule(&reg, &inert, |id| e.entity_exists(id)),
        Err(RuleError::NoActions)
    );
}

#[test]
fn kill_counter_reads_and_mutates_through_the_typed_vocabulary() {
    // The building block of the test-5 conditional: a Condition READS KillCounter.count (Integer, Ge 4)
    // and an AdjustCounter Action MUTATES it — both registry-typed and accepted.
    let reg = registry();
    let mut e = engine();
    let counter = spawn(&mut e);
    let rule = RuleData {
        name: "count kills".into(),
        enabled: true,
        event: "EnemyDied".into(),
        conditions: vec![Condition {
            entity: counter.clone(),
            component: "KillCounter".into(),
            field: "count".into(),
            op: CompareOp::Ge,
            value: FieldValue::Integer(0),
        }],
        actions: vec![Action {
            action: "AdjustCounter".into(),
            entity: counter,
            component: "KillCounter".into(),
            field: "count".into(),
            value: FieldValue::Integer(1),
        }],
    };
    validate_rule(&reg, &rule, |id| e.entity_exists(id)).expect("read+mutate a counter is valid");
    // The comparison operator is semantically usable (the M12.5 runtime seam).
    assert_eq!(
        CompareOp::Ge.eval(&FieldValue::Integer(4), &FieldValue::Integer(4)),
        Some(true)
    );
    assert_eq!(
        CompareOp::Ge.eval(&FieldValue::Integer(3), &FieldValue::Integer(4)),
        Some(false)
    );
}

// ── authoring: one undoable transaction on the Loro doc, survives reload ──────

#[test]
fn authoring_a_rule_is_one_undoable_transaction() {
    let mut e = engine();
    let (counter, zone, sword) = (spawn(&mut e), spawn(&mut e), spawn(&mut e));
    let id = e.alloc_rule_id();
    let rule = test5_rule(&counter, &zone, &sword);

    e.commit(
        "author rule",
        vec![Op::SetRule {
            id: id.clone(),
            rule: rule.clone(),
        }],
    )
    .expect("author");
    // The whole 3-part conditional is one rule, read back intact.
    let stored = e.rule(&id).expect("rule present");
    assert_eq!(stored, rule, "every part round-trips through the document");
    assert_eq!(e.rules().len(), 1);

    // ONE undo removes the entire rule (not one part of it).
    assert!(e.undo());
    assert!(
        e.rule(&id).is_none(),
        "undo removes the whole rule in one step"
    );
    assert!(e.rules().is_empty());

    // Redo restores it exactly.
    assert!(e.redo());
    assert_eq!(e.rule(&id), Some(rule));
}

#[test]
fn editing_a_rule_is_undoable_and_replaces_in_place() {
    let mut e = engine();
    let sword = spawn(&mut e);
    let id = e.alloc_rule_id();
    let v1 = test5_rule(&sword, &sword, &sword);
    e.commit(
        "author",
        vec![Op::SetRule {
            id: id.clone(),
            rule: v1.clone(),
        }],
    )
    .unwrap();

    let mut v2 = v1.clone();
    v2.enabled = false;
    v2.name = "renamed".into();
    e.commit(
        "edit",
        vec![Op::SetRule {
            id: id.clone(),
            rule: v2.clone(),
        }],
    )
    .unwrap();
    assert_eq!(e.rule(&id), Some(v2));
    assert_eq!(e.rules().len(), 1, "edit replaced in place, not appended");

    // Undo the edit → back to v1 (not gone).
    assert!(e.undo());
    assert_eq!(e.rule(&id), Some(v1));
}

#[test]
fn rules_survive_export_reload() {
    let mut e = engine();
    let (counter, zone, sword) = (spawn(&mut e), spawn(&mut e), spawn(&mut e));
    let id = e.alloc_rule_id();
    let rule = test5_rule(&counter, &zone, &sword);
    e.commit(
        "author",
        vec![Op::SetRule {
            id: id.clone(),
            rule: rule.clone(),
        }],
    )
    .unwrap();

    // Reopen: a fresh engine merges the snapshot (the `.mtk` reload path).
    let snapshot = e.snapshot();
    let mut reopened = engine();
    reopened.merge(&snapshot).expect("reload");
    assert_eq!(
        reopened.rule(&id),
        Some(rule),
        "the rule (all conditions + actions) survives reload"
    );
}

// ── the proactive mirror-rule offer (the missing-"off"-switch guard) ─────────

#[test]
fn the_mirror_rule_is_offered_for_an_add_on_enter_rule() {
    let sword = "1_9".to_string();
    // "flame ON when ENTERING FacingBoss"
    let on = RuleData {
        name: "flame on".into(),
        enabled: true,
        event: "StateEntered".into(),
        conditions: vec![Condition {
            entity: "1_8".into(),
            component: "QuestState".into(),
            field: "state".into(),
            op: CompareOp::Eq,
            value: FieldValue::Str("FacingBoss".into()),
        }],
        actions: vec![Action {
            action: "SetField".into(),
            entity: sword,
            component: "Flammable".into(),
            field: "lit".into(),
            value: FieldValue::Bool(true),
        }],
    };
    let mirror = propose_mirror(&on).expect("an enter+set-bool rule gets a mirror offer");
    assert_eq!(mirror.event, "StateExited", "the inverse fires on leaving");
    assert_eq!(mirror.actions.len(), 1);
    assert_eq!(
        mirror.actions[0].value,
        FieldValue::Bool(false),
        "the effect is turned OFF in the mirror"
    );
    assert!(mirror.name.contains("cleanup"));
    assert_eq!(
        mirror.conditions, on.conditions,
        "the state filter carries over"
    );
}

#[test]
fn no_mirror_is_offered_when_there_is_no_well_defined_inverse() {
    // Not an enter event → no pairing.
    let mut not_enter = RuleData {
        name: "on death".into(),
        enabled: true,
        event: "EnemyDied".into(),
        conditions: vec![],
        actions: vec![Action {
            action: "SetField".into(),
            entity: "1_1".into(),
            component: "Flammable".into(),
            field: "lit".into(),
            value: FieldValue::Bool(true),
        }],
    };
    assert!(
        propose_mirror(&not_enter).is_none(),
        "non-enter events have no mirror"
    );

    // Enter event, but the action isn't a boolean toggle → no well-defined inverse.
    not_enter.event = "ZoneEntered".into();
    not_enter.actions[0] = Action {
        action: "AdjustCounter".into(),
        entity: "1_1".into(),
        component: "KillCounter".into(),
        field: "count".into(),
        value: FieldValue::Integer(1),
    };
    assert!(
        propose_mirror(&not_enter).is_none(),
        "a non-boolean effect has no auto inverse (offer nothing rather than guess)"
    );
}

// ── merge: two concurrent rule edits converge without clobber (invariant 1) ──

#[test]
fn concurrent_rule_authoring_merges_without_clobber() {
    // Two peers fork a shared doc and each author a DIFFERENT rule; the merge keeps BOTH (mergeable
    // child slots), and every merged rule re-validates against the registry (invariant 3).
    let reg = registry();
    let mut base = engine();
    let sword = spawn(&mut base);
    let snapshot = base.snapshot();

    let mut peer_a = Engine::new(FlecsWorld::new(), 0xA);
    peer_a.merge(&snapshot).unwrap();
    let mut peer_b = Engine::new(FlecsWorld::new(), 0xB);
    peer_b.merge(&snapshot).unwrap();

    let id_a = peer_a.alloc_rule_id();
    let mut rule_a = test5_rule(&sword, &sword, &sword);
    rule_a.name = "rule A".into();
    peer_a
        .commit(
            "A",
            vec![Op::SetRule {
                id: id_a.clone(),
                rule: rule_a.clone(),
            }],
        )
        .unwrap();

    let id_b = peer_b.alloc_rule_id();
    let mut rule_b = test5_rule(&sword, &sword, &sword);
    rule_b.name = "rule B".into();
    peer_b
        .commit(
            "B",
            vec![Op::SetRule {
                id: id_b.clone(),
                rule: rule_b.clone(),
            }],
        )
        .unwrap();

    // Cross-merge both ways → both peers converge to the SAME two rules (no clobber).
    peer_a.merge(&peer_b.export_updates()).unwrap();
    peer_b.merge(&peer_a.export_updates()).unwrap();

    for peer in [&peer_a, &peer_b] {
        assert_eq!(peer.rules().len(), 2, "both rules survive the merge");
        assert_eq!(peer.rule(&id_a).map(|r| r.name), Some("rule A".into()));
        assert_eq!(peer.rule(&id_b).map(|r| r.name), Some("rule B".into()));
        // Merge-validation (invariant 3): every merged rule is still registry-valid.
        for (_, rule) in peer.rules() {
            validate_rule(&reg, &rule, |id| peer.entity_exists(id))
                .expect("a merged rule re-validates against the registry");
        }
    }
}
