//! M12.2 (ADR-046) — state machines survive close→reopen via the `AuthorStateMachine`/`RemoveStateMachine`
//! replay records (the shell session-restore path), mirroring the rules replay tests. The state-machine
//! data model + registry validation + reachability/order are tested in `core/tests/state_machine.rs`; this
//! guards the SHELL persistence wiring (the Record round-trip).

use std::path::PathBuf;

use metrocalk_core::state_machine::{StateMachine, Transition};
use metrocalk_core::{Condition, Engine, Op, StateMachineId};
use metrocalk_ecs::FlecsWorld;

use metrocalk_editor_shell::capscene::{self, CapScene};
use metrocalk_editor_shell::persist::{Log, Record};
use metrocalk_editor_shell::MeshCatalog;

const N: usize = 50;

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("mtk-test-{name}.jsonl"))
}

fn seeded() -> (Engine<FlecsWorld>, CapScene) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    capscene::seed(&mut engine, &scene, N).expect("seed");
    engine.clear_history();
    (engine, scene)
}

/// The canonical test-#5 machine on the seeded entity `1_0`: Hunting -> ReadyForBoss -> FacingBoss, each
/// transition a registry-fed Rule whose action enters the target state.
fn demo_machine() -> StateMachine {
    let q = "1_0".to_string();
    let mut sm = StateMachine {
        name: "quest".into(),
        entity: q.clone(),
        component: "QuestState".into(),
        field: "state".into(),
        states: vec!["Hunting".into(), "ReadyForBoss".into(), "FacingBoss".into()],
        initial: "Hunting".into(),
        transitions: vec![],
    };
    let mk =
        |sm: &StateMachine, id: &str, from: &str, to: &str, event: &str, conds: Vec<Condition>| {
            Transition {
                id: id.into(),
                from: from.into(),
                to: to.into(),
                rule: metrocalk_core::RuleData {
                    name: format!("{from} -> {to}"),
                    enabled: true,
                    event: event.into(),
                    conditions: conds,
                    actions: vec![sm.enter_action(to)],
                },
            }
        };
    sm.transitions = vec![
        mk(
            &sm,
            "t1",
            "Hunting",
            "ReadyForBoss",
            "EnemyDied",
            vec![Condition {
                entity: q.clone(),
                component: "KillCounter".into(),
                field: "count".into(),
                op: metrocalk_core::CompareOp::Ge,
                value: metrocalk_core::FieldValue::Integer(4),
            }],
        ),
        mk(
            &sm,
            "t2",
            "ReadyForBoss",
            "FacingBoss",
            "ZoneEntered",
            vec![],
        ),
    ];
    sm
}

#[test]
fn a_state_machine_survives_close_then_reopen_via_replay() {
    let log = Log::open(tmp("statemachine"), capscene::fingerprint(N));
    log.clear();

    // run A: author a machine, persist its record.
    let (mut a, _scene_a) = seeded();
    let id = a.alloc_state_machine_id();
    let sm = demo_machine();
    a.commit(
        "author state machine",
        vec![Op::SetStateMachine {
            id: id.clone(),
            sm: sm.clone(),
        }],
    )
    .expect("author");
    log.append(&Record::AuthorStateMachine {
        id: id.as_str().to_string(),
        machine: sm.clone(),
    });
    assert_eq!(a.state_machines().len(), 1);
    drop(a); // close

    // run B: fresh deterministic seed + replay (a true close→reopen).
    let (mut b, scene) = seeded();
    let (applied, _skipped) = log.replay(&mut b, &scene, &MeshCatalog::new());
    assert_eq!(applied, 1, "the AuthorStateMachine record replayed");
    let restored = b
        .state_machine(&StateMachineId::new(id.as_str()))
        .expect("the machine is restored after reopen");
    assert_eq!(restored, sm, "every state + transition survives the reload");
    // The current-state seam reads as the initial state after a fresh reopen (M12.5 will tick it).
    assert_eq!(
        b.state_machine_current(&StateMachineId::new(id.as_str())),
        Some("Hunting".into())
    );
    log.clear();
}

#[test]
fn removing_a_state_machine_persists_across_reopen() {
    let log = Log::open(tmp("statemachine-remove"), capscene::fingerprint(N));
    log.clear();

    let (mut a, _s) = seeded();
    let id = a.alloc_state_machine_id();
    let sm = demo_machine();
    a.commit(
        "author",
        vec![Op::SetStateMachine {
            id: id.clone(),
            sm: sm.clone(),
        }],
    )
    .unwrap();
    log.append(&Record::AuthorStateMachine {
        id: id.as_str().to_string(),
        machine: sm,
    });
    a.commit("remove", vec![Op::RemoveStateMachine { id: id.clone() }])
        .unwrap();
    log.append(&Record::RemoveStateMachine {
        id: id.as_str().to_string(),
    });
    assert!(a.state_machines().is_empty());
    drop(a);

    // Replay author-then-remove → the machine does NOT come back (the removal is durable).
    let (mut b, scene) = seeded();
    let (applied, _skipped) = log.replay(&mut b, &scene, &MeshCatalog::new());
    assert_eq!(applied, 2, "both the author and the remove replayed");
    assert!(
        b.state_machines().is_empty(),
        "a removed machine stays removed after reopen"
    );
    log.clear();
}
