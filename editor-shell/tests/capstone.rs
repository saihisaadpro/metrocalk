//! M12.6 — the **capstone integration backstop** (headless; gates-doc north-star #6, audio-free subset).
//!
//! `core/tests/rule_runtime.rs` + `editor-shell/tests/rule_runtime.rs` prove the Rules-in-Play runtime; this
//! proves the **whole slice compounds**: ONE sentence -> the assembled chain (a knight + a rusty sword made a
//! dynamic physics body + the conditional flame quest), driven through the **real** `Engine` (Loro + the
//! commit pipeline) and the **real** offline `DemoComposer` + `apply_composition` + `build_recording` —
//! exactly the path the packaged `.exe` drives. An integrated break (a piece that isn't an ordinary entity, a
//! Rule that won't fire, a chain Ctrl-Z can't unwind) fails it in CI, not only on the `.exe`. The boxes:
//!   1. **One sentence -> the assembled slice, every sub-piece an ORDINARY entity/component/rule** — no
//!      god-object (the knight + sword are plain entities; the counter/quest/flame are registry components on
//!      the sword; the rules + machine are top-level keyed data, not a privileged object).
//!   2. **The quest FIRES deterministically in Play** — 4 kills in the boss arena ignite the sword, as a
//!      PROJECTION over an in-memory `RuntimeState` (the authored Loro doc stays **bit-identical**; no undo
//!      entry) — ADR-021/ADR-034.
//!   3. **The live truth-state shows WHY** before the 4th kill ("debug by looking": ✅ state = FacingBoss,
//!      ❌ KillCounter = 3 of 4) — off the STABLE truth-state fields, never prose.
//!   4. **Ctrl-Z peels the WHOLE chain back** as ordinary transactions (compose -> make-dynamic -> sword ->
//!      knight), restoring the empty scene bit-for-bit.
//!   5. **It runs OFFLINE** — the composer is available with no network/model, and the applied edit is
//!      deterministic + replayable (same seed -> same decision history).

use metrocalk_core::compose::apply_composition;
use metrocalk_core::rule_runtime::RuleReplay;
use metrocalk_core::stdlib::{standard_actions, standard_components, standard_events};
use metrocalk_core::{Engine, EntityId, FieldValue, Op, Registry};
use metrocalk_ecs::FlecsWorld;

use metrocalk_editor_shell::capscene::{CapResolver, CapScene};
use metrocalk_editor_shell::compose_ai::{Composer, DemoComposer};
use metrocalk_editor_shell::physics_intent;
use metrocalk_editor_shell::play_rules::build_recording;
use std::time::Instant;

/// The one sentence the whole slice is composed from (the gates-doc test #6 scenario, audio-free).
const SENTENCE: &str =
    "a knight picks up a rusty sword that bursts into flame after 4 kills in the boss arena";

// ── fixtures ────────────────────────────────────────────────────────────────────────────────────────

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
    reg
}

fn engine() -> (Engine<FlecsWorld>, CapScene) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    engine.set_capability_resolver(Box::new(CapResolver::from_scene(&scene)));
    (engine, scene)
}

/// Create a named, positioned ordinary entity — ONE undoable transaction (so Ctrl-Z peels it as one step).
fn create_named(engine: &mut Engine<FlecsWorld>, name: &str, x: f64) -> EntityId {
    let id = engine.alloc_entity_id();
    engine
        .commit(
            "create-entity",
            vec![
                Op::CreateEntity { id, parent: None },
                Op::SetField {
                    entity: id,
                    component: "__meta__".into(),
                    field: "name".into(),
                    value: FieldValue::Str(name.into()),
                },
                Op::SetField {
                    entity: id,
                    component: "Transform".into(),
                    field: "x".into(),
                    value: FieldValue::Number(x),
                },
            ],
        )
        .expect("create-entity commits");
    id
}

/// The capstone assembly the `.exe` flow drives, as a sequence of **ordinary undoable transactions**:
///   1. create the knight (an ordinary entity)
///   2. create the rusty sword (an ordinary entity)
///   3. make the sword a dynamic physics body (the #3 physics-pickup leg — RigidBody + Collider, ONE tx)
///   4. ONE SENTENCE -> compose the flame quest onto the sword (the #5 leg — ONE undoable `ai-compose` tx)
///
/// Returns `(knight, sword)`. Four undoable commits in total, so Ctrl-Z x4 peels the whole chain.
fn assemble(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    reg: &Registry<FlecsWorld>,
) -> (EntityId, EntityId) {
    let knight = create_named(engine, "Knight", -1.0);
    let sword = create_named(engine, "Rusty Sword", 1.0);

    // The #3 physics-pickup leg (shipped M8.3): a dead model -> a correct dynamic body in one undoable tx.
    physics_intent::make_dynamic(engine, scene, sword, 2.0)
        .expect("the sword becomes a dynamic body");

    // The #5 flame-quest leg: ONE sentence -> the engine assembles the chain, via the OFFLINE demo composer
    // + the validated `apply_composition` pipeline (the same path the .exe + the MCP server drive).
    let composer = DemoComposer::new(true);
    let grammar = metrocalk_core::composition_grammar(&standard_components());
    let comp = composer
        .propose(SENTENCE, Some(&sword.to_loro_key()), &grammar)
        .expect("the sentence composes the flame quest offline");
    apply_composition(engine, reg, &comp)
        .expect("the composition validates + applies as one undoable tx");

    (knight, sword)
}

/// Replay the 4-kills-in-the-boss-arena scenario over a recording built from `engine`, returning the live
/// replay cursor (a projection — the authored doc is never touched). Fires `EnemyDied` x4 with a `ZoneEntered`
/// after the first kill so the QuestState machine reaches `FacingBoss` before the threshold.
fn play_the_quest(engine: &Engine<FlecsWorld>, reg: &Registry<FlecsWorld>) -> RuleReplay {
    let session = build_recording(engine, reg);
    let mut cur = RuleReplay::new(session.recording);
    cur.fire("EnemyDied", None); // kill 1 -> the hunt begins (Hunting -> ReadyForBoss)
    cur.fire("ZoneEntered", None); // reach the boss arena (ReadyForBoss -> FacingBoss)
    cur.fire("EnemyDied", None); // kill 2
    cur.fire("EnemyDied", None); // kill 3
    cur.fire("EnemyDied", None); // kill 4 -> ignite
    cur
}

// ── box 1: one sentence -> ordinary entities/components/rules, no god-object ───────────────────────────

#[test]
fn the_capstone_slice_is_assembled_from_ordinary_entities_components_and_rules() {
    let reg = registry();
    let (mut engine, scene) = engine();
    let (knight, sword) = assemble(&mut engine, &scene, &reg);

    // The knight + sword are ORDINARY entities (no privileged container holds the slice).
    assert!(engine.entity_exists(knight) && engine.entity_exists(sword));
    assert_eq!(engine.entity_count(), 2, "exactly the knight + the sword");

    // The quest's data lives as ORDINARY registry components on the sword (counter/quest-phase/flame) plus the
    // physics body (RigidBody/Collider) — every one a stdlib component, none a god-object.
    let comps = engine.components_of(sword);
    for needed in [
        "KillCounter",
        "QuestState",
        "Flammable",
        "RigidBody",
        "Collider",
    ] {
        assert!(
            comps.contains_key(needed),
            "the sword carries the ordinary {needed} component (got {:?})",
            comps.keys().collect::<Vec<_>>()
        );
    }

    // The rules + machine are TOP-LEVEL keyed data (the M12.1/M12.2 maps) — not a component on a privileged
    // object. Three ordinary rules (tally + ignite + the offered mirror) + one ordinary machine.
    assert_eq!(
        engine.rules().len(),
        3,
        "tally + ignite + cleanup, each a plain rule"
    );
    assert_eq!(
        engine.state_machines().len(),
        1,
        "one ordinary QuestState machine"
    );

    // Every rule references a REAL, ordinary entity (the sword) — no rule points at a phantom god-object.
    for (_, rule) in engine.rules() {
        for a in &rule.actions {
            let e = EntityId::from_loro_key(&a.entity).expect("a real entity key");
            assert!(
                engine.entity_exists(e),
                "rule action targets an existing entity"
            );
            assert_eq!(
                e, sword,
                "the quest's effects land on the ordinary sword entity"
            );
        }
        for c in &rule.conditions {
            let e = EntityId::from_loro_key(&c.entity).expect("a real entity key");
            assert!(
                engine.entity_exists(e),
                "rule condition reads an existing entity"
            );
        }
    }
}

// ── box 2: the quest fires deterministically in Play, as a projection (never the doc) ──────────────────

#[test]
fn the_quest_ignites_after_four_kills_as_a_projection_never_the_authored_doc() {
    let reg = registry();
    let (mut engine, scene) = engine();
    let (_knight, sword) = assemble(&mut engine, &scene, &reg);
    let sword_key = sword.to_loro_key();

    // Capture the authored doc + undo depth at Play-start.
    let doc_before = engine.snapshot();
    let can_undo_before = engine.can_undo();

    // PLAY: 4 kills in the arena -> the sword ignites in the RUNTIME STATE.
    let cur = play_the_quest(&engine, &reg);
    assert_eq!(
        cur.state().get(&sword_key, "Flammable", "lit"),
        Some(&FieldValue::Bool(true)),
        "the 4th kill ignited the sword (the projection saw the fire)"
    );
    assert_eq!(
        cur.state().get(&sword_key, "KillCounter", "count"),
        Some(&FieldValue::Integer(4)),
        "the counter tallied all four kills"
    );

    // ...but the AUTHORED document is BIT-IDENTICAL and gained no undo entry — running can't corrupt it.
    assert_eq!(
        engine.snapshot(),
        doc_before,
        "running the quest left the Loro document bit-identical (Play is a projection, ADR-021)"
    );
    assert_eq!(
        engine.get_field(sword, "Flammable", "lit"),
        Some(FieldValue::Bool(false)),
        "the authored sword never caught fire — the flame is a projection only"
    );
    assert_eq!(
        engine.can_undo(),
        can_undo_before,
        "a Rule firing in Play is NOT a Loro undo entry (ADR-034)"
    );
}

// ── box 3: the live truth-state shows WHY before the 4th kill (debug by looking) ───────────────────────

#[test]
fn the_live_truth_state_explains_the_block_before_the_fourth_kill() {
    let reg = registry();
    let (mut engine, scene) = engine();
    let (_knight, sword) = assemble(&mut engine, &scene, &reg);
    let sword_key = sword.to_loro_key();

    // Run up to the 3rd kill (in the arena), then read the live truth-state.
    let session = build_recording(&engine, &reg);
    let mut cur = RuleReplay::new(session.recording);
    cur.fire("EnemyDied", None); // kill 1
    cur.fire("ZoneEntered", None); // reach the arena
    cur.fire("EnemyDied", None); // kill 2
    cur.fire("EnemyDied", None); // kill 3

    let truth = cur.truth_state(&sword_key);
    let ignite = truth
        .rules
        .iter()
        .find(|r| r.rule == "r_ai_ignite")
        .expect("the ignite rule is in the sword's truth-state");
    assert!(
        !ignite.fires,
        "the sword must NOT be lit after only 3 kills"
    );

    // The boss-arena condition is satisfied; the kill-count condition is not (3 of 4) — the why, visible.
    let arena = ignite
        .conditions
        .iter()
        .find(|c| c.component == "QuestState")
        .expect("the FacingBoss condition is shown");
    assert!(
        arena.satisfied,
        "the knight has reached the boss arena (state = FacingBoss)"
    );

    let kills = ignite
        .conditions
        .iter()
        .find(|c| c.component == "KillCounter")
        .expect("the KillCounter condition is shown");
    assert!(!kills.satisfied, "the kill threshold is unmet at 3");
    assert_eq!(
        kills.actual,
        Some(FieldValue::Integer(3)),
        "actual count = 3"
    );
    assert_eq!(kills.expected, FieldValue::Integer(4), "threshold = 4");

    // The machine's current state is the boss arena (the gates-doc '✅ state = FacingBoss' display).
    let quest = truth
        .machines
        .iter()
        .find(|m| m.machine == "sm_ai_quest")
        .expect("the QuestState machine is in the truth-state");
    assert_eq!(quest.current, "FacingBoss");

    // The 4th kill flips it.
    cur.fire("EnemyDied", None);
    let after = cur.truth_state(&sword_key);
    assert!(
        after
            .rules
            .iter()
            .find(|r| r.rule == "r_ai_ignite")
            .is_some_and(|r| r.fires),
        "the 4th kill ignites the sword"
    );
}

// ── box 4: Ctrl-Z peels the whole chain back as ordinary transactions ─────────────────────────────────

#[test]
fn ctrl_z_peels_the_whole_chain_back_as_transactions() {
    let reg = registry();
    let (mut engine, scene) = engine();
    let (knight, sword) = assemble(&mut engine, &scene, &reg);

    assert_eq!(engine.entity_count(), 2);
    assert_eq!(engine.rules().len(), 3);
    assert_eq!(engine.state_machines().len(), 1);

    // Undo #1 — the compose (one `ai-compose` tx): the WHOLE quest unwinds at once (the chain is ordinary
    // transactions; the composed slice is not a privileged unit that escapes undo).
    assert!(engine.undo(), "undo the composition");
    assert!(engine.rules().is_empty(), "the composed rules are gone");
    assert!(
        engine.state_machines().is_empty(),
        "the composed machine is gone"
    );
    assert!(
        engine.get_field(sword, "Flammable", "lit").is_none()
            && engine.get_field(sword, "KillCounter", "count").is_none(),
        "the quest's seeded components are gone from the sword"
    );
    assert_eq!(
        engine.entity_count(),
        2,
        "the entities remain (only the quest was undone)"
    );

    // Undo #2 — make-dynamic: the sword loses its physics body.
    assert!(engine.undo(), "undo make-dynamic");
    assert!(
        engine.get_field(sword, "RigidBody", "kind").is_none(),
        "the sword is no longer a dynamic body"
    );
    // Undo #3 + #4 — the sword, then the knight.
    assert!(engine.undo(), "undo the sword");
    assert!(engine.undo(), "undo the knight");

    // The whole chain peeled back to an empty scene — each leg an ordinary, reversible transaction.
    // (Undo is operational, not a byte-snapshot rewind — ADR-002 F2 — so the *functional* state, not the raw
    // container structure, is what's restored; bit-exact restore is Stop's snapshot-merge job, asserted above.)
    assert_eq!(engine.entity_count(), 0, "the scene is empty again");
    assert!(
        !engine.entity_exists(sword) && !engine.entity_exists(knight),
        "both entities are gone"
    );
    assert!(engine.rules().is_empty() && engine.state_machines().is_empty());
    assert!(
        !engine.can_undo(),
        "the whole chain has been peeled back (nothing left to undo)"
    );
}

// ── box 5: deterministic + offline ────────────────────────────────────────────────────────────────────

#[test]
fn the_decision_history_replays_deterministically_and_offline() {
    let reg = registry();
    let (mut engine, scene) = engine();
    let _ = assemble(&mut engine, &scene, &reg);

    // Same authored scene -> same decision history (M8.1 determinism through the whole compose+play chain).
    let a = play_the_quest(&engine, &reg);
    let b = play_the_quest(&engine, &reg);
    assert_eq!(
        a.history_digest(),
        b.history_digest(),
        "the composed quest replays the same decision history every run"
    );

    // Offline by construction: the composer is available with NO network/model, and nothing in this whole
    // flow (compose -> apply -> build_recording -> replay) touches a socket — it is pure `/core` + the shell.
    assert!(
        DemoComposer::new(true).available(),
        "the demo composer composes with the model/network off (AI is a guest)"
    );
}

// ── deliverable 2 re-confirmed at the slice level: Stop restores the pre-Play edit state bit-exactly ───

#[test]
fn stop_restores_the_pre_play_edit_state_bit_exactly_for_the_whole_slice() {
    let reg = registry();
    let (mut engine, scene) = engine();
    let (_knight, sword) = assemble(&mut engine, &scene, &reg);

    // PLAY: snapshot the edit state (what Play captures), then run the quest (the runtime ignites).
    let snapshot = engine.snapshot();
    let _ = play_the_quest(&engine, &reg);

    // STOP: restore from the snapshot — a fresh engine + merge (exactly the Stop command, ADR-034).
    let mut restored = Engine::new(FlecsWorld::new(), 1);
    restored
        .merge(&snapshot)
        .expect("Stop restores the snapshot");

    assert_eq!(
        restored.entity_count(),
        2,
        "the authored knight + sword are restored"
    );
    assert_eq!(
        restored.rules().len(),
        3,
        "the authored quest rules are restored"
    );
    assert_eq!(
        restored.get_field(sword, "Flammable", "lit"),
        Some(FieldValue::Bool(false)),
        "the authored sword is unlit again — a Play-time ignite never leaked into the doc"
    );
}

// ── benchmark: the slice holds the frame budget through assemble + Play (principle 2) ──────────────────

fn percentiles(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    // Integer index math (no f64<->usize casts — keeps clippy's cast-precision lints quiet).
    (v[v.len() / 2], v[v.len() * 99 / 100])
}

/// The composed quest must hold the 16 ms frame budget through both legs: composing it (the one-shot
/// authoring tx) and ticking it under Play (the per-frame cost). Release-only (the `--release` timing
/// discipline). The min-spec rig number is owed-tracked (this box's profile is a partial signal); the
/// `::notice::`-friendly eprintln lets CI surface the number.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release-only timing measurement (discipline: --release for timing)"
)]
fn the_capstone_slice_holds_the_frame_budget() {
    let profile = std::env::var("MTK_PROFILE").unwrap_or_else(|_| "dev-box".to_string());
    let reg = registry();
    let (mut engine, scene) = engine();
    let (_knight, sword) = assemble(&mut engine, &scene, &reg);

    // (1) Compose-apply: re-apply the quest onto the sword (overwriting the same ids) — the authoring cost.
    let composer = DemoComposer::new(true);
    let grammar = metrocalk_core::composition_grammar(&standard_components());
    let comp = composer
        .propose(SENTENCE, Some(&sword.to_loro_key()), &grammar)
        .expect("composes");
    for _ in 0..50 {
        apply_composition(&mut engine, &reg, &comp).expect("warmup apply");
    }
    let mut apply_us = Vec::new();
    for _ in 0..400 {
        let t0 = Instant::now();
        apply_composition(&mut engine, &reg, &comp).expect("apply");
        apply_us.push(t0.elapsed().as_secs_f64() * 1e6);
    }
    let (apply_p50, apply_p99) = percentiles(apply_us);

    // (2) Per-Play-tick: build the recording once, then time a full 5-event quest play (4 kills + the arena),
    // reported per-tick — the per-frame cost the Play loop pays.
    let session = build_recording(&engine, &reg);
    let recording = session.recording;
    for _ in 0..50 {
        let mut cur = RuleReplay::new(recording.clone());
        for _ in 0..5 {
            cur.fire("EnemyDied", None);
        }
        std::hint::black_box(cur.state());
    }
    let mut play_us = Vec::new();
    for _ in 0..400 {
        let mut cur = RuleReplay::new(recording.clone());
        let t0 = Instant::now();
        cur.fire("EnemyDied", None);
        cur.fire("ZoneEntered", None);
        cur.fire("EnemyDied", None);
        cur.fire("EnemyDied", None);
        cur.fire("EnemyDied", None);
        play_us.push(t0.elapsed().as_secs_f64() * 1e6 / 5.0);
        std::hint::black_box(cur.state());
    }
    let (tick_p50, tick_p99) = percentiles(play_us);

    eprintln!(
        "::notice::[M12.6 capstone {profile}] compose-apply p50={apply_p50:.1}us p99={apply_p99:.1}us | \
         play-tick p50={tick_p50:.1}us p99={tick_p99:.1}us (budget 16000us)"
    );
    assert!(
        apply_p99 < 16_000.0,
        "compose-apply holds the budget: p99={apply_p99:.1}us"
    );
    assert!(
        tick_p99 < 16_000.0,
        "the Play tick holds the budget: p99={tick_p99:.1}us"
    );
}
