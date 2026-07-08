//! **Persistent re-import identity — the op-stream override re-binder** (M15.10, ADR-080). The uniquely-
//! enabled half of "re-import keeps all your work": every user override (material · collider · script ·
//! selection · the M15.9 joint/keyframe animation binding) is a **typed op whose entity-reference is re-bound
//! from the previous part's entity onto the geometrically-matched new part's entity** and replayed — the
//! whole re-import as ONE undoable commit.
//!
//! No incumbent does this: Datasmith re-import matches on names/hierarchy-paths and tracks a flat "overridden
//! y/n" flag; here the match is [`metrocalk_interchange`]'s rotation/translation-invariant geometric
//! fingerprint (M15.10 core), and the re-bind is a real op onto the matched entity. **A deleted part's
//! overrides are PRESERVED + FLAGGED** ("this was on a part that no longer exists — reassign or discard?"),
//! never silently lost; a **low-confidence match is surfaced for adjudication**, never auto-applied to a
//! load-bearing override (prefer-miss-over-wrong — a wrong bind silently corrupts, a miss is visible).
//!
//! `ReimportId` is the **stable engine identity carried on the entity**: the part's fingerprint + world
//! centroid + byte-hash, written at import so a later re-import can match the LIVE scene's parts without
//! re-parsing the old file.

use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::FlecsWorld;
use metrocalk_interchange::{
    match_identities, MatchKind, PartFingerprint, PartIdentity, ReimportPlan,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The component that carries a CAD part's **persistent geometric identity** on its entity (M15.10). Written
/// at import; read at re-import to match the live scene's parts. Fields are the fingerprint + placement, so
/// the matcher runs against the SCENE, not a re-parse of the old file.
pub const REIMPORT_ID: &str = "ReimportId";

/// The components whose presence on a CAD part entity is a **user override** to re-bind on re-import (a whole-
/// component copy): the M15.9 joint animation binding + its keyframes, and the M8 physics body/collider.
/// `MeshRenderer.material` is re-bound as a single FIELD (its `.mesh` is re-authored by the new import, so the
/// whole component is NOT copied). Transform / CadPart / `ReimportId` are import-authored (re-created by the
/// new import), never re-bound.
fn rebindable_components() -> [&'static str; 4] {
    [
        crate::kinematics::JOINT,       // the M15.9 animation binding
        crate::kinematics::JOINT_TRACK, // its keyframes
        "RigidBody",                    // physics override (M8)
        "Collider",                     // physics override (M8)
    ]
}

/// A captured set of a part entity's re-bindable overrides — the payload copied onto the matched new entity.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct OverrideSet {
    /// Whole re-bindable components (name → its full field map): Joint / JointTrack / RigidBody / Collider.
    pub components: BTreeMap<String, BTreeMap<String, FieldValue>>,
    /// The `MeshRenderer.material` preset override (a field, not the whole component) — `None` when unset.
    pub material: Option<String>,
}

impl OverrideSet {
    /// `true` when nothing was overridden (a bare, freshly-imported part) — nothing to re-bind or flag.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.components.is_empty() && self.material.is_none()
    }
}

/// Capture the re-bindable overrides on a CAD part entity (before it is replaced by the re-import).
#[must_use]
pub fn capture_overrides(engine: &Engine<FlecsWorld>, entity: EntityId) -> OverrideSet {
    let comps = engine.components_of(entity);
    let mut components = BTreeMap::new();
    for name in rebindable_components() {
        if let Some(fields) = comps.get(name) {
            if !fields.is_empty() {
                // Collect into a BTreeMap so the re-bind ops emit in a deterministic field order.
                let sorted: BTreeMap<String, FieldValue> =
                    fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                components.insert(name.to_string(), sorted);
            }
        }
    }
    let material = comps
        .get("MeshRenderer")
        .and_then(|m| m.get("material"))
        .and_then(|v| match v {
            FieldValue::Str(s) if !s.is_empty() => Some(s.clone()),
            _ => None,
        });
    OverrideSet {
        components,
        material,
    }
}

/// The ops that write a captured [`OverrideSet`] onto `target` (the matched new part entity) — the re-bind.
#[must_use]
pub fn rebind_ops(target: EntityId, ov: &OverrideSet) -> Vec<Op> {
    let mut ops = Vec::new();
    for (comp, fields) in &ov.components {
        for (field, value) in fields {
            ops.push(Op::SetField {
                entity: target,
                component: comp.clone(),
                field: field.clone(),
                value: value.clone(),
            });
        }
    }
    if let Some(mat) = &ov.material {
        ops.push(Op::SetField {
            entity: target,
            component: "MeshRenderer".into(),
            field: "material".into(),
            value: FieldValue::Str(mat.clone()),
        });
    }
    ops
}

/// A previous part's overrides that could NOT be re-bound because the part was removed (a [`MatchKind::Miss`]).
/// Preserved + surfaced — never silently dropped. The UX offers "reassign to another part / discard".
#[derive(Clone, PartialEq, Debug)]
pub struct OrphanedOverride {
    /// The previous part id (its overrides were captured here).
    pub old_id: u64,
    /// The part's name (for the "this material was on <name>" prompt).
    pub name: String,
    /// The captured overrides, preserved for reassignment.
    pub overrides: OverrideSet,
}

/// A low-confidence match surfaced for the user to confirm/reject before its override is re-bound.
#[derive(Clone, PartialEq, Debug)]
pub struct Adjudication {
    /// The previous part id.
    pub old_id: u64,
    /// The proposed new part id.
    pub new_id: u64,
    /// The match confidence `[0,1]`.
    pub confidence: f64,
    /// The overrides that WOULD re-bind on confirm (held, not applied).
    pub overrides: OverrideSet,
}

/// The outcome of planning a re-import re-bind: the ops that auto-re-bind (matched overrides onto the new
/// entities), the flagged orphans (removed parts' overrides), and the low-confidence items to adjudicate.
/// (No `PartialEq`: `Op` is not comparable — assert on `orphans`/`adjudicate`/`rebound`/`ops.len()`.)
#[derive(Clone, Debug, Default)]
pub struct RebindOutcome {
    /// The ops to commit — every matched override re-bound onto its geometrically-matched new entity. One
    /// undoable commit.
    pub ops: Vec<Op>,
    /// Removed parts whose overrides are preserved + flagged (never silently lost).
    pub orphans: Vec<OrphanedOverride>,
    /// Low-confidence matches surfaced for confirm/reject (their overrides held, NOT auto-applied).
    pub adjudicate: Vec<Adjudication>,
    /// How many overrides auto-re-bound (a matched part kept its work with no user action).
    pub rebound: usize,
}

/// Plan the override re-bind for a re-import. `old_entities`/`new_entities` map a matcher part id → the live
/// entity; `plan` is the [`match_identities`] verdict. For each match: capture the old entity's overrides and
/// (auto) re-bind onto the new entity, or (low-confidence) hold for adjudication; for each miss: flag the
/// orphan. **Prefer-miss-over-wrong is enforced by the plan** — a `Miss`/`LowConfidence` never auto-binds.
#[must_use]
pub fn plan_rebind(
    engine: &Engine<FlecsWorld>,
    old_entities: &BTreeMap<u64, EntityId>,
    new_entities: &BTreeMap<u64, EntityId>,
    plan: &ReimportPlan,
    names: &BTreeMap<u64, String>,
) -> RebindOutcome {
    let mut out = RebindOutcome::default();
    for m in &plan.matches {
        let Some(&old_e) = old_entities.get(&m.old_id) else {
            continue;
        };
        let ov = capture_overrides(engine, old_e);
        if ov.is_empty() {
            continue; // a bare part — nothing to preserve
        }
        match m.kind {
            MatchKind::Unchanged | MatchKind::Moved | MatchKind::Strong => {
                if let Some(new_e) = m.new_id.and_then(|nid| new_entities.get(&nid)) {
                    out.ops.extend(rebind_ops(*new_e, &ov));
                    out.rebound += 1;
                } else {
                    // Matched to a new id we don't have an entity for → treat as orphan (never lost).
                    out.orphans.push(OrphanedOverride {
                        old_id: m.old_id,
                        name: names.get(&m.old_id).cloned().unwrap_or_default(),
                        overrides: ov,
                    });
                }
            }
            MatchKind::LowConfidence => {
                if let Some(nid) = m.new_id {
                    out.adjudicate.push(Adjudication {
                        old_id: m.old_id,
                        new_id: nid,
                        confidence: m.confidence,
                        overrides: ov,
                    });
                }
            }
            MatchKind::Miss => {
                out.orphans.push(OrphanedOverride {
                    old_id: m.old_id,
                    name: names.get(&m.old_id).cloned().unwrap_or_default(),
                    overrides: ov,
                });
            }
        }
    }
    out
}

// ── The ReimportId component: the stable geometric identity carried on a part entity ────────────────────────

/// Build the ops that write a part's [`REIMPORT_ID`] onto its entity at import time (the stable geometric
/// identity a later re-import matches against). `pid` is the matcher part id.
#[must_use]
#[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
pub fn set_reimport_id_ops(
    entity: EntityId,
    pid: u64,
    reference: &str,
    mesh_hash: Option<u64>,
    world_centroid: [f64; 3],
    fp: &PartFingerprint,
) -> Vec<Op> {
    let mut ops = Vec::new();
    let mut set = |field: &str, value: FieldValue| {
        ops.push(Op::SetField {
            entity,
            component: REIMPORT_ID.into(),
            field: field.into(),
            value,
        });
    };
    set("pid", FieldValue::Str(format!("{pid:016x}")));
    set("reference", FieldValue::Str(reference.to_string()));
    set(
        "meshHash",
        FieldValue::Str(mesh_hash.map_or_else(String::new, |h| format!("{h:016x}"))),
    );
    set("cx", FieldValue::Number(world_centroid[0]));
    set("cy", FieldValue::Number(world_centroid[1]));
    set("cz", FieldValue::Number(world_centroid[2]));
    set("volume", FieldValue::Number(fp.volume));
    set("area", FieldValue::Number(fp.area));
    set("m0", FieldValue::Number(fp.moments[0]));
    set("m1", FieldValue::Number(fp.moments[1]));
    set("m2", FieldValue::Number(fp.moments[2]));
    set("tris", FieldValue::Integer(i64::from(fp.tri_count)));
    set("chi", FieldValue::Integer(i64::from(fp.chirality)));
    set(
        "surf",
        FieldValue::Str(
            fp.surface_hist
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(","),
        ),
    );
    ops
}

/// Read a part entity's [`REIMPORT_ID`] back into a [`PartIdentity`] — so the matcher can run against the LIVE
/// scene (the old parts) without re-parsing the old file. `None` when the entity carries no `ReimportId`.
#[must_use]
pub fn reimport_identity_of(engine: &Engine<FlecsWorld>, entity: EntityId) -> Option<PartIdentity> {
    let comps = engine.components_of(entity);
    let r = comps.get(REIMPORT_ID)?;
    let str_of = |f: &str| match r.get(f) {
        Some(FieldValue::Str(s)) => Some(s.clone()),
        _ => None,
    };
    let num_of = |f: &str| match r.get(f) {
        Some(FieldValue::Number(n)) => Some(*n),
        #[allow(clippy::cast_precision_loss)]
        Some(FieldValue::Integer(i)) => Some(*i as f64),
        _ => None,
    };
    let pid = u64::from_str_radix(&str_of("pid")?, 16).ok()?;
    let mesh_hash = str_of("meshHash")
        .filter(|s| !s.is_empty())
        .and_then(|s| u64::from_str_radix(&s, 16).ok());
    let mut surface_hist = [0u32; 5];
    if let Some(s) = str_of("surf") {
        for (k, part) in s.split(',').take(5).enumerate() {
            surface_hist[k] = part.trim().parse().unwrap_or(0);
        }
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let tri_count = num_of("tris").unwrap_or(0.0) as u32;
    #[allow(clippy::cast_possible_truncation)]
    let chirality = num_of("chi").unwrap_or(0.0) as i8;
    Some(PartIdentity {
        id: pid,
        reference: str_of("reference").unwrap_or_default(),
        mesh_hash,
        world_centroid: [num_of("cx")?, num_of("cy")?, num_of("cz")?],
        fingerprint: PartFingerprint {
            volume: num_of("volume").unwrap_or(0.0),
            area: num_of("area").unwrap_or(0.0),
            moments: [num_of("m0")?, num_of("m1")?, num_of("m2")?],
            tri_count,
            surface_hist,
            chirality,
        },
        name: String::new(),
        parent: None,
    })
}

/// Match the LIVE scene's previous CAD parts (each `(part id, entity)`) against a freshly-imported set of
/// [`PartIdentity`] — the re-import matcher run against the engine. Returns the plan; the caller feeds it to
/// [`plan_rebind`].
#[must_use]
pub fn match_scene_against(
    engine: &Engine<FlecsWorld>,
    old_entities: &BTreeMap<u64, EntityId>,
    new: &[PartIdentity],
) -> ReimportPlan {
    let old: Vec<PartIdentity> = old_entities
        .values()
        .filter_map(|&e| reimport_identity_of(engine, e))
        .collect();
    match_identities(&old, new)
}

// ── The live re-import orchestration + the never-silent per-part diff report (M15.10 convergence) ────────────

/// One row of the re-import diff — **every** previous + new part accounted for (the M15.7 never-silent
/// discipline applied to re-import: nothing silent, every fate explained). ECS/UI-queryable structured data.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ReimportDiffEntry {
    /// The previous part id (`0` for a purely-new/added part).
    pub old_id: u64,
    /// The re-imported part id it maps to (`None` for a removed part).
    pub new_id: Option<u64>,
    /// The human name.
    pub name: String,
    /// The fate: `"unchanged"` · `"moved"` · `"matched"` (re-bound) · `"adjudicate"` (held for the user) ·
    /// `"removed"` (overrides flagged) · `"added"` (new, bare). Assert off THIS, never UI copy.
    pub kind: String,
    /// Match confidence `[0,1]` (`1.0` for byte-hash unchanged/moved; `0.0` for removed/added).
    pub confidence: f64,
    /// Plain-language why (surfaced to the user — the never-silent reason).
    pub reason: String,
    /// `true` when this part had user overrides that were re-bound / flagged / held (so the UI can highlight
    /// the ones where "keep my work" actually did something).
    pub had_overrides: bool,
}

/// The full re-import outcome the live `land_cad` applies + the React surface renders.
#[derive(Clone, Debug, Default)]
pub struct ReimportSession {
    /// The ops to append to the import commit: the auto-re-binds (matched overrides onto the new entities) +
    /// the deactivation of every previous CAD entity (the stale version; deactivate-not-delete → undoable).
    pub commit_ops: Vec<Op>,
    /// The never-silent per-part diff (matched/moved/added/removed/adjudicate) — every part's fate explained.
    pub report: Vec<ReimportDiffEntry>,
    /// Removed parts whose overrides are preserved + flagged ("reassign or discard").
    pub orphans: Vec<OrphanedOverride>,
    /// Low-confidence matches held for confirm/reject (their overrides preserved, NOT applied).
    pub adjudicate: Vec<Adjudication>,
    /// How many overrides auto-re-bound with no user action.
    pub rebound: usize,
}

/// Orchestrate a re-import over the live scene: match the previous CAD parts (`old_entities`) to the freshly-
/// imported parts (`new_identities` + `new_entities`), then produce the ops to **re-bind every matched
/// override + deactivate every previous entity** (one undoable commit with the import), plus the never-silent
/// per-part diff report, the flagged orphans (removed), and the held adjudications (low-confidence). The
/// **prefer-miss-over-wrong discipline is preserved**: only `Unchanged`/`Moved`/`Strong` auto-re-bind; a
/// `LowConfidence` override is HELD, a `Miss` override is FLAGGED — never a silent wrong-bind.
#[must_use]
pub fn reimport_over_scene(
    engine: &Engine<FlecsWorld>,
    old_entities: &BTreeMap<u64, EntityId>,
    old_names: &BTreeMap<u64, String>,
    new_identities: &[PartIdentity],
    new_entities: &BTreeMap<u64, EntityId>,
) -> ReimportSession {
    let plan = match_scene_against(engine, old_entities, new_identities);
    let rb = plan_rebind(engine, old_entities, new_entities, &plan, old_names);
    let new_names: BTreeMap<u64, String> = new_identities
        .iter()
        .map(|p| (p.id, p.name.clone()))
        .collect();

    let mut session = ReimportSession {
        commit_ops: rb.ops,
        rebound: rb.rebound,
        orphans: rb.orphans,
        adjudicate: rb.adjudicate,
        ..Default::default()
    };

    // Deactivate every PREVIOUS CAD entity — it is the stale version, replaced by the new import's entity.
    // Deactivate-not-delete (ADR-026) so one Ctrl-Z restores the whole re-import. The re-bound overrides
    // already live on the NEW entities; the held/flagged ones are preserved in the session.
    for &e in old_entities.values() {
        session.commit_ops.push(Op::SetActive {
            entity: e,
            active: false,
        });
    }

    // The never-silent per-part diff — one row per previous part, plus one per added part.
    let has_override = |old_id: u64| -> bool {
        old_entities
            .get(&old_id)
            .is_some_and(|&e| !capture_overrides(engine, e).is_empty())
    };
    for m in &plan.matches {
        let name = old_names.get(&m.old_id).cloned().unwrap_or_default();
        let ov = has_override(m.old_id);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let pct = (m.confidence * 100.0).round() as u32;
        let (kind, reason) = match m.kind {
            MatchKind::Unchanged => ("unchanged", "kept — geometry unchanged".to_string()),
            MatchKind::Moved => (
                "moved",
                "kept — same part, moved to a new position; overrides re-bound".to_string(),
            ),
            MatchKind::Strong => (
                "matched",
                format!("kept — edited part matched ({pct}% confidence); overrides re-bound"),
            ),
            MatchKind::LowConfidence => (
                "adjudicate",
                format!("changed a lot ({pct}% confidence) — confirm this is the same part to keep its overrides"),
            ),
            MatchKind::Miss => (
                "removed",
                if ov {
                    "deleted from the CAD — its overrides are held for you to reassign or discard".to_string()
                } else {
                    "deleted from the CAD".to_string()
                },
            ),
        };
        session.report.push(ReimportDiffEntry {
            old_id: m.old_id,
            new_id: m.new_id,
            name,
            kind: kind.to_string(),
            confidence: m.confidence,
            reason,
            had_overrides: ov,
        });
    }
    for &new_id in &plan.added {
        session.report.push(ReimportDiffEntry {
            old_id: 0,
            new_id: Some(new_id),
            name: new_names.get(&new_id).cloned().unwrap_or_default(),
            kind: "added".to_string(),
            confidence: 0.0,
            reason: "new part — no previous overrides".to_string(),
            had_overrides: false,
        });
    }
    session
}
