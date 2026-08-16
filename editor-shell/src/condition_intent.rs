//! Conditionals — the user's **"only if"**, authored as a sentence and stored on the object it is about.
//!
//! The problem this solves: a role card writes a real rule ("when something touches this Coin, collect it
//! and score"), but until now the user could not add a *qualifier*. Editing the shared rule per object would
//! mean a rule copy per object — the thing that makes visual-scripting projects rot.
//!
//! The shape here avoids that entirely:
//!
//! * a clause is a core [`Condition`], stored as canonical JSON in the entity's own `PlayIf.source`;
//! * at Play-start [`expand_subject_rules`] turns the ONE shared `$subject` rule into a per-entity rule
//!   carrying that entity's own clauses, pinned to that entity via [`RuleData::subject`];
//! * the document therefore still holds exactly one pickup rule and one defeat rule, forever, and every
//!   clause is one undoable `SetField` on an ordinary component.
//!
//! Per-object thresholds ("this vault needs 3, that chest needs 5") fall out for free, because each
//! expanded rule carries its own literal.
//!
//! The clause vocabulary is a CATALOGUE of named cards ([`condition_specs`]), not a component/field grid:
//! the card fixes the component, the field and the operator family, so "boolean blindness" and
//! meaningless comparisons (ordering a bool) are unreachable rather than merely discouraged.

use serde::{Deserialize, Serialize};

use metrocalk_core::rules::{CompareOp, Condition, RuleData, SUBJECT_ENTITY};
use metrocalk_core::variant::INSTANCE_META;
use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::{FlecsWorld, World};

use crate::capscene::NAME_FIELD;
use crate::role_intent::{find_score, ROLE_COMPONENT};

/// The component holding a user's clauses (registered in the stdlib).
pub const CONDITION_COMPONENT: &str = "PlayIf";
/// The most AND clauses one object may carry — the stated ceiling, enforced with a reason.
pub const MAX_ALL: usize = 4;
/// The most alternatives in the single OR group.
pub const MAX_ANY: usize = 3;

/// What the user picked, before it becomes a [`Condition`]: a catalogue card plus its filled-in blank.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClauseRequest {
    /// The [`ConditionSpec::kind`] chosen.
    pub kind: String,
    /// The card's blank: a number (`score_at_least`) — ignored by cards that need nothing.
    #[serde(default)]
    pub number: Option<f64>,
    /// The card's blank: another object's key (`other_collected`).
    #[serde(default)]
    pub object: Option<String>,
    /// Join this clause into the OR group instead of the AND list.
    #[serde(default)]
    pub any: bool,
}

/// One clause card — the `RoleSpec` sibling, so the UI renders conditionals with the same grammar as roles.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConditionSpec {
    /// Stable kind key.
    pub kind: &'static str,
    /// Card label.
    pub label: &'static str,
    /// One line of what it means.
    pub blurb: &'static str,
    /// Card glyph.
    pub icon: &'static str,
    /// What the user must supply: `none` | `number` | `object`.
    pub needs: &'static str,
    /// The sentence fragment, with `{n}` / `{name}` blanks — the thing the user actually reads.
    pub reads: &'static str,
}

/// The clause catalogue. Every card names a component+field+operator the roles system actually seeds, so a
/// clause can never reference a field nothing ever writes (which would read as permanently false).
#[must_use]
pub fn condition_specs() -> Vec<ConditionSpec> {
    vec![
        ConditionSpec {
            kind: "score_at_least",
            label: "The Score is at least…",
            blurb: "gate this behind points the player has already earned",
            icon: "★",
            needs: "number",
            reads: "the Score is at least {n}",
        },
        ConditionSpec {
            kind: "score_under",
            label: "The Score is under…",
            blurb: "only while the player is still below a score",
            icon: "☆",
            needs: "number",
            reads: "the Score is under {n}",
        },
        ConditionSpec {
            kind: "still_active",
            label: "It hasn't been used yet",
            blurb: "this object has not been collected or beaten",
            icon: "◆",
            needs: "none",
            reads: "it hasn't been used yet",
        },
        ConditionSpec {
            kind: "touched_by_player",
            label: "The PLAYER touched it",
            blurb: "only when you walk into it — a companion or a stray crate will not do",
            icon: "🎮",
            needs: "none",
            reads: "the player touched it",
        },
        ConditionSpec {
            kind: "touched_by_companion",
            label: "A COMPANION touched it",
            blurb: "only when one of your companions walks into it, not you",
            icon: "♥",
            needs: "none",
            reads: "a companion touched it",
        },
        ConditionSpec {
            kind: "other_gone",
            label: "Another object is gone",
            blurb: "that collectible has been collected, or that enemy beaten",
            icon: "✧",
            needs: "object",
            reads: "{name} is gone",
        },
        ConditionSpec {
            kind: "other_still_there",
            label: "Another object is still there",
            blurb: "that object has NOT been collected or beaten yet",
            icon: "◇",
            needs: "object",
            reads: "{name} is still there",
        },
    ]
}

/// A clause that could not be built, explained in the author's language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConditionError {
    /// No such card.
    UnknownKind(String),
    /// The card's blank was not filled in.
    MissingInput(String),
    /// The clause cannot ever be true (e.g. no Score counter exists yet).
    Impossible(String),
    /// The per-object clause ceiling was reached.
    TooMany(String),
    /// The document write failed.
    Blocked(String),
}

impl std::fmt::Display for ConditionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownKind(k) => write!(f, "there is no condition called \"{k}\""),
            Self::MissingInput(m) | Self::Impossible(m) | Self::TooMany(m) | Self::Blocked(m) => {
                write!(f, "{m}")
            }
        }
    }
}

impl std::error::Error for ConditionError {}

/// An object's authored clauses: the AND list and the single OR group.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Clauses {
    /// Every one of these must hold.
    #[serde(default)]
    pub all: Vec<Condition>,
    /// At least one of these must hold (when non-empty).
    #[serde(default)]
    pub any: Vec<Condition>,
}

impl Clauses {
    /// Whether the object has authored anything at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.all.is_empty() && self.any.is_empty()
    }
}

/// Read an entity's authored clauses (empty when it has none / the JSON is unreadable — a corrupt blob is
/// never trusted into the runtime, matching the persisted-artifact discipline elsewhere).
#[must_use]
pub fn clauses_of<W: World>(engine: &Engine<W>, entity: EntityId) -> Clauses {
    engine
        .resolved_components(entity)
        .get(CONDITION_COMPONENT)
        .and_then(|fields| match fields.get("source") {
            Some(FieldValue::Str(json)) => serde_json::from_str::<Clauses>(json).ok(),
            _ => None,
        })
        .unwrap_or_default()
}

/// Turn a picked card into a real [`Condition`] against this scene, or explain why it can't be one.
///
/// # Errors
/// [`ConditionError`] — unknown card, a missing blank, or a clause that could never be true here.
pub fn build_clause(
    engine: &Engine<FlecsWorld>,
    request: &ClauseRequest,
) -> Result<Condition, ConditionError> {
    let spec = condition_specs()
        .into_iter()
        .find(|s| s.kind == request.kind)
        .ok_or_else(|| ConditionError::UnknownKind(request.kind.clone()))?;

    let number = || -> Result<i64, ConditionError> {
        request
            .number
            .map(|n| {
                #[allow(clippy::cast_possible_truncation)] // a clause threshold, clamped by the UI
                {
                    n.round() as i64
                }
            })
            .ok_or_else(|| ConditionError::MissingInput(format!("{} needs a number", spec.label)))
    };
    let other = || -> Result<EntityId, ConditionError> {
        let key = request.object.as_deref().ok_or_else(|| {
            ConditionError::MissingInput(format!("{} needs you to pick an object", spec.label))
        })?;
        EntityId::from_loro_key(key)
            .filter(|id| engine.entity_exists(*id))
            .ok_or_else(|| {
                ConditionError::Impossible("that object is no longer in the scene".into())
            })
    };
    // The Score counter is the one clause target that may legitimately not exist yet.
    let score = || -> Result<String, ConditionError> {
        find_score(engine).map(|id| id.to_loro_key()).ok_or_else(|| {
            ConditionError::Impossible(
                "there is no Score yet — make something a Collectible or an Enemy first, and one appears"
                    .into(),
            )
        })
    };

    let condition = match spec.kind {
        "score_at_least" | "score_under" => Condition {
            entity: score()?,
            component: "KillCounter".into(),
            field: "count".into(),
            op: if spec.kind == "score_at_least" {
                CompareOp::Ge
            } else {
                CompareOp::Lt
            },
            value: FieldValue::Integer(number()?),
        },
        // `$other` = whoever caused the event. Resolved at dispatch, so ONE shared rule still serves
        // every object — the clause simply reads a field on the toucher instead of on the touched.
        "touched_by_player" | "touched_by_companion" => Condition {
            entity: metrocalk_core::rules::OTHER_ENTITY.into(),
            component: ROLE_COMPONENT.into(),
            field: "role".into(),
            op: CompareOp::Eq,
            value: FieldValue::Str(if spec.kind == "touched_by_player" {
                "player".into()
            } else {
                "companion".into()
            }),
        },
        "still_active" => Condition {
            entity: SUBJECT_ENTITY.into(),
            component: ROLE_COMPONENT.into(),
            field: "active".into(),
            op: CompareOp::Eq,
            value: FieldValue::Bool(true),
        },
        "other_gone" | "other_still_there" => {
            let id = other()?;
            // A clause about another object's fate only means something if that object HAS a role —
            // otherwise `GameRole.active` is never written and the clause is silently, permanently false.
            if !engine.resolved_components(id).contains_key(ROLE_COMPONENT) {
                return Err(ConditionError::Impossible(format!(
                    "\"{}\" has no role yet, so it can never be collected or beaten — give it one first",
                    display_name(engine, id)
                )));
            }
            Condition {
                entity: id.to_loro_key(),
                component: ROLE_COMPONENT.into(),
                field: "active".into(),
                op: CompareOp::Eq,
                value: FieldValue::Bool(spec.kind == "other_still_there"),
            }
        }
        other_kind => return Err(ConditionError::UnknownKind(other_kind.to_string())),
    };
    Ok(condition)
}

/// The ops that add `clause` to `entity`'s clause set — the caller commits them (so a clause is one
/// undoable step, and can ride a larger transaction when the UI applies one clause to many objects).
///
/// # Errors
/// [`ConditionError::TooMany`] at the ceiling, or a duplicate (refused, never silently ignored).
pub fn add_clause_ops(
    engine: &Engine<FlecsWorld>,
    entity: EntityId,
    clause: Condition,
    any: bool,
) -> Result<Vec<Op>, ConditionError> {
    let mut clauses = clauses_of(engine, entity);
    if clauses.all.contains(&clause) || clauses.any.contains(&clause) {
        return Err(ConditionError::TooMany(
            "that condition is already on this object".into(),
        ));
    }
    if any {
        if clauses.any.len() >= MAX_ANY {
            return Err(ConditionError::TooMany(format!(
                "an object can hold at most {MAX_ANY} alternatives — remove one first"
            )));
        }
        clauses.any.push(clause);
    } else {
        if clauses.all.len() >= MAX_ALL {
            return Err(ConditionError::TooMany(format!(
                "an object can hold at most {MAX_ALL} conditions — remove one first"
            )));
        }
        clauses.all.push(clause);
    }
    Ok(write_ops(entity, &clauses))
}

/// The ops that remove one clause by index (`any` selects which list).
///
/// # Errors
/// [`ConditionError::MissingInput`] when the index isn't there.
pub fn remove_clause_ops(
    engine: &Engine<FlecsWorld>,
    entity: EntityId,
    index: usize,
    any: bool,
) -> Result<Vec<Op>, ConditionError> {
    let mut clauses = clauses_of(engine, entity);
    let list = if any {
        &mut clauses.any
    } else {
        &mut clauses.all
    };
    if index >= list.len() {
        return Err(ConditionError::MissingInput(
            "that condition is already gone".into(),
        ));
    }
    list.remove(index);
    if clauses.is_empty() {
        // Nothing left to say — drop the component rather than leaving an empty husk on the object.
        return Ok(vec![Op::RemoveComponent {
            entity,
            component: CONDITION_COMPONENT.into(),
        }]);
    }
    Ok(write_ops(entity, &clauses))
}

fn write_ops(entity: EntityId, clauses: &Clauses) -> Vec<Op> {
    let json = serde_json::to_string(clauses).unwrap_or_else(|_| "{}".into());
    vec![
        Op::SetField {
            entity,
            component: CONDITION_COMPONENT.into(),
            field: "source".into(),
            value: FieldValue::Str(json),
        },
        Op::SetField {
            entity,
            component: CONDITION_COMPONENT.into(),
            field: "join".into(),
            value: FieldValue::Str(if clauses.any.is_empty() {
                "all".into()
            } else {
                "any".into()
            }),
        },
    ]
}

/// **The Play-start expansion.** Every `$subject`-scoped rule becomes one rule per role-carrying entity,
/// pinned to that entity, with that entity's own clauses appended — and the shared original is dropped, so
/// nothing can double-fire. This is what lets one authored rule serve many objects with DIFFERENT
/// conditions while the document stays at exactly one copy of the rule.
///
/// Rules without `$subject` (world rules) pass through untouched.
#[must_use]
pub fn expand_subject_rules<W: World>(
    engine: &Engine<W>,
    rules: Vec<(metrocalk_core::rules::RuleId, RuleData)>,
) -> Vec<(metrocalk_core::rules::RuleId, RuleData)> {
    use metrocalk_core::rules::RuleId;

    // Every entity that can be a rule subject: it carries a role, and it is not a deactivated part
    // (a hidden object must not play — the same guard the roster and the render rebuild use).
    let mut subjects: Vec<(String, EntityId)> = engine
        .entity_ids()
        .into_iter()
        .filter(|id| engine.parent_of(*id).is_none() || engine.is_active(*id))
        .filter(|id| engine.resolved_components(*id).contains_key(ROLE_COMPONENT))
        .map(|id| (id.to_loro_key(), id))
        .collect();
    // Sorted by the raw (peer, counter) so the expansion order is deterministic across peers — the
    // ordering IS the contract here, not an incidental tidy-up.
    subjects.sort_by_key(|s| (s.1.peer, s.1.counter));

    let mut out = Vec::new();
    for (id, rule) in rules {
        if !mentions_subject(&rule) {
            out.push((id, rule));
            continue;
        }
        for (key, entity) in &subjects {
            let clauses = clauses_of(engine, *entity);
            let mut copy = rule.clone();
            copy.subject = Some(key.clone());
            for c in &mut copy.conditions {
                if c.entity == SUBJECT_ENTITY {
                    c.entity.clone_from(key);
                }
            }
            // Per-object POINTS. The shared rule carries a placeholder `+1`; each expanded copy is
            // rewritten to this object's own `GameRole.points`. Without this the Points field the
            // Inspector offers is inert — the rule would always score 1 no matter what the user typed.
            let points = engine
                .resolved_components(*entity)
                .get(ROLE_COMPONENT)
                .and_then(|fields| match fields.get("points") {
                    Some(FieldValue::Integer(i)) if *i != 0 => Some(*i),
                    #[allow(clippy::cast_possible_truncation)]
                    Some(FieldValue::Number(n)) if *n != 0.0 => Some(*n as i64),
                    _ => None,
                });
            for a in &mut copy.actions {
                if a.entity == SUBJECT_ENTITY {
                    a.entity.clone_from(key);
                }
                if let (Some(p), "AdjustCounter") = (points, a.action.as_str()) {
                    a.value = FieldValue::Integer(p);
                }
            }
            // The user's own clauses, resolved the same way (a `$subject` clause is about THIS object).
            let resolve = |mut c: Condition| {
                if c.entity == SUBJECT_ENTITY {
                    c.entity.clone_from(key);
                }
                c
            };
            copy.conditions
                .extend(clauses.all.iter().cloned().map(resolve));
            copy.any_of.extend(clauses.any.iter().cloned().map(resolve));
            // `#` never occurs in a loro key (they are hex + one `_`), so the id stays collision-free.
            out.push((RuleId::new(format!("{}#{key}", id.as_str())), copy));
        }
    }
    out
}

fn mentions_subject(rule: &RuleData) -> bool {
    rule.conditions
        .iter()
        .chain(rule.any_of.iter())
        .any(|c| c.entity == SUBJECT_ENTITY)
        || rule.actions.iter().any(|a| a.entity == SUBJECT_ENTITY)
}

fn display_name(engine: &Engine<FlecsWorld>, id: EntityId) -> String {
    engine
        .resolved_components(id)
        .get(INSTANCE_META)
        .and_then(|m| match m.get(NAME_FIELD) {
            Some(FieldValue::Str(s)) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_else(|| id.to_loro_key())
}

/// Render one clause as the sentence fragment the user reads ("the Score is at least 3").
#[must_use]
pub fn describe_clause(engine: &Engine<FlecsWorld>, c: &Condition) -> String {
    let n = match &c.value {
        FieldValue::Integer(i) => i.to_string(),
        FieldValue::Number(f) => format!("{f}"),
        FieldValue::Bool(b) => b.to_string(),
        FieldValue::Str(s) => s.clone(),
    };
    if c.entity == metrocalk_core::rules::OTHER_ENTITY {
        return match &c.value {
            FieldValue::Str(role) if role == "player" => "the player touched it".into(),
            FieldValue::Str(role) if role == "companion" => "a companion touched it".into(),
            FieldValue::Str(role) => format!("a {role} touched it"),
            other => format!("the toucher's {} is {other:?}", c.field),
        };
    }
    match (c.component.as_str(), c.field.as_str(), c.op) {
        ("KillCounter", "count", CompareOp::Ge) => format!("the Score is at least {n}"),
        ("KillCounter", "count", CompareOp::Lt) => format!("the Score is under {n}"),
        (comp, "active", CompareOp::Eq) if comp == ROLE_COMPONENT => {
            let subject = c.entity == SUBJECT_ENTITY;
            let gone = matches!(c.value, FieldValue::Bool(false));
            let who = if subject {
                "it".to_string()
            } else {
                EntityId::from_loro_key(&c.entity)
                    .map_or_else(|| c.entity.clone(), |id| display_name(engine, id))
            };
            match (subject, gone) {
                (true, false) => "it hasn't been used yet".into(),
                (true, true) => "it has already been used".into(),
                (false, true) => format!("{who} is gone"),
                (false, false) => format!("{who} is still there"),
            }
        }
        _ => format!("{}.{} {} {n}", c.component, c.field, op_word(c.op)),
    }
}

fn op_word(op: CompareOp) -> &'static str {
    match op {
        CompareOp::Eq => "is",
        CompareOp::Ne => "is not",
        CompareOp::Lt => "is under",
        CompareOp::Le => "is at most",
        CompareOp::Gt => "is over",
        CompareOp::Ge => "is at least",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capscene::{CapResolver, CapScene};
    use metrocalk_core::rules::{Action, RuleId};
    use metrocalk_core::Registry;

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
                    Op::SetField {
                        entity: id,
                        component: "Transform".into(),
                        field: "x".into(),
                        value: FieldValue::Number(x),
                    },
                ],
            )
            .expect("spawns");
        id
    }

    /// The pickup rule plus its scoring action — the shape whose `+1` the expansion rewrites.
    fn subject_rule_with_score() -> (RuleId, RuleData) {
        let (id, mut rule) = subject_rule();
        rule.actions.push(Action {
            action: "AdjustCounter".into(),
            entity: "1_99".into(),
            component: "KillCounter".into(),
            field: "count".into(),
            value: FieldValue::Integer(1),
        });
        (id, rule)
    }

    /// The shipped pickup rule's shape: `$subject` in both a condition and an action.
    fn subject_rule() -> (RuleId, RuleData) {
        (
            RuleId::new("rule_role_pickup"),
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
                actions: vec![Action {
                    action: "SetField".into(),
                    entity: SUBJECT_ENTITY.into(),
                    component: ROLE_COMPONENT.into(),
                    field: "active".into(),
                    value: FieldValue::Bool(false),
                }],
            },
        )
    }

    #[test]
    fn a_clause_is_one_undoable_edit_on_the_object_itself() {
        let (mut engine, scene, registry) = world();
        let coin = spawn(&mut engine, 0.0);
        crate::role_intent::assign_role(&mut engine, &scene, &registry, coin, "collectible")
            .expect("role");
        let clause = build_clause(
            &engine,
            &ClauseRequest {
                kind: "score_at_least".into(),
                number: Some(3.0),
                ..ClauseRequest::default()
            },
        )
        .expect("clause");
        let ops = add_clause_ops(&engine, coin, clause, false).expect("ops");
        engine.commit("only-if", ops).expect("commits");

        let stored = clauses_of(&engine, coin);
        assert_eq!(stored.all.len(), 1);
        assert_eq!(
            describe_clause(&engine, &stored.all[0]),
            "the Score is at least 3"
        );

        assert!(engine.undo(), "one Ctrl-Z");
        assert!(clauses_of(&engine, coin).is_empty(), "the clause is gone");
    }

    #[test]
    fn expansion_gives_each_object_its_own_rule_and_its_own_clauses() {
        let (mut engine, scene, registry) = world();
        let cheap = spawn(&mut engine, 0.0);
        let vault = spawn(&mut engine, 4.0);
        for e in [cheap, vault] {
            crate::role_intent::assign_role(&mut engine, &scene, &registry, e, "collectible")
                .expect("role");
        }
        // Only the vault is gated.
        let clause = build_clause(
            &engine,
            &ClauseRequest {
                kind: "score_at_least".into(),
                number: Some(3.0),
                ..ClauseRequest::default()
            },
        )
        .expect("clause");
        let ops = add_clause_ops(&engine, vault, clause, false).expect("ops");
        engine.commit("only-if", ops).expect("commits");

        let expanded = expand_subject_rules(&engine, vec![subject_rule()]);
        // One rule per role-carrying entity — and the shared original is GONE (no double-fire).
        assert!(
            expanded.iter().all(|(id, _)| id.as_str().contains('#')),
            "the un-expanded shared rule must not survive"
        );
        let vault_key = vault.to_loro_key();
        let cheap_key = cheap.to_loro_key();
        let vault_rule = expanded
            .iter()
            .find(|(_, r)| r.subject.as_deref() == Some(vault_key.as_str()))
            .expect("vault rule");
        let cheap_rule = expanded
            .iter()
            .find(|(_, r)| r.subject.as_deref() == Some(cheap_key.as_str()))
            .expect("cheap rule");
        assert_eq!(
            vault_rule.1.conditions.len(),
            2,
            "role clause + user clause"
        );
        assert_eq!(
            cheap_rule.1.conditions.len(),
            1,
            "ungated stays as authored"
        );
        // No `$subject` survives expansion — every slot is a real entity.
        for (_, r) in &expanded {
            assert!(r.conditions.iter().all(|c| c.entity != SUBJECT_ENTITY));
            assert!(r.actions.iter().all(|a| a.entity != SUBJECT_ENTITY));
        }
    }

    #[test]
    fn each_object_scores_its_own_points_not_a_hardcoded_one() {
        let (mut engine, scene, registry) = world();
        let coin = spawn(&mut engine, 0.0);
        let vault = spawn(&mut engine, 4.0);
        for e in [coin, vault] {
            crate::role_intent::assign_role(&mut engine, &scene, &registry, e, "collectible")
                .expect("role");
        }
        // The user sets the vault to 10 points in the Inspector — an ordinary field edit.
        engine
            .commit(
                "tune",
                vec![Op::SetField {
                    entity: vault,
                    component: ROLE_COMPONENT.into(),
                    field: "points".into(),
                    value: FieldValue::Integer(10),
                }],
            )
            .expect("commits");

        let expanded = expand_subject_rules(&engine, vec![subject_rule_with_score()]);
        let points_for = |e: EntityId| -> i64 {
            let key = e.to_loro_key();
            expanded
                .iter()
                .find(|(_, r)| r.subject.as_deref() == Some(key.as_str()))
                .and_then(|(_, r)| {
                    r.actions
                        .iter()
                        .find(|a| a.action == "AdjustCounter")
                        .map(|a| match a.value {
                            FieldValue::Integer(i) => i,
                            _ => -1,
                        })
                })
                .expect("a rule for this object")
        };
        assert_eq!(
            points_for(vault),
            10,
            "the vault scores what the user typed"
        );
        assert_eq!(points_for(coin), 1, "an untouched object keeps the default");
    }

    #[test]
    fn impossible_clauses_are_refused_with_a_reason_and_the_ceiling_holds() {
        let (mut engine, scene, registry) = world();
        let coin = spawn(&mut engine, 0.0);
        let rock = spawn(&mut engine, 2.0);

        // No Score exists yet → the score card explains itself instead of authoring a dead clause.
        let msg = build_clause(
            &engine,
            &ClauseRequest {
                kind: "score_at_least".into(),
                number: Some(1.0),
                ..ClauseRequest::default()
            },
        )
        .expect_err("no score yet")
        .to_string();
        assert!(msg.contains("no Score yet"), "{msg}");

        crate::role_intent::assign_role(&mut engine, &scene, &registry, coin, "collectible")
            .expect("role");
        // A roleless object can never be "gone" — refused, named.
        let msg = build_clause(
            &engine,
            &ClauseRequest {
                kind: "other_gone".into(),
                object: Some(rock.to_loro_key()),
                ..ClauseRequest::default()
            },
        )
        .expect_err("roleless")
        .to_string();
        assert!(msg.contains("no role yet"), "{msg}");

        // The ceiling refuses the 5th AND clause, with a way out.
        for n in 1..=MAX_ALL {
            let c = build_clause(
                &engine,
                &ClauseRequest {
                    kind: "score_at_least".into(),
                    number: Some(f64::from(u32::try_from(n).unwrap_or(1))),
                    ..ClauseRequest::default()
                },
            )
            .expect("clause");
            let ops = add_clause_ops(&engine, coin, c, false).expect("under the cap");
            engine.commit("only-if", ops).expect("commits");
        }
        let one_more = build_clause(
            &engine,
            &ClauseRequest {
                kind: "score_at_least".into(),
                number: Some(99.0),
                ..ClauseRequest::default()
            },
        )
        .expect("clause");
        let msg = add_clause_ops(&engine, coin, one_more, false)
            .expect_err("over the cap")
            .to_string();
        assert!(msg.contains("at most"), "{msg}");
    }

    #[test]
    fn removing_the_last_clause_drops_the_component_entirely() {
        let (mut engine, scene, registry) = world();
        let coin = spawn(&mut engine, 0.0);
        crate::role_intent::assign_role(&mut engine, &scene, &registry, coin, "collectible")
            .expect("role");
        let clause = build_clause(
            &engine,
            &ClauseRequest {
                kind: "still_active".into(),
                ..ClauseRequest::default()
            },
        )
        .expect("clause");
        let ops = add_clause_ops(&engine, coin, clause, false).expect("ops");
        engine.commit("only-if", ops).expect("commits");
        assert!(engine.components_of(coin).contains_key(CONDITION_COMPONENT));

        let ops = remove_clause_ops(&engine, coin, 0, false).expect("remove");
        engine.commit("clear-only-if", ops).expect("commits");
        assert!(
            !engine.components_of(coin).contains_key(CONDITION_COMPONENT),
            "no empty husk left behind"
        );
    }
}
