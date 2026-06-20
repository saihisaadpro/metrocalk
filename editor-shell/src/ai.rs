//! The **AI seam** — schema-validated transactional patches (the marketplace→generate gate, invariant 3).
//!
//! "AI is a guest, never the foundation": every AI / generation output that mutates the scene enters
//! here as a **constrained, schema-validated patch** and is applied through the **one commit pipeline**
//! — validated against the registry schema + engine state, rejected-as-UX on failure, undoable on
//! success. There is **no raw/unvalidated LLM mutation path**. This is the MCP-surface contract: the
//! generation stream-in rides it **live**, and the AI-**edit** sibling ("make it rustier") is the *same*
//! generic `apply_ai_patch` — built + unit-tested at the function level, with its live editor command/UI
//! the **next increment** of this seam (`MeterAction::Edit` is not yet exercised by a live path). The
//! MCP *server* itself likewise stays a seam.
//!
//! The patch is **not** arbitrary RFC-6902 (which could touch anything): it's a small allow-listed set
//! of ops, each checked — the entity must exist, the component must be a known kind, the field must be
//! in that kind's schema, and the value's JSON type must match the field's declared type. All-or-nothing.

use metrocalk_core::{ComponentMeta, Engine, EntityId, FieldType, FieldValue, Op};
use metrocalk_ecs::World;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::bridge::{ProjectionDelta, ProjectionOp, RejectInfo};

/// One allow-listed AI patch operation. The only scene mutation an AI/generation output may request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum PatchOp {
    /// Set a component field on an existing entity (the generation mesh-swap + the AI-edit sink).
    SetField {
        id: String,
        component: String,
        field: String,
        value: Json,
    },
}

/// An AI patch — a client op id (for optimistic echo / rejection-as-UX, like an `EditTx`) + the ops.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiPatch {
    pub client_op_id: String,
    pub ops: Vec<PatchOp>,
}

/// Validate `patch` against the registry `schema` + engine state and — if **every** op is valid — apply
/// it through the one commit pipeline as a single undoable transaction (inv. 3). On any invalid op the
/// **whole** patch is rejected (all-or-nothing, rejection-as-UX); nothing is applied. Returns the
/// `ProjectionDelta` to echo: `confirms` + the ops on success, or `rejects` with the specific reason.
pub fn apply_ai_patch<W: World>(
    engine: &mut Engine<W>,
    schema: &[ComponentMeta],
    label: &str,
    patch: &AiPatch,
) -> ProjectionDelta {
    let mut ops = Vec::with_capacity(patch.ops.len());
    let mut proj = Vec::with_capacity(patch.ops.len());

    for p in &patch.ops {
        match validate_op(engine, schema, p) {
            Ok((op, projop)) => {
                ops.push(op);
                proj.push(projop);
            }
            Err(reason) => return reject(&patch.client_op_id, reason),
        }
    }

    match engine.commit(label, ops) {
        Ok(()) => ProjectionDelta {
            ops: proj,
            confirms: vec![patch.client_op_id.clone()],
            rejects: vec![],
        },
        Err(e) => reject(&patch.client_op_id, e.to_string()),
    }
}

/// Validate one op against the schema + engine, returning the pipeline `Op` + the projection echo, or a
/// specific rejection reason.
fn validate_op<W: World>(
    engine: &Engine<W>,
    schema: &[ComponentMeta],
    p: &PatchOp,
) -> Result<(Op, ProjectionOp), String> {
    match p {
        PatchOp::SetField {
            id,
            component,
            field,
            value,
        } => {
            let eid =
                EntityId::from_loro_key(id).ok_or_else(|| format!("malformed entity id '{id}'"))?;
            if !engine.entity_exists(eid) {
                return Err(format!("entity '{id}' does not exist"));
            }
            let meta = schema
                .iter()
                .find(|m| &m.name == component)
                .ok_or_else(|| format!("unknown component '{component}'"))?;
            let spec = meta
                .fields
                .iter()
                .find(|f| &f.name == field)
                .ok_or_else(|| format!("component '{component}' has no field '{field}'"))?;
            let fv = coerce(value, spec.ty)
                .ok_or_else(|| format!("value for {component}.{field} is not a {:?}", spec.ty))?;
            Ok((
                Op::SetField {
                    entity: eid,
                    component: component.clone(),
                    field: field.clone(),
                    value: fv,
                },
                ProjectionOp::SetField {
                    id: id.clone(),
                    component: component.clone(),
                    field: field.clone(),
                    value: value.clone(),
                },
            ))
        }
    }
}

/// **Strict** JSON→`FieldValue` coercion (unlike the lenient editor path): the JSON type must match the
/// field's declared type, so an over-reaching/malformed AI value is rejected, not silently coerced.
fn coerce(v: &Json, ty: FieldType) -> Option<FieldValue> {
    match ty {
        FieldType::Integer => v.as_i64().map(FieldValue::Integer),
        FieldType::Number => v.as_f64().map(FieldValue::Number),
        FieldType::Boolean => v.as_bool().map(FieldValue::Bool),
        FieldType::String => v.as_str().map(|s| FieldValue::Str(s.to_string())),
    }
}

fn reject(client_op_id: &str, reason: String) -> ProjectionDelta {
    ProjectionDelta {
        ops: vec![],
        confirms: vec![],
        rejects: vec![RejectInfo {
            client_op_id: client_op_id.to_string(),
            reason,
        }],
    }
}
