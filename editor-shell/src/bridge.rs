//! The core↔editor bridge: translate the editor's `EditTx` into a real `/core` commit and produce
//! the `ProjectionDelta` the editor projects — **through the real [`Engine`], no MockCore**. The JSON
//! shapes mirror the M2.5 editor's `protocol.ts` exactly (camelCase), so the same `ProjectionDelta` /
//! `EditTx` ride the M2.4 transport whether the peer is this Rust core (desktop) or the in-browser
//! WASM core.
//!
//! Rejections carry the **real** pipeline reason ("every 'no' explained"): the commit pipeline's
//! all-or-nothing validation (unknown entity, etc.) is the authoritative source of a "no" here;
//! semantic compat-ranking of binds is M3, not M2.

use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::World;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

// ── wire types (mirror editor/src/transport/protocol.ts) ───────────────────────

/// A per-entity projection op the editor applies to its store. Tagged by `op`, camelCase — identical
/// to the TS `ProjectionOp`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum ProjectionOp {
    #[serde(rename_all = "camelCase")]
    Upsert {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_id: Option<Option<String>>,
    },
    Remove {
        id: String,
    },
    #[serde(rename_all = "camelCase")]
    SetField {
        id: String,
        component: String,
        field: String,
        value: Json,
    },
    #[serde(rename_all = "camelCase")]
    RemoveField {
        id: String,
        component: String,
        field: String,
    },
    AddEdge {
        from: String,
        rel: String,
        to: String,
    },
    RemoveEdge {
        from: String,
        rel: String,
        to: String,
    },
}

/// A committed delta from the core (authoritative ops + which optimistic ops it confirms/rejects).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectionDelta {
    pub ops: Vec<ProjectionOp>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub confirms: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejects: Vec<RejectInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RejectInfo {
    pub client_op_id: String,
    pub reason: String,
}

/// A structured edit intent (the part of `EditTx` the core acts on). Tagged by `kind`, camelCase.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EditIntent {
    #[serde(rename_all = "camelCase")]
    SetField {
        id: String,
        component: String,
        field: String,
        value: Json,
    },
    Bind {
        from: String,
        rel: String,
        to: String,
    },
}

/// An edit transaction from the editor (UI→core). `patches` (JSON-Patch) is carried for parity with
/// the AI layer but the core acts on the structured `intent`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditTx {
    pub client_op_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub patches: Vec<Json>,
    pub intent: EditIntent,
}

// ── the bridge ─────────────────────────────────────────────────────────────────

/// Apply one editor `EditTx` to the real `engine` and return the `ProjectionDelta` to echo back:
/// `confirms` the client op on success, or `rejects` it with the pipeline's real reason on failure.
/// The engine state is the only source of truth; a rejected edit leaves it untouched (the pipeline is
/// all-or-nothing, M1.6).
pub fn apply_edit<W: World>(engine: &mut Engine<W>, tx: &EditTx) -> ProjectionDelta {
    match &tx.intent {
        EditIntent::SetField {
            id,
            component,
            field,
            value,
        } => {
            let Some(eid) = EntityId::from_loro_key(id) else {
                return reject(&tx.client_op_id, format!("malformed entity id '{id}'"));
            };
            let Some(fv) = json_to_field(value) else {
                return reject(
                    &tx.client_op_id,
                    format!("unsupported value for {component}.{field}"),
                );
            };
            let op = Op::SetField {
                entity: eid,
                component: component.clone(),
                field: field.clone(),
                value: fv,
            };
            match engine.commit(&tx.label, vec![op]) {
                Ok(()) => ProjectionDelta {
                    ops: vec![ProjectionOp::SetField {
                        id: id.clone(),
                        component: component.clone(),
                        field: field.clone(),
                        value: value.clone(),
                    }],
                    confirms: vec![tx.client_op_id.clone()],
                    rejects: vec![],
                },
                Err(e) => reject(&tx.client_op_id, e.to_string()),
            }
        }
        EditIntent::Bind { from, rel, to } => {
            let (Some(f), Some(t)) = (EntityId::from_loro_key(from), EntityId::from_loro_key(to))
            else {
                return reject(
                    &tx.client_op_id,
                    "bind references a malformed entity id".into(),
                );
            };
            let op = Op::AddBinding {
                from: f,
                kind: rel.clone(),
                to: t,
            };
            match engine.commit(&tx.label, vec![op]) {
                Ok(()) => ProjectionDelta {
                    ops: vec![ProjectionOp::AddEdge {
                        from: from.clone(),
                        rel: rel.clone(),
                        to: to.clone(),
                    }],
                    confirms: vec![tx.client_op_id.clone()],
                    rejects: vec![],
                },
                Err(e) => reject(&tx.client_op_id, e.to_string()),
            }
        }
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

/// Map a JSON scalar to a `/core` [`FieldValue`]. JSON has no int/float distinction, so an integral
/// number becomes `Integer`, otherwise `Number`.
fn json_to_field(v: &Json) -> Option<FieldValue> {
    match v {
        Json::Bool(b) => Some(FieldValue::Bool(*b)),
        Json::String(s) => Some(FieldValue::Str(s.clone())),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(FieldValue::Integer(i))
            } else {
                n.as_f64().map(FieldValue::Number)
            }
        }
        _ => None,
    }
}
