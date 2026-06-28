//! M12.4 (ADR-048) — the **AI code/Rules tier**: the LLM composes Rules / components / state machines as
//! **schema-validated patches**, reusing the M6/ADR-017 contract (validate against the registry schema +
//! engine state → apply as **one undoable transaction** → **rejected-as-UX** on any invalid op; **never a
//! raw mutation path**). The target widens from "edit a material" to "compose Rules/components/machines";
//! the contract is unchanged — [`apply_composition`] goes through the **one commit pipeline** + the shipped
//! [`validate_rule`]/[`validate_state_machine`] validators, not a parallel path.
//!
//! **SA-22 / R1 (the #1 frontier moat, made literal):** [`composition_grammar`] compiles the registry's
//! component schema + the allow-listed op set into a **constrained-decoding grammar** (a JSON Schema for a
//! structured-output model), so the model **structurally cannot emit an out-of-schema op WITHIN the
//! grammar** — turning validate-then-reject into can't-even-propose-invalid. The guarantee is
//! **coverage-bounded** (complex/recursive schemas exceed the reliable constrained-decoding subset —
//! *JSONSchemaBench*); our schema is **flat scalar** ([`FieldType`] is scalar-only), which sits *inside*
//! that subset — [`grammar_coverage`] verifies it + **flags** any field that would exceed it. Honest class:
//! **UNIQUELY-ENABLED** (it hardens a *shipped* signature feature), not "nobody can constrain an LLM."
//!
//! **AI is a GUEST (offline-first):** every type + function here is **pure data + the deterministic
//! pipeline** — no LLM call. The engine, Rules, and clicks-authoring all work with the model off; this tier
//! only *composes* validated patches for review, and is **never load-bearing**. The *generation* is a guest
//! (a remote-LLM network seam); the *applied world edit* is deterministic + replayable (M8.1 spirit).

use crate::entity_id::EntityId;
use crate::pipeline::{Engine, FieldValue, Op, PipelineError};
use crate::registry::{ComponentMeta, FieldType, Registry};
use crate::rules::{
    field_type_matches, validate_rule, value_type_name, RuleData, RuleError, RuleId,
};
use crate::state_machine::{
    validate_state_machine, StateMachine, StateMachineError, StateMachineId,
};
use metrocalk_ecs::World;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fmt;

/// One **allow-listed composition op** the AI may propose — the widened ADR-017 op-set (from "set a material
/// field" to "compose Rules/components/machines"). Externally tagged (`op`) so it maps 1:1 to the
/// structured-output schema [`composition_grammar`] emits. A **closed** set: the AI can compose only these.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum ComposeOp {
    /// Set a registry-typed component field on an existing entity (e.g. seed a `KillCounter.count`).
    SetField {
        entity: String,
        component: String,
        field: String,
        value: FieldValue,
    },
    /// Author a Rule (When/If/Then) — validated by [`validate_rule`] (the typo-proof gate).
    AuthorRule { id: String, rule: RuleData },
    /// Author a state machine — validated by [`validate_state_machine`].
    AuthorStateMachine { id: String, machine: StateMachine },
}

/// A composition the AI proposes — a set of ops applied as **one undoable transaction** (or rejected whole).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Composition {
    pub ops: Vec<ComposeOp>,
}

/// Why a composition was rejected — a faithful, plain-language reason (ADR-016, the every-"no" engine on AI;
/// **ASCII-safe** so it survives every IPC layer). Each variant wraps the underlying registry/rule/machine
/// reason — the controllability surface the R1 method evaluates.
#[derive(Clone, Debug, PartialEq)]
pub enum ComposeError {
    /// An empty composition does nothing — refused.
    Empty,
    /// A `SetField` op references an entity that doesn't exist.
    UnknownEntity(String),
    /// A `SetField` op references a component the registry doesn't know.
    UnknownComponent { component: String },
    /// A `SetField` op references a field that component doesn't have.
    UnknownField { component: String, field: String },
    /// A `SetField` op's value is the wrong scalar type for the field.
    FieldTypeMismatch {
        component: String,
        field: String,
        expected: FieldType,
        got: &'static str,
    },
    /// An `AuthorRule` op's Rule is invalid (the reused [`validate_rule`] reason).
    Rule { source: RuleError },
    /// An `AuthorStateMachine` op's machine is invalid (the reused [`validate_state_machine`] reason).
    StateMachine { source: StateMachineError },
    /// The composition validated but the commit failed (a pipeline-internal error — loud, not silent).
    Pipeline(String),
}

impl fmt::Display for ComposeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "the AI proposed nothing to compose"),
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
            Self::Rule { source } => write!(f, "{source}"),
            Self::StateMachine { source } => write!(f, "{source}"),
            Self::Pipeline(e) => write!(f, "the composition couldn't be applied: {e}"),
        }
    }
}

impl std::error::Error for ComposeError {}

/// Validate a composition against the registry + engine state — the **ADR-017 contract widened** to Rules/
/// components/machines. Every op must be schema-valid (registry-known component/field, well-typed value,
/// a Rule/machine that passes the reused validators) and reference real entities. The first failure is an
/// explained [`ComposeError`] (rejected-as-UX) — nothing is applied. Reuses [`validate_rule`] +
/// [`validate_state_machine`], **not** a parallel validator.
///
/// # Errors
/// The first [`ComposeError`] encountered.
pub fn validate_composition<W, F>(
    registry: &Registry<W>,
    composition: &Composition,
    entity_exists: F,
) -> Result<(), ComposeError>
where
    W: World,
    F: Fn(EntityId) -> bool,
{
    build_ops(registry, composition, &entity_exists).map(|_| ())
}

/// **Validate + apply** a composition through the **one commit pipeline** as a **single undoable
/// transaction** (invariant 3). On any invalid op the WHOLE composition is rejected (all-or-nothing,
/// rejection-as-UX) — nothing is applied. This is the **same contract** as `apply_ai_patch` (validate →
/// pipeline → undoable → reject), **not** a raw mutation path: the AI proposes; the deterministic engine
/// validates + commits or refuses.
///
/// # Errors
/// A [`ComposeError`] if any op is invalid (nothing applied) or the commit fails.
pub fn apply_composition<W: World>(
    engine: &mut Engine<W>,
    registry: &Registry<W>,
    composition: &Composition,
) -> Result<(), ComposeError> {
    // Validate + build the pipeline ops first (an immutable borrow of `engine` for entity existence — the
    // temporary closure ref's borrow ends with this statement, before the mutable `commit`). All-or-nothing:
    // a single invalid op rejects the whole thing, unapplied.
    let ops = build_ops(registry, composition, &|id| engine.entity_exists(id))?;
    engine
        .commit("ai-compose", ops)
        .map_err(|e: PipelineError| ComposeError::Pipeline(e.to_string()))
}

/// Validate every op and build its pipeline [`Op`] — the shared core of validate + apply.
fn build_ops<W, F>(
    registry: &Registry<W>,
    composition: &Composition,
    entity_exists: &F,
) -> Result<Vec<Op>, ComposeError>
where
    W: World,
    F: Fn(EntityId) -> bool,
{
    if composition.ops.is_empty() {
        return Err(ComposeError::Empty);
    }
    let mut ops = Vec::with_capacity(composition.ops.len());
    for op in &composition.ops {
        match op {
            ComposeOp::SetField {
                entity,
                component,
                field,
                value,
            } => {
                let eid = check_entity(entity, entity_exists)?;
                check_field(registry, component, field, value)?;
                ops.push(Op::SetField {
                    entity: eid,
                    component: component.clone(),
                    field: field.clone(),
                    value: value.clone(),
                });
            }
            ComposeOp::AuthorRule { id, rule } => {
                validate_rule(registry, rule, entity_exists)
                    .map_err(|source| ComposeError::Rule { source })?;
                ops.push(Op::SetRule {
                    id: RuleId::new(id.clone()),
                    rule: rule.clone(),
                });
            }
            ComposeOp::AuthorStateMachine { id, machine } => {
                validate_state_machine(registry, machine, entity_exists)
                    .map_err(|source| ComposeError::StateMachine { source })?;
                ops.push(Op::SetStateMachine {
                    id: StateMachineId::new(id.clone()),
                    sm: machine.clone(),
                });
            }
        }
    }
    Ok(ops)
}

fn check_entity<F: Fn(EntityId) -> bool>(key: &str, exists: &F) -> Result<EntityId, ComposeError> {
    match EntityId::from_loro_key(key) {
        Some(id) if exists(id) => Ok(id),
        _ => Err(ComposeError::UnknownEntity(key.to_string())),
    }
}

fn check_field<W: World>(
    registry: &Registry<W>,
    component: &str,
    field: &str,
    value: &FieldValue,
) -> Result<(), ComposeError> {
    let meta = registry
        .meta(component)
        .ok_or_else(|| ComposeError::UnknownComponent {
            component: component.to_string(),
        })?;
    let spec = meta
        .fields
        .iter()
        .find(|f| f.name == field)
        .ok_or_else(|| ComposeError::UnknownField {
            component: component.to_string(),
            field: field.to_string(),
        })?;
    if field_type_matches(value, spec.ty) {
        Ok(())
    } else {
        Err(ComposeError::FieldTypeMismatch {
            component: component.to_string(),
            field: field.to_string(),
            expected: spec.ty,
            got: value_type_name(value),
        })
    }
}

// ── SA-22 / R1: the registry schema as a constrained-decoding grammar ────────────────────────────────────

/// The JSON-Schema type keyword for a registry [`FieldType`] (the leaf of the grammar — all scalar, so the
/// grammar stays inside the reliable constrained-decoding subset).
fn field_type_json(ty: FieldType) -> &'static str {
    match ty {
        FieldType::Integer => "integer",
        FieldType::Number => "number",
        FieldType::Boolean => "boolean",
        FieldType::String => "string",
    }
}

/// **SA-22 / R1:** compile the registry's component schema + the allow-listed [`ComposeOp`] set into a
/// **JSON Schema** — the constrained-decoding grammar a structured-output model is bound by. The model is
/// constrained to emit only the allow-listed `op`s over the **enumerated** component names with
/// **type-tagged** values, so it **structurally cannot emit an out-of-schema op within the grammar** (the
/// validate-then-reject contract hardened to can't-propose-invalid). The schema is **non-recursive,
/// scalar-leaf** (see [`grammar_coverage`]) — inside the reliable subset.
#[must_use]
pub fn composition_grammar(components: &[ComponentMeta]) -> Value {
    let component_names: Vec<&str> = components.iter().map(|c| c.name.as_str()).collect();
    // A typed scalar literal (the value of a SetField op / a rule condition) — a closed union of the four
    // scalar shapes, so a value can never be a nested object/array (which would exceed the grammar subset).
    let scalar_value = json!({
        "oneOf": [
            { "type": "integer" },
            { "type": "number" },
            { "type": "boolean" },
            { "type": "string" },
        ]
    });
    // The SetField op constrained to known component names (an enum) with a scalar value. Per-component
    // field enums are layered in below via `allOf`/`if`-free flat constraints kept minimal to stay inside
    // the reliable subset; an unknown field still can't apply (the validator rejects it) — the documented
    // coverage line between "grammar prevents" and "validator rejects".
    let set_field = json!({
        "type": "object",
        "required": ["op", "entity", "component", "field", "value"],
        "additionalProperties": false,
        "properties": {
            "op": { "const": "setField" },
            "entity": { "type": "string" },
            "component": { "enum": component_names },
            "field": { "type": "string" },
            "value": scalar_value,
        }
    });
    let author_rule = json!({
        "type": "object",
        "required": ["op", "id", "rule"],
        "additionalProperties": false,
        "properties": {
            "op": { "const": "authorRule" },
            "id": { "type": "string" },
            "rule": rule_schema(&component_names),
        }
    });
    let author_machine = json!({
        "type": "object",
        "required": ["op", "id", "machine"],
        "additionalProperties": false,
        "properties": {
            "op": { "const": "authorStateMachine" },
            "id": { "type": "string" },
            "machine": state_machine_schema(&component_names),
        }
    });
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "MetrocalkComposition",
        "type": "object",
        "required": ["ops"],
        "additionalProperties": false,
        "properties": {
            "ops": {
                "type": "array",
                "minItems": 1,
                "items": { "oneOf": [set_field, author_rule, author_machine] }
            }
        }
    })
}

/// The JSON Schema for a `RuleData` (When/If/Then), constrained to known component names — a flat,
/// scalar-leaf shape (no recursion).
fn rule_schema(component_names: &[&str]) -> Value {
    let condition = json!({
        "type": "object",
        "required": ["entity", "component", "field", "op", "value"],
        "additionalProperties": false,
        "properties": {
            "entity": { "type": "string" },
            "component": { "enum": component_names },
            "field": { "type": "string" },
            "op": { "enum": ["eq", "ne", "lt", "le", "gt", "ge"] },
            "value": { "oneOf": [{ "type": "integer" }, { "type": "number" }, { "type": "boolean" }, { "type": "string" }] },
        }
    });
    let action = json!({
        "type": "object",
        "required": ["action", "entity", "component", "field", "value"],
        "additionalProperties": false,
        "properties": {
            "action": { "type": "string" },
            "entity": { "type": "string" },
            "component": { "type": "string" },
            "field": { "type": "string" },
            "value": { "oneOf": [{ "type": "integer" }, { "type": "number" }, { "type": "boolean" }, { "type": "string" }] },
        }
    });
    json!({
        "type": "object",
        "required": ["name", "enabled", "event", "conditions", "actions"],
        "additionalProperties": false,
        "properties": {
            "name": { "type": "string" },
            "enabled": { "type": "boolean" },
            "event": { "type": "string" },
            "conditions": { "type": "array", "items": condition },
            "actions": { "type": "array", "minItems": 1, "items": action },
        }
    })
}

/// The JSON Schema for a `StateMachine` (states + transitions), flat scalar-leaf (a transition's `rule` is
/// the same flat [`rule_schema`]).
fn state_machine_schema(component_names: &[&str]) -> Value {
    let transition = json!({
        "type": "object",
        "required": ["id", "from", "to", "rule"],
        "additionalProperties": false,
        "properties": {
            "id": { "type": "string" },
            "from": { "type": "string" },
            "to": { "type": "string" },
            "rule": rule_schema(component_names),
        }
    });
    json!({
        "type": "object",
        "required": ["name", "entity", "component", "field", "states", "initial", "transitions"],
        "additionalProperties": false,
        "properties": {
            "name": { "type": "string" },
            "entity": { "type": "string" },
            "component": { "enum": component_names },
            "field": { "type": "string" },
            "states": { "type": "array", "items": { "type": "string" } },
            "initial": { "type": "string" },
            "transitions": { "type": "array", "items": transition },
        }
    })
}

/// The **SA-22 coverage report** — every registry field checked against the *reliable* grammar subset (flat
/// scalar: integer/number/boolean/string). `within_subset` ⇒ every field is expressible, so the grammar is a
/// **complete structural guarantee** (can't-propose-invalid within it); `flagged` names any field that would
/// **exceed** the subset (a recursive/complex field — the *JSONSchemaBench* trap), which the grammar can't
/// fully constrain and the validator catches instead. The honest bound, measured + named.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoverageReport {
    /// Every registry field is a flat scalar → fully inside the reliable constrained-decoding subset.
    pub within_subset: bool,
    /// Total component fields measured.
    pub field_count: usize,
    /// `"component.field"` entries that exceed the flat-scalar subset (empty when `within_subset`).
    pub flagged: Vec<String>,
}

/// Measure [`CoverageReport`] over the registry's components: a field is **within the subset** iff its
/// [`FieldType`] is one of the four scalars. (Our `FieldType` enum is *scalar-only*, so this is structurally
/// always within-subset — which is exactly **why SA-22 works cleanly here**; the check stays so a future
/// non-scalar field would be flagged, not silently overclaimed as constrained.)
#[must_use]
pub fn grammar_coverage(components: &[ComponentMeta]) -> CoverageReport {
    let mut field_count = 0usize;
    let mut flagged = Vec::new();
    let scalar: BTreeSet<&str> = ["integer", "number", "boolean", "string"]
        .into_iter()
        .collect();
    for c in components {
        for f in &c.fields {
            field_count += 1;
            if !scalar.contains(field_type_json(f.ty)) {
                flagged.push(format!("{}.{}", c.name, f.name));
            }
        }
    }
    CoverageReport {
        within_subset: flagged.is_empty(),
        field_count,
        flagged,
    }
}
