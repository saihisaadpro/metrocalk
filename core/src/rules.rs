//! M12.1 (ADR-045) — the **Rules layer**: When / If / Then as **registry-fed, typo-proof data**.
//!
//! A rule declares *when* an event fires, *if* a set of registry-typed conditions hold, *then* a set of
//! registry-typed actions run. The differentiator is **typo-proof by construction**: every event, every
//! condition (`component`/`field`/operator/value) and every action is drawn from — and validated against —
//! the [`crate::registry::Registry`] the engine already knows, so there is **no free-text logic, no typos,
//! no nil-refs** (the canonical test #5 — "the rusty sword catches fire, but only after 4 kills **and** the
//! boss arena"). A rule the registry can't satisfy is **Blocked + explained** ([`RuleError`], ADR-016).
//!
//! This module is the **data model + the registry-fed validator + the mirror-rule proposer** — pure data
//! (serde, no Loro/Flecs leak) so the same types persist on the Loro document, cross to the editor, and are
//! wasm-portable. Authoring a rule is one undoable transaction (the [`crate::pipeline::Op::SetRule`] op);
//! **running** a rule is M12.5 (named, not built here). The **honest ceiling** (`docs/.../test #5`): actions
//! are a CLOSED registry vocabulary (verbs over component fields) — genuinely algorithmic behaviour is the
//! M12.3 WASM-plugin tier, never a free-code action here.

use crate::entity_id::EntityId;
use crate::pipeline::FieldValue;
use crate::registry::{FieldType, Registry};
use metrocalk_ecs::World;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;

/// The action verb that invokes a **WASM plugin** (M12.3 / ADR-047) — the *honest ceiling*. A `RunPlugin`
/// action's `component` slot names a registry-known plugin ([`crate::registry::PluginMeta`]); the
/// algorithmic work runs sandboxed in `/plugins`, and its effect comes back as an undoable transaction.
/// Rules **orchestrate** (when/if); a plugin **computes** — the line is drawn here and held.
pub const RUN_PLUGIN_ACTION: &str = "RunPlugin";

/// A stable rule identifier — the key under the Loro `rules` map. Peer-namespaced (allocated by
/// [`crate::pipeline::Engine::alloc_rule_id`]) so two peers authoring rules concurrently never collide.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RuleId(String);

impl RuleId {
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

impl fmt::Display for RuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The comparison operator in an If-condition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareOp {
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
}

impl CompareOp {
    /// Evaluate `lhs <op> rhs`. `None` when the two values aren't comparable (different scalar kinds —
    /// the value-type mismatch the validator already blocks at authoring time). Integers and numbers
    /// compare across the int/float line; bools and strings compare within their kind. The runtime use of
    /// this is M12.5 — it lives here so the typed vocabulary is semantically complete + unit-testable now.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // counter/threshold magnitudes are tiny; the compare is a gameplay gate, not a measurement
    pub fn eval(self, lhs: &FieldValue, rhs: &FieldValue) -> Option<bool> {
        let ord = match (lhs, rhs) {
            (FieldValue::Integer(a), FieldValue::Integer(b)) => a.partial_cmp(b),
            (FieldValue::Number(a), FieldValue::Number(b)) => a.partial_cmp(b),
            (FieldValue::Integer(a), FieldValue::Number(b)) => (*a as f64).partial_cmp(b),
            (FieldValue::Number(a), FieldValue::Integer(b)) => a.partial_cmp(&(*b as f64)),
            (FieldValue::Bool(a), FieldValue::Bool(b)) => a.partial_cmp(b),
            (FieldValue::Str(a), FieldValue::Str(b)) => a.partial_cmp(b),
            _ => return None,
        }?;
        Some(match self {
            Self::Eq => ord == Ordering::Equal,
            Self::Ne => ord != Ordering::Equal,
            Self::Lt => ord == Ordering::Less,
            Self::Le => ord != Ordering::Greater,
            Self::Gt => ord == Ordering::Greater,
            Self::Ge => ord != Ordering::Less,
        })
    }
}

/// One **If**-condition: `<entity>.<component>.<field> <op> <value>` — every part registry-typed. The
/// `entity` is the editor's id-space string (an [`EntityId`] Loro key), so the same struct round-trips
/// between the core, the document, and the React builder unchanged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Condition {
    /// The entity whose component is read (an [`EntityId::to_loro_key`] string).
    pub entity: String,
    /// The registry component kind (e.g. `KillCounter`).
    pub component: String,
    /// The component field (e.g. `count`).
    pub field: String,
    /// The comparison operator.
    pub op: CompareOp,
    /// The literal compared against (its scalar kind must match the field's registry type).
    pub value: FieldValue,
}

/// One **Then**-action: a registry action verb applied to `<entity>.<component>.<field> = <value>`. The
/// action vocabulary is **closed** (the honest ceiling) — `SetField`, `AdjustCounter`, … — never free code.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Action {
    /// The registry action verb (e.g. `SetField`).
    pub action: String,
    /// The entity the action targets (an [`EntityId::to_loro_key`] string).
    pub entity: String,
    /// The registry component kind to mutate.
    pub component: String,
    /// The component field to mutate.
    pub field: String,
    /// The value to set / adjust by (its scalar kind must match the field's registry type).
    pub value: FieldValue,
}

/// A **Rule** — When (a registry event) / If (all conditions hold) / Then (actions). Structured data
/// persisted on the Loro document (mergeable, survives reload); pure serde, no Loro/Flecs leak.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuleData {
    /// A human label for the rule (non-empty).
    pub name: String,
    /// Whether the rule is active (authoring-time flag; running is M12.5).
    pub enabled: bool,
    /// The registry event that triggers the rule (**When**).
    pub event: String,
    /// **If** — conditions that must ALL hold (AND semantics, per test #5). Empty = always-true.
    pub conditions: Vec<Condition>,
    /// **Then** — actions run when the rule fires (must be non-empty: a rule has to *do* something).
    pub actions: Vec<Action>,
}

/// Why a rule was rejected — every variant a **plain-language** explanation (ADR-016: Blocked + explained,
/// never a silent accept of typo'd / free-text vocabulary). This is what makes the builder typo-proof: an
/// event/component/field/action the registry doesn't know, or a value of the wrong type, is refused *with a
/// reason*, not nil-ref'd at runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleError {
    /// The rule has no name.
    EmptyName,
    /// A rule with no actions does nothing — refused.
    NoActions,
    /// The `When` event isn't in the registry vocabulary.
    UnknownEvent(String),
    /// A `Then` action verb isn't in the registry vocabulary.
    UnknownAction(String),
    /// A `RunPlugin` action references a plugin the registry doesn't know (M12.3 / ADR-047).
    UnknownPlugin(String),
    /// A condition/action references an entity that doesn't exist.
    UnknownEntity(String),
    /// A condition/action references a component the registry doesn't know.
    UnknownComponent { component: String },
    /// A condition/action references a field that component doesn't have.
    UnknownField { component: String, field: String },
    /// A condition/action's value is the wrong scalar type for the field.
    FieldTypeMismatch {
        component: String,
        field: String,
        expected: FieldType,
        got: &'static str,
    },
}

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => write!(f, "this rule needs a name"),
            Self::NoActions => write!(
                f,
                "this rule has no actions - add at least one thing for it to do"
            ),
            Self::UnknownEvent(e) => {
                write!(
                    f,
                    "'{e}' isn't an event the engine knows - pick one from the list"
                )
            }
            Self::UnknownAction(a) => {
                write!(
                    f,
                    "'{a}' isn't an action the engine knows - pick one from the list"
                )
            }
            Self::UnknownPlugin(p) => {
                write!(
                    f,
                    "'{p}' isn't a plugin the engine knows - install or pick a registered plugin"
                )
            }
            Self::UnknownEntity(e) => write!(f, "the entity '{e}' no longer exists"),
            Self::UnknownComponent { component } => {
                write!(f, "'{component}' isn't a component the engine knows")
            }
            Self::UnknownField { component, field } => {
                write!(f, "'{component}' has no field '{field}'")
            }
            Self::FieldTypeMismatch {
                component,
                field,
                expected,
                got,
            } => write!(
                f,
                "{component}.{field} expects a {expected:?} value, but a {got} was given"
            ),
        }
    }
}

impl std::error::Error for RuleError {}

/// Validate a rule **against the registry** (ADR-045 deliverable 2 / ADR-016). Every event, component,
/// field, action, and value type must be registry-known and well-typed, and every referenced entity must
/// exist (`entity_exists`, typically `|id| engine.entity_exists(id)`). The first failure is returned as a
/// plain-language [`RuleError`] — the **Blocked + explained** gate. A registry-fed builder offers only valid
/// options, so this is the backstop that also guards the sentence (M12.4) and the merged-document paths.
///
/// # Errors
/// The first [`RuleError`] encountered (empty name · no actions · unknown event/action/entity/component/
/// field · value-type mismatch).
pub fn validate_rule<W, F>(
    registry: &Registry<W>,
    rule: &RuleData,
    entity_exists: F,
) -> Result<(), RuleError>
where
    W: World,
    F: Fn(EntityId) -> bool,
{
    if rule.name.trim().is_empty() {
        return Err(RuleError::EmptyName);
    }
    if !registry.has_event(&rule.event) {
        return Err(RuleError::UnknownEvent(rule.event.clone()));
    }
    if rule.actions.is_empty() {
        return Err(RuleError::NoActions);
    }
    for c in &rule.conditions {
        check_entity(&c.entity, &entity_exists)?;
        check_field(registry, &c.component, &c.field, &c.value)?;
    }
    for a in &rule.actions {
        if !registry.has_action(&a.action) {
            return Err(RuleError::UnknownAction(a.action.clone()));
        }
        check_entity(&a.entity, &entity_exists)?;
        if a.action == RUN_PLUGIN_ACTION {
            // The Rules->plugin boundary (the honest ceiling, M12.3): a `RunPlugin` action's `component`
            // slot names a REGISTERED plugin (typo-proof — reveal/explain applies); the algorithmic work
            // lives in the sandboxed plugin, not in Rules. `field`/`value` carry the plugin's own input
            // contract, not a registry-typed component field, so the field-type check doesn't apply.
            if !registry.has_plugin(&a.component) {
                return Err(RuleError::UnknownPlugin(a.component.clone()));
            }
        } else {
            check_field(registry, &a.component, &a.field, &a.value)?;
        }
    }
    Ok(())
}

fn check_entity<F: Fn(EntityId) -> bool>(key: &str, exists: &F) -> Result<(), RuleError> {
    match EntityId::from_loro_key(key) {
        Some(id) if exists(id) => Ok(()),
        _ => Err(RuleError::UnknownEntity(key.to_string())),
    }
}

fn check_field<W: World>(
    registry: &Registry<W>,
    component: &str,
    field: &str,
    value: &FieldValue,
) -> Result<(), RuleError> {
    let meta = registry
        .meta(component)
        .ok_or_else(|| RuleError::UnknownComponent {
            component: component.to_string(),
        })?;
    let spec = meta
        .fields
        .iter()
        .find(|f| f.name == field)
        .ok_or_else(|| RuleError::UnknownField {
            component: component.to_string(),
            field: field.to_string(),
        })?;
    if field_type_matches(value, spec.ty) {
        Ok(())
    } else {
        Err(RuleError::FieldTypeMismatch {
            component: component.to_string(),
            field: field.to_string(),
            expected: spec.ty,
            got: value_type_name(value),
        })
    }
}

/// Whether `value`'s scalar kind satisfies the registry field type. A `Number` field also accepts an
/// `Integer` literal (a whole number is a valid number — the int-vs-number JSON footgun), but an `Integer`
/// field rejects a `Number` (no silent truncation).
pub(crate) fn field_type_matches(value: &FieldValue, ty: FieldType) -> bool {
    matches!(
        (value, ty),
        (FieldValue::Integer(_), FieldType::Integer)
            | (
                FieldValue::Integer(_) | FieldValue::Number(_),
                FieldType::Number
            )
            | (FieldValue::Bool(_), FieldType::Boolean)
            | (FieldValue::Str(_), FieldType::String)
    )
}

pub(crate) fn value_type_name(v: &FieldValue) -> &'static str {
    match v {
        FieldValue::Integer(_) => "integer",
        FieldValue::Number(_) => "number",
        FieldValue::Bool(_) => "boolean",
        FieldValue::Str(_) => "string",
    }
}

/// The standard enter→exit event pairings the mirror-rule offer understands.
fn paired_exit_event(event: &str) -> Option<&'static str> {
    match event {
        "StateEntered" => Some("StateExited"),
        "ZoneEntered" => Some("ZoneExited"),
        _ => None,
    }
}

/// **Propose the inverse "cleanup" rule** for an add-on-enter rule (ADR-045 deliverable 4 / test #5 box 2)
/// — *"remove the flame when leaving `FacingBoss`?"*, because half of all game bugs are the missing "off"
/// switch. If `rule` triggers on an **enter** event (`StateEntered`/`ZoneEntered`) and at least one action
/// sets a boolean effect, return a mirror rule on the paired **exit** event with those boolean actions
/// flipped. The engine **offers** this for the user to accept (it authors as its own undoable rule) — never
/// forces it. `None` when there's no well-defined inverse (not an enter event, or no boolean-set action).
#[must_use]
pub fn propose_mirror(rule: &RuleData) -> Option<RuleData> {
    let exit = paired_exit_event(&rule.event)?;
    let inverse_actions: Vec<Action> = rule
        .actions
        .iter()
        .filter_map(|a| match a.value {
            FieldValue::Bool(b) => Some(Action {
                value: FieldValue::Bool(!b),
                ..a.clone()
            }),
            _ => None,
        })
        .collect();
    if inverse_actions.is_empty() {
        return None;
    }
    Some(RuleData {
        name: format!("{} (cleanup)", rule.name),
        enabled: rule.enabled,
        event: exit.to_string(),
        conditions: rule.conditions.clone(),
        actions: inverse_actions,
    })
}
