//! M12.5 (ADR-049) — **Rules in Play**, the shell-level non-destructive guard (test-first).
//!
//! `core/tests/rule_runtime.rs` proves the runtime + truth-state + determinism in pure `/core`; this guards
//! the **shell wiring** against the adversarial failures the ADR calls out, with a **real `Engine`** (Loro +
//! the commit pipeline) in the loop:
//!   1. **A Rule firing in Play is a render/runtime projection, NEVER the authored doc** (ADR-021/ADR-034):
//!      running the authored rules over the Play recording leaves the engine's Loro document **bit-identical**
//!      and adds **no undo entry** — the authored scene can't be corrupted by running.
//!   2. **Stop restores the pre-Play edit state bit-exactly** with rules in the loop (a Play-time counter
//!      change is wiped — the restore-from-snapshot guard, now with Rules running).
//!   3. **The recording built from the engine replays deterministically** (same scene → same decision
//!      history — the shell wiring preserves the M8.1 guarantee).
//!   4. **A non-deterministic plugin is flagged out** of the Play recording (deliverable 5).

use metrocalk_core::rule_runtime::{DecisionKind, RuleReplay};
use metrocalk_core::stdlib::{standard_actions, standard_components, standard_events};
use metrocalk_core::{
    Action, CompareOp, Condition, Engine, FieldValue, Op, PluginMeta, Registry, RuleData, RuleId,
    StateMachine, StateMachineId, Transition,
};
use metrocalk_ecs::FlecsWorld;

use metrocalk_editor_shell::play_rules::build_recording;

const SWORD: &str = "1_0";

// ── fixtures ──────────────────────────────────────────────────────────────────────────────────────

fn registry() -> Registry<FlecsWorld> {
    let mut reg = Registry::new(FlecsWorld::new());
    for c in standard_components() {
        let _ = reg.register(c);
    }
    for e in standard_events() {
        reg.register_event(e);
    }
    for a in standard_actions() {
        reg.register_action(a);
    }
    // The deterministic stdlib plugin + a synthetic non-deterministic one (the determinism gate).
    reg.register_plugin(PluginMeta::new("arrange", "deterministic arrange", true));
    reg.register_plugin(PluginMeta::new("chaos", "non-deterministic", false));
    reg
}

/// An engine seeded with the sword entity (loro key `1_0`) carrying the test-#5 components, plus the kill
/// tally rule, the ignite rule, and the quest machine — all authored through the real commit pipeline.
fn authored_scene() -> Engine<FlecsWorld> {
    let mut e = Engine::new(FlecsWorld::new(), 1);
    let sword = e.alloc_entity_id();
    assert_eq!(sword.to_loro_key(), SWORD, "deterministic first id");
    e.commit(
        "seed sword",
        vec![
            Op::CreateEntity {
                id: sword,
                parent: None,
            },
            Op::SetField {
                entity: sword,
                component: "KillCounter".into(),
                field: "count".into(),
                value: FieldValue::Integer(0),
            },
            Op::SetField {
                entity: sword,
                component: "Zone".into(),
                field: "current".into(),
                value: FieldValue::Str("BossArena".into()),
            },
            Op::SetField {
                entity: sword,
                component: "Flammable".into(),
                field: "lit".into(),
                value: FieldValue::Bool(false),
            },
            Op::SetField {
                entity: sword,
                component: "QuestState".into(),
                field: "state".into(),
                value: FieldValue::Str("FacingBoss".into()),
            },
        ],
    )
    .expect("seed");

    // r_count: When EnemyDied -> AdjustCounter count += 1
    e.commit(
        "author count",
        vec![Op::SetRule {
            id: RuleId::new("r_count"),
            rule: RuleData {
                name: "tally".into(),
                enabled: true,
                event: "EnemyDied".into(),
                conditions: vec![],
                actions: vec![Action {
                    action: "AdjustCounter".into(),
                    entity: SWORD.into(),
                    component: "KillCounter".into(),
                    field: "count".into(),
                    value: FieldValue::Integer(1),
                }],
            },
        }],
    )
    .expect("count rule");

    // r_ignite: When EnemyDied, If count>=4 AND zone==BossArena, Then Flammable.lit = true
    e.commit(
        "author ignite",
        vec![Op::SetRule {
            id: RuleId::new("r_ignite"),
            rule: RuleData {
                name: "ignite".into(),
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
                    action: "SetField".into(),
                    entity: SWORD.into(),
                    component: "Flammable".into(),
                    field: "lit".into(),
                    value: FieldValue::Bool(true),
                }],
            },
        }],
    )
    .expect("ignite rule");
    e.clear_history();
    e
}

/// Replay `n` EnemyDied kills over a recording built from `engine`.
fn play_kills(engine: &Engine<FlecsWorld>, reg: &Registry<FlecsWorld>, n: u64) -> RuleReplay {
    let session = build_recording(engine, reg);
    let mut rec = session.recording;
    for f in 0..n {
        rec.add_event(f, "EnemyDied", None);
    }
    let mut cur = RuleReplay::new(rec);
    cur.seek(n);
    cur
}

// ── deliverables 1 + 6: running Rules is a projection, NEVER the authored doc ───────────────────────

#[test]
fn a_rule_firing_in_play_is_a_projection_never_the_loro_doc_or_undo() {
    let reg = registry();
    let engine = authored_scene();

    // Capture the authored doc + undo state at Play-start.
    let doc_before = engine.snapshot();
    let can_undo_before = engine.can_undo();

    // PLAY: run 5 kills — the 4th ignites the sword in the RUNTIME STATE.
    let cur = play_kills(&engine, &reg, 5);
    assert_eq!(
        cur.state().get(SWORD, "Flammable", "lit"),
        Some(&FieldValue::Bool(true)),
        "the rule fired in Play (the projection saw the fire)"
    );

    // ...but the AUTHORED document is bit-identical, and no undo entry was created — running can't corrupt
    // the authored scene (ADR-021/034). The sword in the doc is still unlit.
    assert_eq!(
        engine.snapshot(),
        doc_before,
        "running the rules left the Loro document BIT-IDENTICAL (Play is render-only)"
    );
    assert_eq!(
        engine.get_field(
            metrocalk_core::EntityId::from_loro_key(SWORD).unwrap(),
            "Flammable",
            "lit"
        ),
        Some(FieldValue::Bool(false)),
        "the authored sword never caught fire — the fire is a projection only"
    );
    assert_eq!(
        engine.can_undo(),
        can_undo_before,
        "a Rule firing in Play is NOT a Loro undo entry (only authoring one is)"
    );
}

// ── deliverable 2: Stop restores the pre-Play edit state bit-exactly (rules in the loop) ─────────────

#[test]
fn stop_restores_pre_play_state_bit_exactly_with_rules_running() {
    let reg = registry();
    let mut engine = authored_scene();

    // PLAY: snapshot the edit state (what the Play command captures), then run the rules.
    let snapshot = engine.snapshot();
    let _ = play_kills(&engine, &reg, 5); // the runtime state ignites — the doc is untouched

    // A Play-time edit that LEAKS into the engine (in the live shell edits are disabled in Play; this models
    // the adversarial "a Play-time change survives Stop" case the snapshot-restore must wipe).
    let leaked = engine.alloc_rule_id();
    engine
        .commit(
            "leaked play-time rule",
            vec![Op::SetRule {
                id: leaked.clone(),
                rule: RuleData {
                    name: "leak".into(),
                    enabled: true,
                    event: "EnemyDied".into(),
                    conditions: vec![],
                    actions: vec![Action {
                        action: "SetField".into(),
                        entity: SWORD.into(),
                        component: "Flammable".into(),
                        field: "lit".into(),
                        value: FieldValue::Bool(true),
                    }],
                },
            }],
        )
        .expect("leak commits");
    assert!(
        engine.rule(&leaked).is_some(),
        "the leak is present pre-Stop"
    );

    // STOP: restore from the snapshot — a fresh engine + merge (exactly the Stop command, ADR-034).
    let mut restored = Engine::new(FlecsWorld::new(), 1);
    restored
        .merge(&snapshot)
        .expect("Stop restores the snapshot");

    assert!(
        restored.rule(&leaked).is_none(),
        "the Play-time leak is WIPED — Stop restores the pre-Play edit state bit-exactly"
    );
    assert_eq!(
        restored.rules().len(),
        2,
        "the authored rules (count + ignite) are restored, and only those"
    );
    assert_eq!(
        restored.get_field(
            metrocalk_core::EntityId::from_loro_key(SWORD).unwrap(),
            "Flammable",
            "lit"
        ),
        Some(FieldValue::Bool(false)),
        "the authored sword is unlit again (a Play-time counter/fire change is gone)"
    );
}

// ── deliverable 4/5: deterministic-from-engine + non-deterministic-plugin flag ───────────────────────

#[test]
fn the_recording_built_from_the_engine_replays_deterministically() {
    let reg = registry();
    let engine = authored_scene();
    let a = play_kills(&engine, &reg, 6);
    let b = play_kills(&engine, &reg, 6);
    assert_eq!(
        a.history_digest(),
        b.history_digest(),
        "the same authored scene replays the same decision history (M8.1 through the shell wiring)"
    );
    // And the decision history is faithful: the ignite FieldSet is recorded.
    assert!(
        a.history().iter().any(
            |d| matches!(&d.kind, DecisionKind::FieldSet { component, field, .. }
                if component == "Flammable" && field == "lit")
        ),
        "the ignite is in the decision history"
    );
}

#[test]
fn a_machine_drives_the_quest_in_play() {
    // A state machine authored on the engine ticks in Play (a transition IS a Rule). Hunting -> ReadyForBoss
    // on the first kill — the quest advances as a projection.
    let reg = registry();
    let mut engine = authored_scene();
    // Reset the quest to Hunting + author the machine.
    let sword = metrocalk_core::EntityId::from_loro_key(SWORD).unwrap();
    engine
        .commit(
            "reset quest",
            vec![Op::SetField {
                entity: sword,
                component: "QuestState".into(),
                field: "state".into(),
                value: FieldValue::Str("Hunting".into()),
            }],
        )
        .unwrap();
    let mut t = Transition {
        id: "t1".into(),
        from: "Hunting".into(),
        to: "ReadyForBoss".into(),
        rule: RuleData {
            name: "-> ready".into(),
            enabled: true,
            event: "EnemyDied".into(),
            conditions: vec![],
            actions: vec![Action {
                action: "SetField".into(),
                entity: SWORD.into(),
                component: "QuestState".into(),
                field: "state".into(),
                value: FieldValue::Str("ReadyForBoss".into()),
            }],
        },
    };
    t.from = "Hunting".into();
    engine
        .commit(
            "author machine",
            vec![Op::SetStateMachine {
                id: StateMachineId::new("sm_quest"),
                sm: StateMachine {
                    name: "quest".into(),
                    entity: SWORD.into(),
                    component: "QuestState".into(),
                    field: "state".into(),
                    states: vec!["Hunting".into(), "ReadyForBoss".into()],
                    initial: "Hunting".into(),
                    transitions: vec![t],
                },
            }],
        )
        .unwrap();

    let cur = play_kills(&engine, &reg, 1);
    let truth = cur.truth_state(SWORD);
    let machine = truth
        .machines
        .iter()
        .find(|m| m.machine == "sm_quest")
        .unwrap();
    assert_eq!(
        machine.current, "ReadyForBoss",
        "the machine advanced in Play (projection); the authored doc stays at Hunting"
    );
    // The authored doc is untouched (still Hunting).
    assert_eq!(
        engine.get_field(sword, "QuestState", "state"),
        Some(FieldValue::Str("Hunting".into())),
        "the authored quest state is unchanged by running"
    );
}

#[test]
fn a_non_deterministic_plugin_rule_is_flagged_out_of_the_play_recording() {
    let reg = registry();
    let mut engine = authored_scene();
    // Author a rule whose RunPlugin action targets the non-deterministic `chaos` plugin.
    engine
        .commit(
            "author chaos rule",
            vec![Op::SetRule {
                id: RuleId::new("r_chaos"),
                rule: RuleData {
                    name: "chaos".into(),
                    enabled: true,
                    event: "EnemyDied".into(),
                    conditions: vec![],
                    actions: vec![Action {
                        action: "RunPlugin".into(),
                        entity: SWORD.into(),
                        component: "chaos".into(), // the non-deterministic plugin
                        field: "input".into(),
                        value: FieldValue::Str("{}".into()),
                    }],
                },
            }],
        )
        .expect("chaos rule authors (it's registry-valid; determinism is a Play-time gate, not authoring)");

    let session = build_recording(&engine, &reg);
    assert!(
        session.flagged.iter().any(|f| f.rule == "r_chaos"),
        "the non-deterministic-plugin rule is flagged out of the Play recording (deliverable 5)"
    );
    assert!(
        !session
            .recording
            .rules
            .iter()
            .any(|(id, _)| id.as_str() == "r_chaos"),
        "the flagged rule is NOT in the deterministic recording — it can't poison the replay"
    );
    // The deterministic authored rules are still in.
    assert!(session
        .recording
        .rules
        .iter()
        .any(|(id, _)| id.as_str() == "r_ignite"));
}
