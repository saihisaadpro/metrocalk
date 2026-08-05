//! The **match cook** — turn authored editor entities into runtime kernel definitions.
//!
//! Until this module existed the only way to run a match inside the editor was a hand-written
//! `build_match()` in the Tauri shell: the kernel ran, and it rendered, but nothing the user authored
//! reached it. That made the demonstration prove *mechanics* without proving a *workflow*. This module is
//! the missing conversion layer, and it is deliberately shaped like the rest of this codebase's authoring
//! seams (`csg_intent`, `sdf_intent`, `physics_intent`): pure logic over [`Engine`] components, headless,
//! and covered by the workspace test suite rather than by a shell-only integration path.
//!
//! ## The contract
//!
//! * **Read-only.** Cooking never mutates the authored document. It takes `&Engine`, not `&mut`, so the
//!   type system enforces what ADR-021/034 require of a projection: no ops, no undo entries, no CRDT
//!   writes. A scene that fails to cook is left exactly as the author wrote it.
//! * **Deterministic.** Entities are read in `(peer, counter)` order, every collection is ordered, and the
//!   artifact carries an FNV-1a digest (the same hash family the kernel's own definition digest uses) over
//!   a canonical encoding. The same authored document always cooks to the same bytes.
//! * **Versioned.** [`MATCH_COOK_SCHEMA_VERSION`] rides the artifact so a stale cook can be detected
//!   rather than silently replayed against a newer reader.
//! * **Diagnosed, not guessed.** The kernel's own validators return `&'static str` reasons with no idea
//!   which entity caused them. The cook front-runs every one of those checks and anchors its refusal to
//!   the authored entity and field, because "invalid one-lane match: wave ticks, interval, and live cap
//!   must be positive and future" is not something an author can act on.
//! * **Neutral.** The artifact is plain serde data, so it can be inspected, diffed, stored and compared
//!   without linking the kernel. [`CookedMatch::build`] is the separate, equally deterministic step that
//!   instantiates the kernel from it.
//!
//! ## Units and axes
//!
//! Authors work in metres and seconds because that is what the viewport and every other editor panel use.
//! The kernel is authoritative in integer millimetres and fixed ticks and must never learn about floats.
//! This module is the single conversion, so no other code has to know either side's units.
//!
//! The match plane is the viewport's ground plane: authored `Transform.x` → kernel `x`, authored
//! `Transform.z` → kernel `y`. `Transform.y` is render height and carries no gameplay meaning.
//!
//! The live document writes translation as `x`/`y`/`z` (that is what `capscene::place_mesh`, the gizmo
//! and the renderer all use) while the registry schema declares `px`/`py`/`pz`, and the animation binding
//! registry treats both as the same channel. Reading only one of the two would silently cook every actor
//! to the origin, so [`authored_position`] accepts either and says so when neither is present.

use std::collections::{BTreeMap, BTreeSet};

use metrocalk_core::{Engine, EntityId, FieldValue};
use metrocalk_ecs::FlecsWorld;
use metrocalk_gameplay::{
    ActorKind, ActorSpawn, BasicAttackSpec, Bounty, CombatStats, DamageSchool, DeathRule, LaneId,
    LanePosition, LaneSpec, MatchConfig, MatchRuntime, OneLaneMatch, PlayerId, RectMm, StatGrowth,
    TeamId, Vec2Mm, WaveId, WaveSpec, WaveUnitSpec,
};
use serde::{Deserialize, Serialize};

/// Match-wide settings. Exactly one entity in a match scene carries this.
pub const MATCH_SETTINGS: &str = "MatchSettings";
/// The lane corridor's width. Exactly one entity carries this; its waypoints are separate entities.
pub const MATCH_LANE: &str = "MatchLane";
/// One ordered point on the lane centreline. Its position comes from the entity's `Transform`.
pub const LANE_WAYPOINT: &str = "LaneWaypoint";
/// One authored actor — a hero, a structure or a standing objective. Position comes from `Transform`.
pub const MATCH_ACTOR: &str = "MatchActor";
/// One repeating wave schedule.
pub const MATCH_WAVE: &str = "MatchWave";

/// The cooked-artifact schema version. Bump on any change to [`CookedMatch`]'s shape or meaning.
pub const MATCH_COOK_SCHEMA_VERSION: u32 = 2;

/// The single lane the MOB-2 one-route profile permits.
const LANE: LaneId = LaneId(0);
/// The local player. One authored hero may be `owned`; it answers to this identity.
const LOCAL_PLAYER: PlayerId = PlayerId(1);

/// Millimetres per authored metre. The one place the unit systems meet.
const MM_PER_METRE: f64 = 1_000.0;
/// Authored coordinates beyond this (metres) cannot be represented in the kernel's `i32` millimetres.
const MAX_AUTHORED_METRES: f64 = 2_000_000.0;

// ─────────────────────────────────────────────────────────────────────────────────────────────────────
// Diagnostics
// ─────────────────────────────────────────────────────────────────────────────────────────────────────

/// Whether a diagnostic blocks the cook or merely warns about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CookSeverity {
    /// The scene still cooks; the author should know something is off.
    Warning,
    /// The scene cannot cook. No artifact is produced.
    Error,
}

/// One thing the cook has to say about the authored scene, anchored to what the author can actually see.
///
/// `entity` is the Loro key the hierarchy and inspector use, so a shell can select the offending object
/// directly from a diagnostic rather than making the author hunt for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CookDiagnostic {
    pub severity: CookSeverity,
    /// Stable machine-readable code. Tests and the UI match on this, never on the prose.
    pub code: String,
    /// Plain language: what is wrong, and what to do about it.
    pub message: String,
    /// The authored entity at fault, as a Loro key. `None` for whole-scene problems.
    pub entity: Option<String>,
    /// The authored component at fault.
    pub component: Option<String>,
    /// The authored field at fault.
    pub field: Option<String>,
}

impl CookDiagnostic {
    fn scene(severity: CookSeverity, code: &str, message: impl Into<String>) -> Self {
        Self {
            severity,
            code: code.to_owned(),
            message: message.into(),
            entity: None,
            component: None,
            field: None,
        }
    }

    fn at(
        severity: CookSeverity,
        code: &str,
        entity: EntityId,
        component: &str,
        field: Option<&str>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            code: code.to_owned(),
            message: message.into(),
            entity: Some(entity.to_loro_key()),
            component: Some(component.to_owned()),
            field: field.map(ToOwned::to_owned),
        }
    }
}

/// The result of one cook. Both halves are always present: diagnostics are produced whether or not an
/// artifact was, so a warning is never lost just because the cook succeeded.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CookOutcome {
    /// The artifact, or `None` if any diagnostic is an [`CookSeverity::Error`].
    pub cooked: Option<CookedMatch>,
    /// Errors first, then warnings; stable order within each.
    pub diagnostics: Vec<CookDiagnostic>,
}

impl CookOutcome {
    #[must_use]
    pub fn ok(&self) -> bool {
        self.cooked.is_some()
    }

    pub fn errors(&self) -> impl Iterator<Item = &CookDiagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == CookSeverity::Error)
    }

    /// Every diagnostic code present, deduplicated and ordered — the shape tests assert on.
    #[must_use]
    pub fn codes(&self) -> Vec<String> {
        let unique: BTreeSet<&str> = self.diagnostics.iter().map(|d| d.code.as_str()).collect();
        unique.into_iter().map(ToOwned::to_owned).collect()
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────────
// The cooked artifact
// ─────────────────────────────────────────────────────────────────────────────────────────────────────

/// A cooked actor, already in kernel units.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CookedActor {
    pub id: u64,
    pub team: u8,
    /// `"hero"`, `"structure"`, `"minion"` or `"objective"`.
    pub role: String,
    /// `true` when the local player commands this actor.
    pub owned: bool,
    pub x_mm: i32,
    pub y_mm: i32,
    pub max_health: u32,
    pub move_speed_mm_per_tick: u32,
    pub physical_reduction_bps: u16,
    /// What killing this actor pays the killer, and what it grows per level. Both default to nothing, so
    /// a scene authored before these fields existed cooks to exactly the match it cooked before.
    pub bounty_gold: u32,
    pub bounty_experience: u32,
    pub health_per_level: u32,
    pub attack: Option<CookedAttack>,
    /// `Some(delay)` = respawn at the authored position after `delay` ticks; `None` = stays dead. A
    /// structure whose loss ends the match is expressed by `objective_for` instead.
    pub respawn_delay_ticks: Option<u32>,
    /// `Some(team)` = destroying this actor hands `team` the win.
    pub objective_for: Option<u8>,
    /// The authored entity this actor came from — the traceability the evidence chain needs.
    pub source: String,
}

/// A cooked basic attack, already in kernel units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CookedAttack {
    pub range_mm: u32,
    pub damage: u32,
    pub windup_ticks: u16,
    pub cooldown_ticks: u32,
}

/// A cooked lane corridor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CookedLane {
    /// Centreline points in authored waypoint order, in millimetres.
    pub centerline: Vec<(i32, i32)>,
    pub half_width_mm: u32,
    /// The authored waypoint entities, in the same order as `centerline`.
    pub sources: Vec<String>,
}

/// A cooked wave schedule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CookedWave {
    pub id: u32,
    pub team: u8,
    pub spawn_progress_mm: u64,
    pub goal_progress_mm: u64,
    pub aggro_range_mm: u32,
    pub first_tick: u64,
    pub interval_ticks: u32,
    pub max_alive: u32,
    /// Per-unit spacing behind the spawn point, in millimetres.
    pub unit_spacing_mm: i64,
    pub unit_count: u32,
    pub unit_max_health: u32,
    pub unit_move_speed_mm_per_tick: u32,
    pub unit_physical_reduction_bps: u16,
    pub unit_bounty_gold: u32,
    pub unit_bounty_experience: u32,
    pub unit_attack: Option<CookedAttack>,
    pub source: String,
}

/// The deterministic, versioned output of one cook: everything the kernel needs, and nothing it does not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CookedMatch {
    pub schema_version: u32,
    /// FNV-1a over the canonical encoding of everything below. Two documents that cook to the same
    /// definitions share this digest; any authored change that matters to the runtime changes it.
    pub digest: String,
    pub seed: u64,
    pub tick_rate_hz: u16,
    pub max_match_ticks: u64,
    /// Map bounds in millimetres: `(min_x, min_y, max_x, max_y)`.
    pub bounds_mm: (i32, i32, i32, i32),
    pub lane: CookedLane,
    pub actors: Vec<CookedActor>,
    pub waves: Vec<CookedWave>,
}

impl CookedMatch {
    /// Instantiate the kernel from this artifact.
    ///
    /// Kept separate from cooking so the artifact can be produced, stored, inspected and compared without
    /// the kernel, and so a failure here is unambiguously a *kernel* refusal rather than an authoring one.
    /// The cook validates everything the kernel validates, so in practice this only fails if the artifact
    /// was tampered with or the two drifted — which the tests treat as a bug, not a user error.
    pub fn build(&self) -> Result<OneLaneMatch, String> {
        if self.schema_version != MATCH_COOK_SCHEMA_VERSION {
            return Err(format!(
                "this cooked match is schema version {} but this build reads version {}",
                self.schema_version, MATCH_COOK_SCHEMA_VERSION
            ));
        }
        let (min_x, min_y, max_x, max_y) = self.bounds_mm;
        let config = MatchConfig {
            tick_rate_hz: self.tick_rate_hz,
            map_bounds: RectMm::new(Vec2Mm::new(min_x, min_y), Vec2Mm::new(max_x, max_y)),
            max_match_ticks: self.max_match_ticks,
            ..MatchConfig::default()
        };
        let mut runtime = MatchRuntime::new(config, self.seed).map_err(debug)?;

        for actor in &self.actors {
            runtime.spawn_actor(&kernel_spawn(actor)).map_err(debug)?;
        }
        runtime.start().map_err(debug)?;

        let lane = LaneSpec {
            id: LANE,
            centerline: self
                .lane
                .centerline
                .iter()
                .map(|(x, y)| Vec2Mm::new(*x, *y))
                .collect(),
            half_width_mm: self.lane.half_width_mm,
        };
        let waves = self.waves.iter().map(kernel_wave).collect();
        OneLaneMatch::attach(runtime, &lane, vec![], waves).map_err(debug)
    }

    /// The authored entity behind one cooked actor id, for tracing runtime state back to the document.
    #[must_use]
    pub fn source_of(&self, actor_id: u64) -> Option<&str> {
        self.actors
            .iter()
            .find(|actor| actor.id == actor_id)
            .map(|actor| actor.source.as_str())
    }
}

/// One cooked actor as the kernel's spawn record.
///
/// Extracted from `build` so each translation stays readable on its own; `build` itself is then just the
/// ordered pipeline config -> actors -> start -> lane -> waves -> attach.
fn kernel_spawn(actor: &CookedActor) -> ActorSpawn {
    let death_rule = match (actor.objective_for, actor.respawn_delay_ticks) {
        (Some(team), _) => DeathRule::ObjectiveVictory {
            winner: TeamId(team),
        },
        (None, Some(delay)) => DeathRule::Respawn {
            delay_ticks: delay,
            at: Vec2Mm::new(actor.x_mm, actor.y_mm),
        },
        (None, None) => DeathRule::StayDead,
    };
    ActorSpawn {
        id: metrocalk_gameplay::ActorId(actor.id),
        owner: actor.owned.then_some(LOCAL_PLAYER),
        team: TeamId(actor.team),
        kind: actor_kind(&actor.role),
        position: Vec2Mm::new(actor.x_mm, actor.y_mm),
        stats: CombatStats {
            max_health: actor.max_health,
            max_resource: 0,
            move_speed_mm_per_tick: actor.move_speed_mm_per_tick,
            physical_reduction_bps: actor.physical_reduction_bps,
            magic_reduction_bps: 0,
            crit_chance_bps: 0,
            crit_damage_bps: metrocalk_gameplay::BASIS_POINTS,
            physical_penetration_bps: 0,
            magic_penetration_bps: 0,
            lifesteal_bps: 0,
        },
        growth: StatGrowth {
            max_health_per_level: actor.health_per_level,
            ..StatGrowth::NONE
        },
        bounty: Bounty {
            gold: actor.bounty_gold,
            experience: actor.bounty_experience,
        },
        abilities: vec![],
        basic_attack: actor.attack.map(kernel_attack),
        death_rule,
    }
}

/// One cooked wave as the kernel's authored schedule.
fn kernel_wave(wave: &CookedWave) -> WaveSpec {
    WaveSpec {
        id: WaveId(wave.id),
        team: TeamId(wave.team),
        spawn: LanePosition {
            lane: LANE,
            progress_mm: wave.spawn_progress_mm,
        },
        goal: LanePosition {
            lane: LANE,
            progress_mm: wave.goal_progress_mm,
        },
        aggro_range_mm: wave.aggro_range_mm,
        target_priority: vec![ActorKind::Structure, ActorKind::Hero],
        cast_ability: None,
        first_tick: wave.first_tick,
        interval_ticks: wave.interval_ticks,
        max_alive: wave.max_alive,
        units: (0..wave.unit_count)
            .map(|index| WaveUnitSpec {
                progress_offset_mm: -wave.unit_spacing_mm * i64::from(index),
                stats: CombatStats {
                    max_health: wave.unit_max_health,
                    max_resource: 0,
                    move_speed_mm_per_tick: wave.unit_move_speed_mm_per_tick,
                    physical_reduction_bps: wave.unit_physical_reduction_bps,
                    magic_reduction_bps: 0,
                    crit_chance_bps: 0,
                    crit_damage_bps: metrocalk_gameplay::BASIS_POINTS,
                    physical_penetration_bps: 0,
                    magic_penetration_bps: 0,
                    lifesteal_bps: 0,
                },
                bounty: Bounty {
                    gold: wave.unit_bounty_gold,
                    experience: wave.unit_bounty_experience,
                },
                abilities: vec![],
                basic_attack: wave.unit_attack.map(kernel_attack),
            })
            .collect(),
    }
}

fn kernel_attack(attack: CookedAttack) -> BasicAttackSpec {
    BasicAttackSpec {
        range_mm: attack.range_mm,
        damage: attack.damage,
        school: DamageSchool::Physical,
        windup_ticks: attack.windup_ticks,
        cooldown_ticks: attack.cooldown_ticks,
    }
}

fn actor_kind(role: &str) -> ActorKind {
    match role {
        "hero" => ActorKind::Hero,
        "minion" => ActorKind::Minion,
        "objective" => ActorKind::Objective,
        // `cook` rejects any other role, so `structure` is the only remaining possibility. Defaulting
        // rather than panicking keeps a tampered artifact from taking the shell down.
        _ => ActorKind::Structure,
    }
}

fn debug<E: std::fmt::Debug>(error: E) -> String {
    format!("{error:?}")
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────────
// Reading authored fields
// ─────────────────────────────────────────────────────────────────────────────────────────────────────

/// One authored entity's components, already fetched.
type Components = BTreeMap<String, BTreeMap<String, FieldValue>>;

/// Every entity carrying `component`, in `(peer, counter)` order.
///
/// Sorting here — rather than trusting `entity_ids()`'s order — is what makes the cook reproducible
/// across sessions, machines and CRDT merge histories.
fn authored_with(engine: &Engine<FlecsWorld>, component: &str) -> Vec<(EntityId, Components)> {
    let mut ids = engine.entity_ids();
    ids.sort_unstable_by_key(|id| (id.peer, id.counter));
    ids.into_iter()
        .filter_map(|id| {
            let comps: Components = engine
                .components_of(id)
                .into_iter()
                .map(|(name, fields)| (name, fields.into_iter().collect()))
                .collect();
            comps.contains_key(component).then_some((id, comps))
        })
        .collect()
}

/// Read a numeric field, accepting both JSON integer and float encodings.
///
/// A whole number authored through JSON arrives as [`FieldValue::Integer`], not `Number`. Matching only
/// one arm is the exact bug that once made an authored `scale = 2` silently fall back to its default, so
/// every numeric read in this module goes through here.
fn number(comps: &Components, component: &str, field: &str) -> Option<f64> {
    match comps.get(component)?.get(field)? {
        FieldValue::Number(n) => Some(*n),
        #[allow(clippy::cast_precision_loss)]
        FieldValue::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

fn integer(comps: &Components, component: &str, field: &str) -> Option<i64> {
    match comps.get(component)?.get(field)? {
        FieldValue::Integer(i) => Some(*i),
        // A whole float is a legitimate authoring of an integer field; a fractional one is not, and is
        // reported by the caller's range check rather than silently truncated here.
        FieldValue::Number(n) if n.is_finite() && n.fract() == 0.0 && n.abs() < 9e18 =>
        {
            #[allow(clippy::cast_possible_truncation)]
            Some(*n as i64)
        }
        _ => None,
    }
}

fn text<'a>(comps: &'a Components, component: &str, field: &str) -> Option<&'a str> {
    match comps.get(component)?.get(field)? {
        FieldValue::Str(s) => Some(s.as_str()),
        _ => None,
    }
}

fn flag(comps: &Components, component: &str, field: &str) -> Option<bool> {
    match comps.get(component)?.get(field)? {
        FieldValue::Bool(b) => Some(*b),
        _ => None,
    }
}

/// A cook that is still collecting diagnostics. Every read goes through it so a failure reports the
/// authored anchor instead of an anonymous `None`.
struct Cook {
    diagnostics: Vec<CookDiagnostic>,
    failed: bool,
}

impl Cook {
    fn new() -> Self {
        Self {
            diagnostics: Vec::new(),
            failed: false,
        }
    }

    fn error(
        &mut self,
        code: &str,
        entity: EntityId,
        component: &str,
        field: Option<&str>,
        message: impl Into<String>,
    ) {
        self.failed = true;
        self.diagnostics.push(CookDiagnostic::at(
            CookSeverity::Error,
            code,
            entity,
            component,
            field,
            message,
        ));
    }

    fn warn(
        &mut self,
        code: &str,
        entity: EntityId,
        component: &str,
        field: Option<&str>,
        message: impl Into<String>,
    ) {
        self.diagnostics.push(CookDiagnostic::at(
            CookSeverity::Warning,
            code,
            entity,
            component,
            field,
            message,
        ));
    }

    fn scene_error(&mut self, code: &str, message: impl Into<String>) {
        self.failed = true;
        self.diagnostics
            .push(CookDiagnostic::scene(CookSeverity::Error, code, message));
    }

    /// The entity's ground-plane position in millimetres, as `(x, y)` in kernel axes.
    ///
    /// Accepts both spellings of the translation channel (see this module's header). Returns `None` and
    /// reports it when the entity has no usable position at all, so the caller can skip the actor rather
    /// than cook it to a silent `(0, 0)`.
    fn authored_position(
        &mut self,
        comps: &Components,
        entity: EntityId,
        owner: &str,
    ) -> Option<(i32, i32)> {
        let Some(transform) = comps.get("Transform") else {
            self.error(
                "no-position",
                entity,
                owner,
                None,
                "This object has no Transform, so it has no position on the map.",
            );
            return None;
        };
        let axis = |names: [&str; 2]| -> Option<(&'static str, f64)> {
            for name in names {
                if let Some(value) = transform.get(name) {
                    let number = match value {
                        FieldValue::Number(n) => Some(*n),
                        #[allow(clippy::cast_precision_loss)]
                        FieldValue::Integer(i) => Some(*i as f64),
                        _ => None,
                    };
                    if let Some(number) = number {
                        // Leaked to a `'static` name for the diagnostic anchor; both candidates are
                        // literals so this is just picking which one was found.
                        let found = if name.len() == 1 { "x" } else { "px" };
                        return Some((found, number));
                    }
                }
            }
            None
        };
        let (x_field, x) = axis(["x", "px"]).or_else(|| {
            self.error(
                "no-position",
                entity,
                owner,
                Some("Transform.x"),
                "This object's Transform has no X position.",
            );
            None
        })?;
        let (_, z) = axis(["z", "pz"]).or_else(|| {
            self.error(
                "no-position",
                entity,
                owner,
                Some("Transform.z"),
                "This object's Transform has no Z position.",
            );
            None
        })?;
        let x_name = if x_field == "x" { "x" } else { "px" };
        let z_name = if x_field == "x" { "z" } else { "pz" };
        Some((
            self.mm_from(x, entity, "Transform", x_name),
            self.mm_from(z, entity, "Transform", z_name),
        ))
    }

    /// A required metre-valued field, converted to millimetres.
    fn metres_mm(
        &mut self,
        comps: &Components,
        entity: EntityId,
        component: &str,
        field: &str,
    ) -> i32 {
        let Some(value) = number(comps, component, field) else {
            self.error(
                "missing-field",
                entity,
                component,
                Some(field),
                format!("`{field}` is required and must be a number of metres."),
            );
            return 0;
        };
        self.mm_from(value, entity, component, field)
    }

    fn mm_from(&mut self, metres: f64, entity: EntityId, component: &str, field: &str) -> i32 {
        if !metres.is_finite() {
            self.error(
                "not-finite",
                entity,
                component,
                Some(field),
                format!("`{field}` is {metres}; positions must be ordinary finite numbers."),
            );
            return 0;
        }
        if metres.abs() > MAX_AUTHORED_METRES {
            self.error(
                "out-of-range",
                entity,
                component,
                Some(field),
                format!(
                    "`{field}` is {metres} m, past the ±{MAX_AUTHORED_METRES} m the match runtime can \
                     represent. Move this closer to the origin."
                ),
            );
            return 0;
        }
        // `round` is half-away-from-zero and deterministic, and the range check above guarantees the
        // result fits an `i32`, so this cast cannot wrap.
        #[allow(clippy::cast_possible_truncation)]
        {
            (metres * MM_PER_METRE).round() as i32
        }
    }

    /// A required positive integer field, range-checked into `u32`.
    fn positive_u32(
        &mut self,
        comps: &Components,
        entity: EntityId,
        component: &str,
        field: &str,
        what: &str,
    ) -> u32 {
        let Some(value) = integer(comps, component, field) else {
            self.error(
                "missing-field",
                entity,
                component,
                Some(field),
                format!("`{field}` is required and must be a whole number ({what})."),
            );
            return 1;
        };
        match u32::try_from(value) {
            Ok(0) | Err(_) => {
                self.error(
                    "out-of-range",
                    entity,
                    component,
                    Some(field),
                    format!(
                        "`{field}` is {value}; {what} must be between 1 and {}.",
                        u32::MAX
                    ),
                );
                1
            }
            Ok(positive) => positive,
        }
    }

    /// An optional non-negative integer field, range-checked into `u32`.
    fn optional_u32(
        &mut self,
        comps: &Components,
        entity: EntityId,
        component: &str,
        field: &str,
    ) -> Option<u32> {
        let value = integer(comps, component, field)?;
        if let Ok(fits) = u32::try_from(value) {
            return Some(fits);
        }
        self.error(
            "out-of-range",
            entity,
            component,
            Some(field),
            format!("`{field}` is {value}; it must be zero or a positive whole number."),
        );
        None
    }

    /// An optional whole number, refused at cook time if it exceeds the kernel's own ceiling.
    ///
    /// Bounding here rather than letting `spawn_actor` refuse it is what keeps the standing invariant
    /// true: anything the cook accepts, the kernel must accept. An out-of-range value surfaces as an
    /// authored-content error naming the field, not as an opaque runtime rejection.
    fn bounded_u32(
        &mut self,
        comps: &Components,
        entity: EntityId,
        component: &str,
        field: &str,
        ceiling: u32,
    ) -> u32 {
        match self.optional_u32(comps, entity, component, field) {
            Some(value) if value > ceiling => {
                self.error(
                    "out-of-range",
                    entity,
                    component,
                    Some(field),
                    format!("`{field}` is {value}; it must be at most {ceiling}."),
                );
                0
            }
            Some(value) => value,
            None => 0,
        }
    }

    /// Authored metres-per-second → kernel millimetres-per-tick.
    fn speed_mm_per_tick(
        &mut self,
        comps: &Components,
        entity: EntityId,
        component: &str,
        field: &str,
        tick_rate_hz: u16,
    ) -> u32 {
        let Some(metres_per_second) = number(comps, component, field) else {
            self.error(
                "missing-field",
                entity,
                component,
                Some(field),
                format!("`{field}` is required and must be a speed in metres per second."),
            );
            return 0;
        };
        if !metres_per_second.is_finite() || metres_per_second < 0.0 {
            self.error(
                "out-of-range",
                entity,
                component,
                Some(field),
                format!("`{field}` is {metres_per_second}; a speed must be zero or positive."),
            );
            return 0;
        }
        let per_tick = (metres_per_second * MM_PER_METRE / f64::from(tick_rate_hz)).round();
        if per_tick > f64::from(u32::MAX) {
            self.error(
                "out-of-range",
                entity,
                component,
                Some(field),
                format!("`{field}` is {metres_per_second} m/s, faster than the runtime can move."),
            );
            return 0;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let per_tick = per_tick as u32;
        if per_tick == 0 && metres_per_second > 0.0 {
            self.warn(
                "speed-rounds-to-zero",
                entity,
                component,
                Some(field),
                format!(
                    "`{field}` is {metres_per_second} m/s, which rounds to zero movement per tick at \
                     {tick_rate_hz} Hz. This actor will stand still."
                ),
            );
        }
        per_tick
    }

    /// Read an optional basic attack. Present only if `attackDamage` is authored.
    fn attack(
        &mut self,
        comps: &Components,
        entity: EntityId,
        component: &str,
        prefix: &str,
    ) -> Option<CookedAttack> {
        let damage_field = format!("{prefix}Damage");
        let damage = self.optional_u32(comps, entity, component, &damage_field)?;
        if damage == 0 {
            self.error(
                "out-of-range",
                entity,
                component,
                Some(&damage_field),
                format!(
                    "`{damage_field}` is 0; an attack that deals no damage should be removed \
                         instead of authored as zero."
                ),
            );
            return None;
        }
        let range_field = format!("{prefix}Range");
        let range_mm = self.metres_mm(comps, entity, component, &range_field);
        let range_mm = match u32::try_from(range_mm) {
            Ok(0) | Err(_) => {
                self.error(
                    "out-of-range",
                    entity,
                    component,
                    Some(&range_field),
                    format!("`{range_field}` must be a positive distance in metres."),
                );
                1
            }
            Ok(positive) => positive,
        };
        let windup = self
            .optional_u32(comps, entity, component, &format!("{prefix}WindupTicks"))
            .unwrap_or(1);
        let windup = u16::try_from(windup).unwrap_or(u16::MAX);
        let cooldown = self
            .optional_u32(comps, entity, component, &format!("{prefix}CooldownTicks"))
            .unwrap_or(4)
            .max(1);
        Some(CookedAttack {
            range_mm,
            damage,
            windup_ticks: windup,
            cooldown_ticks: cooldown,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────────
// The cook
// ─────────────────────────────────────────────────────────────────────────────────────────────────────

/// Cook the authored scene into runtime kernel definitions.
///
/// Takes `&Engine` — cooking is a read. Returns diagnostics whether or not it succeeded.
#[must_use]
#[allow(clippy::too_many_lines)] // one linear pass over the authored schema; splitting it hides the order
pub fn cook_match(engine: &Engine<FlecsWorld>) -> CookOutcome {
    let mut cook = Cook::new();

    // ── settings ─────────────────────────────────────────────────────────────────────────────────────
    let settings = authored_with(engine, MATCH_SETTINGS);
    let (settings_id, settings_comps) = match settings.len() {
        0 => {
            cook.scene_error(
                "no-match-settings",
                "This scene has no match settings, so there is nothing to run. Add a Match Settings \
                 object to define the play area and match length.",
            );
            return finish(cook, None);
        }
        1 => settings[0].clone(),
        n => {
            for (id, _) in settings.iter().skip(1) {
                cook.error(
                    "duplicate-match-settings",
                    *id,
                    MATCH_SETTINGS,
                    None,
                    format!(
                        "This scene has {n} Match Settings objects. A match has exactly one; delete \
                         the extras."
                    ),
                );
            }
            return finish(cook, None);
        }
    };

    let tick_rate_hz = match integer(&settings_comps, MATCH_SETTINGS, "tickRateHz") {
        None => 30,
        Some(value) => match u16::try_from(value) {
            Ok(rate) if rate > 0 => rate,
            _ => {
                cook.error(
                    "out-of-range",
                    settings_id,
                    MATCH_SETTINGS,
                    Some("tickRateHz"),
                    format!(
                        "`tickRateHz` is {value}; it must be between 1 and {}.",
                        u16::MAX
                    ),
                );
                30
            }
        },
    };
    let seed = integer(&settings_comps, MATCH_SETTINGS, "seed")
        .map_or(0x4D4F_4241_5345_4544, i64::cast_unsigned);
    let max_match_ticks = match integer(&settings_comps, MATCH_SETTINGS, "maxTicks") {
        None => 3_000,
        Some(value) => match u64::try_from(value) {
            Ok(ticks) if ticks > 0 && ticks <= metrocalk_gameplay::MAX_MATCH_TICKS => ticks,
            _ => {
                cook.error(
                    "out-of-range",
                    settings_id,
                    MATCH_SETTINGS,
                    Some("maxTicks"),
                    format!(
                        "`maxTicks` is {value}; a match must run between 1 and {} ticks.",
                        metrocalk_gameplay::MAX_MATCH_TICKS
                    ),
                );
                3_000
            }
        },
    };

    let min_x = cook.metres_mm(&settings_comps, settings_id, MATCH_SETTINGS, "boundsMinX");
    let min_y = cook.metres_mm(&settings_comps, settings_id, MATCH_SETTINGS, "boundsMinZ");
    let max_x = cook.metres_mm(&settings_comps, settings_id, MATCH_SETTINGS, "boundsMaxX");
    let max_y = cook.metres_mm(&settings_comps, settings_id, MATCH_SETTINGS, "boundsMaxZ");
    if min_x >= max_x || min_y >= max_y {
        cook.error(
            "empty-bounds",
            settings_id,
            MATCH_SETTINGS,
            Some("boundsMaxX"),
            "The play area has no width or no depth. Its maximum X and Z must both be greater than \
             its minimum.",
        );
    }
    let bounds = RectMm::new(Vec2Mm::new(min_x, min_y), Vec2Mm::new(max_x, max_y));

    // ── lane ─────────────────────────────────────────────────────────────────────────────────────────
    let lanes = authored_with(engine, MATCH_LANE);
    let (lane_id, lane_comps) = match lanes.len() {
        0 => {
            cook.scene_error(
                "no-lane",
                "This scene has no lane. Add a Lane object and at least two Lane Waypoints so units \
                 have a route to walk.",
            );
            return finish(cook, None);
        }
        1 => lanes[0].clone(),
        n => {
            for (id, _) in lanes.iter().skip(1) {
                cook.error(
                    "duplicate-lane",
                    *id,
                    MATCH_LANE,
                    None,
                    format!(
                        "This scene has {n} lanes. The one-lane match profile supports exactly one; \
                         delete the extras."
                    ),
                );
            }
            return finish(cook, None);
        }
    };
    let half_width_mm = cook.metres_mm(&lane_comps, lane_id, MATCH_LANE, "halfWidth");
    let half_width_mm = match u32::try_from(half_width_mm) {
        Ok(0) | Err(_) => {
            cook.error(
                "out-of-range",
                lane_id,
                MATCH_LANE,
                Some("halfWidth"),
                "`halfWidth` must be a positive distance in metres — it is how far either side of the \
                 centreline a move order is allowed to go.",
            );
            1
        }
        Ok(positive) => positive,
    };

    let mut waypoints: Vec<(i64, EntityId, Vec2Mm)> = Vec::new();
    for (id, comps) in authored_with(engine, LANE_WAYPOINT) {
        let Some(index) = integer(&comps, LANE_WAYPOINT, "index") else {
            cook.error(
                "missing-field",
                id,
                LANE_WAYPOINT,
                Some("index"),
                "`index` is required — it is what puts this waypoint in order along the lane.",
            );
            continue;
        };
        let Some((x, y)) = cook.authored_position(&comps, id, LANE_WAYPOINT) else {
            continue;
        };
        if !bounds.contains(Vec2Mm::new(x, y)) {
            cook.error(
                "outside-play-area",
                id,
                LANE_WAYPOINT,
                None,
                "This waypoint sits outside the play area. Move it inside the bounds, or widen them.",
            );
        }
        waypoints.push((index, id, Vec2Mm::new(x, y)));
    }
    // Author-visible order is the `index` field; ties break on entity identity so the result is still
    // deterministic while the duplicate is reported.
    waypoints.sort_by_key(|(index, id, _)| (*index, id.peer, id.counter));
    for pair in waypoints.windows(2) {
        if pair[0].0 == pair[1].0 {
            cook.error(
                "duplicate-waypoint-index",
                pair[1].1,
                LANE_WAYPOINT,
                Some("index"),
                format!(
                    "Two waypoints share index {}. Give each one its own position in the order.",
                    pair[0].0
                ),
            );
        }
        if pair[0].2 == pair[1].2 {
            cook.error(
                "duplicate-waypoint-position",
                pair[1].1,
                LANE_WAYPOINT,
                None,
                "Two consecutive waypoints are in the same place, so the lane has a zero-length \
                 segment. Move one of them.",
            );
        }
    }
    if waypoints.len() < 2 {
        cook.error(
            "lane-too-short",
            lane_id,
            MATCH_LANE,
            None,
            format!(
                "The lane has {} waypoint(s). It needs at least two to describe a route.",
                waypoints.len()
            ),
        );
    }
    if waypoints.len() > usize::from(metrocalk_gameplay::MAX_LANE_POINTS_PER_LANE_BUDGET) {
        cook.error(
            "lane-too-long",
            lane_id,
            MATCH_LANE,
            None,
            format!(
                "The lane has {} waypoints; the runtime accepts at most {}.",
                waypoints.len(),
                metrocalk_gameplay::MAX_LANE_POINTS_PER_LANE_BUDGET
            ),
        );
    }
    let lane_length_mm = lane_length(&waypoints);

    // ── actors ───────────────────────────────────────────────────────────────────────────────────────
    let mut actors: Vec<CookedActor> = Vec::new();
    let mut owned_heroes = 0_usize;
    for (ordinal, (id, comps)) in authored_with(engine, MATCH_ACTOR).into_iter().enumerate() {
        let role = text(&comps, MATCH_ACTOR, "role").unwrap_or("").to_owned();
        if !matches!(role.as_str(), "hero" | "structure" | "minion" | "objective") {
            cook.error(
                "unknown-role",
                id,
                MATCH_ACTOR,
                Some("role"),
                format!(
                    "`role` is {role:?}; it must be one of hero, structure, minion or objective."
                ),
            );
            continue;
        }
        let team = read_team(
            &mut cook,
            &comps,
            id,
            MATCH_ACTOR,
            "it decides who this actor fights",
        );
        let Some((x, y)) = cook.authored_position(&comps, id, MATCH_ACTOR) else {
            continue;
        };
        if !bounds.contains(Vec2Mm::new(x, y)) {
            cook.error(
                "outside-play-area",
                id,
                MATCH_ACTOR,
                None,
                "This actor stands outside the play area. Move it inside the bounds, or widen them.",
            );
        }
        // Every actor is projected onto the lane by the runtime, so an actor further from the
        // centreline than the corridor is half-wide cannot be placed. Catching it here names the object.
        if !waypoints.is_empty() && distance_to_lane(&waypoints, Vec2Mm::new(x, y)) > half_width_mm
        {
            cook.error(
                "outside-lane-corridor",
                id,
                MATCH_ACTOR,
                None,
                "This actor stands outside the lane corridor. Move it onto the lane, or widen the \
                 lane's half-width.",
            );
        }

        let max_health = cook.positive_u32(&comps, id, MATCH_ACTOR, "health", "health");
        let move_speed_mm_per_tick = if role == "structure" {
            // A building does not walk. Authoring a speed on one is a mistake worth naming rather than
            // silently honouring.
            if number(&comps, MATCH_ACTOR, "speed").is_some_and(|s| s > 0.0) {
                cook.warn(
                    "structure-has-speed",
                    id,
                    MATCH_ACTOR,
                    Some("speed"),
                    "Structures do not move; this speed is ignored.",
                );
            }
            0
        } else {
            cook.speed_mm_per_tick(&comps, id, MATCH_ACTOR, "speed", tick_rate_hz)
        };
        let physical_reduction_bps = match cook.optional_u32(&comps, id, MATCH_ACTOR, "armourBps") {
            Some(bps) if bps >= u32::from(metrocalk_gameplay::BASIS_POINTS) => {
                cook.error(
                    "out-of-range",
                    id,
                    MATCH_ACTOR,
                    Some("armourBps"),
                    format!(
                        "`armourBps` is {bps}; armour must be below {} basis points or the actor \
                         would be invulnerable.",
                        metrocalk_gameplay::BASIS_POINTS
                    ),
                );
                0
            }
            Some(bps) => u16::try_from(bps).unwrap_or_default(),
            None => 0,
        };
        // Bounded here against the SAME ceilings the kernel enforces: anything the cook accepts, the
        // kernel must accept, so an out-of-range authored bounty has to be refused at cook time rather
        // than surfacing as an opaque `InvalidActor` from `spawn_actor`.
        let bounty_gold = cook.bounded_u32(
            &comps,
            id,
            MATCH_ACTOR,
            "bountyGold",
            metrocalk_gameplay::MAX_BOUNTY_GOLD,
        );
        let bounty_experience = cook.bounded_u32(
            &comps,
            id,
            MATCH_ACTOR,
            "bountyExperience",
            metrocalk_gameplay::MAX_BOUNTY_EXPERIENCE,
        );
        let health_per_level = cook.bounded_u32(
            &comps,
            id,
            MATCH_ACTOR,
            "healthPerLevel",
            metrocalk_gameplay::MAX_STAT_GROWTH_PER_LEVEL,
        );
        let attack = cook.attack(&comps, id, MATCH_ACTOR, "attack");

        let owned = flag(&comps, MATCH_ACTOR, "owned").unwrap_or(false);
        if owned {
            owned_heroes += 1;
            if role != "hero" {
                cook.error(
                    "owned-non-hero",
                    id,
                    MATCH_ACTOR,
                    Some("owned"),
                    "Only a hero can be the player's actor. Change this to a hero, or clear `owned`.",
                );
            }
        }
        let objective_for = if flag(&comps, MATCH_ACTOR, "objective").unwrap_or(false) {
            Some(1_u8.wrapping_sub(team.min(1)))
        } else {
            None
        };
        let respawn_delay_ticks = cook.optional_u32(&comps, id, MATCH_ACTOR, "respawnDelayTicks");
        if objective_for.is_some() && respawn_delay_ticks.is_some() {
            cook.warn(
                "objective-respawn-ignored",
                id,
                MATCH_ACTOR,
                Some("respawnDelayTicks"),
                "Destroying this actor ends the match, so its respawn delay never applies.",
            );
        }

        actors.push(CookedActor {
            // Kernel actor ids are assigned by authored order, not derived from the 128-bit entity id:
            // a hash into 64 bits could collide, and a collision here would silently merge two actors.
            // The mapping back to the document is recorded in `source`.
            id: (ordinal as u64).saturating_add(1),
            team,
            role,
            owned,
            x_mm: x,
            y_mm: y,
            max_health,
            move_speed_mm_per_tick,
            physical_reduction_bps,
            bounty_gold,
            bounty_experience,
            health_per_level,
            attack,
            respawn_delay_ticks,
            objective_for,
            source: id.to_loro_key(),
        });
    }
    if actors.is_empty() {
        cook.scene_error(
            "no-actors",
            "This scene has no match actors. Add at least a hero for the player to command.",
        );
    }
    if owned_heroes == 0 && !actors.is_empty() {
        cook.scene_error(
            "no-player-hero",
            "No actor is marked as the player's. Tick `owned` on the hero the player should command, \
             otherwise there is nothing to give orders to.",
        );
    }
    if owned_heroes > 1 {
        cook.scene_error(
            "multiple-player-heroes",
            format!("{owned_heroes} actors are marked as the player's; exactly one may be."),
        );
    }

    // ── waves ────────────────────────────────────────────────────────────────────────────────────────
    let mut waves: Vec<CookedWave> = Vec::new();
    for (ordinal, (id, comps)) in authored_with(engine, MATCH_WAVE).into_iter().enumerate() {
        let team = read_team(
            &mut cook,
            &comps,
            id,
            MATCH_WAVE,
            "it decides which side this wave fights for",
        );
        let spawn = cook.metres_mm(&comps, id, MATCH_WAVE, "spawnProgress");
        let goal = cook.metres_mm(&comps, id, MATCH_WAVE, "goalProgress");
        let spawn_progress_mm =
            lane_progress(&mut cook, id, "spawnProgress", spawn, lane_length_mm);
        let goal_progress_mm = lane_progress(&mut cook, id, "goalProgress", goal, lane_length_mm);

        // The runtime refuses a wave whose units cannot see anything ("bot aggro and target priority must
        // be non-empty"), so a zero range is an authoring error rather than a wave that simply never
        // fights. Catching it here names the wave.
        let aggro_range_mm = cook.metres_mm(&comps, id, MATCH_WAVE, "aggroRange");
        let aggro_range_mm = match u32::try_from(aggro_range_mm) {
            Ok(0) | Err(_) => {
                cook.error(
                    "out-of-range",
                    id,
                    MATCH_WAVE,
                    Some("aggroRange"),
                    "`aggroRange` must be a positive distance in metres — it is how far this wave's \
                     units look for something to attack, so at zero they would never fight.",
                );
                1
            }
            Ok(positive) => positive,
        };

        // The runtime requires the first spawn to be strictly in the future — a wave scheduled for tick 0
        // would want to spawn before the match has begun.
        let first_tick = u64::from(cook.positive_u32(
            &comps,
            id,
            MATCH_WAVE,
            "firstTick",
            "the tick of the first spawn",
        ));
        let interval_ticks = cook.positive_u32(
            &comps,
            id,
            MATCH_WAVE,
            "intervalTicks",
            "the gap between spawns",
        );
        let max_alive = cook.positive_u32(
            &comps,
            id,
            MATCH_WAVE,
            "maxAlive",
            "how many of this wave's units may live at once",
        );
        let unit_count = cook.positive_u32(
            &comps,
            id,
            MATCH_WAVE,
            "unitCount",
            "how many units spawn together",
        );
        if unit_count > max_alive {
            cook.error(
                "wave-batch-exceeds-cap",
                id,
                MATCH_WAVE,
                Some("unitCount"),
                format!(
                    "This wave spawns {unit_count} units at once but allows only {max_alive} alive. \
                     Raise the live cap or spawn fewer."
                ),
            );
        }
        if unit_count > u32::from(metrocalk_gameplay::MAX_UNITS_PER_WAVE_BUDGET) {
            cook.error(
                "wave-batch-exceeds-cap",
                id,
                MATCH_WAVE,
                Some("unitCount"),
                format!(
                    "This wave spawns {unit_count} units at once; the runtime accepts at most {}.",
                    metrocalk_gameplay::MAX_UNITS_PER_WAVE_BUDGET
                ),
            );
        }

        let unit_spacing = cook.metres_mm(&comps, id, MATCH_WAVE, "unitSpacing");
        let unit_max_health =
            cook.positive_u32(&comps, id, MATCH_WAVE, "unitHealth", "unit health");
        let unit_move_speed_mm_per_tick =
            cook.speed_mm_per_tick(&comps, id, MATCH_WAVE, "unitSpeed", tick_rate_hz);
        let unit_attack = cook.attack(&comps, id, MATCH_WAVE, "unitAttack");

        // A wave's units are spread along the lane around the spawn point: unit `i` sits at
        // `spawn - spacing * i`. Positive spacing trails them behind the spawn, negative spacing leads
        // them ahead of it, and either direction can run off an end of the lane — at which point the
        // runtime's signed-progress arithmetic refuses the whole definition with no idea which wave did
        // it. Bounding both ends here names the wave and the field.
        let extreme = i64::from(unit_spacing) * i64::from(unit_count.saturating_sub(1));
        let spawn = i64::try_from(spawn_progress_mm).unwrap_or(i64::MAX);
        let (nearest, furthest) = if extreme >= 0 {
            (spawn.saturating_sub(extreme), spawn)
        } else {
            (spawn, spawn.saturating_sub(extreme))
        };
        if nearest < 0 {
            cook.error(
                "wave-batch-off-lane",
                id,
                MATCH_WAVE,
                Some("unitSpacing"),
                "This wave's units would be placed before the start of the lane. Spawn it further \
                 along, or reduce the spacing or unit count.",
            );
        }
        if furthest > i64::try_from(lane_length_mm).unwrap_or(i64::MAX) {
            cook.error(
                "wave-batch-off-lane",
                id,
                MATCH_WAVE,
                Some("unitSpacing"),
                "This wave's units would be placed past the end of the lane. Spawn it further back, \
                 or reduce the spacing or unit count.",
            );
        }

        let unit_bounty_gold = cook.bounded_u32(
            &comps,
            id,
            MATCH_WAVE,
            "unitBountyGold",
            metrocalk_gameplay::MAX_BOUNTY_GOLD,
        );
        let unit_bounty_experience = cook.bounded_u32(
            &comps,
            id,
            MATCH_WAVE,
            "unitBountyExperience",
            metrocalk_gameplay::MAX_BOUNTY_EXPERIENCE,
        );

        waves.push(CookedWave {
            id: u32::try_from(ordinal).unwrap_or(u32::MAX).saturating_add(1),
            unit_bounty_gold,
            unit_bounty_experience,
            team,
            spawn_progress_mm,
            goal_progress_mm,
            aggro_range_mm,
            first_tick,
            interval_ticks,
            max_alive,
            unit_spacing_mm: i64::from(unit_spacing),
            unit_count,
            unit_max_health,
            unit_move_speed_mm_per_tick,
            unit_physical_reduction_bps: 0,
            unit_attack,
            source: id.to_loro_key(),
        });
    }
    if waves.len() > usize::from(metrocalk_gameplay::MAX_WAVE_SCHEDULE_BUDGET) {
        cook.scene_error(
            "too-many-waves",
            format!(
                "This scene has {} waves; the runtime accepts at most {}.",
                waves.len(),
                metrocalk_gameplay::MAX_WAVE_SCHEDULE_BUDGET
            ),
        );
    }
    // The runtime reserves every wave's live cap up front, so the budget must cover the standing actors
    // plus every reservation. Checking it here says which scene is too big, not just that one is.
    let reserved = waves
        .iter()
        .try_fold(u32::try_from(actors.len()).unwrap_or(u32::MAX), |sum, w| {
            sum.checked_add(w.max_alive)
        });
    let budget = MatchConfig::default().max_actors;
    match reserved {
        Some(total) if total <= budget => {}
        _ => cook.scene_error(
            "actor-budget-exceeded",
            format!(
                "This scene's {} actors plus its waves' live caps exceed the runtime's budget of \
                 {budget} actors. Lower a wave's live cap.",
                actors.len()
            ),
        ),
    }

    if cook.failed {
        return finish(cook, None);
    }

    let cooked = CookedMatch {
        schema_version: MATCH_COOK_SCHEMA_VERSION,
        digest: String::new(),
        seed,
        tick_rate_hz,
        max_match_ticks,
        bounds_mm: (min_x, min_y, max_x, max_y),
        lane: CookedLane {
            centerline: waypoints.iter().map(|(_, _, p)| (p.x, p.y)).collect(),
            half_width_mm,
            sources: waypoints
                .iter()
                .map(|(_, id, _)| id.to_loro_key())
                .collect(),
        },
        actors,
        waves,
    };
    let digest = digest_of(&cooked);
    finish(
        cook,
        Some(CookedMatch {
            digest: format!("{digest:016x}"),
            ..cooked
        }),
    )
}

fn finish(cook: Cook, cooked: Option<CookedMatch>) -> CookOutcome {
    let mut diagnostics = cook.diagnostics;
    // Errors first so the shell shows the blocking problems above the advisory ones; the sort is stable,
    // so within a severity the authored discovery order is preserved.
    diagnostics.sort_by_key(|d| match d.severity {
        CookSeverity::Error => 0,
        CookSeverity::Warning => 1,
    });
    CookOutcome {
        cooked: if cook.failed { None } else { cooked },
        diagnostics,
    }
}

/// Total lane length in millimetres, by the same integer arithmetic the kernel uses.
fn lane_length(waypoints: &[(i64, EntityId, Vec2Mm)]) -> u64 {
    waypoints
        .windows(2)
        .map(|pair| integer_distance(pair[0].2, pair[1].2))
        .sum()
}

/// Integer distance between two points, matching the kernel's `integer_sqrt` of a squared distance.
fn integer_distance(a: Vec2Mm, b: Vec2Mm) -> u64 {
    let dx = i64::from(b.x) - i64::from(a.x);
    let dy = i64::from(b.y) - i64::from(a.y);
    let squared = u128::from(dx.unsigned_abs()).pow(2) + u128::from(dy.unsigned_abs()).pow(2);
    u64::try_from(integer_sqrt(squared)).unwrap_or(u64::MAX)
}

/// Floor of the square root, by Newton's method on integers — no floats, so it is bit-identical
/// everywhere the cook runs.
fn integer_sqrt(value: u128) -> u128 {
    if value < 2 {
        return value;
    }
    let mut guess = value;
    let mut next = value.div_ceil(2);
    while next < guess {
        guess = next;
        next = u128::midpoint(guess, value / guess);
    }
    guess
}

/// Shortest distance from a point to the lane polyline, in millimetres (rounded down).
fn distance_to_lane(waypoints: &[(i64, EntityId, Vec2Mm)], point: Vec2Mm) -> u32 {
    let mut best = u64::MAX;
    for pair in waypoints.windows(2) {
        best = best.min(distance_to_segment(pair[0].2, pair[1].2, point));
    }
    if waypoints.len() == 1 {
        best = integer_distance(waypoints[0].2, point);
    }
    u32::try_from(best).unwrap_or(u32::MAX)
}

fn distance_to_segment(a: Vec2Mm, b: Vec2Mm, p: Vec2Mm) -> u64 {
    let (ax, ay) = (i128::from(a.x), i128::from(a.y));
    let (bx, by) = (i128::from(b.x), i128::from(b.y));
    let (px, py) = (i128::from(p.x), i128::from(p.y));
    let (dx, dy) = (bx - ax, by - ay);
    let len_sq = dx * dx + dy * dy;
    if len_sq == 0 {
        return integer_distance(a, p);
    }
    // Clamped projection parameter as an exact rational t = dot / len_sq, kept in integers.
    let dot = (px - ax) * dx + (py - ay) * dy;
    let t = dot.clamp(0, len_sq);
    // The closest point, rounded to the nearest millimetre. Rounding here can only move the answer by
    // under a millimetre, which is far inside any authored corridor width.
    let cx = ax + (dx * t) / len_sq;
    let cy = ay + (dy * t) / len_sq;
    let (ex, ey) = (px - cx, py - cy);
    u64::try_from(integer_sqrt((ex * ex + ey * ey).unsigned_abs())).unwrap_or(u64::MAX)
}

/// Read a required team number, reporting a missing or out-of-range value against the right component.
///
/// Actors and waves both carry one and both need the same two refusals; sharing the reader keeps their
/// wording identical, which matters because the author sees both in the same list.
fn read_team(
    cook: &mut Cook,
    comps: &Components,
    entity: EntityId,
    component: &str,
    why: &str,
) -> u8 {
    let Some(value) = integer(comps, component, "team") else {
        cook.error(
            "missing-field",
            entity,
            component,
            Some("team"),
            format!("`team` is required — {why}."),
        );
        return 0;
    };
    u8::try_from(value).unwrap_or_else(|_| {
        cook.error(
            "out-of-range",
            entity,
            component,
            Some("team"),
            format!("`team` is {value}; teams are numbered 0 to {}.", u8::MAX),
        );
        0
    })
}

/// A lane length in metres, for a human-facing message only. Lane lengths are bounded far below the
/// precision limit, and this is never fed back into gameplay arithmetic.
fn lane_length_metres(mm: u64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    {
        mm as f64 / MM_PER_METRE
    }
}

/// Convert an authored along-lane distance into a validated lane progress.
fn lane_progress(
    cook: &mut Cook,
    entity: EntityId,
    field: &str,
    metres_mm: i32,
    lane_length_mm: u64,
) -> u64 {
    if metres_mm < 0 {
        cook.error(
            "out-of-range",
            entity,
            MATCH_WAVE,
            Some(field),
            format!(
                "`{field}` is negative; distances along the lane start at 0 at its first waypoint."
            ),
        );
        return 0;
    }
    let progress = metres_mm.unsigned_abs().into();
    if progress > lane_length_mm {
        cook.error(
            "past-lane-end",
            entity,
            MATCH_WAVE,
            Some(field),
            format!(
                "`{field}` is {:.3} m along the lane, but the lane is only {:.3} m long.",
                f64::from(metres_mm) / MM_PER_METRE,
                lane_length_metres(lane_length_mm)
            ),
        );
        return lane_length_mm;
    }
    progress
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────────
// The digest
// ─────────────────────────────────────────────────────────────────────────────────────────────────────

/// FNV-1a over a canonical encoding — the same hash family the kernel's own definition digest uses.
///
/// This deliberately hashes the *fields*, not a serde representation: it must not change because a
/// serialization format did, and it must not include the digest itself.
fn digest_of(cooked: &CookedMatch) -> u64 {
    let mut hash = Fnv::new();
    hash.bytes(b"metrocalk-match-cook-v1");
    hash.u64(u64::from(cooked.schema_version));
    hash.u64(cooked.seed);
    hash.u64(u64::from(cooked.tick_rate_hz));
    hash.u64(cooked.max_match_ticks);
    for value in [
        cooked.bounds_mm.0,
        cooked.bounds_mm.1,
        cooked.bounds_mm.2,
        cooked.bounds_mm.3,
    ] {
        hash.i32(value);
    }
    hash.u64(u64::from(cooked.lane.half_width_mm));
    hash.u64(cooked.lane.centerline.len() as u64);
    for (x, y) in &cooked.lane.centerline {
        hash.i32(*x);
        hash.i32(*y);
    }
    hash.u64(cooked.actors.len() as u64);
    for actor in &cooked.actors {
        hash.u64(actor.id);
        hash.u64(u64::from(actor.team));
        hash.bytes(actor.role.as_bytes());
        hash.u64(u64::from(actor.owned));
        hash.i32(actor.x_mm);
        hash.i32(actor.y_mm);
        hash.u64(u64::from(actor.max_health));
        hash.u64(u64::from(actor.move_speed_mm_per_tick));
        hash.u64(u64::from(actor.physical_reduction_bps));
        hash.u64(u64::from(actor.bounty_gold));
        hash.u64(u64::from(actor.bounty_experience));
        hash.u64(u64::from(actor.health_per_level));
        hash.attack(actor.attack);
        hash.u64(actor.respawn_delay_ticks.map_or(u64::MAX, u64::from));
        hash.u64(actor.objective_for.map_or(u64::MAX, u64::from));
    }
    hash.u64(cooked.waves.len() as u64);
    for wave in &cooked.waves {
        hash.u64(u64::from(wave.id));
        hash.u64(u64::from(wave.team));
        hash.u64(wave.spawn_progress_mm);
        hash.u64(wave.goal_progress_mm);
        hash.u64(u64::from(wave.aggro_range_mm));
        hash.u64(wave.first_tick);
        hash.u64(u64::from(wave.interval_ticks));
        hash.u64(u64::from(wave.max_alive));
        hash.u64(wave.unit_spacing_mm.cast_unsigned());
        hash.u64(u64::from(wave.unit_count));
        hash.u64(u64::from(wave.unit_max_health));
        hash.u64(u64::from(wave.unit_move_speed_mm_per_tick));
        hash.u64(u64::from(wave.unit_physical_reduction_bps));
        hash.u64(u64::from(wave.unit_bounty_gold));
        hash.u64(u64::from(wave.unit_bounty_experience));
        hash.attack(wave.unit_attack);
    }
    hash.finish()
}

struct Fnv(u64);

impl Fnv {
    const fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn i32(&mut self, value: i32) {
        self.bytes(&value.to_le_bytes());
    }

    fn attack(&mut self, attack: Option<CookedAttack>) {
        match attack {
            None => self.bytes(b"no-attack"),
            Some(spec) => {
                self.bytes(b"attack");
                self.u64(u64::from(spec.range_mm));
                self.u64(u64::from(spec.damage));
                self.u64(u64::from(spec.windup_ticks));
                self.u64(u64::from(spec.cooldown_ticks));
            }
        }
    }

    const fn finish(&self) -> u64 {
        self.0
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────────
// Authoring
// ─────────────────────────────────────────────────────────────────────────────────────────────────────

/// The entities one [`author_starter_match`] call created, so a shell can select and frame them.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthoredMatch {
    pub settings: String,
    pub lane: String,
    pub waypoints: Vec<String>,
    pub actors: Vec<String>,
    pub waves: Vec<String>,
}

/// Whether this scene already has match settings — the shell uses it to offer "create" or "edit".
#[must_use]
pub fn scene_has_match(engine: &Engine<FlecsWorld>) -> bool {
    !authored_with(engine, MATCH_SETTINGS).is_empty()
}

/// One row of the starter roster. A named struct rather than a tuple so the table below reads as data —
/// and so nobody has to count positional fields to see which flag is `owned` and which is `objective`.
struct StarterActor {
    name: &'static str,
    role: &'static str,
    team: i64,
    /// The player commands this actor. Exactly one row may set it.
    owned: bool,
    /// Destroying this actor ends the match.
    objective: bool,
    /// Position along the lane, in metres.
    x: f64,
    health: i64,
    /// Metres per second — the authoring unit. The cook converts it with the authored tick rate.
    speed: f64,
    /// `(range in metres, damage)`.
    attack: Option<(f64, i64)>,
}

/// The reference one-lane scenario: two cores 12 m apart and a player hero between them.
const STARTER_ROSTER: [StarterActor; 3] = [
    StarterActor {
        name: "Blue Core",
        role: "structure",
        team: 0,
        owned: false,
        objective: true,
        x: 0.0,
        health: 2_000,
        speed: 0.0,
        attack: None,
    },
    StarterActor {
        name: "Red Core",
        role: "structure",
        team: 1,
        owned: false,
        objective: true,
        x: 12.0,
        health: 2_000,
        speed: 0.0,
        attack: None,
    },
    StarterActor {
        name: "Hero",
        role: "hero",
        team: 0,
        owned: true,
        objective: false,
        x: 1.5,
        health: 1_400,
        speed: 7.8,
        attack: Some((1.1, 110)),
    },
];

/// Author a complete, valid, playable starter match as **one undoable transaction**.
///
/// This is the discoverable entry point: a first-time author gets a scene that cooks and runs on the
/// first try, then edits it — rather than being asked to guess a component schema. Everything it writes
/// is ordinary authored data on ordinary entities, so every value is editable in the inspector, moves
/// with the gizmo, saves with the project and undoes in one step.
///
/// The numbers match the reference one-lane scenario the kernel's own tests use: a 12 m lane between two
/// cores, a player hero, and a repeating three-unit enemy wave.
///
/// # Errors
/// Propagates a [`metrocalk_core::PipelineError`] if the transaction is rejected.
#[allow(clippy::too_many_lines)] // one linear authoring transaction; splitting it hides the order
pub fn author_starter_match(
    engine: &mut Engine<FlecsWorld>,
) -> Result<AuthoredMatch, metrocalk_core::PipelineError> {
    use metrocalk_core::Op;

    let mut ops = Vec::new();
    let set = |ops: &mut Vec<Op>, entity, component: &str, field: &str, value| {
        ops.push(Op::SetField {
            entity,
            component: component.to_owned(),
            field: field.to_owned(),
            value,
        });
    };
    let num = FieldValue::Number;
    let int = FieldValue::Integer;

    let settings = engine.alloc_entity_id();
    ops.push(Op::CreateEntity {
        id: settings,
        parent: None,
    });
    set(
        &mut ops,
        settings,
        NAME_COMPONENT,
        NAME_FIELD,
        FieldValue::Str("Match Settings".into()),
    );
    for (field, value) in [
        ("boundsMinX", -2.0),
        ("boundsMinZ", -2.0),
        ("boundsMaxX", 14.0),
        ("boundsMaxZ", 2.0),
    ] {
        set(&mut ops, settings, MATCH_SETTINGS, field, num(value));
    }
    set(&mut ops, settings, MATCH_SETTINGS, "tickRateHz", int(30));
    set(&mut ops, settings, MATCH_SETTINGS, "maxTicks", int(3_000));

    let lane = engine.alloc_entity_id();
    ops.push(Op::CreateEntity {
        id: lane,
        parent: None,
    });
    set(
        &mut ops,
        lane,
        NAME_COMPONENT,
        NAME_FIELD,
        FieldValue::Str("Lane".into()),
    );
    set(&mut ops, lane, MATCH_LANE, "halfWidth", num(0.9));

    let mut waypoints = Vec::new();
    for (index, x) in [(0_i64, 0.0_f64), (1, 12.0)] {
        let id = engine.alloc_entity_id();
        ops.push(Op::CreateEntity {
            id,
            parent: Some(lane),
        });
        set(
            &mut ops,
            id,
            NAME_COMPONENT,
            NAME_FIELD,
            FieldValue::Str(format!("Waypoint {}", index + 1)),
        );
        set(&mut ops, id, LANE_WAYPOINT, "index", int(index));
        for (field, value) in [("x", x), ("y", 0.0), ("z", 0.0)] {
            set(&mut ops, id, "Transform", field, num(value));
        }
        waypoints.push(id);
    }

    let mut actors = Vec::new();
    for actor in STARTER_ROSTER {
        let StarterActor {
            name,
            role,
            team,
            owned,
            objective,
            x,
            health,
            speed,
            attack,
        } = actor;
        let id = engine.alloc_entity_id();
        ops.push(Op::CreateEntity { id, parent: None });
        set(
            &mut ops,
            id,
            NAME_COMPONENT,
            NAME_FIELD,
            FieldValue::Str(name.into()),
        );
        set(
            &mut ops,
            id,
            MATCH_ACTOR,
            "role",
            FieldValue::Str(role.into()),
        );
        set(&mut ops, id, MATCH_ACTOR, "team", int(team));
        set(&mut ops, id, MATCH_ACTOR, "health", int(health));
        set(&mut ops, id, MATCH_ACTOR, "speed", num(speed));
        set(&mut ops, id, MATCH_ACTOR, "armourBps", int(1_000));
        if owned {
            set(&mut ops, id, MATCH_ACTOR, "owned", FieldValue::Bool(true));
            set(&mut ops, id, MATCH_ACTOR, "respawnDelayTicks", int(30));
        }
        if objective {
            set(
                &mut ops,
                id,
                MATCH_ACTOR,
                "objective",
                FieldValue::Bool(true),
            );
        }
        if let Some((range, damage)) = attack {
            set(&mut ops, id, MATCH_ACTOR, "attackRange", num(range));
            set(&mut ops, id, MATCH_ACTOR, "attackDamage", int(damage));
            set(&mut ops, id, MATCH_ACTOR, "attackWindupTicks", int(1));
            set(&mut ops, id, MATCH_ACTOR, "attackCooldownTicks", int(4));
        }
        for (field, value) in [("x", x), ("y", 0.0), ("z", 0.0)] {
            set(&mut ops, id, "Transform", field, num(value));
        }
        actors.push(id);
    }

    let wave = engine.alloc_entity_id();
    ops.push(Op::CreateEntity {
        id: wave,
        parent: None,
    });
    set(
        &mut ops,
        wave,
        NAME_COMPONENT,
        NAME_FIELD,
        FieldValue::Str("Red Wave".into()),
    );
    set(&mut ops, wave, MATCH_WAVE, "team", int(1));
    set(&mut ops, wave, MATCH_WAVE, "spawnProgress", num(11.5));
    set(&mut ops, wave, MATCH_WAVE, "goalProgress", num(0.0));
    set(&mut ops, wave, MATCH_WAVE, "aggroRange", num(2.4));
    set(&mut ops, wave, MATCH_WAVE, "firstTick", int(4));
    set(&mut ops, wave, MATCH_WAVE, "intervalTicks", int(24));
    set(&mut ops, wave, MATCH_WAVE, "maxAlive", int(8));
    set(&mut ops, wave, MATCH_WAVE, "unitCount", int(3));
    set(&mut ops, wave, MATCH_WAVE, "unitSpacing", num(0.4));
    set(&mut ops, wave, MATCH_WAVE, "unitHealth", int(320));
    set(&mut ops, wave, MATCH_WAVE, "unitSpeed", num(5.7));
    set(&mut ops, wave, MATCH_WAVE, "unitAttackRange", num(0.8));
    set(&mut ops, wave, MATCH_WAVE, "unitAttackDamage", int(40));
    set(&mut ops, wave, MATCH_WAVE, "unitAttackWindupTicks", int(1));
    set(
        &mut ops,
        wave,
        MATCH_WAVE,
        "unitAttackCooldownTicks",
        int(6),
    );

    engine.commit("author-starter-match", ops)?;
    Ok(AuthoredMatch {
        settings: settings.to_loro_key(),
        lane: lane.to_loro_key(),
        waypoints: waypoints.iter().map(EntityId::to_loro_key).collect(),
        actors: actors.iter().map(EntityId::to_loro_key).collect(),
        waves: vec![wave.to_loro_key()],
    })
}

/// The component and field the hierarchy shows an entity's name under, so authored match objects arrive
/// already named instead of as hex ids. Reused from the existing scene vocabulary rather than redeclared,
/// so a match object is named by exactly the same mechanism as every other object in the outliner.
use metrocalk_core::variant::INSTANCE_META as NAME_COMPONENT;

use crate::capscene::NAME_FIELD;

#[cfg(test)]
mod tests;
