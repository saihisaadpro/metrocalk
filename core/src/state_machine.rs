//! M12.2 (ADR-046) — **state machines as data**, transitions are M12.1 Rules.
//!
//! A state machine is named **states** + **transitions** over an entity's state field
//! (`<entity>.<component>.<field>`, e.g. `QuestState.state`). The differentiator is that it is **not a
//! bespoke FSM engine**: each [`Transition`] **is** an M12.1 [`RuleData`] (When/If guard + a single
//! "enter `to`" set-state action), so the layer reuses the Rules model + the registry-fed, typo-proof,
//! transactional discipline of [`crate::rules`] rather than forking a parallel logic model. The canonical
//! hard case ([`docs/validation/gates-and-ux-tests.md`] test #5): the `QuestState` machine
//! Hunting -> ReadyForBoss -> FacingBoss.
//!
//! This module is the **data model + the registry-fed validator** — pure serde data (no Loro/Flecs leak),
//! so the same types persist on the Loro document ([`crate::pipeline::Op::SetStateMachine`]), cross to the
//! React editor, and are wasm-portable. Authoring/editing a machine is **one undoable transaction**;
//! **running** the machine (ticking the current state on events, the live truth-state debugger) is **M12.5**
//! — the [`StateMachine`] defines the `current` slot ([`crate::pipeline::Engine::state_machine_current`])
//! but this tier never advances it (the named seam).
//!
//! Validation (ADR-046 deliverable 4): no dangling transition (`from`/`to` target a real state), a
//! reachability **warning** for islands (explained, not rejected), and a **deterministic** tie-break order
//! over simultaneous transitions ([`StateMachine::ordered_transitions`]) — the determinism discipline
//! applied to logic too. A machine the registry can't satisfy is **Blocked + explained**
//! ([`StateMachineError`], ADR-016).

use crate::entity_id::EntityId;
use crate::pipeline::FieldValue;
use crate::registry::{FieldType, Registry};
use crate::rules::{validate_rule, Action, RuleData, RuleError};
use metrocalk_ecs::World;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// The registry action verb a transition uses to **enter** its target state: `SetField` on the machine's
/// state field. The builder constructs each transition's `Then` from this verb so the effect can never typo
/// the state field — and the validator asserts every transition really sets the machine's state to `to`.
pub const ENTER_STATE_ACTION: &str = "SetField";

/// A stable state-machine identifier — the key under the Loro `state_machines` map. Peer-namespaced
/// (allocated by [`crate::pipeline::Engine::alloc_state_machine_id`]) so two peers authoring machines
/// concurrently never collide.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StateMachineId(String);

impl StateMachineId {
    /// Wrap a raw id string (e.g. one read back off the document, or a test fixture).
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    /// The raw id string (the Loro map key).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StateMachineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One **transition** — a graph edge from one state to another, guarded by an M12.1 Rule. The transition
/// **is** a [`RuleData`]: `rule.event` is the **When**, `rule.conditions` the extra **If**, and
/// `rule.actions` the **Then** (canonically the single "enter `to`" set-state action). `from`/`to` are the
/// graph endpoints — and the runtime's `current == from` precondition (applied in M12.5). The `id` is a
/// **stable** edge id (the React Flow edge id / e2e key — never the label).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    /// Stable edge id (peer-namespaced via [`crate::pipeline::Engine::alloc_transition_id`]).
    pub id: String,
    /// Source state (must be one of the machine's `states`).
    pub from: String,
    /// Target state (must be one of the machine's `states`).
    pub to: String,
    /// The transition's guard + effect as an M12.1 Rule (the reuse, not a parallel model).
    pub rule: RuleData,
}

/// A **state machine** as data on the Loro document (ADR-046): named states + transitions, each transition
/// an M12.1 Rule. Stored under the top-level mergeable `state_machines` map (the ADR-026 pattern — distinct
/// machines merge without clobber, survive reload). The `current` runtime slot is **defined** here and read
/// by [`crate::pipeline::Engine::state_machine_current`] (defaulting to `initial`); **M12.5 ticks it** — this
/// tier never advances it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateMachine {
    /// A human label for the machine (non-empty).
    pub name: String,
    /// The entity carrying the state component (an [`EntityId::to_loro_key`] string).
    pub entity: String,
    /// The registry component holding the current state (e.g. `QuestState`).
    pub component: String,
    /// The state field on that component (e.g. `state`) — a registry `String` field.
    pub field: String,
    /// The named states (graph nodes), in author order. Non-empty, unique, no empty names.
    pub states: Vec<String>,
    /// The initial state (must be one of `states`); also the default `current` until M12.5 ticks it.
    pub initial: String,
    /// The transitions (graph edges).
    pub transitions: Vec<Transition>,
}

impl StateMachine {
    /// The canonical "enter `to`" action: `SetField <entity>.<component>.<field> = to`. The builder
    /// constructs each transition's `Then` from this so the effect can never typo the state field, and
    /// [`validate_state_machine`] asserts each transition's rule carries it.
    #[must_use]
    pub fn enter_action(&self, to: &str) -> Action {
        Action {
            action: ENTER_STATE_ACTION.to_string(),
            entity: self.entity.clone(),
            component: self.component.clone(),
            field: self.field.clone(),
            value: FieldValue::Str(to.to_string()),
        }
    }

    /// Transitions in the **deterministic tie-break order** simultaneous candidates are considered in
    /// (ADR-046 deliverable 4): by `from`, then triggering `event`, then `to`, then `id`. Two transitions
    /// out of the same state on the same event therefore have a stable, reproducible order across runs and
    /// machines — the determinism discipline applied to logic. M12.5 fires the first whose conditions hold;
    /// defining the order **now** is what makes that future firing deterministic (and unit-testable today).
    #[must_use]
    pub fn ordered_transitions(&self) -> Vec<&Transition> {
        let mut ts: Vec<&Transition> = self.transitions.iter().collect();
        // The total tie-break order: `from`, then triggering `event`, then `to`, then `id` — a documented,
        // reproducible ordering so simultaneous transitions are never nondeterministic.
        ts.sort_by(|a, b| {
            a.from
                .cmp(&b.from)
                .then_with(|| a.rule.event.cmp(&b.rule.event))
                .then_with(|| a.to.cmp(&b.to))
                .then_with(|| a.id.cmp(&b.id))
        });
        ts
    }
}

/// Why a state machine was rejected — every variant a **plain-language, ASCII-safe** explanation (ADR-016:
/// Blocked + explained, never a silent accept of a dangling/typo'd machine). ASCII so the reason stays
/// legible through every IPC layer (the M12.1 lesson — [`RuleError`] is ASCII for the same reason).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateMachineError {
    /// The machine has no name.
    EmptyName,
    /// A machine with no states is meaningless — refused.
    NoStates,
    /// A state with an empty name.
    EmptyStateName,
    /// Two states share a name.
    DuplicateState(String),
    /// The initial state isn't one of the declared states.
    InitialNotAState(String),
    /// The machine targets an entity that doesn't exist.
    UnknownEntity(String),
    /// The machine's state component isn't a component the registry knows.
    UnknownComponent(String),
    /// The machine's state field isn't a field of that component.
    UnknownField { component: String, field: String },
    /// The machine's state field isn't a string field (states are string-valued).
    StateFieldNotString { component: String, field: String },
    /// Two transitions share an id.
    DuplicateTransitionId(String),
    /// A transition's `from` isn't one of the declared states (a dangling edge).
    DanglingFrom { transition: String, state: String },
    /// A transition's `to` isn't one of the declared states (a dangling edge).
    DanglingTo { transition: String, state: String },
    /// The transition's underlying Rule is itself invalid — the typo-proof reuse of
    /// [`validate_rule`] (an unknown event/condition/action/value, surfaced with its own reason).
    TransitionRule {
        transition: String,
        source: RuleError,
    },
    /// A transition's effect doesn't set the machine's state field to `to` — it isn't a real transition.
    NotAStateChange { transition: String },
}

impl fmt::Display for StateMachineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => write!(f, "this state machine needs a name"),
            Self::NoStates => write!(f, "this state machine has no states - add at least one"),
            Self::EmptyStateName => write!(f, "a state needs a name"),
            Self::DuplicateState(s) => write!(f, "there are two states named '{s}' - names must be unique"),
            Self::InitialNotAState(s) => {
                write!(f, "the initial state '{s}' isn't one of this machine's states")
            }
            Self::UnknownEntity(e) => write!(f, "the entity '{e}' no longer exists"),
            Self::UnknownComponent(c) => write!(f, "'{c}' isn't a component the engine knows"),
            Self::UnknownField { component, field } => {
                write!(f, "'{component}' has no field '{field}'")
            }
            Self::StateFieldNotString { component, field } => write!(
                f,
                "{component}.{field} isn't a text field, so it can't hold a state name"
            ),
            Self::DuplicateTransitionId(id) => {
                write!(f, "two transitions share the id '{id}'")
            }
            Self::DanglingFrom { state, .. } => write!(
                f,
                "a transition starts from '{state}', which isn't one of this machine's states"
            ),
            Self::DanglingTo { state, .. } => write!(
                f,
                "a transition points to '{state}', which isn't one of this machine's states"
            ),
            Self::TransitionRule { source, .. } => write!(f, "{source}"),
            Self::NotAStateChange { transition: _ } => write!(
                f,
                "a transition doesn't actually change the state - its action must enter the target state"
            ),
        }
    }
}

impl std::error::Error for StateMachineError {}

/// The non-fatal findings of [`validate_state_machine`] — currently the **reachability** warning. A valid
/// machine can still have **island** states no transition reaches from `initial`; that is **explained**
/// (deliverable 4), not rejected, because an island may be authored before its incoming transition exists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StateMachineReport {
    /// States not reachable from `initial` by following transitions (in the machine's author order) — a
    /// warning the editor surfaces in plain language, never a silent accept.
    pub unreachable: Vec<String>,
}

/// Validate a state machine **against the registry** (ADR-046 deliverable 3/4 / ADR-016). The machine's
/// state target (`entity.component.field`) must be a real registry `String` field on a live entity; every
/// state name must be non-empty + unique; `initial` must be a state; and every transition must (a) have a
/// unique id, (b) target real states (no **dangling** edge), (c) be a **valid M12.1 Rule** — the reuse of
/// [`validate_rule`], so a typo'd event/condition/action is Blocked + explained — and (d) actually **enter**
/// its `to` state. A passing machine returns a [`StateMachineReport`] whose `unreachable` list is the
/// reachability **warning**. This same gate re-checks a merged machine (invariant 1).
///
/// # Errors
/// The first [`StateMachineError`] encountered (Blocked + explained).
pub fn validate_state_machine<W, F>(
    registry: &Registry<W>,
    sm: &StateMachine,
    entity_exists: F,
) -> Result<StateMachineReport, StateMachineError>
where
    W: World,
    F: Fn(EntityId) -> bool,
{
    if sm.name.trim().is_empty() {
        return Err(StateMachineError::EmptyName);
    }
    check_state_target(registry, sm, &entity_exists)?;

    if sm.states.is_empty() {
        return Err(StateMachineError::NoStates);
    }
    let mut states: BTreeSet<&str> = BTreeSet::new();
    for s in &sm.states {
        if s.trim().is_empty() {
            return Err(StateMachineError::EmptyStateName);
        }
        if !states.insert(s.as_str()) {
            return Err(StateMachineError::DuplicateState(s.clone()));
        }
    }
    if !states.contains(sm.initial.as_str()) {
        return Err(StateMachineError::InitialNotAState(sm.initial.clone()));
    }

    let mut ids: BTreeSet<&str> = BTreeSet::new();
    for t in &sm.transitions {
        if !ids.insert(t.id.as_str()) {
            return Err(StateMachineError::DuplicateTransitionId(t.id.clone()));
        }
        if !states.contains(t.from.as_str()) {
            return Err(StateMachineError::DanglingFrom {
                transition: t.id.clone(),
                state: t.from.clone(),
            });
        }
        if !states.contains(t.to.as_str()) {
            return Err(StateMachineError::DanglingTo {
                transition: t.id.clone(),
                state: t.to.clone(),
            });
        }
        // A transition IS an M12.1 Rule — reuse the validator (typo-proof When/If/Then), not a fork.
        validate_rule(registry, &t.rule, &entity_exists).map_err(|source| {
            StateMachineError::TransitionRule {
                transition: t.id.clone(),
                source,
            }
        })?;
        // ...and its effect must actually enter `to` (set the machine's own state field to `to`).
        if !sets_state(sm, &t.rule, &t.to) {
            return Err(StateMachineError::NotAStateChange {
                transition: t.id.clone(),
            });
        }
    }

    Ok(StateMachineReport {
        unreachable: unreachable_states(sm),
    })
}

/// Validate the machine's own state target: the entity exists, the component is registry-known, the field
/// exists on it, and it's a `String` field (states are string-valued). This makes even a zero-transition
/// machine typo-proof — "states are an ordinary registry-fed slice of an entity" (the gates-doc bar).
fn check_state_target<W, F>(
    registry: &Registry<W>,
    sm: &StateMachine,
    entity_exists: &F,
) -> Result<(), StateMachineError>
where
    W: World,
    F: Fn(EntityId) -> bool,
{
    match EntityId::from_loro_key(&sm.entity) {
        Some(id) if entity_exists(id) => {}
        _ => return Err(StateMachineError::UnknownEntity(sm.entity.clone())),
    }
    let meta = registry
        .meta(&sm.component)
        .ok_or_else(|| StateMachineError::UnknownComponent(sm.component.clone()))?;
    let spec = meta
        .fields
        .iter()
        .find(|f| f.name == sm.field)
        .ok_or_else(|| StateMachineError::UnknownField {
            component: sm.component.clone(),
            field: sm.field.clone(),
        })?;
    if spec.ty != FieldType::String {
        return Err(StateMachineError::StateFieldNotString {
            component: sm.component.clone(),
            field: sm.field.clone(),
        });
    }
    Ok(())
}

/// Whether `rule` carries the canonical "enter `to`" action on the machine's own state field — i.e. the
/// transition really sets `<entity>.<component>.<field> = to`. This is what makes a Rule a *transition*
/// (vs. an arbitrary side-effect rule), so a state graph can never wire an edge that doesn't move the state.
fn sets_state(sm: &StateMachine, rule: &RuleData, to: &str) -> bool {
    let want = FieldValue::Str(to.to_string());
    rule.actions.iter().any(|a| {
        a.action == ENTER_STATE_ACTION
            && a.entity == sm.entity
            && a.component == sm.component
            && a.field == sm.field
            && a.value == want
    })
}

/// The states **not reachable** from `initial` by following transitions (`from` -> `to`), returned in the
/// machine's author order (deterministic). A DFS from `initial`; any declared state not visited is an island.
fn unreachable_states(sm: &StateMachine) -> Vec<String> {
    let mut adj: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for t in &sm.transitions {
        adj.entry(t.from.as_str()).or_default().push(t.to.as_str());
    }
    let mut reached: BTreeSet<&str> = BTreeSet::new();
    let mut stack = vec![sm.initial.as_str()];
    while let Some(s) = stack.pop() {
        if !reached.insert(s) {
            continue;
        }
        if let Some(tos) = adj.get(s) {
            stack.extend(tos.iter().copied());
        }
    }
    sm.states
        .iter()
        .filter(|s| !reached.contains(s.as_str()))
        .cloned()
        .collect()
}
