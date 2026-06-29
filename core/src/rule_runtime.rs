//! M12.5 (ADR-049) — **Rules in Play + the live truth-state debugger**, the *runtime + debug* half of the
//! Rules layer. M12.1–M12.4 make Rules/state machines **authorable**; this module makes them **run** — and,
//! distinctively, **debuggable by looking** (north-star test #5 boxes 3–4).
//!
//! Three properties, each a deliberate **reuse**, not a new model:
//! 1. **Running Rules is a projection, never the authored doc** (ADR-021/ADR-034). A tick mutates a
//!    [`RuntimeState`] — an in-memory field store the Play session owns — and **never** the ECS/Loro
//!    document. A Rule firing in Play is **not** a Loro undo entry (authoring one, M12.1, is); Stop drops the
//!    runtime state and restores the pre-Play edit state bit-exactly. This module is **pure data + logic** (no
//!    Loro/Flecs leak), exactly like [`crate::rules`] / [`crate::state_machine`] — wasm-portable, the
//!    server-authoritative web runtime is the same seam Play is (ADR-006).
//! 2. **Time-travel reuses the M8.4 sim-replay channel** (`physics::replay`). [`RuleRecording`] is the
//!    `physics::replay::Recording` sibling (initial state + the ordered, frame-stamped input log);
//!    [`RuleReplay`] is the `Replay` cursor — [`RuleReplay::advance`] ticks one frame, [`RuleReplay::seek`]
//!    scrubs (**rewind = rebuild-from-recording**, the deterministic-by-rebuild P3 path, then replay
//!    forward). The decision history scrubs over the **same** deterministic-rebuild channel physics already
//!    uses — **not** a parallel logic-replay. `explain_rule` is `explain_contact`'s sibling (the M3.1/M8.4
//!    explain engine on running logic).
//! 3. **Determinism is M8.1's** — same seed + same ordered input log → the **same decision history**
//!    bit-identically (a bug is repeatable, not a ghost). A **non-deterministic plugin** (M12.3) cannot enter
//!    the lockstep path: [`partition_deterministic`] flags it out **before** the recording is built.
//!
//! The [`TruthState`] read ([`RuleReplay::truth_state`]) is a **debug projection** — `&self`, non-mutating
//! (the M8.4 `diagnostics()` discipline: reading the overlay must never perturb the run it inspects), off the
//! per-frame hot path (invariant 4). Its structured fields (`satisfied` / `actual` / `expected`) are the
//! stable assertion surface; the `display` copy is the human string the overlay renders ("✅ `state =
//! FacingBoss`", "❌ `KillCounter = 3 of 4`") — debug by *looking*, not `Debug.Log`.

use crate::pipeline::FieldValue;
use crate::rules::{Action, Condition, RuleData, RuleId, RUN_PLUGIN_ACTION};
use crate::state_machine::{StateMachine, StateMachineId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::HashMap;

// ── the projection-only runtime state (NEVER Loro — ADR-021/034) ─────────────────────────────────

/// The **runtime state** a Play session mutates — `entity -> component -> field -> value`, seeded from the
/// authored scene at Play-start, advanced by rule actions each tick, and **dropped on Stop**. This is the
/// projection the running Rules write to **instead of** the ECS/Loro authored document (ADR-021/ADR-034): a
/// Rule firing here can never corrupt the authored doc or land on the undo stack. `BTreeMap` throughout so
/// iteration + the serialized [`Self::digest`] are **deterministic** (the determinism discipline applied to
/// the debug surface too).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RuntimeState {
    // entity (loro key) -> component -> field -> value
    fields: BTreeMap<String, BTreeMap<String, BTreeMap<String, FieldValue>>>,
}

impl RuntimeState {
    /// An empty runtime state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read one field's current runtime value (`None` if unset). The read the truth-state debugger + the
    /// condition evaluator both go through — one source of truth, so "did it fire" and "what the debugger
    /// shows" can never drift.
    #[must_use]
    pub fn get(&self, entity: &str, component: &str, field: &str) -> Option<&FieldValue> {
        self.fields.get(entity)?.get(component)?.get(field)
    }

    /// Set one field's runtime value (the projection write — never a Loro commit).
    pub fn set(&mut self, entity: &str, component: &str, field: &str, value: FieldValue) {
        self.fields
            .entry(entity.to_string())
            .or_default()
            .entry(component.to_string())
            .or_default()
            .insert(field.to_string(), value);
    }

    /// Seed an entity's components into the runtime state at Play-start (typically the engine's
    /// **resolved** components — base + overrides). Existing values are overwritten.
    pub fn seed(
        &mut self,
        entity: &str,
        components: &HashMap<String, HashMap<String, FieldValue>>,
    ) {
        for (component, fields) in components {
            for (field, value) in fields {
                self.set(entity, component, field, value.clone());
            }
        }
    }

    /// A stable, deterministic digest of the whole runtime state — the equality key for the determinism
    /// guard (the `physics::replay::Replay::world_hash` sibling). Same seed + inputs → same digest.
    #[must_use]
    pub fn digest(&self) -> String {
        // BTreeMap ordering makes this canonical; serialization of plain scalars is infallible.
        serde_json::to_string(&self.fields).expect("RuntimeState is always serializable")
    }
}

// ── the input log (the M8.4 `InputEvent` sibling) ────────────────────────────────────────────────

/// One **runtime input event** — the `physics::replay::InputEvent` sibling. Frame-stamped + ordered so the
/// decision history is **deterministic** (the M8.1/M8.4 ordered-input substrate, now for logic). `event` is a
/// registry event name (the rule's **When**, e.g. `EnemyDied`); `subject` is the entity the event concerns
/// (e.g. the enemy that died), `None` for a scene-global event. Applied **before** the tick of its `frame`
/// (so `frame: 0` perturbs the very first tick — the same convention physics inputs use).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuleEvent {
    /// The frame at which this event fires.
    pub frame: u64,
    /// The registry event name (the rule's **When**).
    pub event: String,
    /// The entity the event concerns (a loro key), or `None` for a global event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}

// ── the decision history (what time-travel scrubs over) ──────────────────────────────────────────

/// One entry in a [`RuleReplay`]'s **decision history** — a single, frame-stamped consequence of a tick. The
/// history is the time-travelable record (test #5 box 4): scrub back to a frame and read exactly *when* a
/// counter incremented / a transition fired. Deterministic-by-rebuild (the M8.4 guarantee), **distinct from
/// Loro undo** (these are runtime decisions, not authored edits).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DecisionKind {
    /// A rule's When matched + its If held → it fired.
    RuleFired { rule: String, name: String },
    /// A counter field changed (the `AdjustCounter` verb) — `from`/`to` make "when did it increment" exact.
    CounterChanged {
        entity: String,
        component: String,
        field: String,
        from: FieldValue,
        to: FieldValue,
    },
    /// A field was set (the `SetField` verb) — e.g. `Flammable.lit = true` (the sword catches fire).
    FieldSet {
        entity: String,
        component: String,
        field: String,
        value: FieldValue,
    },
    /// A state machine transitioned `from -> to` (the quest advanced).
    StateTransition {
        machine: String,
        from: String,
        to: String,
    },
    /// A `RunPlugin` action invoked a (deterministic) sandboxed plugin. The plugin's algorithmic *effect*
    /// runs in `/plugins` (`!Send`, out of this pure path) and returns as an ADR-017 patch; the deterministic
    /// **invocation** is recorded here so the decision history stays faithful + replayable.
    PluginInvoked { plugin: String },
}

/// A frame-stamped [`DecisionKind`] — one row of the decision history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecisionEvent {
    /// The frame on which the decision happened.
    pub frame: u64,
    /// What happened.
    #[serde(flatten)]
    pub kind: DecisionKind,
}

// ── the truth-state debugger projection (test #5 box 3 — "debug by looking") ─────────────────────

/// One If-condition's **live truth** at the current frame — the satisfied/unsatisfied fact made *visible*.
/// The structured fields (`satisfied` / `actual` / `expected`) are the **stable assertion surface**; `display`
/// is the human overlay copy (the UI prefixes ✅/❌ from `satisfied`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConditionTruth {
    /// Whether the condition currently holds.
    pub satisfied: bool,
    /// The entity read.
    pub entity: String,
    /// The component read.
    pub component: String,
    /// The field read.
    pub field: String,
    /// The current runtime value (`None` if the field is unset — an unsatisfied read, not a crash).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<FieldValue>,
    /// The value the condition compares against.
    pub expected: FieldValue,
    /// The human overlay copy, e.g. `"KillCounter = 3 of 4"` (the ✅/❌ comes from `satisfied`).
    pub display: String,
}

/// One rule's **live truth** for the clicked entity — does it fire right now, and *why / why not*, made
/// visible per-condition. This is the heart of "debug by looking" (test #5 box 3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleTruth {
    /// The rule id (stable — the e2e/UI key, never the label).
    pub rule: String,
    /// The rule's human name.
    pub name: String,
    /// The rule's When event.
    pub event: String,
    /// Whether **all** conditions currently hold (it would fire on its event now).
    pub fires: bool,
    /// Each condition's live truth, in author order.
    pub conditions: Vec<ConditionTruth>,
}

/// A state machine's **live current state** for the clicked entity (test #5 box 3 — "✅ `state =
/// FacingBoss`"). The current state is read from the runtime state field the transitions write.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineTruth {
    /// The machine id (stable key).
    pub machine: String,
    /// The machine's human name.
    pub name: String,
    /// The state field (e.g. `state`).
    pub field: String,
    /// The live current state (e.g. `FacingBoss`).
    pub current: String,
    /// The human overlay copy, e.g. `"state = FacingBoss"`.
    pub display: String,
}

/// The full **truth-state** for one entity at the current frame — the debug projection the overlay renders on
/// click. A pure read over the runtime state (`&self`); rendering it never perturbs the run (the M8.4
/// non-mutating-overlay discipline).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TruthState {
    /// The entity inspected (a loro key).
    pub entity: String,
    /// Every rule that references this entity (in a condition or an action), with its live truth.
    pub rules: Vec<RuleTruth>,
    /// Every state machine driving this entity, with its live current state.
    pub machines: Vec<MachineTruth>,
}

// ── the determinism guard (deliverable 5) ────────────────────────────────────────────────────────

/// A rule excluded from the deterministic replay path because it invokes a **non-deterministic** plugin
/// (M12.3 / ADR-047). Surfaced (never silently dropped) so the user sees *why* a rule won't run in Play —
/// the M8.1 lockstep gate made legible.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlaggedRule {
    /// The excluded rule id.
    pub rule: String,
    /// The plain-language reason.
    pub reason: String,
}

/// Partition rules into the **deterministic** set (eligible for the Play/replay lockstep) and the **flagged**
/// set (excluded — a `RunPlugin` action targets a plugin that is not known-deterministic). `plugin_kind`
/// answers `name -> Some(true|false)` for a known plugin, `None` for an unknown one (treated as
/// non-deterministic — a rule may not gamble determinism on an unregistered plugin). This runs **before** the
/// recording is built, so a non-deterministic plugin can never poison the replay (deliverable 5 / the
/// adversarial "a non-deterministic plugin poisons the replay" guard).
///
/// Returns `(deterministic, flagged)`, both in input order.
#[must_use]
pub fn partition_deterministic<F>(
    rules: &[(RuleId, RuleData)],
    plugin_kind: F,
) -> (Vec<(RuleId, RuleData)>, Vec<FlaggedRule>)
where
    F: Fn(&str) -> Option<bool>,
{
    let mut kept = Vec::new();
    let mut flagged = Vec::new();
    for (id, rule) in rules {
        let offender = rule.actions.iter().find_map(|a| {
            if a.action == RUN_PLUGIN_ACTION && plugin_kind(&a.component) != Some(true) {
                Some(a.component.clone())
            } else {
                None
            }
        });
        if let Some(plugin) = offender {
            flagged.push(FlaggedRule {
                rule: id.to_string(),
                reason: format!(
                    "'{plugin}' isn't a known-deterministic plugin, so this rule is held out of the \
                     deterministic Play/replay path (a non-deterministic plugin can't run in lockstep)"
                ),
            });
        } else {
            kept.push((id.clone(), rule.clone()));
        }
    }
    (kept, flagged)
}

// ── the recording (the M8.4 `Recording` sibling) ─────────────────────────────────────────────────

/// The full deterministic description of a Rules-in-Play run — the `physics::replay::Recording` sibling:
/// the authored rules + machines (a **snapshot**; edits are disabled in Play, ADR-034), the **initial**
/// runtime state seeded from the authored scene at Play-start, and the ordered, frame-stamped **event log**.
/// Replaying it re-derives the decision history **bit-identically** (the M8.1 native-determinism verdict
/// through logic). It never re-reads Loro — it re-runs the Rules tick.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RuleRecording {
    /// The authored rules (id-keyed), evaluated in **sorted-by-id** order each tick (deterministic).
    pub rules: Vec<(RuleId, RuleData)>,
    /// The authored state machines (id-keyed) whose transitions tick alongside the rules.
    pub machines: Vec<(StateMachineId, StateMachine)>,
    /// The runtime state at frame 0 (seeded from the authored scene — `resolved_components`).
    pub initial: RuntimeState,
    /// The frame-stamped input log (kept sorted by frame — the deterministic input channel).
    pub events: Vec<RuleEvent>,
}

impl RuleRecording {
    /// A recording over a seeded initial state, with the given rules + machines. Rules are **sorted by id**
    /// so evaluation order is stable across runs and machines (the determinism discipline).
    #[must_use]
    pub fn new(
        initial: RuntimeState,
        mut rules: Vec<(RuleId, RuleData)>,
        mut machines: Vec<(StateMachineId, StateMachine)>,
    ) -> Self {
        rules.sort_by(|a, b| a.0.cmp(&b.0));
        machines.sort_by(|a, b| a.0.cmp(&b.0));
        Self {
            rules,
            machines,
            initial,
            events: Vec::new(),
        }
    }

    /// Record an input event at `frame` (kept frame-sorted, stable within a frame — the
    /// `physics::replay::Recording::add_input` sibling, so a scrub replays it deterministically).
    pub fn add_event(&mut self, frame: u64, event: impl Into<String>, subject: Option<String>) {
        self.events.push(RuleEvent {
            frame,
            event: event.into(),
            subject,
        });
        // Stable sort preserves within-frame insertion order — two events on the same frame keep their
        // authored order, so the decision history is fully deterministic.
        self.events.sort_by_key(|e| e.frame);
    }

    /// The seeded frame-0 runtime state: the initial scene overlaid with each machine's `initial` state
    /// (where the machine's state field isn't already seeded). The single starting point a rewind rebuilds
    /// from — so a scrub is deterministic-by-rebuild, never a snapshot deserialize (the M8.4 #910-sidestep).
    #[must_use]
    pub fn seeded_state(&self) -> RuntimeState {
        let mut state = self.initial.clone();
        for (_, sm) in &self.machines {
            if state.get(&sm.entity, &sm.component, &sm.field).is_none() {
                state.set(
                    &sm.entity,
                    &sm.component,
                    &sm.field,
                    FieldValue::Str(sm.initial.clone()),
                );
            }
        }
        state
    }
}

// ── the replay cursor (the M8.4 `Replay` sibling — the live timeline engine) ──────────────────────

/// A live **cursor** over a [`RuleRecording`] — owns the runtime state + the decision history at the current
/// [`Self::frame`]. The `physics::replay::Replay` sibling and the M12.5 timeline engine: [`Self::advance`]
/// ticks one deterministic frame; [`Self::seek`] scrubs (**rewind = rebuild-from-recording**, then replay
/// forward — the deterministic-by-rebuild P3 path, *not* a snapshot deserialize). Holds **no Loro/undo
/// state** — the runtime state + history are a regenerable projection (ADR-021).
#[derive(Clone, Debug)]
pub struct RuleReplay {
    recording: RuleRecording,
    state: RuntimeState,
    history: Vec<DecisionEvent>,
    frame: u64,
}

impl RuleReplay {
    /// A cursor at frame 0 over `recording`, with the runtime state seeded from it.
    #[must_use]
    pub fn new(recording: RuleRecording) -> Self {
        let state = recording.seeded_state();
        Self {
            recording,
            state,
            history: Vec::new(),
            frame: 0,
        }
    }

    /// The current frame.
    #[must_use]
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// The recording driving this cursor.
    #[must_use]
    pub fn recording(&self) -> &RuleRecording {
        &self.recording
    }

    /// The current runtime state (a read — never mutate it outside the tick).
    #[must_use]
    pub fn state(&self) -> &RuntimeState {
        &self.state
    }

    /// The decision history up to (and including) the current frame, in occurrence order.
    #[must_use]
    pub fn history(&self) -> &[DecisionEvent] {
        &self.history
    }

    /// A stable digest of the decision history — the equality key for the determinism guard + a stable field
    /// the e2e can assert off (never the overlay copy). Same recording → same digest, every replay.
    #[must_use]
    pub fn history_digest(&self) -> String {
        serde_json::to_string(&self.history).expect("decision history is always serializable")
    }

    /// Consume the cursor, yielding its final runtime state (the `physics::replay::Replay::into_parts`
    /// sibling) — so a scrubbed frame's state can be inspected after the cursor is dropped.
    #[must_use]
    pub fn into_state(self) -> RuntimeState {
        self.state
    }

    /// One deterministic tick: apply this frame's events (firing matching rules + advancing matching
    /// transitions, mutating the runtime state, appending decision events), then bump the cursor. Identical
    /// given an identical recording — the determinism guarantee (M8.1 through logic).
    pub fn advance(&mut self) {
        let frame = self.frame;
        // Snapshot this frame's events (a small clone) so the dispatch can mutate `self` without aliasing the
        // recording's event log.
        let events: Vec<RuleEvent> = self
            .recording
            .events
            .iter()
            .filter(|e| e.frame == frame)
            .cloned()
            .collect();
        for ev in &events {
            self.dispatch(ev);
        }
        self.frame += 1;
    }

    /// Scrub to `target`: on a **rewind**, rebuild the runtime state + clear the history from the recording's
    /// seeded frame 0 (the deterministic-by-rebuild path — no snapshot deserialize), then replay forward to
    /// `target`. The resulting state + history are bit-identical to a continuous run to `target` — so
    /// resume-from-scrub is deterministic (the M8.4 P3 guarantee, now for the decision history).
    pub fn seek(&mut self, target: u64) {
        if target < self.frame {
            self.state = self.recording.seeded_state();
            self.history.clear();
            self.frame = 0;
        }
        while self.frame < target {
            self.advance();
        }
    }

    /// Fire a **live** event now (the interactive-Play input channel): record it into the recording at the
    /// current head frame, then advance one tick. Recording it (not just dispatching) is what makes the
    /// decision history **time-travelable** — a later [`Self::seek`] back rebuilds + replays the same event,
    /// deterministically (the M8.4 `add_input` + `advance`, fused for live Play). Returns the new frame.
    ///
    /// Intended for firing at the timeline **head** (during live Play). After a scrub-back, `seek` forward to
    /// the head before firing, so the appended event lands in chronological order.
    pub fn fire(&mut self, event: impl Into<String>, subject: Option<String>) -> u64 {
        self.recording.events.push(RuleEvent {
            frame: self.frame,
            event: event.into(),
            subject,
        });
        // Stable sort keeps within-frame insertion order — the appended head event stays last on its frame.
        self.recording.events.sort_by_key(|e| e.frame);
        self.advance();
        self.frame
    }

    // ── the tick internals ───────────────────────────────────────────────────────────────────────

    /// Dispatch one event: fire every matching rule whose conditions hold (in **sorted-by-id order**, each
    /// evaluated against the live — possibly already-mutated — state, so a counter rule's increment is visible
    /// to a threshold rule **this same tick**: the 4th kill both reaches `count = 4` *and* ignites the sword).
    /// That cascade is order-dependent, but the order is **fixed + documented** (sorted id, the ADR-046
    /// determinism discipline), so the decision history is fully reproducible. Then advance every machine whose
    /// current state has a matching, satisfied transition (the first in deterministic `ordered_transitions`
    /// order) — rules settle (counters/effects), then the quest phase reacts to the result.
    fn dispatch(&mut self, ev: &RuleEvent) {
        // Rules — sequential in sorted-id order; re-check each rule's conditions against the live state so an
        // earlier rule's effect cascades into a later rule this same tick. `rule_to_fire` does the (cheap,
        // allocation-free) check under an immutable borrow and clones **only a firing rule's** id/name/actions
        // — so a non-firing rule costs nothing extra on the per-frame hot path (inv. 4), even @N rules.
        for i in 0..self.recording.rules.len() {
            if let Some((id, name, actions)) = self.rule_to_fire(i, &ev.event) {
                self.history.push(DecisionEvent {
                    frame: self.frame,
                    kind: DecisionKind::RuleFired { rule: id, name },
                });
                for a in &actions {
                    self.apply_action(a);
                }
            }
        }

        // Machines — a transition IS an M12.1 Rule (reuse). Fire the first deterministically-ordered
        // transition out of the current state whose event matches + conditions hold (cloning nothing unless
        // one fires — the same hot-path discipline as the rules loop).
        for mi in 0..self.recording.machines.len() {
            if let Some(tf) = self.transition_to_fire(mi, &ev.event) {
                // The transition's effect IS entering `to` (M12.2's validator guarantees the rule's action is
                // exactly the enter-state SetField) — so we set the state field directly + record the
                // transition, rather than re-running the action (which would double-record the same change).
                self.state.set(
                    &tf.entity,
                    &tf.component,
                    &tf.field,
                    FieldValue::Str(tf.to.clone()),
                );
                self.history.push(DecisionEvent {
                    frame: self.frame,
                    kind: DecisionKind::StateTransition {
                        machine: tf.machine,
                        from: tf.from,
                        to: tf.to,
                    },
                });
            }
        }
    }

    /// If machine `mi` has a satisfied transition out of its current state on `ev_event`, the owned move to
    /// apply (the first in deterministic `ordered_transitions` order — ADR-046's tie-break). Cloned **only on
    /// a fire**, under an immutable borrow dropped before the caller mutates `self`.
    fn transition_to_fire(&self, mi: usize, ev_event: &str) -> Option<TransitionFire> {
        let (mid, sm) = &self.recording.machines[mi];
        let cur = self.current_state(sm);
        let t = sm.ordered_transitions().into_iter().find(|t| {
            t.from == cur && t.rule.event == ev_event && self.conditions_hold(&t.rule.conditions)
        })?;
        Some(TransitionFire {
            machine: mid.to_string(),
            entity: sm.entity.clone(),
            component: sm.component.clone(),
            field: sm.field.clone(),
            from: cur,
            to: t.to.clone(),
        })
    }

    /// Apply one Then-action to the runtime state (never Loro), recording its decision. The verb set is the
    /// **closed** M12.1 vocabulary; an unknown verb is impossible (validated at authoring) and ignored.
    fn apply_action(&mut self, a: &Action) {
        match a.action.as_str() {
            "SetField" => {
                let already = self.state.get(&a.entity, &a.component, &a.field) == Some(&a.value);
                if !already {
                    self.state
                        .set(&a.entity, &a.component, &a.field, a.value.clone());
                    self.history.push(DecisionEvent {
                        frame: self.frame,
                        kind: DecisionKind::FieldSet {
                            entity: a.entity.clone(),
                            component: a.component.clone(),
                            field: a.field.clone(),
                            value: a.value.clone(),
                        },
                    });
                }
            }
            "AdjustCounter" => {
                let from = self
                    .state
                    .get(&a.entity, &a.component, &a.field)
                    .cloned()
                    .unwrap_or(FieldValue::Integer(0));
                let to = add_numeric(&from, &a.value);
                if to != from {
                    self.state
                        .set(&a.entity, &a.component, &a.field, to.clone());
                    self.history.push(DecisionEvent {
                        frame: self.frame,
                        kind: DecisionKind::CounterChanged {
                            entity: a.entity.clone(),
                            component: a.component.clone(),
                            field: a.field.clone(),
                            from,
                            to,
                        },
                    });
                }
            }
            RUN_PLUGIN_ACTION => {
                // The honest ceiling: a deterministic plugin's algorithmic effect runs in the `/plugins`
                // sandbox (`!Send`, out of this pure path) and returns as an ADR-017 patch. Here we record the
                // deterministic invocation so the decision history is faithful + replayable. A
                // non-deterministic plugin never reaches this point (excluded by `partition_deterministic`).
                self.history.push(DecisionEvent {
                    frame: self.frame,
                    kind: DecisionKind::PluginInvoked {
                        plugin: a.component.clone(),
                    },
                });
            }
            _ => {}
        }
    }

    /// A machine's live current state — read from the runtime state field its transitions write (one source
    /// of truth), defaulting to the machine's `initial` if the field is somehow unset.
    #[must_use]
    pub fn current_state(&self, sm: &StateMachine) -> String {
        match self.state.get(&sm.entity, &sm.component, &sm.field) {
            Some(FieldValue::Str(s)) => s.clone(),
            _ => sm.initial.clone(),
        }
    }

    /// If rule `i` would fire on `ev_event`, the owned `(id, name, actions)` to apply — cloned **only on a
    /// fire** (a non-firing rule allocates nothing). The condition check runs under an immutable borrow that
    /// is dropped before the caller mutates `self`, so the per-frame loop stays borrow-clean + cheap.
    fn rule_to_fire(&self, i: usize, ev_event: &str) -> Option<(String, String, Vec<Action>)> {
        let (id, rule) = &self.recording.rules[i];
        if rule.enabled && rule.event == ev_event && self.conditions_hold(&rule.conditions) {
            Some((id.to_string(), rule.name.clone(), rule.actions.clone()))
        } else {
            None
        }
    }

    fn conditions_hold(&self, conditions: &[Condition]) -> bool {
        conditions.iter().all(|c| self.condition_satisfied(c))
    }

    /// The **cheap** condition check used on the per-frame tick: just the boolean, no display-string
    /// allocation. The full [`Self::eval_condition`] (with the human copy) is reserved for the click-time
    /// debugger, so the hot path never builds strings it throws away.
    fn condition_satisfied(&self, c: &Condition) -> bool {
        match self.state.get(&c.entity, &c.component, &c.field) {
            Some(v) => c.op.eval(v, &c.value) == Some(true),
            None => false,
        }
    }

    /// Evaluate one condition into a full [`ConditionTruth`] (with the human overlay copy) — the debugger
    /// read. Shares [`Self::condition_satisfied`] for the boolean, so what fires and what the overlay shows
    /// can never disagree.
    fn eval_condition(&self, c: &Condition) -> ConditionTruth {
        let actual = self.state.get(&c.entity, &c.component, &c.field).cloned();
        let satisfied = self.condition_satisfied(c);
        let display = render_condition(c, actual.as_ref());
        ConditionTruth {
            satisfied,
            entity: c.entity.clone(),
            component: c.component.clone(),
            field: c.field.clone(),
            actual,
            expected: c.value.clone(),
            display,
        }
    }

    // ── the debug projection (test #5 box 3) ───────────────────────────────────────────────────────

    /// The **live truth-state** for `entity` at the current frame (the click-to-debug projection): every rule
    /// that references the entity (in a condition or an action) with its per-condition truth, and every state
    /// machine driving the entity with its live current state. A pure read (`&self`) — rendering it never
    /// perturbs the run (the M8.4 non-mutating-overlay discipline). The structured fields are the assertion
    /// surface; `display` is the human copy.
    #[must_use]
    pub fn truth_state(&self, entity: &str) -> TruthState {
        let rules = self
            .recording
            .rules
            .iter()
            .filter(|(_, r)| rule_references(r, entity))
            .map(|(id, r)| {
                let conditions: Vec<ConditionTruth> = r
                    .conditions
                    .iter()
                    .map(|c| self.eval_condition(c))
                    .collect();
                RuleTruth {
                    rule: id.to_string(),
                    name: r.name.clone(),
                    event: r.event.clone(),
                    fires: conditions.iter().all(|c| c.satisfied),
                    conditions,
                }
            })
            .collect();

        let machines = self
            .recording
            .machines
            .iter()
            .filter(|(_, sm)| sm.entity == entity)
            .map(|(id, sm)| {
                let current = self.current_state(sm);
                MachineTruth {
                    machine: id.to_string(),
                    name: sm.name.clone(),
                    field: sm.field.clone(),
                    display: format!("{} = {current}", sm.field),
                    current,
                }
            })
            .collect();

        TruthState {
            entity: entity.to_string(),
            rules,
            machines,
        }
    }

    /// **Explain** why a rule did or didn't fire at the current frame (the M3.1/M8.4 `explain_contact`
    /// sibling, on logic) — a plain-language narration of the live truth-state, *shown not logged*. `None` if
    /// the rule isn't in the recording. If every condition holds it's "ready"; otherwise it names the **first
    /// blocking** condition (the one a user would fix first).
    #[must_use]
    pub fn explain_rule(&self, rule_id: &str) -> Option<String> {
        let (_, rule) = self
            .recording
            .rules
            .iter()
            .find(|(id, _)| id.as_str() == rule_id)?;
        if !rule.enabled {
            return Some(format!(
                "'{}' is disabled, so it won't fire (enable it to run in Play)",
                rule.name
            ));
        }
        let first_block = rule
            .conditions
            .iter()
            .map(|c| self.eval_condition(c))
            .find(|t| !t.satisfied);
        Some(match first_block {
            None => format!(
                "'{}' is ready — every condition holds, so it fires on {}",
                rule.name, rule.event
            ),
            Some(t) => {
                let actual = t
                    .actual
                    .as_ref()
                    .map_or_else(|| "unset".to_string(), fv_str);
                format!(
                    "'{}' is blocked: {}.{} is {actual}, but the rule needs {} {} (waiting on {})",
                    rule.name,
                    t.component,
                    t.field,
                    op_word(c_op(rule, &t)),
                    fv_str(&t.expected),
                    rule.event
                )
            }
        })
    }
}

// ── free helpers ─────────────────────────────────────────────────────────────────────────────────

/// The owned data needed to apply a state-machine transition (so the borrow of the recording is released
/// before the cursor mutates itself). Built only when a transition actually fires.
struct TransitionFire {
    machine: String,
    entity: String,
    component: String,
    field: String,
    from: String,
    to: String,
}

/// Whether a rule references `entity` in any condition or action (the truth-state debugger's "which rules
/// touch this entity" filter).
fn rule_references(rule: &RuleData, entity: &str) -> bool {
    rule.conditions.iter().any(|c| c.entity == entity)
        || rule.actions.iter().any(|a| a.entity == entity)
}

/// Add two numeric `FieldValue`s for `AdjustCounter` — integer + integer stays an integer (no float drift on a
/// whole-number counter); any number operand promotes to a number. Non-numeric operands leave the value
/// unchanged (the validator already type-checks `AdjustCounter` targets, so this is a defensive no-op).
fn add_numeric(cur: &FieldValue, delta: &FieldValue) -> FieldValue {
    match (cur, delta) {
        (FieldValue::Integer(a), FieldValue::Integer(b)) => FieldValue::Integer(a + b),
        #[allow(clippy::cast_precision_loss)]
        // counter magnitudes are tiny; this is a gameplay tally
        (FieldValue::Integer(a), FieldValue::Number(b)) => FieldValue::Number(*a as f64 + b),
        #[allow(clippy::cast_precision_loss)]
        (FieldValue::Number(a), FieldValue::Integer(b)) => FieldValue::Number(a + *b as f64),
        (FieldValue::Number(a), FieldValue::Number(b)) => FieldValue::Number(a + b),
        _ => cur.clone(),
    }
}

/// Render a condition's human overlay copy. An ordering comparison reads as "`Component` = actual of expected"
/// (the test-#5 "`KillCounter = 3 of 4`"); an equality/other reads as "`Component.field` = actual (want …)".
fn render_condition(c: &Condition, actual: Option<&FieldValue>) -> String {
    use crate::rules::CompareOp::{Ge, Gt, Le, Lt};
    let actual_s = actual.map_or_else(|| "—".to_string(), fv_str);
    match c.op {
        Lt | Le | Gt | Ge => {
            format!("{} = {actual_s} of {}", c.component, fv_str(&c.value))
        }
        _ => format!(
            "{}.{} = {actual_s} (want {} {})",
            c.component,
            c.field,
            op_word(c.op),
            fv_str(&c.value)
        ),
    }
}

/// The `CompareOp` of the original condition behind a [`ConditionTruth`] (the truth carries the resolved
/// fields, not the op; `explain_rule` re-reads it from the rule for the narration).
fn c_op(rule: &RuleData, t: &ConditionTruth) -> crate::rules::CompareOp {
    rule.conditions
        .iter()
        .find(|c| c.entity == t.entity && c.component == t.component && c.field == t.field)
        .map_or(crate::rules::CompareOp::Eq, |c| c.op)
}

/// A plain-language word for a comparison operator (the explain narration — "at least", "exactly", …).
fn op_word(op: crate::rules::CompareOp) -> &'static str {
    use crate::rules::CompareOp::{Eq, Ge, Gt, Le, Lt, Ne};
    match op {
        Eq => "to be exactly",
        Ne => "to not be",
        Lt => "to be under",
        Le => "to be at most",
        Gt => "to be over",
        Ge => "to be at least",
    }
}

/// Render a `FieldValue` as a terse human string (the overlay/explain copy).
fn fv_str(v: &FieldValue) -> String {
    match v {
        FieldValue::Integer(i) => i.to_string(),
        FieldValue::Number(n) => n.to_string(),
        FieldValue::Bool(b) => b.to_string(),
        FieldValue::Str(s) => s.clone(),
    }
}
