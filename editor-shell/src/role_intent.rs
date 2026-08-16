//! Gameplay roles — ONE commit turns any asset into a live gameplay participant.
//!
//! The ease-of-making-a-game seam: an imported mesh or a Shape Studio solid is *dead geometry* until
//! it has behaviour, and wiring that by hand takes four surfaces (Inspector components, the Relations
//! reveal, the Logic rule builder, the Animate timeline). A **role** is the curated bundle: pick
//! "Collectible" and one undoable transaction writes the `GameRole` component, the spin animation,
//! the shared Score counter, the pickup rule (written once, scoped per-entity via `$subject`), and a
//! visible binding to the Score — then Play makes it real: the loop fires `Touched` when something
//! rolls into it, the rule collects it, the play overlay hides it, the score climbs.
//!
//! Everything here is plain pipeline ops (SetField / AddPair / AddBinding / SetRule) in ONE
//! `engine.commit("assign-role", …)` — one Ctrl-Z removes the whole role. The authored document is
//! the only truth; Play-time consequences live in the rules `RuntimeState` + render overlay and are
//! discarded on Stop (ADR-034/049 discipline).

use std::fmt;

use serde::Deserialize;

use metrocalk_core::caps::canonical;
use metrocalk_core::rules::{Action, CompareOp, Condition, RuleData, RuleId, SUBJECT_ENTITY};
use metrocalk_core::variant::INSTANCE_META;
use metrocalk_core::{Engine, EntityId, FieldValue, Op, Registry};
use metrocalk_ecs::FlecsWorld;
use serde::Serialize;

use crate::animation_intent::author_spin_ops;
use crate::capscene::{CapScene, NAME_FIELD};
use crate::companion_intent;

/// The component a role writes (registered in the stdlib).
pub const ROLE_COMPONENT: &str = "GameRole";
/// The shared score counter entity's display name.
pub const SCORE_NAME: &str = "Score";
/// The ONE pickup rule's stable id — every collectible shares it ($subject scopes it per entity),
/// so re-assigning roles rewrites the same rule instead of accumulating copies.
pub const PICKUP_RULE_ID: &str = "rule_role_pickup";
/// The ONE defeat rule's stable id — every enemy shares it, `$subject`-scoped the same way.
pub const DEFEAT_RULE_ID: &str = "rule_role_defeat";
/// The ONE vanish rule's stable id — every vanishing object shares it, `$subject`-scoped.
pub const VANISH_RULE_ID: &str = "rule_role_vanish";
/// The ONE hazard rule's stable id — every hazard shares it; `$other` scopes the harm to the toucher.
pub const HAZARD_RULE_ID: &str = "rule_role_hazard";
/// A player's starting health.
pub const DEFAULT_PLAYER_HP: i64 = 3;
/// A vanishing object's default countdown, seconds.
pub const DEFAULT_VANISH_SECONDS: f64 = 3.0;
/// A collectible's default touch radius, metres.
pub const DEFAULT_TOUCH_RADIUS: f64 = 0.9;
/// One full spin every 2 seconds (60 000 animation ticks per second).
pub const SPIN_PERIOD_TICKS: i64 = 120_000;

/// A role something can't do is a sentence, never a silent no-op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoleError {
    /// Unknown role kind.
    UnknownRole(String),
    /// The entity is gone.
    MissingEntity,
    /// The transaction failed (validation reason inside).
    Blocked(String),
}

impl fmt::Display for RoleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownRole(r) => write!(f, "there is no role called \"{r}\""),
            Self::MissingEntity => write!(f, "that object is no longer in the scene"),
            Self::Blocked(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for RoleError {}

/// One assignable role, in the author's language.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleSpec {
    /// Recipe kind (`GameRole.role`).
    pub kind: &'static str,
    /// Card label.
    pub label: &'static str,
    /// What it is for, one line.
    pub blurb: &'static str,
    /// Card glyph.
    pub icon: &'static str,
    /// Exactly what assigning it adds — the legible-cost line.
    pub adds: &'static str,
}

/// The assignable roles.
#[must_use]
pub fn role_specs() -> Vec<RoleSpec> {
    vec![
        RoleSpec {
            kind: "collectible",
            label: "Collectible",
            blurb: "Spins; vanishes and scores when something touches it",
            icon: "✦",
            adds: "spin animation · touch trigger · pickup rule · +1 on the Score counter",
        },
        RoleSpec {
            kind: "solid",
            label: "Solid obstacle",
            blurb: "An immovable body other things collide with",
            icon: "▦",
            adds: "fixed physics body · auto-fit collider",
        },
        RoleSpec {
            kind: "prop",
            label: "Physics prop",
            blurb: "Falls, rolls and collides under gravity",
            icon: "◍",
            adds: "dynamic physics body · auto-fit collider",
        },
        RoleSpec {
            kind: "spinner",
            label: "Spinner",
            blurb: "Turns forever — ambient motion",
            icon: "↻",
            adds: "looping spin animation",
        },
        RoleSpec {
            kind: "companion",
            label: "Companion",
            blurb: "Follows your props, patrols your waypoints, fights your enemies",
            icon: "♥",
            adds:
                "dynamic physics body · auto-fit collider · a live brain (follow / patrol / attack)",
        },
        RoleSpec {
            kind: "enemy",
            label: "Enemy",
            blurb: "Companions attack it; it falls when struck",
            icon: "☠",
            adds:
                "dynamic physics body · auto-fit collider · the defeat rule · +1 Score when beaten",
        },
        RoleSpec {
            kind: "waypoint",
            label: "Waypoint",
            blurb: "A patrol stop — companions with nothing to follow walk the chain in order",
            icon: "⚑",
            adds: "a numbered patrol marker (no physics)",
        },
        RoleSpec {
            kind: "vanishing",
            label: "Vanishing",
            blurb: "Disappears a few seconds into the run — a crumbling platform, a fuse, a timed gate",
            icon: "⏳",
            adds: "a countdown · the vanish rule (tune the seconds, and gate it with Only if…)",
        },
        RoleSpec {
            kind: "hazard",
            label: "Hazard",
            blurb: "Hurts whoever walks into it — spikes, lava, a falling rock",
            icon: "⚡",
            adds: "the hurt rule — it damages the TOUCHER, not itself (tune the damage in Points)",
        },
        RoleSpec {
            kind: "player",
            label: "Player",
            blurb: "YOU, during Play — drive it with the arrow keys or WASD; companions follow you first",
            icon: "🎮",
            adds: "dynamic physics body · auto-fit collider · live keyboard control while playing",
        },
    ]
}

/// What every role command answers with (the wire type). `applied` set = the role landed; `reason`
/// set = a plain-language refusal with NO scene change; `added` is the legible what-you-got list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleReply {
    /// The role that landed (None on refusal / clear).
    pub applied: Option<String>,
    /// The entity acted on.
    pub entity: Option<String>,
    /// What the assignment added, in the author's language.
    pub added: Vec<String>,
    /// The shared Score entity's key, when a collectible wired one.
    pub score_entity: Option<String>,
    /// Friendly one-line summary.
    pub message: String,
    /// Plain-language refusal — set iff nothing changed.
    pub reason: Option<String>,
}

impl RoleReply {
    /// A refusal that changed nothing, explained.
    #[must_use]
    pub fn refusal(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self {
            message: reason.clone(),
            reason: Some(reason),
            ..Self::default()
        }
    }
}

/// One roster row for the Gameplay panel.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleRow {
    /// Entity loro key.
    pub entity: String,
    /// Display name.
    pub name: String,
    /// Assigned role kind.
    pub role: String,
}

/// The roles overview: who has a role, and the score (LIVE during Play, authored otherwise).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleStatusInfo {
    /// Every entity with a role.
    pub roster: Vec<RoleRow>,
    /// The Score counter's current value (the rules RuntimeState during Play).
    pub score: i64,
    /// The Score entity's key, if one exists.
    pub score_entity: Option<String>,
    /// How many collectibles remain uncollected (equals roster collectibles when not playing).
    pub remaining: i64,
    /// Live companion status lines during Play ("following", "hunting Skeleton"); empty otherwise.
    #[serde(default)]
    pub companions: Vec<CompanionRow>,
    /// True while playing once every objective is done (all enemies beaten AND all
    /// collectibles collected — and there was at least one of either). Stop resets it.
    #[serde(default)]
    pub won: bool,
    /// The player's live health during Play — hearts a player can actually see. `None` outside Play or
    /// when nothing carries Health. Health you cannot see is health you cannot play around.
    #[serde(default)]
    pub health: Option<HealthInfo>,
    /// The most recent **near miss** during Play: a rule whose When matched but whose "only if" did not
    /// hold, already rendered as one sentence with the actual value ("the Score is 0, needs at least 3").
    /// This is the answer to "I touched it and nothing happened". Cleared on Stop.
    #[serde(default)]
    pub blocked: Option<BlockedInfo>,
}

/// A live health readout for the Gameplay panel.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthInfo {
    /// Whose health this is (display name).
    pub name: String,
    /// Current hit points.
    pub hp: i64,
    /// The maximum, for the hearts row.
    pub max_hp: i64,
}

/// One near-miss, in the author's language.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockedInfo {
    /// The object the event was about (its display name).
    pub name: String,
    /// The rule that did not fire.
    pub rule: String,
    /// Why — the first failing clause with its live value.
    pub why: String,
}

/// One companion's live one-liner for the Gameplay panel.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanionRow {
    /// Entity key.
    pub entity: String,
    /// Display name.
    pub name: String,
    /// What the brain is doing right now, in the author's language.
    pub doing: String,
}

/// What an assignment did — the reply's legible facts.
#[derive(Clone, Debug)]
pub struct RoleOutcome {
    /// The role that landed.
    pub role: String,
    /// The shared Score entity (created or reused) for collectibles.
    pub score_entity: Option<String>,
    /// Whether this assignment created the Score entity.
    pub score_created: bool,
    /// Plain-language list of what was added.
    pub added: Vec<String>,
}

/// The entity's current role, if any.
#[must_use]
pub fn role_of(engine: &Engine<FlecsWorld>, entity: EntityId) -> Option<String> {
    match engine.get_field(entity, ROLE_COMPONENT, "role") {
        Some(FieldValue::Str(role)) if !role.is_empty() => Some(role),
        _ => None,
    }
}

/// Find the shared Score counter: the KillCounter entity named [`SCORE_NAME`], else any KillCounter.
#[must_use]
pub fn find_score(engine: &Engine<FlecsWorld>) -> Option<EntityId> {
    let mut ids = engine.entity_ids();
    ids.sort_unstable_by_key(|id| (id.peer, id.counter));
    let mut fallback = None;
    for id in ids {
        let comps = engine.components_of(id);
        if !comps.contains_key("KillCounter") {
            continue;
        }
        let named_score = matches!(
            comps.get(INSTANCE_META).and_then(|m| m.get(NAME_FIELD)),
            Some(FieldValue::Str(name)) if name == SCORE_NAME
        );
        if named_score {
            return Some(id);
        }
        fallback.get_or_insert(id);
    }
    fallback
}

/// Every entity carrying a role: `(loro key, display name, role, authored position, radius)` —
/// the roster the Gameplay panel lists and the Play loop caches for touch detection.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)] // f32 is the render/sim unit
pub fn roster(engine: &Engine<FlecsWorld>) -> Vec<(String, String, String, [f32; 3], f32)> {
    let mut ids = engine.entity_ids();
    ids.sort_unstable_by_key(|id| (id.peer, id.counter));
    let mut out = Vec::new();
    for id in ids {
        // M9.2 deactivate-not-delete: a deactivated PART is hidden from the world, so it must
        // not play either — an invisible coin must never score. Same guard as the render rebuild.
        if engine.parent_of(id).is_some() && !engine.is_active(id) {
            continue;
        }
        let comps = engine.resolved_components(id);
        let Some(role_fields) = comps.get(ROLE_COMPONENT) else {
            continue;
        };
        let Some(FieldValue::Str(role)) = role_fields.get("role") else {
            continue;
        };
        // Numeric reads match BOTH arms — the codified Integer-vs-Number footgun.
        let num = |fields: &std::collections::HashMap<String, FieldValue>, key: &str| -> f32 {
            match fields.get(key) {
                Some(FieldValue::Number(n)) => *n as f32,
                Some(FieldValue::Integer(i)) => *i as f32,
                _ => 0.0,
            }
        };
        let pos = comps
            .get("Transform")
            .map_or([0.0; 3], |t| [num(t, "x"), num(t, "y"), num(t, "z")]);
        let radius = {
            let r = num(role_fields, "radius");
            if r > 0.0 {
                r
            } else {
                DEFAULT_TOUCH_RADIUS as f32
            }
        };
        let name = crate::capscene::entity_name(engine, id).unwrap_or_else(|| id.to_loro_key());
        out.push((id.to_loro_key(), name, role.clone(), pos, radius));
    }
    out
}

/// The ONE pickup rule, `$subject`-scoped: touching any active collectible collects THAT one and
/// scores one point on the shared counter.
fn pickup_rule(score_key: &str) -> RuleData {
    RuleData {
        name: "Collect on touch".into(),
        enabled: true,
        event: "Touched".into(),
        conditions: vec![Condition {
            entity: SUBJECT_ENTITY.into(),
            component: ROLE_COMPONENT.into(),
            field: "active".into(),
            op: CompareOp::Eq,
            value: FieldValue::Bool(true),
        }],
        any_of: vec![],
        subject: None,
        actions: vec![
            Action {
                action: "SetField".into(),
                entity: SUBJECT_ENTITY.into(),
                component: ROLE_COMPONENT.into(),
                field: "active".into(),
                value: FieldValue::Bool(false),
            },
            Action {
                action: "AdjustCounter".into(),
                entity: score_key.into(),
                component: "KillCounter".into(),
                field: "count".into(),
                value: FieldValue::Integer(1),
            },
        ],
    }
}

/// The ONE defeat rule, `$subject`-scoped: a companion's strike on any active enemy defeats THAT
/// one and scores one point — the exact shape of the pickup rule, on the `Struck` event.
fn defeat_rule(score_key: &str) -> RuleData {
    RuleData {
        name: "Defeat on strike".into(),
        enabled: true,
        event: "Struck".into(),
        conditions: vec![Condition {
            entity: SUBJECT_ENTITY.into(),
            component: ROLE_COMPONENT.into(),
            field: "active".into(),
            op: CompareOp::Eq,
            value: FieldValue::Bool(true),
        }],
        any_of: vec![],
        subject: None,
        actions: vec![
            Action {
                action: "SetField".into(),
                entity: SUBJECT_ENTITY.into(),
                component: ROLE_COMPONENT.into(),
                field: "active".into(),
                value: FieldValue::Bool(false),
            },
            Action {
                action: "AdjustCounter".into(),
                entity: score_key.into(),
                component: "KillCounter".into(),
                field: "count".into(),
                value: FieldValue::Integer(1),
            },
        ],
    }
}

/// The ONE vanish rule, `$subject`-scoped: when an object's countdown elapses, it goes away. The
/// consequence is the same `GameRole.active` flag every other role uses, so the generic effect
/// read-back projects it to the viewport with no special case.
fn vanish_rule() -> RuleData {
    RuleData {
        name: "Vanish when the countdown ends".into(),
        enabled: true,
        event: "After".into(),
        conditions: vec![Condition {
            entity: SUBJECT_ENTITY.into(),
            component: ROLE_COMPONENT.into(),
            field: "active".into(),
            op: CompareOp::Eq,
            value: FieldValue::Bool(true),
        }],
        any_of: vec![],
        subject: None,
        actions: vec![Action {
            action: "SetField".into(),
            entity: SUBJECT_ENTITY.into(),
            component: ROLE_COMPONENT.into(),
            field: "active".into(),
            value: FieldValue::Bool(false),
        }],
    }
}

/// The ONE hazard rule. The action targets `$other` — the entity that walked into it — which is the
/// whole point: before `$other` existed, "the spike hurts whoever stepped on it" could not be said.
fn hazard_rule() -> RuleData {
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
            value: FieldValue::Integer(1),
        }],
    }
}

fn set(entity: EntityId, component: &str, field: &str, value: FieldValue) -> Op {
    Op::SetField {
        entity,
        component: component.into(),
        field: field.into(),
        value,
    }
}

fn provides(scene: &CapScene, ops: &mut Vec<Op>, entity: EntityId, cap: &str) {
    if let Some(&target) = scene.caps.get(&canonical(cap)) {
        ops.push(Op::AddPair {
            entity,
            rel: scene.rels.provides,
            target,
        });
    }
}

/// Assign `role` to `entity` as ONE undoable transaction. Idempotent per part: re-assigning never
/// duplicates tracks, rules, bindings or the Score entity.
///
/// # Errors
/// A plain-language [`RoleError`]; the document is untouched on any failure (all-or-nothing commit).
#[allow(clippy::too_many_lines)] // one readable transaction builder per role, not branching logic
pub fn assign_role(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    registry: &Registry<FlecsWorld>,
    entity: EntityId,
    role: &str,
) -> Result<RoleOutcome, RoleError> {
    if !engine.entity_exists(entity) {
        return Err(RoleError::MissingEntity);
    }
    if !role_specs().iter().any(|spec| spec.kind == role) {
        return Err(RoleError::UnknownRole(role.to_string()));
    }
    let mut ops: Vec<Op> = Vec::new();
    let mut added: Vec<String> = Vec::new();
    let mut score_entity = None;
    let mut score_created = false;

    // The role identity itself.
    ops.push(set(
        entity,
        ROLE_COMPONENT,
        "role",
        FieldValue::Str(role.into()),
    ));
    ops.push(set(
        entity,
        ROLE_COMPONENT,
        "active",
        FieldValue::Bool(true),
    ));
    provides(scene, &mut ops, entity, "Gameplay");

    match role {
        "collectible" => {
            ops.push(set(
                entity,
                ROLE_COMPONENT,
                "radius",
                FieldValue::Number(DEFAULT_TOUCH_RADIUS),
            ));
            ops.push(set(
                entity,
                ROLE_COMPONENT,
                "points",
                FieldValue::Integer(1),
            ));
            // The spin (idempotent — empty ops when the tracks already exist).
            let spin = author_spin_ops(engine, entity, SPIN_PERIOD_TICKS)
                .map_err(|e| RoleError::Blocked(format!("the spin could not be authored: {e}")))?;
            if !spin.is_empty() {
                added.push("spin animation".into());
            }
            ops.extend(spin);
            // The shared Score counter — reuse or create in this same transaction.
            let score_id = if let Some(id) = find_score(engine) {
                id
            } else {
                let id = engine.alloc_entity_id();
                ops.push(Op::CreateEntity { id, parent: None });
                for (field, value) in [("x", 0.0f64), ("y", 0.0), ("z", 4.0)] {
                    ops.push(set(id, "Transform", field, FieldValue::Number(value)));
                }
                ops.push(set(id, "KillCounter", "count", FieldValue::Integer(0)));
                ops.push(set(
                    id,
                    INSTANCE_META,
                    NAME_FIELD,
                    FieldValue::Str(SCORE_NAME.into()),
                ));
                provides(scene, &mut ops, id, "Counter");
                score_created = true;
                added.push("a Score counter".into());
                id
            };
            let score_key = score_id.to_loro_key();
            // The ONE pickup rule (validated, then written by stable id — an overwrite, never a copy).
            let pickup = pickup_rule(&score_key);
            metrocalk_core::validate_rule(registry, &pickup, |id| {
                id == score_id || engine.entity_exists(id)
            })
            .map_err(|e| RoleError::Blocked(format!("the pickup rule was refused: {e}")))?;
            ops.push(Op::SetRule {
                id: RuleId::new(PICKUP_RULE_ID),
                rule: pickup,
            });
            added.push("the pickup rule".into());
            // The visible relationship: collectible → Score (a tracking line + a Relations edge).
            let already_bound = engine
                .bindings()
                .iter()
                .any(|(from, _, to)| *from == entity && *to == score_id);
            if !already_bound {
                ops.push(Op::AddBinding {
                    from: entity,
                    kind: crate::capscene::TRACKS.into(),
                    to: score_id,
                });
                added.push("a binding to the Score".into());
            }
            added.push("touch trigger".into());
            score_entity = Some(score_key);
        }
        "solid" => {
            ops.push(set(
                entity,
                "RigidBody",
                "kind",
                FieldValue::Str("fixed".into()),
            ));
            ops.push(set(
                entity,
                "Collider",
                "shape",
                FieldValue::Str("convexHull".into()),
            ));
            provides(scene, &mut ops, entity, "Physics");
            provides(scene, &mut ops, entity, "Collision");
            added.push("a fixed physics body".into());
            added.push("an auto-fit collider".into());
        }
        "prop" => {
            ops.push(set(
                entity,
                "RigidBody",
                "kind",
                FieldValue::Str("dynamic".into()),
            ));
            ops.push(set(entity, "RigidBody", "mass", FieldValue::Number(1.0)));
            ops.push(set(
                entity,
                "Collider",
                "shape",
                FieldValue::Str("convexHull".into()),
            ));
            provides(scene, &mut ops, entity, "Physics");
            provides(scene, &mut ops, entity, "Collision");
            added.push("a dynamic physics body".into());
            added.push("an auto-fit collider".into());
        }
        "spinner" => {
            let spin = author_spin_ops(engine, entity, SPIN_PERIOD_TICKS)
                .map_err(|e| RoleError::Blocked(format!("the spin could not be authored: {e}")))?;
            if !spin.is_empty() {
                added.push("looping spin animation".into());
            }
            ops.extend(spin);
        }
        "companion" => {
            // A companion is a REAL dynamic body (gravity + collision honest) with a brain
            // the Play loop runs. The three tunables land as ordinary fields — the Inspector
            // edits them like any number; the card needs no configuration to work.
            for (field, value) in [
                ("speed", companion_intent::DEFAULT_SPEED),
                ("range", companion_intent::DEFAULT_ATTACK_RANGE),
                ("follow", companion_intent::DEFAULT_FOLLOW_DISTANCE),
                ("radius", companion_intent::DEFAULT_AGGRO),
            ] {
                ops.push(set(
                    entity,
                    ROLE_COMPONENT,
                    field,
                    FieldValue::Number(value),
                ));
            }
            ops.push(set(
                entity,
                "RigidBody",
                "kind",
                FieldValue::Str("dynamic".into()),
            ));
            ops.push(set(entity, "RigidBody", "mass", FieldValue::Number(1.0)));
            ops.push(set(
                entity,
                "Collider",
                "shape",
                FieldValue::Str("convexHull".into()),
            ));
            provides(scene, &mut ops, entity, "Physics");
            provides(scene, &mut ops, entity, "Collision");
            added.push("a dynamic physics body".into());
            added.push("an auto-fit collider".into());
            added.push("a live brain — follows props, patrols waypoints, attacks enemies".into());
        }
        "enemy" => {
            ops.push(set(
                entity,
                ROLE_COMPONENT,
                "points",
                FieldValue::Integer(1),
            ));
            ops.push(set(
                entity,
                "RigidBody",
                "kind",
                FieldValue::Str("dynamic".into()),
            ));
            ops.push(set(entity, "RigidBody", "mass", FieldValue::Number(1.0)));
            ops.push(set(
                entity,
                "Collider",
                "shape",
                FieldValue::Str("convexHull".into()),
            ));
            provides(scene, &mut ops, entity, "Physics");
            provides(scene, &mut ops, entity, "Collision");
            // The shared Score + the ONE defeat rule (same shape as the pickup rule: struck →
            // inactive + score; the hide is the same play-only projection).
            let score_id = if let Some(id) = find_score(engine) {
                id
            } else {
                let id = engine.alloc_entity_id();
                ops.push(Op::CreateEntity { id, parent: None });
                for (field, value) in [("x", 0.0f64), ("y", 0.0), ("z", 4.0)] {
                    ops.push(set(id, "Transform", field, FieldValue::Number(value)));
                }
                ops.push(set(id, "KillCounter", "count", FieldValue::Integer(0)));
                ops.push(set(
                    id,
                    INSTANCE_META,
                    NAME_FIELD,
                    FieldValue::Str(SCORE_NAME.into()),
                ));
                provides(scene, &mut ops, id, "Counter");
                score_created = true;
                added.push("a Score counter".into());
                id
            };
            let score_key = score_id.to_loro_key();
            let defeat = defeat_rule(&score_key);
            metrocalk_core::validate_rule(registry, &defeat, |id| {
                id == score_id || engine.entity_exists(id)
            })
            .map_err(|e| RoleError::Blocked(format!("the defeat rule was refused: {e}")))?;
            ops.push(Op::SetRule {
                id: RuleId::new(DEFEAT_RULE_ID),
                rule: defeat,
            });
            added.push("the defeat rule".into());
            added.push("a dynamic physics body".into());
            added.push("an auto-fit collider".into());
            score_entity = Some(score_key);
        }
        "vanishing" => {
            ops.push(set(
                entity,
                ROLE_COMPONENT,
                "speed",
                FieldValue::Number(DEFAULT_VANISH_SECONDS),
            ));
            let vanish = vanish_rule();
            metrocalk_core::validate_rule(registry, &vanish, |id| engine.entity_exists(id))
                .map_err(|e| RoleError::Blocked(format!("the vanish rule was refused: {e}")))?;
            ops.push(Op::SetRule {
                id: RuleId::new(VANISH_RULE_ID),
                rule: vanish,
            });
            added.push(format!("a {DEFAULT_VANISH_SECONDS}s countdown"));
            added.push("the vanish rule".into());
        }
        "hazard" => {
            ops.push(set(
                entity,
                ROLE_COMPONENT,
                "points",
                FieldValue::Integer(1),
            ));
            let hazard = hazard_rule();
            metrocalk_core::validate_rule(registry, &hazard, |id| engine.entity_exists(id))
                .map_err(|e| RoleError::Blocked(format!("the hazard rule was refused: {e}")))?;
            ops.push(Op::SetRule {
                id: RuleId::new(HAZARD_RULE_ID),
                rule: hazard,
            });
            added.push("the hurt rule (damages the toucher)".into());
        }
        "player" => {
            ops.push(set(
                entity,
                ROLE_COMPONENT,
                "speed",
                FieldValue::Number(companion_intent::DEFAULT_PLAYER_SPEED),
            ));
            ops.push(set(
                entity,
                "RigidBody",
                "kind",
                FieldValue::Str("dynamic".into()),
            ));
            ops.push(set(entity, "RigidBody", "mass", FieldValue::Number(1.0)));
            ops.push(set(
                entity,
                "Collider",
                "shape",
                FieldValue::Str("convexHull".into()),
            ));
            provides(scene, &mut ops, entity, "Physics");
            provides(scene, &mut ops, entity, "Collision");
            added.push("a dynamic physics body".into());
            added.push("an auto-fit collider".into());
            ops.push(set(
                entity,
                "Health",
                "hp",
                FieldValue::Integer(DEFAULT_PLAYER_HP),
            ));
            ops.push(set(
                entity,
                "Health",
                "maxHp",
                FieldValue::Integer(DEFAULT_PLAYER_HP),
            ));
            provides(scene, &mut ops, entity, "Health");
            added.push(format!("{DEFAULT_PLAYER_HP} health"));
            added.push("keyboard control during Play — arrows / WASD".into());
        }
        "waypoint" => {
            // Patrol order = the next free index in the chain (stable, gap-tolerant).
            let next = i64::try_from(
                roster(engine)
                    .iter()
                    .filter(|(_, _, role, _, _)| role == "waypoint")
                    .count(),
            )
            .unwrap_or(0)
                + 1;
            ops.push(set(
                entity,
                ROLE_COMPONENT,
                "points",
                FieldValue::Integer(next),
            ));
            added.push(format!("patrol stop #{next}"));
        }
        _ => unreachable!("validated against role_specs above"),
    }

    engine
        .commit("assign-role", ops)
        .map_err(|e| RoleError::Blocked(format!("the role could not be applied: {e}")))?;
    Ok(RoleOutcome {
        role: role.to_string(),
        score_entity,
        score_created,
        added,
    })
}

/// Remove the entity's role AND what roles add (physics body, collider, animation tracks, the role
/// component, its Score binding) in one undoable commit. The shared Score entity and the pickup rule
/// stay — other collectibles may use them.
///
/// # Errors
/// [`RoleError::MissingEntity`] / a commit failure (all-or-nothing).
pub fn clear_role(engine: &mut Engine<FlecsWorld>, entity: EntityId) -> Result<(), RoleError> {
    if !engine.entity_exists(entity) {
        return Err(RoleError::MissingEntity);
    }
    if role_of(engine, entity).is_none() {
        return Err(RoleError::Blocked(
            "that object has no role to clear".to_string(),
        ));
    }
    let comps = engine.components_of(entity);
    let mut ops: Vec<Op> = Vec::new();
    for component in [ROLE_COMPONENT, "RigidBody", "Collider", "AnimationTrack"] {
        if comps.contains_key(component) {
            ops.push(Op::RemoveComponent {
                entity,
                component: component.into(),
            });
        }
    }
    for (from, kind, to) in engine.bindings() {
        if from == entity {
            ops.push(Op::RemoveBinding { from, kind, to });
        }
    }
    engine
        .commit("clear-role", ops)
        .map_err(|e| RoleError::Blocked(format!("the role could not be cleared: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capscene::CapResolver;

    fn world() -> (Engine<FlecsWorld>, CapScene, Registry<FlecsWorld>) {
        let mut ecs = FlecsWorld::new();
        let scene = CapScene::intern(&mut ecs);
        let mut engine = Engine::new(ecs, 1);
        engine.set_capability_resolver(Box::new(CapResolver::from_scene(&scene)));
        let mut registry = Registry::new(FlecsWorld::new());
        for meta in metrocalk_core::stdlib::standard_components() {
            registry.register(meta).expect("stdlib registers");
        }
        for event in metrocalk_core::stdlib::standard_events() {
            registry.register_event(event);
        }
        for action in metrocalk_core::stdlib::standard_actions() {
            registry.register_action(action);
        }
        (engine, scene, registry)
    }

    fn spawn(engine: &mut Engine<FlecsWorld>, x: f64) -> EntityId {
        let id = engine.alloc_entity_id();
        engine
            .commit(
                "spawn",
                vec![
                    Op::CreateEntity { id, parent: None },
                    set(id, "Transform", "x", FieldValue::Number(x)),
                    set(id, "Transform", "y", FieldValue::Number(0.0)),
                    set(id, "Transform", "z", FieldValue::Number(0.0)),
                ],
            )
            .expect("spawns");
        id
    }

    #[test]
    fn a_collectible_assignment_is_one_undoable_transaction_with_everything() {
        let (mut engine, scene, registry) = world();
        let pickup = spawn(&mut engine, 1.0);

        let outcome =
            assign_role(&mut engine, &scene, &registry, pickup, "collectible").expect("assigns");
        assert_eq!(outcome.role, "collectible");
        assert!(
            outcome.score_created,
            "the first collectible creates the Score"
        );
        let score_key = outcome.score_entity.clone().expect("score exists");
        let score = EntityId::from_loro_key(&score_key).unwrap();

        // Everything landed: role fields, spin tracks, score entity, the rule, the binding.
        assert_eq!(role_of(&engine, pickup).as_deref(), Some("collectible"));
        assert!(
            engine.components_of(pickup).contains_key("AnimationTrack"),
            "the spin is authored"
        );
        assert!(engine.entity_exists(score));
        assert!(
            engine.rule(&RuleId::new(PICKUP_RULE_ID)).is_some(),
            "the pickup rule exists"
        );
        assert!(
            engine
                .bindings()
                .iter()
                .any(|(from, _, to)| *from == pickup && *to == score),
            "the collectible is visibly bound to the Score"
        );

        // ONE Ctrl-Z removes the whole thing — role, spin, score entity, rule, binding.
        assert!(engine.undo(), "undo runs");
        assert!(role_of(&engine, pickup).is_none(), "role gone");
        assert!(!engine.components_of(pickup).contains_key("AnimationTrack"));
        assert!(
            !engine.entity_exists(score),
            "the created Score went with it"
        );
        assert!(
            engine.rule(&RuleId::new(PICKUP_RULE_ID)).is_none(),
            "rule gone"
        );
    }

    #[test]
    fn a_second_collectible_reuses_the_score_and_rewrites_the_same_rule() {
        let (mut engine, scene, registry) = world();
        let a = spawn(&mut engine, 1.0);
        let b = spawn(&mut engine, 3.0);
        let first = assign_role(&mut engine, &scene, &registry, a, "collectible").unwrap();
        let second = assign_role(&mut engine, &scene, &registry, b, "collectible").unwrap();
        assert!(
            first.score_created && !second.score_created,
            "one Score, reused"
        );
        assert_eq!(first.score_entity, second.score_entity);
        // Re-assigning A is idempotent — no duplicate tracks, no error.
        let again = assign_role(&mut engine, &scene, &registry, a, "collectible").unwrap();
        assert!(!again.added.contains(&"spin animation".to_string()));
    }

    #[test]
    fn solid_and_prop_write_real_physics_components() {
        let (mut engine, scene, registry) = world();
        let wall = spawn(&mut engine, 0.0);
        let ball = spawn(&mut engine, 2.0);
        assign_role(&mut engine, &scene, &registry, wall, "solid").unwrap();
        assign_role(&mut engine, &scene, &registry, ball, "prop").unwrap();
        assert_eq!(
            engine.get_field(wall, "RigidBody", "kind"),
            Some(FieldValue::Str("fixed".into()))
        );
        assert_eq!(
            engine.get_field(ball, "RigidBody", "kind"),
            Some(FieldValue::Str("dynamic".into()))
        );
        assert_eq!(
            engine.get_field(ball, "Collider", "shape"),
            Some(FieldValue::Str("convexHull".into()))
        );
        let listed = roster(&engine);
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().any(|(_, _, role, _, _)| role == "solid"));
    }

    #[test]
    fn clear_role_removes_what_roles_add_but_keeps_the_shared_score() {
        let (mut engine, scene, registry) = world();
        let pickup = spawn(&mut engine, 1.0);
        let other = spawn(&mut engine, 5.0);
        assign_role(&mut engine, &scene, &registry, pickup, "collectible").unwrap();
        assign_role(&mut engine, &scene, &registry, other, "collectible").unwrap();
        let score = find_score(&engine).expect("score exists");

        clear_role(&mut engine, pickup).expect("clears");
        assert!(role_of(&engine, pickup).is_none());
        assert!(!engine.components_of(pickup).contains_key("AnimationTrack"));
        assert!(engine.entity_exists(score), "the shared Score survives");
        assert!(
            engine.rule(&RuleId::new(PICKUP_RULE_ID)).is_some(),
            "rule survives"
        );
        // The cleared entity's binding is gone; the other collectible's remains.
        assert!(!engine.bindings().iter().any(|(from, _, _)| *from == pickup));
        assert!(engine.bindings().iter().any(|(from, _, _)| *from == other));
    }

    #[test]
    fn refusals_are_plain_sentences() {
        let (mut engine, scene, registry) = world();
        let e = spawn(&mut engine, 0.0);
        let msg = assign_role(&mut engine, &scene, &registry, e, "boss")
            .expect_err("unknown role")
            .to_string();
        assert!(msg.contains("boss"), "{msg}");
        let msg = clear_role(&mut engine, e)
            .expect_err("nothing to clear")
            .to_string();
        assert!(msg.contains("no role"), "{msg}");
    }

    #[test]
    fn a_companion_gets_a_body_and_its_brain_numbers_in_one_commit() {
        let (mut engine, scene, registry) = world();
        let dog = spawn(&mut engine, 0.0);
        let outcome =
            assign_role(&mut engine, &scene, &registry, dog, "companion").expect("assigns");
        assert_eq!(outcome.role, "companion");
        assert_eq!(
            engine.get_field(dog, "RigidBody", "kind"),
            Some(FieldValue::Str("dynamic".into())),
            "a companion is a REAL body — gravity and collision apply"
        );
        assert_eq!(
            engine.get_field(dog, ROLE_COMPONENT, "speed"),
            Some(FieldValue::Number(companion_intent::DEFAULT_SPEED)),
            "the tunables land as plain Inspector-editable fields"
        );
        // One Ctrl-Z removes the whole companion.
        assert!(engine.undo());
        assert!(role_of(&engine, dog).is_none());
        assert!(!engine.components_of(dog).contains_key("RigidBody"));
    }

    #[test]
    fn an_enemy_authors_the_shared_defeat_rule_and_score() {
        let (mut engine, scene, registry) = world();
        let skeleton = spawn(&mut engine, 4.0);
        let outcome =
            assign_role(&mut engine, &scene, &registry, skeleton, "enemy").expect("assigns");
        assert!(outcome.score_created, "the first enemy creates the Score");
        assert!(
            engine.rule(&RuleId::new(DEFEAT_RULE_ID)).is_some(),
            "the defeat rule exists"
        );
        // A collectible joining later REUSES the same Score entity.
        let coin = spawn(&mut engine, -4.0);
        let coin_outcome =
            assign_role(&mut engine, &scene, &registry, coin, "collectible").expect("assigns");
        assert!(!coin_outcome.score_created, "one Score for the whole game");
        assert_eq!(outcome.score_entity, coin_outcome.score_entity);
    }

    #[test]
    fn waypoints_number_themselves_in_assignment_order() {
        let (mut engine, scene, registry) = world();
        let a = spawn(&mut engine, 0.0);
        let b = spawn(&mut engine, 2.0);
        let c = spawn(&mut engine, 4.0);
        for wp in [a, b, c] {
            assign_role(&mut engine, &scene, &registry, wp, "waypoint").expect("assigns");
        }
        let orders: Vec<i64> = [a, b, c]
            .iter()
            .map(|id| match engine.get_field(*id, ROLE_COMPONENT, "points") {
                Some(FieldValue::Integer(i)) => i,
                other => panic!("waypoint order missing: {other:?}"),
            })
            .collect();
        assert_eq!(orders, vec![1, 2, 3], "the patrol chain is ordered");
        assert!(
            !engine.components_of(a).contains_key("RigidBody"),
            "a waypoint is geography, not a body"
        );
    }
}
