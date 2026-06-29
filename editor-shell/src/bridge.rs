//! The core↔editor bridge: translate the editor's `EditTx` into a real `/core` commit and produce
//! the `ProjectionDelta` the editor projects — **through the real [`Engine`], no MockCore**. The JSON
//! shapes mirror the M2.5 editor's `protocol.ts` exactly (camelCase), so the same `ProjectionDelta` /
//! `EditTx` ride the M2.4 transport whether the peer is this Rust core (desktop) or the in-browser
//! WASM core.
//!
//! Rejections carry the **real** pipeline reason ("every 'no' explained"): the commit pipeline's
//! all-or-nothing validation (unknown entity, etc.) is the authoritative source of a "no" here;
//! semantic compat-ranking of binds is M3, not M2.

use std::collections::{HashMap, HashSet};

use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::{Entity, World};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::reveal::{required_caps, Rels};

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
        /// Deactivate-not-delete state (ADR-026): `Some(false)` marks a deactivated ("deleted",
        /// recoverable) entity so the hierarchy can dim/strike it — and so the mark SURVIVES a reload
        /// (the persisted SetActive replays, `project_full` re-emits `active:false`). Absent ⇒ unchanged.
        #[serde(skip_serializing_if = "Option::is_none")]
        active: Option<bool>,
        /// M14.2 (ADR-058) — the salient type for the type-icon/thumbnail fallback, classified server-side
        /// from the entity's components so the hierarchy needs no component subscription (M2.5). Absent ⇒
        /// the UI keeps the entity's prior kind. Set by [`enrich_relational`] on the projection path only.
        #[serde(skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        /// M14.2 (ADR-058) — the live relational summary (the C6 closure): requires/provides/bound/
        /// needsBinding keyed off the REAL `(Requires/Provides, cap)` ECS pairs + `bindings()`. A read/render
        /// projection — never authored into the doc. Set by [`enrich_relational`]; absent ⇒ UI keeps prior.
        #[serde(skip_serializing_if = "Option::is_none")]
        rel: Option<RelSummary>,
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

/// M14.2 (ADR-058) — the live per-entity relational summary, the C6 closure. Mirrors the TS `RelSummary`
/// (camelCase). Computed from the real `(Requires/Provides, cap)` ECS pairs + `bindings()`; a read/render
/// projection that NEVER enters the op-stream/Loro doc (zero determinism impact). Rides the `Upsert` op so
/// it lands on the hierarchy SUMMARY (a row re-renders only when its relational status flips — M2.5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RelSummary {
    /// Capability names this entity REQUIRES (non-empty ⇒ a requirer).
    pub requires: Vec<String>,
    /// Capability names this entity PROVIDES.
    pub provides: Vec<String>,
    /// Count of this entity's outgoing bindings (BindsTo edges).
    pub bound: usize,
    /// A required capability is not yet satisfied by an existing binding ("needs a binding") — the
    /// authoritative requirer signal (the same predicate `actions_for`'s `Bind…` availability uses).
    pub needs_binding: bool,
    /// This entity is a group/identity parent node. (Reserved; group membership is also carried by `parentId`.)
    pub is_group: bool,
}

/// A committed delta from the core (authoritative ops + which optimistic ops it confirms/rejects).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectionDelta {
    pub ops: Vec<ProjectionOp>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub confirms: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejects: Vec<RejectInfo>,
    /// `true` only for a server-initiated **full re-projection** (`project_full` — sent on connect, undo,
    /// sim-restart, project open/new). The UI projection store treats it as a REPLACE (drop stale
    /// entities/edges), not an incremental merge — so e.g. an undone bind's edge can't linger. Default
    /// `false` (an incremental/echo delta); skipped on the wire when false to keep deltas lean.
    #[serde(default, skip_serializing_if = "is_false")]
    pub full: bool,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
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
                    full: false,
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
                    full: false,
                },
                Err(e) => reject(&tx.client_op_id, e.to_string()),
            }
        }
    }
}

/// Project the whole live scene from the real engine into a single `ProjectionDelta` (the initial
/// load the editor's store applies). Each entity becomes an `upsert` (id as name until a `Name`
/// component lands) with one `setField` per component field; each binding becomes an `addEdge`. This
/// is the `/core` → editor seam the desktop shell sends over the M2.4 Channel on connect; the browser
/// WASM core projects the
/// same way (ADR-006). No `confirms`/`rejects` — it's a server-initiated load, not an echo.
pub fn project_full<W: World>(engine: &Engine<W>) -> ProjectionDelta {
    let mut ops = Vec::new();
    for id in engine.entity_ids() {
        let key = id.to_loro_key();
        let parent = engine.parent_of(id).map(|p| p.to_loro_key());
        let comps = engine.components_of(id);
        ops.push(ProjectionOp::Upsert {
            id: key.clone(),
            name: Some(entity_label(&comps, &key)),
            parent_id: Some(parent),
            active: Some(engine.is_active(id)), // carry deactivate state so it survives reload (R-NEXT-2)
            kind: None, // filled by `enrich_relational` (M14.2) at the send boundary
            rel: None,
        });
        for (component, fields) in comps {
            for (field, value) in fields {
                ops.push(ProjectionOp::SetField {
                    id: key.clone(),
                    component: component.clone(),
                    field,
                    value: field_to_json(&value),
                });
            }
        }
    }
    for (from, rel, to) in engine.bindings() {
        ops.push(ProjectionOp::AddEdge {
            from: from.to_loro_key(),
            rel,
            to: to.to_loro_key(),
        });
    }
    ProjectionDelta {
        ops,
        confirms: vec![],
        rejects: vec![],
        full: true, // a FULL re-projection → the UI store REPLACES (drops stale entities/edges, e.g. on undo)
    }
}

/// Project a **single** entity into a delta — an `upsert` + one `setField` per component field. The
/// targeted echo for a newly-created entity (e.g. M3.2 describe-to-create), so the editor learns the
/// new entity without re-projecting the whole scene (deltas only, invariant 2).
pub fn project_entity<W: World>(engine: &Engine<W>, id: EntityId) -> ProjectionDelta {
    let key = id.to_loro_key();
    let parent = engine.parent_of(id).map(|p| p.to_loro_key());
    let comps = engine.components_of(id);
    let mut ops = vec![ProjectionOp::Upsert {
        id: key.clone(),
        name: Some(entity_label(&comps, &key)),
        parent_id: Some(parent),
        active: Some(engine.is_active(id)), // carry deactivate state (R-NEXT-2)
        kind: None, // filled by `enrich_relational` (M14.2) at the send boundary
        rel: None,
    }];
    for (component, fields) in comps {
        for (field, value) in fields {
            ops.push(ProjectionOp::SetField {
                id: key.clone(),
                component: component.clone(),
                field,
                value: field_to_json(&value),
            });
        }
    }
    ProjectionDelta {
        ops,
        confirms: vec![],
        rejects: vec![],
        full: false, // an incremental single-entity echo — merged, not a replace
    }
}

/// The entity's display name for the projection — the user-set `__meta__.name` (M10.6 rename) if present,
/// else the loro key. Takes the ALREADY-fetched components (project_full/project_entity fetch them once per
/// entity) so it adds **no extra `components_of` call** — the projection stays single-fetch per entity at 5k.
fn entity_label(
    comps: &std::collections::HashMap<String, std::collections::HashMap<String, FieldValue>>,
    key: &str,
) -> String {
    comps
        .get("__meta__")
        .and_then(|m| m.get("name"))
        .and_then(|v| match v {
            FieldValue::Str(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_else(|| key.to_string())
}

fn field_to_json(v: &FieldValue) -> Json {
    match v {
        FieldValue::Integer(i) => Json::from(*i),
        FieldValue::Number(n) => Json::from(*n),
        FieldValue::Bool(b) => Json::from(*b),
        FieldValue::Str(s) => Json::from(s.clone()),
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
        full: false,
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

/// M14.2 (ADR-058) — the C6 relational projection. Fill each `Upsert` op's `kind` + `rel` (the live
/// binding/requirer truth) from the REAL `(Requires/Provides, cap)` ECS pairs + `engine.bindings()`,
/// **reusing** the M3.1 [`required_caps`] query + the `actions_for` satisfaction correlation — so the
/// hierarchy/Requirers read structured truth off the projection (retiring the brittle `HealthBar`
/// component-name filter / MockCore's `Socket`/`Provides` fiction). A **read/render projection** — never
/// authored into the doc (zero determinism impact). Computed off the discrete projection path (a full
/// re-projection / a targeted echo), **never** per-frame (invariant 4). A delta carrying no `Upsert` is a
/// no-op. `kind` is derived from THIS delta's own `SetField` components (no extra fetch); absent when the
/// delta carries no fields for that entity (the UI then keeps the entity's prior kind).
// `cap_name` is the app-owned, default-hasher registry map (mirrors `reveal`'s `implicit_hasher` allow).
#[allow(clippy::implicit_hasher)]
pub fn enrich_relational<W: World>(
    delta: &mut ProjectionDelta,
    engine: &Engine<W>,
    rels: Rels,
    cap_name: &HashMap<Entity, String>,
) {
    if !delta
        .ops
        .iter()
        .any(|op| matches!(op, ProjectionOp::Upsert { .. }))
    {
        return;
    }
    // Component names per upserted id, from THIS delta's `SetField` ops (so `kind` needs no extra fetch).
    // Owned (not `&str` into `delta.ops`) so the mutable pass below doesn't conflict with this borrow.
    let mut comp_names: HashMap<String, Vec<String>> = HashMap::new();
    for op in &delta.ops {
        if let ProjectionOp::SetField { id, component, .. } = op {
            comp_names
                .entry(id.clone())
                .or_default()
                .push(component.clone());
        }
    }
    // One bindings scan → from-entity → its bound providers (for `bound` + the satisfaction set).
    let mut bound_to: HashMap<EntityId, Vec<EntityId>> = HashMap::new();
    for (from, _rel, to) in engine.bindings() {
        bound_to.entry(from).or_default().push(to);
    }
    for op in &mut delta.ops {
        let ProjectionOp::Upsert {
            id, kind, rel, ..
        } = op
        else {
            continue;
        };
        let Some(eid) = EntityId::from_loro_key(id) else {
            continue;
        };
        let Some(ecs) = engine.ecs_entity(eid) else {
            continue;
        };
        let req_caps = required_caps(engine.world(), ecs, rels);
        let requires: Vec<String> = req_caps
            .iter()
            .filter_map(|c| cap_name.get(c).cloned())
            .collect();
        let provides: Vec<String> = engine
            .world()
            .targets(ecs, rels.provides)
            .iter()
            .filter_map(|c| cap_name.get(c).cloned())
            .collect();
        // The needs-binding predicate: a required cap NOT satisfied by an existing binding (correlate each
        // outgoing binding to the caps its bound provider actually provides — the exact `actions_for` logic).
        let mut satisfied: HashSet<Entity> = HashSet::new();
        let bound = bound_to.get(&eid).map_or(0, |tos| {
            for to in tos {
                if let Some(to_ecs) = engine.ecs_entity(*to) {
                    for cap in engine.world().targets(to_ecs, rels.provides) {
                        satisfied.insert(cap);
                    }
                }
            }
            tos.len()
        });
        let needs_binding = req_caps.iter().any(|c| !satisfied.contains(c));
        *rel = Some(RelSummary {
            requires,
            provides,
            bound,
            needs_binding,
            is_group: false,
        });
        if let Some(cs) = comp_names.get(id.as_str()) {
            *kind = Some(classify_kind(cs));
        }
    }
}

/// Classify an entity's salient type from its component names — the type-icon vocabulary the hierarchy
/// renders (kept in sync with the TS `deriveKind`). Generic over `&str`/`String` so the projection path
/// (owned `String`s) and the unit test (`&str` literals) share one implementation.
fn classify_kind<S: AsRef<str>>(components: &[S]) -> String {
    let has = |n: &str| components.iter().any(|c| c.as_ref() == n);
    if has("Light") {
        "light"
    } else if has("Camera") {
        "camera"
    } else if has("RigidBody") || has("Collider") {
        "physics"
    } else if has("AudioSource") {
        "audio"
    } else if has("HealthBar") {
        "requirer"
    } else if has("MeshRenderer") {
        "mesh"
    } else {
        "default"
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_kind_keys_off_the_real_component_vocabulary() {
        assert_eq!(classify_kind(&["Transform", "HealthBar"]), "requirer");
        assert_eq!(classify_kind(&["Transform", "MeshRenderer"]), "mesh");
        assert_eq!(classify_kind(&["Light"]), "light");
        assert_eq!(classify_kind(&["Camera"]), "camera");
        assert_eq!(classify_kind(&["RigidBody", "MeshRenderer"]), "physics");
        assert_eq!(classify_kind(&["Transform"]), "default");
        // a renderable requirer (HealthBar + a mesh) reads as a requirer (the binding state is the salient cue)
        assert_eq!(classify_kind(&["HealthBar", "MeshRenderer"]), "requirer");
    }
}
