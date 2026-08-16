//! M12.5 (ADR-049) — **Rules in Play + the live truth-state debugger**, headless proofs.
//!
//! The runtime + debug half of test #5: authored Rules/machines **run** as a projection, the live
//! truth-state is **visible** on click, the decision history is **time-travelable** over the M8.4 channel,
//! and the run is **deterministic** (same seed/inputs → same history). Every property here is pure `/core`
//! (no Loro/Flecs, no Engine) — running Rules mutates a [`RuntimeState`], never the authored doc.
//!
//! The canonical scenario (the rusty sword): a `KillCounter` rule + the ignite rule + a `QuestState` machine
//! Hunting -> ReadyForBoss -> FacingBoss. Kill 3 enemies, walk to the arena -> the sword does **not** burn;
//! click it -> "✅ `state = FacingBoss`, ❌ `KillCounter = 3 of 4`"; the 4th kill ignites it.

use metrocalk_core::rule_runtime::{
    partition_deterministic, DecisionKind, RuleRecording, RuleReplay, RuntimeState,
};
use metrocalk_core::rules::{Action, CompareOp, Condition, RuleData, RuleId};
use metrocalk_core::state_machine::{StateMachine, StateMachineId, Transition};
use metrocalk_core::FieldValue;

const SWORD: &str = "1_0";

// ── fixtures ──────────────────────────────────────────────────────────────────────────────────────

/// `When EnemyDied -> AdjustCounter sword.KillCounter.count += 1` (the kill tally — itself a rule).
fn count_rule() -> (RuleId, RuleData) {
    (
        RuleId::new("r_count"),
        RuleData {
            name: "tally kills".into(),
            enabled: true,
            event: "EnemyDied".into(),
            conditions: vec![],
            any_of: vec![],
            subject: None,
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

/// `When EnemyDied, If KillCounter.count >= 4 AND Zone.current == BossArena, Then Flammable.lit = true`.
/// Sorts AFTER `r_count`, so within one EnemyDied tick the counter increments first and this rule sees it.
fn ignite_rule() -> (RuleId, RuleData) {
    (
        RuleId::new("r_ignite"),
        RuleData {
            name: "rusty sword ignites".into(),
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
            any_of: vec![],
            subject: None,
            actions: vec![Action {
                action: "SetField".into(),
                entity: SWORD.into(),
                component: "Flammable".into(),
                field: "lit".into(),
                value: FieldValue::Bool(true),
            }],
        },
    )
}

/// The `QuestState` machine on the sword: Hunting -> ReadyForBoss (on the first EnemyDied) -> FacingBoss
/// (on entering the arena). A transition IS an M12.1 Rule carrying the enter-state action.
fn quest_machine() -> (StateMachineId, StateMachine) {
    let enter = |to: &str, event: &str, id: &str| Transition {
        id: id.into(),
        from: String::new(), // set below
        to: to.into(),
        rule: RuleData {
            name: format!("-> {to}"),
            enabled: true,
            event: event.into(),
            conditions: vec![],
            any_of: vec![],
            subject: None,
            actions: vec![Action {
                action: "SetField".into(),
                entity: SWORD.into(),
                component: "QuestState".into(),
                field: "state".into(),
                value: FieldValue::Str(to.into()),
            }],
        },
    };
    let mut t1 = enter("ReadyForBoss", "EnemyDied", "t1");
    t1.from = "Hunting".into();
    let mut t2 = enter("FacingBoss", "ZoneEntered", "t2");
    t2.from = "ReadyForBoss".into();
    (
        StateMachineId::new("sm_quest"),
        StateMachine {
            name: "quest".into(),
            entity: SWORD.into(),
            component: "QuestState".into(),
            field: "state".into(),
            states: vec!["Hunting".into(), "ReadyForBoss".into(), "FacingBoss".into()],
            initial: "Hunting".into(),
            transitions: vec![t1, t2],
        },
    )
}

/// The seeded frame-0 scene: a fresh kill counter, the player already in the boss arena, not yet on fire.
fn initial_state() -> RuntimeState {
    let mut s = RuntimeState::new();
    s.set(SWORD, "KillCounter", "count", FieldValue::Integer(0));
    s.set(
        SWORD,
        "Zone",
        "current",
        FieldValue::Str("BossArena".into()),
    );
    s.set(SWORD, "Flammable", "lit", FieldValue::Bool(false));
    s
}

/// The full test-#5 recording: both rules + the quest machine + the kill/zone event log.
/// Events: f0 kill (count 1, Hunting->ReadyForBoss), f1 enter arena (ReadyForBoss->FacingBoss),
/// f2 kill (2), f3 kill (3), f4 kill (4 -> ignites).
fn test5_recording() -> RuleRecording {
    let mut rec = RuleRecording::new(
        initial_state(),
        vec![count_rule(), ignite_rule()],
        vec![quest_machine()],
    );
    rec.add_event(0, "EnemyDied", None);
    rec.add_event(1, "ZoneEntered", Some(SWORD.into()));
    rec.add_event(2, "EnemyDied", None);
    rec.add_event(3, "EnemyDied", None);
    rec.add_event(4, "EnemyDied", None);
    rec
}

// ── deliverable 3: the live truth-state debugger ("debug by looking") ───────────────────────────────

#[test]
fn click_the_sword_after_three_kills_shows_the_live_truth_state() {
    // Kill 3 enemies + walk to the arena, then PAUSE (advance through frames 0..=3, cursor at frame 4).
    let mut cur = RuleReplay::new(test5_recording());
    cur.seek(4);

    let truth = cur.truth_state(SWORD);

    // The machine is visible at its live state: ✅ state = FacingBoss (assert the STABLE field, not copy).
    let machine = truth
        .machines
        .iter()
        .find(|m| m.machine == "sm_quest")
        .expect("the quest machine drives the sword");
    assert_eq!(machine.current, "FacingBoss", "✅ state = FacingBoss");
    assert_eq!(machine.display, "state = FacingBoss");

    // The ignite rule is shown NOT firing, with the blocking condition made visible: ❌ KillCounter 3 of 4.
    let ignite = truth
        .rules
        .iter()
        .find(|r| r.rule == "r_ignite")
        .expect("the ignite rule references the sword");
    assert!(!ignite.fires, "the sword does not burn yet");
    let counter_cond = &ignite.conditions[0];
    assert!(!counter_cond.satisfied, "❌ the kill threshold is unmet");
    assert_eq!(counter_cond.actual, Some(FieldValue::Integer(3)), "is 3");
    assert_eq!(counter_cond.expected, FieldValue::Integer(4), "of 4");
    assert_eq!(
        counter_cond.display, "KillCounter = 3 of 4",
        "the human overlay copy matches test #5"
    );
    // The zone condition IS satisfied (the player walked to the arena) — the why is per-condition, not binary.
    assert!(ignite.conditions[1].satisfied, "✅ in the boss arena");
}

#[test]
fn the_fourth_kill_ignites_the_sword_as_a_runtime_projection() {
    let rec = test5_recording();
    let pristine_initial = rec.initial.clone();
    let mut cur = RuleReplay::new(rec);
    cur.seek(5); // process the 4th kill (frame 4)

    // The effect landed in the RUNTIME STATE (the projection) — the sword is on fire.
    assert_eq!(
        cur.state().get(SWORD, "Flammable", "lit"),
        Some(&FieldValue::Bool(true)),
        "the 4th kill ignites the sword"
    );
    assert_eq!(
        cur.state().get(SWORD, "KillCounter", "count"),
        Some(&FieldValue::Integer(4)),
        "the counter reached 4"
    );
    // ...and the decision history records the ignite as a FieldSet on the right frame.
    let lit_frame = cur.history().iter().find_map(|d| match &d.kind {
        DecisionKind::FieldSet {
            component, field, ..
        } if component == "Flammable" && field == "lit" => Some(d.frame),
        _ => None,
    });
    assert_eq!(lit_frame, Some(4), "ignited exactly on the 4th-kill frame");

    // ADR-021/034: the recording's seeded INITIAL is untouched — the run never wrote back to its own source
    // (the rebuild stays pristine, so a Stop/scrub restores the pre-Play state bit-exactly).
    assert_eq!(
        cur.recording().initial,
        pristine_initial,
        "running the rules never mutated the authored/initial state"
    );
    assert_eq!(
        pristine_initial.get(SWORD, "Flammable", "lit"),
        Some(&FieldValue::Bool(false)),
        "the authored sword is still unlit"
    );
}

#[test]
fn explain_rule_narrates_the_blocked_rule_faithfully() {
    let mut cur = RuleReplay::new(test5_recording());
    cur.seek(4); // 3 kills

    let why = cur.explain_rule("r_ignite").expect("the rule exists");
    // A faithful, plain-language reason naming the real blocker — shown, not logged.
    assert!(why.contains("blocked"), "{why}");
    assert!(
        why.contains("KillCounter") && why.contains("count"),
        "{why}"
    );
    assert!(
        why.contains("is 3") && why.contains('4'),
        "names the actual (3) and the needed threshold (4): {why}"
    );

    // Once it fires, the explanation flips to ready.
    cur.seek(5);
    let ready = cur.explain_rule("r_ignite").unwrap();
    assert!(ready.contains("ready"), "{ready}");

    // An unknown rule id is None, not a panic.
    assert!(cur.explain_rule("r_nope").is_none());
}

#[test]
fn reading_the_truth_state_never_perturbs_the_run() {
    // The adversarial "the overlay perturbs the run it inspects" guard (the M8.4 non-mutating discipline):
    // querying the truth-state / explain every frame must not change the decision history vs never querying.
    let mut overlay_on = RuleReplay::new(test5_recording());
    let mut overlay_off = RuleReplay::new(test5_recording());
    for _ in 0..5 {
        overlay_on.advance();
        let _ = overlay_on.truth_state(SWORD); // overlay open: read every frame
        let _ = overlay_on.explain_rule("r_ignite");
        overlay_off.advance(); // overlay closed: never read
    }
    assert_eq!(
        overlay_on.history_digest(),
        overlay_off.history_digest(),
        "reading the truth-state must NOT perturb the decision history"
    );
    assert_eq!(overlay_on.state().digest(), overlay_off.state().digest());
}

// ── deliverable 4: time-travel the decision history (over the M8.4 channel) ──────────────────────────

#[test]
fn the_decision_history_replays_bit_identically() {
    // M8.1/M8.4: same seed + inputs -> the same decision history. A fresh replay seeked to N reproduces a
    // continuous run to N, bit-for-bit (the determinism gate — a bug is repeatable, not a ghost).
    let mut cont = RuleReplay::new(test5_recording());
    for _ in 0..5 {
        cont.advance();
    }
    let continuous_history = cont.history_digest();
    let continuous_state = cont.state().digest();

    let mut replay = RuleReplay::new(test5_recording());
    replay.seek(5);
    assert_eq!(
        continuous_history,
        replay.history_digest(),
        "replay-to-N reproduces the continuous decision history (M8.1 P2 through logic)"
    );
    assert_eq!(continuous_state, replay.state().digest());
}

#[test]
fn resume_from_scrub_equals_continuous_across_cycles() {
    // The M8.4 P3 guarantee, now for the decision history: scrub back (rebuild-from-recording), resume
    // forward, and the end state must equal the continuous reference EVERY cycle — divergence shows up over
    // repeated rewind/replay, so a single lucky match isn't proof.
    let mut reference = RuleReplay::new(test5_recording());
    reference.seek(5);
    let reference_state = reference.state().digest();
    let reference_history = reference.history_digest();

    let mut cursor = RuleReplay::new(test5_recording());
    for cycle in 0..3 {
        cursor.seek(2); // scrub back to before the boss arena
        assert_eq!(cursor.frame(), 2);
        cursor.seek(5); // resume forward
        assert_eq!(
            reference_state,
            cursor.state().digest(),
            "resume-from-scrub equals the continuous run on cycle {cycle} (P3 — deterministic-by-rebuild)"
        );
        assert_eq!(
            reference_history,
            cursor.history_digest(),
            "history too, cycle {cycle}"
        );
    }
}

#[test]
fn live_fired_events_are_recorded_and_stay_time_travelable() {
    // The interactive-Play input channel: fire events one at a time (as the player kills enemies), and the
    // decision history is still rebuildable by scrub — a live event is RECORDED, not just dispatched.
    let rec = RuleRecording::new(
        initial_state(),
        vec![count_rule(), ignite_rule()],
        vec![quest_machine()],
    );
    let mut cur = RuleReplay::new(rec);
    cur.fire("EnemyDied", None); // frame 0 -> count 1
    cur.fire("ZoneEntered", Some(SWORD.into())); // frame 1 -> FacingBoss
    cur.fire("EnemyDied", None); // frame 2 -> count 2
    cur.fire("EnemyDied", None); // frame 3 -> count 3
    assert_eq!(cur.frame(), 4);
    assert_eq!(
        cur.state().get(SWORD, "KillCounter", "count"),
        Some(&FieldValue::Integer(3))
    );

    // Scrub back over the live-recorded history and resume — deterministic-by-rebuild, exactly like a
    // pre-recorded run (the live events were appended to the recording, so the rewind replays them).
    let head = cur.history_digest();
    cur.seek(2);
    assert_eq!(
        cur.state().get(SWORD, "KillCounter", "count"),
        Some(&FieldValue::Integer(1)),
        "scrubbed back to count = 1 over the live-recorded timeline"
    );
    cur.seek(4);
    assert_eq!(
        cur.history_digest(),
        head,
        "resume reproduces the live history bit-for-bit"
    );
}

#[test]
fn scrub_back_pinpoints_when_the_counter_incremented() {
    // "Scrub backward to watch exactly when the counter incremented" (test #5 box 4). Each CounterChanged is
    // frame-stamped, so the history answers WHEN the counter reached 3 — debug by time-travel.
    let mut cur = RuleReplay::new(test5_recording());
    cur.seek(5);

    let reached_three = cur.history().iter().find_map(|d| match &d.kind {
        DecisionKind::CounterChanged { to, .. } if *to == FieldValue::Integer(3) => Some(d.frame),
        _ => None,
    });
    assert_eq!(reached_three, Some(3), "the counter hit 3 on frame 3");

    // Scrubbing the cursor to that frame reproduces exactly that moment (count == 3, not yet 4).
    cur.seek(4); // cursor at frame 4 == state after processing frames 0..=3
    assert_eq!(
        cur.state().get(SWORD, "KillCounter", "count"),
        Some(&FieldValue::Integer(3)),
        "scrubbed back to count = 3"
    );
}

// ── deliverable 5: the determinism guard (non-deterministic plugin excluded) ─────────────────────────

#[test]
fn a_non_deterministic_plugin_is_flagged_out_of_the_replay_path() {
    let run_plugin = |id: &str, plugin: &str| {
        (
            RuleId::new(id),
            RuleData {
                name: format!("run {plugin}"),
                enabled: true,
                event: "EnemyDied".into(),
                conditions: vec![],
                any_of: vec![],
                subject: None,
                actions: vec![Action {
                    action: "RunPlugin".into(),
                    entity: SWORD.into(),
                    component: plugin.into(), // the RunPlugin slot names the plugin
                    field: "input".into(),
                    value: FieldValue::Str("{}".into()),
                }],
            },
        )
    };
    let rules = vec![
        run_plugin("r_arrange", "arrange"), // deterministic
        run_plugin("r_chaos", "chaos"),     // non-deterministic
    ];

    // The registry's determinism flag (M12.3 PluginMeta.deterministic) gates the lockstep path.
    let plugin_kind = |name: &str| match name {
        "arrange" => Some(true),
        "chaos" => Some(false),
        _ => None,
    };
    let (kept, flagged) = partition_deterministic(&rules, plugin_kind);

    assert_eq!(kept.len(), 1, "only the deterministic-plugin rule is kept");
    assert_eq!(kept[0].0.as_str(), "r_arrange");
    assert_eq!(
        flagged.len(),
        1,
        "the non-deterministic-plugin rule is flagged out"
    );
    assert_eq!(flagged[0].rule, "r_chaos");
    assert!(
        flagged[0].reason.contains("chaos") && flagged[0].reason.contains("deterministic"),
        "the exclusion is explained, not silent: {}",
        flagged[0].reason
    );

    // The deterministic plugin runs in the lockstep path: its invocation is recorded deterministically.
    let mut rec = RuleRecording::new(RuntimeState::new(), kept, vec![]);
    rec.add_event(0, "EnemyDied", None);
    let mut cur = RuleReplay::new(rec);
    cur.advance();
    let invoked = cur
        .history()
        .iter()
        .any(|d| matches!(&d.kind, DecisionKind::PluginInvoked { plugin } if plugin == "arrange"));
    assert!(
        invoked,
        "the deterministic plugin's invocation is in the decision history"
    );
}

// ── $subject — one rule covers every entity of a kind (the gameplay-roles substrate) ─────────────

/// `When Touched, If $subject.GameRole.active == true, Then $subject.GameRole.active = false AND
/// score.KillCounter.count += 1` — ONE rule; the fired event's subject decides which pickup it acts on.
#[test]
fn subject_sentinel_resolves_per_event_and_ignores_subjectless_fires() {
    const SCORE: &str = "1_9";
    const PICKUP_A: &str = "1_5";
    const PICKUP_B: &str = "1_6";
    let pickup_rule = (
        RuleId::new("r_pickup"),
        RuleData {
            name: "collect on touch".into(),
            enabled: true,
            event: "Touched".into(),
            conditions: vec![Condition {
                entity: "$subject".into(),
                component: "GameRole".into(),
                field: "active".into(),
                op: CompareOp::Eq,
                value: FieldValue::Bool(true),
            }],
            any_of: vec![],
            subject: None,
            actions: vec![
                Action {
                    action: "SetField".into(),
                    entity: "$subject".into(),
                    component: "GameRole".into(),
                    field: "active".into(),
                    value: FieldValue::Bool(false),
                },
                Action {
                    action: "AdjustCounter".into(),
                    entity: SCORE.into(),
                    component: "KillCounter".into(),
                    field: "count".into(),
                    value: FieldValue::Integer(1),
                },
            ],
        },
    );
    let mut seeded = RuntimeState::new();
    seeded.set(PICKUP_A, "GameRole", "active", FieldValue::Bool(true));
    seeded.set(PICKUP_B, "GameRole", "active", FieldValue::Bool(true));
    let rec = RuleRecording::new(seeded, vec![pickup_rule], vec![]);
    let mut replay = RuleReplay::new(rec);

    // Touch A: ONLY A is collected; B stays live; the score is 1.
    replay.fire("Touched", Some(PICKUP_A.into()));
    assert_eq!(
        replay.state().get(PICKUP_A, "GameRole", "active"),
        Some(&FieldValue::Bool(false)),
        "the touched pickup is collected"
    );
    assert_eq!(
        replay.state().get(PICKUP_B, "GameRole", "active"),
        Some(&FieldValue::Bool(true)),
        "the OTHER pickup is untouched — $subject scopes the rule to the event's entity"
    );
    assert_eq!(
        replay.state().get(SCORE, "KillCounter", "count"),
        Some(&FieldValue::Integer(1))
    );

    // Touch A again: the active==true guard fails — no double score.
    replay.fire("Touched", Some(PICKUP_A.into()));
    assert_eq!(
        replay.state().get(SCORE, "KillCounter", "count"),
        Some(&FieldValue::Integer(1)),
        "an already-collected pickup scores nothing"
    );

    // A subjectless fire: the $subject condition cannot resolve — nothing happens, nothing panics.
    replay.fire("Touched", None);
    assert_eq!(
        replay.state().get(SCORE, "KillCounter", "count"),
        Some(&FieldValue::Integer(1)),
        "an event with no subject is a no-op for a $subject rule"
    );

    // Touch B: the same ONE rule handles the second pickup.
    replay.fire("Touched", Some(PICKUP_B.into()));
    assert_eq!(
        replay.state().get(SCORE, "KillCounter", "count"),
        Some(&FieldValue::Integer(2))
    );
    // The decision history records the RESOLVED entities, not the sentinel.
    let resolved_sets: Vec<&str> = replay
        .history()
        .iter()
        .filter_map(|d| match &d.kind {
            DecisionKind::FieldSet { entity, .. } => Some(entity.as_str()),
            _ => None,
        })
        .collect();
    assert!(resolved_sets.contains(&PICKUP_A) && resolved_sets.contains(&PICKUP_B));
    assert!(
        !resolved_sets.contains(&"$subject"),
        "history never shows the raw sentinel"
    );
}

// ── conditionals: the OR group, the subject pin, and the near-miss record ────────────────────────

/// A gate rule shaped like the shipped pickup rule: `When Touched, If count >= n, Then lit = true`.
fn gate_rule(n: i64) -> (RuleId, RuleData) {
    (
        RuleId::new("r_gate"),
        RuleData {
            name: "open the vault".into(),
            enabled: true,
            event: "Touched".into(),
            conditions: vec![Condition {
                entity: SWORD.into(),
                component: "KillCounter".into(),
                field: "count".into(),
                op: CompareOp::Ge,
                value: FieldValue::Integer(n),
            }],
            any_of: vec![],
            subject: None,
            actions: vec![Action {
                action: "SetField".into(),
                entity: SWORD.into(),
                component: "Flammable".into(),
                field: "lit".into(),
                value: FieldValue::Bool(true),
            }],
        },
    )
}

fn lit(replay: &RuleReplay) -> bool {
    matches!(
        replay.state().get(SWORD, "Flammable", "lit"),
        Some(FieldValue::Bool(true))
    )
}

/// Run one `Touched` event on `SWORD` against `rule` over a state seeded by `seed`.
fn run_touch(rule: (RuleId, RuleData), seed: &[(&str, &str, FieldValue)]) -> RuleReplay {
    let mut state = RuntimeState::new();
    for (component, field, value) in seed {
        state.set(SWORD, component, field, value.clone());
    }
    let mut rec = RuleRecording::new(state, vec![rule], vec![]);
    rec.add_event(0, "Touched", Some(SWORD.into()));
    let mut replay = RuleReplay::new(rec);
    replay.advance();
    replay
}

fn blocked_reasons(replay: &RuleReplay) -> Vec<String> {
    replay
        .history()
        .iter()
        .filter_map(|d| match &d.kind {
            DecisionKind::RuleBlocked { why, .. } => Some(why.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn an_or_group_needs_only_one_alternative_and_still_requires_every_and_clause() {
    // "…only if it is active, AND EITHER the key is taken OR the score is at least 3."
    let with_or = |count: i64, active: bool, key: bool| {
        let (id, mut rule) = gate_rule(3);
        let score_clause = rule.conditions.remove(0);
        rule.conditions = vec![Condition {
            entity: SWORD.into(),
            component: "GameRole".into(),
            field: "active".into(),
            op: CompareOp::Eq,
            value: FieldValue::Bool(true),
        }];
        rule.any_of = vec![
            Condition {
                entity: SWORD.into(),
                component: "Flammable".into(),
                field: "key".into(),
                op: CompareOp::Eq,
                value: FieldValue::Bool(true),
            },
            score_clause,
        ];
        run_touch(
            (id, rule),
            &[
                ("GameRole", "active", FieldValue::Bool(active)),
                ("KillCounter", "count", FieldValue::Integer(count)),
                ("Flammable", "key", FieldValue::Bool(key)),
            ],
        )
    };

    assert!(!lit(&with_or(0, true, false)), "no alternative holds");
    assert!(
        lit(&with_or(5, true, false)),
        "the score route alone opens it"
    );
    assert!(lit(&with_or(0, true, true)), "the key route alone opens it");
    assert!(
        !lit(&with_or(5, false, true)),
        "an OR group never waives the AND clauses"
    );
}

#[test]
fn a_blocked_rule_records_why_with_the_actual_runtime_value() {
    let replay = run_touch(
        gate_rule(3),
        &[("KillCounter", "count", FieldValue::Integer(1))],
    );
    assert!(!lit(&replay), "the threshold is not met");
    let why = blocked_reasons(&replay);
    assert_eq!(why.len(), 1, "the near miss is recorded exactly once");
    // The whole point: the ACTUAL value is in the sentence, not just the requirement.
    assert!(
        why[0].contains('1') && why[0].contains('3'),
        "why should name the actual value and the threshold: {}",
        why[0]
    );
}

#[test]
fn a_firing_rule_records_no_near_miss_and_an_unrelated_event_is_silent() {
    let fired = run_touch(
        gate_rule(1),
        &[("KillCounter", "count", FieldValue::Integer(5))],
    );
    assert!(lit(&fired));
    assert!(blocked_reasons(&fired).is_empty(), "no noise when it fires");

    // A different event entirely is not a near miss.
    let mut state = RuntimeState::new();
    state.set(SWORD, "KillCounter", "count", FieldValue::Integer(0));
    let mut rec = RuleRecording::new(state, vec![gate_rule(3)], vec![]);
    rec.add_event(0, "EnemyDied", Some(SWORD.into()));
    let mut other = RuleReplay::new(rec);
    other.advance();
    assert!(
        blocked_reasons(&other).is_empty(),
        "another event is not a near miss"
    );
}

#[test]
fn a_subject_pinned_rule_fires_only_for_its_own_object() {
    const OTHER: &str = "1_9";
    let pinned = || {
        let (id, mut rule) = gate_rule(0);
        rule.subject = Some(SWORD.to_string()); // the Play-start expansion's pin
        (id, rule)
    };
    let mut state = RuntimeState::new();
    state.set(SWORD, "KillCounter", "count", FieldValue::Integer(9));

    // An event about ANOTHER object must not fire this object's rule, nor log a near miss.
    let mut rec = RuleRecording::new(state.clone(), vec![pinned()], vec![]);
    rec.add_event(0, "Touched", Some(OTHER.into()));
    let mut theirs = RuleReplay::new(rec);
    theirs.advance();
    assert!(!lit(&theirs), "a pinned rule ignores other subjects");
    assert!(
        blocked_reasons(&theirs).is_empty(),
        "another object's event is not this object's near miss"
    );

    // Its own event still fires it.
    let mine = run_touch(
        pinned(),
        &[("KillCounter", "count", FieldValue::Integer(9))],
    );
    assert!(lit(&mine), "the pinned subject fires normally");
}

// ── `$other`: the entity that CAUSED the event ───────────────────────────────────────────────────

/// "Only the PLAYER may collect this" — a clause about the TOUCHER, not the touched. Before `$other`
/// existed this was not merely unwired, it was inexpressible: a rule had no way to name the toucher.
#[test]
fn a_clause_about_the_toucher_distinguishes_who_walked_into_it() {
    const PLAYER: &str = "1_a";
    const DOG: &str = "1_b";

    let rule = |_n: i64| {
        (
            RuleId::new("r_player_only"),
            RuleData {
                name: "player-only pickup".into(),
                enabled: true,
                event: "Touched".into(),
                conditions: vec![Condition {
                    entity: metrocalk_core::rules::OTHER_ENTITY.into(),
                    component: "GameRole".into(),
                    field: "role".into(),
                    op: CompareOp::Eq,
                    value: FieldValue::Str("player".into()),
                }],
                any_of: vec![],
                subject: None,
                actions: vec![Action {
                    action: "SetField".into(),
                    entity: SWORD.into(),
                    component: "Flammable".into(),
                    field: "lit".into(),
                    value: FieldValue::Bool(true),
                }],
            },
        )
    };

    let run = |toucher: &str| {
        let mut state = RuntimeState::new();
        state.set(PLAYER, "GameRole", "role", FieldValue::Str("player".into()));
        state.set(DOG, "GameRole", "role", FieldValue::Str("companion".into()));
        let mut rec = RuleRecording::new(state, vec![rule(0)], vec![]);
        rec.add_event_from(0, "Touched", Some(SWORD.into()), Some(toucher.to_string()));
        let mut replay = RuleReplay::new(rec);
        replay.advance();
        replay
    };

    assert!(lit(&run(PLAYER)), "the player's touch collects it");
    let by_dog = run(DOG);
    assert!(!lit(&by_dog), "the companion's touch must NOT collect it");
    // …and the near miss names the real reason, so the author is never left guessing.
    let why = blocked_reasons(&by_dog);
    assert_eq!(
        why.len(),
        1,
        "the companion's touch is a recorded near miss"
    );
    assert!(
        why[0].contains("player"),
        "the reason should name the role it wanted: {}",
        why[0]
    );
}

#[test]
fn an_event_with_no_other_leaves_a_toucher_clause_unsatisfied_rather_than_firing() {
    let rule = (
        RuleId::new("r_needs_other"),
        RuleData {
            name: "player-only".into(),
            enabled: true,
            event: "Touched".into(),
            conditions: vec![Condition {
                entity: metrocalk_core::rules::OTHER_ENTITY.into(),
                component: "GameRole".into(),
                field: "role".into(),
                op: CompareOp::Eq,
                value: FieldValue::Str("player".into()),
            }],
            any_of: vec![],
            subject: None,
            actions: vec![Action {
                action: "SetField".into(),
                entity: SWORD.into(),
                component: "Flammable".into(),
                field: "lit".into(),
                value: FieldValue::Bool(true),
            }],
        },
    );
    let mut rec = RuleRecording::new(RuntimeState::new(), vec![rule], vec![]);
    rec.add_event(0, "Touched", Some(SWORD.into())); // an emitter that knew no toucher
    let mut replay = RuleReplay::new(rec);
    replay.advance();
    assert!(
        !lit(&replay),
        "an unresolvable $other fails closed — it never fires on a guess"
    );
}

// ── health: Damage / Heal, with the clamps every game expects ────────────────────────────────────

fn hp_of(replay: &RuleReplay, who: &str) -> i64 {
    match replay.state().get(who, "Health", "hp") {
        Some(FieldValue::Integer(i)) => *i,
        other => panic!("expected integer hp, got {other:?}"),
    }
}

/// A hazard harms the TOUCHER (`$other`), not itself — and never past zero.
#[test]
fn damage_hurts_the_toucher_and_floors_at_zero() {
    const HERO: &str = "1_a";
    let hazard = (
        RuleId::new("r_hazard"),
        RuleData {
            name: "Hurt whoever touches it".into(),
            enabled: true,
            event: "Touched".into(),
            conditions: vec![],
            any_of: vec![],
            subject: None,
            actions: vec![Action {
                action: "Damage".into(),
                entity: metrocalk_core::rules::OTHER_ENTITY.into(),
                component: "Health".into(),
                field: "hp".into(),
                value: FieldValue::Integer(2),
            }],
        },
    );

    let mut state = RuntimeState::new();
    state.set(HERO, "Health", "hp", FieldValue::Integer(3));
    state.set(HERO, "Health", "maxHp", FieldValue::Integer(3));
    let mut rec = RuleRecording::new(state, vec![hazard], vec![]);
    for frame in 0..3 {
        rec.add_event_from(frame, "Touched", Some(SWORD.into()), Some(HERO.into()));
    }
    let mut replay = RuleReplay::new(rec);

    replay.advance();
    assert_eq!(hp_of(&replay, HERO), 1, "3 - 2 = 1");
    replay.advance();
    assert_eq!(hp_of(&replay, HERO), 0, "1 - 2 floors at 0, never negative");
    replay.advance();
    assert_eq!(hp_of(&replay, HERO), 0, "and stays there");
    // The spike does NOT hurt itself.
    assert!(
        replay.state().get(SWORD, "Health", "hp").is_none(),
        "the hazard damages the toucher, not the touched"
    );
}

#[test]
fn heal_never_exceeds_the_maximum() {
    const HERO: &str = "1_a";
    let potion = (
        RuleId::new("r_potion"),
        RuleData {
            name: "Heal".into(),
            enabled: true,
            event: "Touched".into(),
            conditions: vec![],
            any_of: vec![],
            subject: None,
            actions: vec![Action {
                action: "Heal".into(),
                entity: HERO.into(),
                component: "Health".into(),
                field: "hp".into(),
                value: FieldValue::Integer(5),
            }],
        },
    );
    let mut state = RuntimeState::new();
    state.set(HERO, "Health", "hp", FieldValue::Integer(1));
    state.set(HERO, "Health", "maxHp", FieldValue::Integer(3));
    let mut rec = RuleRecording::new(state, vec![potion], vec![]);
    rec.add_event(0, "Touched", Some(SWORD.into()));
    let mut replay = RuleReplay::new(rec);
    replay.advance();
    assert_eq!(hp_of(&replay, HERO), 3, "healed to the cap, not past it");
}
