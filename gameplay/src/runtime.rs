use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Display, Formatter};

use crate::model::{
    apportion, integer_sqrt_ceil, level_for_experience, passive_gold_owed, streak_gold, AbilityAim,
    AbilityDelivery, AbilityEffect, AbilityId, AbilitySpec, AbilityTargeting, ActorId, ActorKind,
    ActorProvenance, ActorSpawn, BasicAttackSpec, Bounty, CombatStats, ControlKind, ControlMask,
    DamageSchool, DeathRule, DynamicActorProvenance, DynamicActorSpawn, GoldReason, ImpactShape,
    MatchConfig, MatchEndReason, MatchOutcome, MatchPhase, ModifierId, ModifierOp, PlayerId,
    ProjectileId, RectMm, RuntimeError, ScoreView, StatGrowth, StatKind, StatModifier,
    StatusEffectId, StatusEffectView, TeamId, Tick, Vec2Mm, BASIS_POINTS, EXPERIENCE_CAP,
    MAX_ABILITY_DEFINITIONS, MAX_ABILITY_RANK,
};
use crate::protocol::{
    ActorIntent, ActorView, BasicAttackProgress, CastProgress, CastTarget, CommandKind,
    CommandReceipt, CommandRejectReason, CommandRejection, CooldownView, DamageCause, FrameDigest,
    MatchEvent, PlayerCommand, ProjectileView, ServerFrame, WorldDigest,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AbilityState {
    pub(crate) id: AbilityId,
    pub(crate) ready_at: Tick,
    /// Always at least 1. Rank 0 is deliberately not a state: starting at 1 keeps every ability castable
    /// from spawn, so no cast can ever fail for want of a rank, and the point-spend invariant
    /// `Σ(rank − 1) + unspent == level − 1` stays exactly derivable from the level.
    pub(crate) rank: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingCast {
    pub(crate) ability: AbilityId,
    pub(crate) target: CastTarget,
    pub(crate) resolves_at: Tick,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingAttack {
    pub(crate) target: ActorId,
    pub(crate) resolves_at: Tick,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BasicAttackState {
    pub(crate) spec: BasicAttackSpec,
    pub(crate) ready_at: Tick,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DueAction {
    Cast(PendingCast),
    Attack(PendingAttack),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SubmissionRecord {
    pub(crate) command: PlayerCommand,
    pub(crate) outcome: Result<CommandReceipt, CommandRejection>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AdmittedHealthEffect {
    Damage {
        source: ActorId,
        target: ActorId,
        cause: DamageCause,
        amount: u32,
        school: DamageSchool,
    },
    Heal {
        source: ActorId,
        target: ActorId,
        ability: AbilityId,
        amount: u32,
    },
}

/// Same-tick health settlement is order-independent for actor truth: admitted healing is capped first, then
/// admitted damage is applied, and death is classified once after settlement. Within one phase, stable source
/// IDs order causal contribution events and choose kill credit only; they cannot change survival.
pub const HEALTH_SETTLEMENT_POLICY: &str = "heal-then-damage-batch-v1";

/// Stable diagnostic returned by the explicit deterministic-state invariant audit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvariantViolation {
    pub actor: Option<ActorId>,
    pub detail: &'static str,
}

impl Display for InvariantViolation {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        if let Some(actor) = self.actor {
            write!(
                formatter,
                "runtime invariant failed for actor {}: {}",
                actor.get(),
                self.detail
            )
        } else {
            write!(formatter, "runtime invariant failed: {}", self.detail)
        }
    }
}

impl Error for InvariantViolation {}

/// One timed restriction-and-buff bundle bound to an actor.
///
/// It owns the modifier IDs it created. Expiry removes both together, atomically, so a slow's speed penalty
/// cannot survive the slow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StatusEffect {
    pub(crate) id: StatusEffectId,
    /// Remaining absorption, if this effect is a shield.
    ///
    /// A shield lives here rather than in its own per-actor list so it inherits the co-expiry rule for
    /// free: the pool cannot outlive the effect that granted it, exactly as a slow's speed penalty cannot
    /// outlive the slow. A separate list would need its own bound, order, audit, codec and expiry to
    /// re-implement what this one already proves.
    pub(crate) shield_pool: u32,
    /// The last tick on which this effect still restricts. It is removed at the top of `expires_at + 1`, so a
    /// duration of N applied during tick T covers ticks T+1 through T+N inclusive — exactly N ticks of
    /// gameplay effect, which is what a designer authoring "a 30-tick stun" means.
    pub(crate) expires_at: Tick,
    pub(crate) controls: ControlMask,
    pub(crate) modifiers: Vec<ModifierId>,
}

/// One travelling ability delivery.
///
/// It carries the launching team rather than reading it from `source`, because the caster may die, despawn,
/// or change nothing at all while the missile is still in the air — a projectile's friend-or-foe rules are
/// decided at launch and cannot be re-derived from an actor that no longer exists. Everything else the
/// impact needs (speed, hit radius, footprint, effect) is read from the immutable ability spec, so no copy
/// of it can drift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProjectileState {
    pub(crate) id: ProjectileId,
    pub(crate) source: ActorId,
    pub(crate) team: TeamId,
    pub(crate) ability: AbilityId,
    pub(crate) position: Vec2Mm,
    /// Fixed flight terminus decided at launch. Reaching it ends the flight; there is no separate expiry
    /// deadline, because a redundant one could disagree with the geometry it is supposed to describe.
    pub(crate) end: Vec2Mm,
    /// Effect magnitude resolved from the caster's ability rank **at launch**.
    ///
    /// Carried rather than re-read for the same reason `team` is: a rank-up while the missile is in the air
    /// must not retroactively rewrite the damage it was fired with.
    pub(crate) amount: u32,
}

/// One contributor's most recent damaging contact with an actor.
///
/// Keyed by contributor and kept sorted by it, so the assist set is a function of the SET of contributors
/// rather than of the order damage happened to arrive in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DamageCredit {
    pub(crate) source: ActorId,
    pub(crate) last_tick: Tick,
}

/// One permanent scoreboard line.
///
/// Opened for every hero and **never removed for the whole match**, carrying its own owner/team snapshot.
/// That is deliberate: `DeathRule::Despawn` removes an actor from authoritative state entirely, and IDs are
/// never reused, so an actor-resident scoreboard would evaporate at the moment it matters most.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ScoreEntry {
    pub(crate) actor: ActorId,
    pub(crate) owner: Option<PlayerId>,
    pub(crate) team: TeamId,
    pub(crate) since_tick: Tick,
    /// Gold minted by kills, shutdowns and assists. Bounded relationally by the counters below, so a
    /// tampered payload cannot claim income it has no kills or assists to account for.
    pub(crate) earned_gold: u32,
    /// Derived cache of `passive_gold_owed(tick − since_tick)`, re-derived and compared exactly every
    /// frame. It is NOT encoded: the entire passive half of the economy is closed-form and untamperable.
    pub(crate) passive_gold_paid: u32,
    pub(crate) kills: u32,
    pub(crate) deaths: u32,
    pub(crate) assists: u32,
    pub(crate) kill_streak: u16,
    pub(crate) best_kill_streak: u16,
}

impl ScoreEntry {
    fn view(&self) -> ScoreView {
        ScoreView {
            actor: self.actor,
            owner: self.owner,
            team: self.team,
            gold: self.earned_gold.saturating_add(self.passive_gold_paid),
            kills: self.kills,
            deaths: self.deaths,
            assists: self.assists,
            kill_streak: self.kill_streak,
            best_kill_streak: self.best_kill_streak,
        }
    }
}

/// One death, snapshotted at the instant it happened so progression can settle after the health batch.
///
/// The victim's position and credit list are captured here because `apply_death_rule` may despawn the
/// actor before progression runs, and a despawned body can still be worth a bounty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KillRecord {
    victim: ActorId,
    victim_team: TeamId,
    victim_position: Vec2Mm,
    victim_streak: u16,
    bounty: Bounty,
    killer: ActorId,
    assisters: Vec<ActorId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActorState {
    pub(crate) id: ActorId,
    pub(crate) owner: Option<PlayerId>,
    pub(crate) team: TeamId,
    pub(crate) kind: ActorKind,
    pub(crate) position: Vec2Mm,
    pub(crate) destination: Option<Vec2Mm>,
    /// Authored stats, never mutated after spawn. The one non-tautological anchor of the stat system: a
    /// restore validates THIS, so it must stay something a payload cannot assert freely.
    pub(crate) authored_stats: CombatStats,
    /// Immutable authored per-level growth.
    pub(crate) growth: StatGrowth,
    /// Immutable authored worth of killing this actor.
    pub(crate) bounty: Bounty,
    /// Total experience earned, hard-capped at `EXPERIENCE_CAP`. Experience past max level is not banked,
    /// which is what turns an unbounded counter into a quantity a restore audit can check.
    pub(crate) experience: u32,
    /// Derived cache: exactly `level_for_experience(experience)`. Never encoded, so a payload cannot
    /// assert a level its own experience does not produce.
    pub(crate) level: u8,
    pub(crate) unspent_ability_points: u8,
    /// Attempts since this actor's last critical strike, driving the pseudo-random distribution.
    ///
    /// This is the ONLY mutable state the RNG has. The generator itself is keyed and stateless, so the
    /// single thing that can desync is this counter — and it is checkpointed, digested and audited like
    /// every other piece of simulation truth.
    pub(crate) crit_attempts: u16,
    /// Recent damaging contributors, strictly sorted by source and bounded by the configured budget.
    /// Lives on the victim so it inherits the per-actor machinery that is already bounded, audited,
    /// encoded, hashed, and destroyed with the actor — nobody can assist on the death of an actor that
    /// no longer exists.
    pub(crate) damage_credits: Vec<DamageCredit>,
    /// Derived cache: exactly `authored_stats.with_growth(growth, level)`. The durable truth a modifier
    /// resolves against, and itself re-derived rather than stored so growth cannot be forged.
    pub(crate) base_stats: CombatStats,
    /// Applied modifiers, kept sorted by ID and unique. This plus `base_stats` is the whole stat state.
    pub(crate) modifiers: Vec<StatModifier>,
    /// Active timed status effects, kept sorted by ID. Each OWNS the modifiers it applied and takes them
    /// with it when it expires, so a buff can never outlive the effect that granted it.
    pub(crate) status_effects: Vec<StatusEffect>,
    /// Derived cache: the union of every active effect's restrictions. Audited like `stats`.
    pub(crate) control_mask: ControlMask,
    /// Derived cache: exactly `base_stats.with_modifiers(&modifiers)`. Every read site in the simulation
    /// uses this so the hot path never re-aggregates, and the frame-boundary audit re-derives it to prove
    /// the cache has not drifted — the same reconstructible-not-trusted discipline the checkpoints use.
    pub(crate) stats: CombatStats,
    pub(crate) health: u32,
    pub(crate) resource: u32,
    pub(crate) abilities: Vec<AbilityState>,
    pub(crate) cast: Option<PendingCast>,
    pub(crate) basic_attack: Option<BasicAttackState>,
    pub(crate) attack: Option<PendingAttack>,
    pub(crate) death_rule: DeathRule,
    pub(crate) respawn_at: Option<Tick>,
    /// Immutable creation record. It is assigned once by the authoritative allocator, never mutated by
    /// gameplay, hashed into the world digest, and re-derived on restore, so a downstream controller can
    /// bind an actor to the schedule that created it instead of trusting a stored membership list.
    pub(crate) provenance: ActorProvenance,
}

impl ActorState {
    fn from_spawn(
        spawn: &ActorSpawn,
        abilities: Vec<AbilityId>,
        provenance: ActorProvenance,
    ) -> Self {
        Self {
            id: spawn.id,
            owner: spawn.owner,
            team: spawn.team,
            kind: spawn.kind,
            position: spawn.position,
            destination: None,
            authored_stats: spawn.stats,
            growth: spawn.growth,
            bounty: spawn.bounty,
            experience: 0,
            // Level 1 is the identity of `with_growth`, so the authored stats ARE the level-1 base stats.
            level: 1,
            unspent_ability_points: 0,
            crit_attempts: 0,
            damage_credits: Vec::new(),
            base_stats: spawn.stats,
            modifiers: Vec::new(),
            status_effects: Vec::new(),
            control_mask: ControlMask::EMPTY,
            stats: spawn.stats,
            health: spawn.stats.max_health,
            resource: spawn.stats.max_resource,
            abilities: abilities
                .into_iter()
                .map(|id| AbilityState {
                    id,
                    ready_at: 0,
                    rank: 1,
                })
                .collect(),
            cast: None,
            basic_attack: spawn
                .basic_attack
                .map(|spec| BasicAttackState { spec, ready_at: 0 }),
            attack: None,
            death_rule: spawn.death_rule,
            respawn_at: None,
            provenance,
        }
    }

    const fn alive(&self) -> bool {
        self.health > 0
    }

    /// Re-derive the stat cache and re-clamp the pools it bounds.
    ///
    /// Lowering `max_health` must pull current health down with it, or the very next frame-boundary audit
    /// would fail on `health > max_health`. Health is only ever clamped DOWN here: resolving stats must not
    /// heal, and a heal is a damage-pipeline concern, not a stat-aggregation one.
    fn resolve_controls(&mut self) {
        self.control_mask = self
            .status_effects
            .iter()
            .fold(ControlMask::EMPTY, |mask, effect| {
                mask.union(effect.controls)
            });
    }

    fn resolve_stats(&mut self) {
        self.stats = self.base_stats.with_modifiers(&self.modifiers);
        self.health = self.health.min(self.stats.max_health);
        self.resource = self.resource.min(self.stats.max_resource);
    }

    /// Re-derive the whole stat chain from experience down.
    ///
    /// This is the ONLY site that writes `level` or `base_stats`. Both are derived caches, and the
    /// frame-boundary audit re-derives and compares both, so any other writer would be found — but a single
    /// writer is what makes that guarantee cheap to reason about.
    fn resolve_growth(&mut self) {
        self.level = level_for_experience(self.experience);
        self.base_stats = self.authored_stats.with_growth(self.growth, self.level);
        self.resolve_stats();
    }

    fn view(&self) -> ActorView {
        ActorView {
            id: self.id,
            owner: self.owner,
            team: self.team,
            kind: self.kind,
            position: self.position,
            destination: self.destination,
            health: self.health,
            max_health: self.stats.max_health,
            resource: self.resource,
            max_resource: self.stats.max_resource,
            move_speed_mm_per_tick: self.stats.move_speed_mm_per_tick,
            physical_reduction_bps: self.stats.physical_reduction_bps,
            magic_reduction_bps: self.stats.magic_reduction_bps,
            controls: self.control_mask,
            alive: self.alive(),
            cast: self.cast.map(|cast| CastProgress {
                ability: cast.ability,
                target: cast.target,
                resolves_at: cast.resolves_at,
            }),
            basic_attack: self.attack.map(|attack| BasicAttackProgress {
                target: attack.target,
                resolves_at: attack.resolves_at,
            }),
            basic_attack_ready_at: self.basic_attack.map(|attack| attack.ready_at),
            basic_attack_range_mm: self.basic_attack.map(|attack| attack.spec.range_mm),
            respawn_at: self.respawn_at,
            level: self.level,
            experience: self.experience,
            unspent_ability_points: self.unspent_ability_points,
            provenance: self.provenance,
            cooldowns: self
                .abilities
                .iter()
                .map(|ability| CooldownView {
                    ability: ability.id,
                    ready_at: ability.ready_at,
                    rank: ability.rank,
                })
                .collect(),
        }
    }
}

/// Single-owner authoritative match state.
///
/// Actors and ability definitions stay sorted by stable ID. Tick-local commands are sorted by authenticated
/// player, sequence, and actor before execution. These choices make arrival/insertion order an input fact only
/// where the protocol explicitly says it is.
#[derive(Clone)]
pub struct MatchRuntime {
    pub(crate) config: MatchConfig,
    pub(crate) seed: u64,
    pub(crate) tick: Tick,
    pub(crate) phase: MatchPhase,
    pub(crate) ability_specs: Vec<AbilitySpec>,
    pub(crate) actors: Vec<ActorState>,
    /// Live projectiles, kept sorted by their monotonic ID. Sorted order is the canonical resolution order,
    /// so two runtimes cannot resolve simultaneous impacts in different sequences.
    pub(crate) projectiles: Vec<ProjectileState>,
    /// Permanent scoreboard, one line per hero, sorted by actor. Keyed on `ActorId` rather than `PlayerId`
    /// because bot heroes have no owner and a one-lane bot match is the only composed match this crate has.
    /// The budget is a LIFETIME budget: a line is never removed, so a match cannot open more than the
    /// configured number however many heroes come and go.
    pub(crate) scores: Vec<ScoreEntry>,
    pub(crate) player_roster: BTreeSet<PlayerId>,
    pub(crate) queued_commands: BTreeMap<Tick, Vec<PlayerCommand>>,
    pub(crate) last_submissions: BTreeMap<PlayerId, SubmissionRecord>,
    pub(crate) dirty: BTreeSet<ActorId>,
    pub(crate) removed: BTreeSet<ActorId>,
    pub(crate) removed_projectiles: BTreeSet<ProjectileId>,
    pub(crate) dirty_scores: BTreeSet<ActorId>,
    pub(crate) pending_events: Vec<MatchEvent>,
    pub(crate) rejection_events_in_frame: u32,
    pub(crate) objective_claims: BTreeSet<TeamId>,
    pub(crate) next_dynamic_actor_id: Option<u64>,
    pub(crate) next_modifier_id: Option<u64>,
    pub(crate) next_status_effect_id: Option<u64>,
    pub(crate) next_projectile_id: Option<u64>,
    pub(crate) checkpoint_ready: bool,
}

impl MatchRuntime {
    pub fn new(config: MatchConfig, seed: u64) -> Result<Self, RuntimeError> {
        config.validate()?;
        Ok(Self {
            config,
            seed,
            tick: 0,
            phase: MatchPhase::Setup,
            ability_specs: Vec::new(),
            actors: Vec::new(),
            projectiles: Vec::new(),
            scores: Vec::new(),
            player_roster: BTreeSet::new(),
            queued_commands: BTreeMap::new(),
            last_submissions: BTreeMap::new(),
            dirty: BTreeSet::new(),
            removed: BTreeSet::new(),
            removed_projectiles: BTreeSet::new(),
            dirty_scores: BTreeSet::new(),
            pending_events: Vec::new(),
            rejection_events_in_frame: 0,
            objective_claims: BTreeSet::new(),
            next_dynamic_actor_id: Some(1),
            next_modifier_id: Some(1),
            next_status_effect_id: Some(1),
            next_projectile_id: Some(1),
            checkpoint_ready: false,
        })
    }

    #[must_use]
    pub const fn tick(&self) -> Tick {
        self.tick
    }

    #[must_use]
    pub const fn phase(&self) -> MatchPhase {
        self.phase
    }

    #[must_use]
    pub const fn config(&self) -> MatchConfig {
        self.config
    }

    pub fn register_ability(&mut self, spec: AbilitySpec) -> Result<(), RuntimeError> {
        self.require_setup("abilities can only be registered during setup")?;
        self.checkpoint_ready = false;
        spec.validate()?;
        match self
            .ability_specs
            .binary_search_by_key(&spec.id, |candidate| candidate.id)
        {
            Ok(_) => Err(RuntimeError::DuplicateAbility(spec.id)),
            Err(index) => {
                if self.ability_specs.len()
                    >= usize::try_from(MAX_ABILITY_DEFINITIONS).unwrap_or(usize::MAX)
                {
                    return Err(RuntimeError::InvalidAbility(
                        spec.id,
                        "global ability definition budget exceeded",
                    ));
                }
                self.ability_specs.insert(index, spec);
                Ok(())
            }
        }
    }

    pub fn spawn_actor(&mut self, spawn: &ActorSpawn) -> Result<(), RuntimeError> {
        self.require_setup("actors can only be spawned during setup in MOB-1")?;
        self.insert_actor(spawn, ActorProvenance::Authored)
    }

    fn insert_actor(
        &mut self,
        spawn: &ActorSpawn,
        provenance: ActorProvenance,
    ) -> Result<(), RuntimeError> {
        self.checkpoint_ready = false;
        if u32::try_from(self.actors.len()).unwrap_or(u32::MAX) >= self.config.max_actors {
            return Err(RuntimeError::ActorBudgetExceeded);
        }
        let adds_player = spawn
            .owner
            .is_some_and(|owner| !self.player_roster.contains(&owner));
        if adds_player
            && u32::try_from(self.player_roster.len()).unwrap_or(u32::MAX)
                >= self.config.max_players
        {
            return Err(RuntimeError::PlayerBudgetExceeded);
        }
        spawn.stats.validate()?;
        spawn.growth.validate()?;
        spawn.bounty.validate()?;
        if let Some(basic_attack) = spawn.basic_attack {
            basic_attack.validate()?;
        }
        // A hero opens a permanent scoreboard line. Checked BEFORE the actor is inserted so a refused
        // budget leaves no half-registered hero behind.
        if spawn.kind == ActorKind::Hero
            && self
                .scores
                .binary_search_by_key(&spawn.id, |entry| entry.actor)
                .is_err()
            && self.scores.len() >= usize::from(self.config.max_score_entries)
        {
            return Err(RuntimeError::ScoreBudgetExceeded);
        }
        if let DeathRule::Respawn { delay_ticks, at } = spawn.death_rule {
            if delay_ticks == 0 {
                return Err(RuntimeError::InvalidActor("respawn delay must be positive"));
            }
            if !self.config.map_bounds.contains(at) {
                return Err(RuntimeError::ActorOutOfBounds(spawn.id));
            }
        }
        if !self.config.map_bounds.contains(spawn.position) {
            return Err(RuntimeError::ActorOutOfBounds(spawn.id));
        }
        if u32::try_from(spawn.abilities.len()).unwrap_or(u32::MAX)
            > self.config.max_abilities_per_actor
        {
            return Err(RuntimeError::InvalidActor("ability budget exceeded"));
        }

        let mut abilities = spawn.abilities.clone();
        abilities.sort_unstable();
        abilities.dedup();
        if abilities.len() != spawn.abilities.len() {
            return Err(RuntimeError::InvalidActor("duplicate equipped ability"));
        }
        for id in &abilities {
            self.ability_spec(*id)
                .ok_or(RuntimeError::UnknownAbility(*id))?;
        }

        match self
            .actors
            .binary_search_by_key(&spawn.id, |actor| actor.id)
        {
            Ok(_) => Err(RuntimeError::DuplicateActor(spawn.id)),
            Err(index) => {
                let id = spawn.id;
                self.actors
                    .insert(index, ActorState::from_spawn(spawn, abilities, provenance));
                if let Some(owner) = spawn.owner {
                    self.player_roster.insert(owner);
                }
                if self
                    .next_dynamic_actor_id
                    .is_some_and(|next| id.get() >= next)
                {
                    self.next_dynamic_actor_id = id.get().checked_add(1);
                }
                if spawn.kind == ActorKind::Hero {
                    // Authored actors arrive in caller order, so insert at the sorted position rather than
                    // pushing: the strict-sort audit is what makes the scoreboard binary-searchable.
                    if let Err(slot) = self
                        .scores
                        .binary_search_by_key(&spawn.id, |entry| entry.actor)
                    {
                        self.scores.insert(
                            slot,
                            ScoreEntry {
                                actor: spawn.id,
                                owner: spawn.owner,
                                team: spawn.team,
                                since_tick: self.tick,
                                earned_gold: 0,
                                passive_gold_paid: 0,
                                kills: 0,
                                deaths: 0,
                                assists: 0,
                                kill_streak: 0,
                                best_kill_streak: 0,
                            },
                        );
                        self.dirty_scores.insert(spawn.id);
                    }
                }
                self.dirty.insert(id);
                Ok(())
            }
        }
    }

    /// Spawn one bounded server-owned actor during an active match. IDs are monotonic and never reused.
    ///
    /// The caller declares *why* the actor exists; the runtime, not the caller, stamps the allocation
    /// ordinal. The ordinal is the never-reused dynamic ID itself, so provenance cannot later be grafted
    /// onto a different actor without the mismatch being detectable by the frame-boundary audit.
    pub fn spawn_dynamic_actor(
        &mut self,
        spawn: &DynamicActorSpawn,
    ) -> Result<ActorId, RuntimeError> {
        if self.phase != MatchPhase::Active {
            return Err(RuntimeError::InvalidPhase(
                "dynamic actors require an active match",
            ));
        }
        let raw_id = self
            .next_dynamic_actor_id
            .ok_or(RuntimeError::ActorIdExhausted)?;
        let id = ActorId(raw_id);
        let provenance = match spawn.provenance {
            DynamicActorProvenance::Generic => ActorProvenance::Dynamic {
                spawn_ordinal: raw_id,
            },
            DynamicActorProvenance::Wave { wave_id, unit_slot } => ActorProvenance::Wave {
                wave_id,
                unit_slot,
                spawn_ordinal: raw_id,
            },
        };
        let actor = ActorSpawn {
            id,
            owner: None,
            team: spawn.team,
            kind: spawn.kind,
            position: spawn.position,
            stats: spawn.stats,
            growth: spawn.growth,
            bounty: spawn.bounty,
            abilities: spawn.abilities.clone(),
            basic_attack: spawn.basic_attack,
            death_rule: spawn.death_rule,
        };
        self.insert_actor(&actor, provenance)?;
        self.next_dynamic_actor_id = raw_id.checked_add(1);
        self.pending_events
            .push(MatchEvent::ActorSpawned { actor: id });
        Ok(id)
    }

    /// Apply one stat modifier to an actor and return its monotonic identity.
    ///
    /// This is a server-owned rule effect, never a player command: items, buffs, auras, and status effects
    /// all land here. The runtime — not the caller — allocates the ID, because allocation order defines both
    /// the sequential `PercentMult` order and override precedence, so a caller-chosen ID would let content
    /// silently change the arithmetic.
    pub fn apply_stat_modifier(
        &mut self,
        actor: ActorId,
        kind: StatKind,
        op: ModifierOp,
    ) -> Result<ModifierId, RuntimeError> {
        if self.phase != MatchPhase::Active {
            return Err(RuntimeError::InvalidPhase(
                "stat modifiers require an active match",
            ));
        }
        let limit = usize::from(self.config.max_modifiers_per_actor);
        let raw_id = self
            .next_modifier_id
            .ok_or(RuntimeError::ModifierIdExhausted)?;
        let index = self
            .actors
            .binary_search_by_key(&actor, |candidate| candidate.id)
            .map_err(|_| RuntimeError::UnknownActor(actor))?;
        if self.actors[index].modifiers.len() >= limit {
            return Err(RuntimeError::InvalidActor("actor modifier budget exceeded"));
        }

        let id = ModifierId(raw_id);
        // IDs are monotonic, so a push keeps the vector sorted by construction; the invariant audit proves it.
        self.actors[index]
            .modifiers
            .push(StatModifier { id, kind, op });
        self.actors[index].resolve_stats();
        self.next_modifier_id = raw_id.checked_add(1);
        self.checkpoint_ready = false;
        self.dirty.insert(actor);
        Ok(id)
    }

    /// Remove one previously applied modifier. Removing an unknown ID is an error, not a silent no-op, so a
    /// double-expire in the layer above surfaces instead of hiding.
    pub fn remove_stat_modifier(
        &mut self,
        actor: ActorId,
        modifier: ModifierId,
    ) -> Result<(), RuntimeError> {
        // Phase-guarded exactly like `apply_stat_modifier`. Without this, a removal after the terminal frame
        // would mutate state and clear `checkpoint_ready` with no way to emit another frame — permanently
        // preventing capture of an otherwise valid completed match.
        if self.phase != MatchPhase::Active {
            return Err(RuntimeError::InvalidPhase(
                "stat modifiers require an active match",
            ));
        }
        let index = self
            .actors
            .binary_search_by_key(&actor, |candidate| candidate.id)
            .map_err(|_| RuntimeError::UnknownActor(actor))?;
        let position = self.actors[index]
            .modifiers
            .iter()
            .position(|entry| entry.id == modifier)
            .ok_or(RuntimeError::UnknownModifier(modifier))?;
        self.actors[index].modifiers.remove(position);
        self.actors[index].resolve_stats();
        self.checkpoint_ready = false;
        self.dirty.insert(actor);
        Ok(())
    }

    /// Apply one timed status effect: a bundle of crowd-control restrictions plus the stat modifiers it
    /// grants, both expiring together.
    ///
    /// Atomic: if any part fails, nothing is applied and no identity is consumed. A `Stun` additionally
    /// **interrupts** an in-progress cast or basic attack, because a stun that let a channel finish would not
    /// be crowd control in any sense a player would recognise.
    pub fn apply_status_effect(
        &mut self,
        actor: ActorId,
        duration_ticks: u32,
        controls: &[ControlKind],
        modifiers: &[(StatKind, ModifierOp)],
    ) -> Result<StatusEffectId, RuntimeError> {
        self.apply_status_effect_with_shield(actor, duration_ticks, controls, modifiers, 0)
    }

    /// Apply a timed effect that additionally carries an absorption pool.
    ///
    /// A shield is expressed this way rather than as its own concept so its pool expires WITH the effect
    /// that granted it - the same co-expiry rule that stops a slow's speed penalty outliving the slow.
    pub fn apply_status_effect_with_shield(
        &mut self,
        actor: ActorId,
        duration_ticks: u32,
        controls: &[ControlKind],
        modifiers: &[(StatKind, ModifierOp)],
        shield_pool: u32,
    ) -> Result<StatusEffectId, RuntimeError> {
        if self.phase != MatchPhase::Active {
            return Err(RuntimeError::InvalidPhase(
                "status effects require an active match",
            ));
        }
        if duration_ticks == 0 {
            return Err(RuntimeError::InvalidActor(
                "status effect duration must be positive",
            ));
        }
        let index = self
            .actors
            .binary_search_by_key(&actor, |candidate| candidate.id)
            .map_err(|_| RuntimeError::UnknownActor(actor))?;
        if self.actors[index].status_effects.len()
            >= usize::from(self.config.max_status_effects_per_actor)
        {
            return Err(RuntimeError::InvalidActor(
                "actor status effect budget exceeded",
            ));
        }
        let raw_id = self
            .next_status_effect_id
            .ok_or(RuntimeError::StatusEffectIdExhausted)?;
        let expires_at = self
            .tick
            .checked_add(u64::from(duration_ticks))
            .ok_or(RuntimeError::TickOverflow)?;

        // Applied to a clone first: a mid-way modifier-budget rejection must not leave a half-built effect.
        let mut candidate = self.clone();
        let mut applied = Vec::with_capacity(modifiers.len());
        for (kind, op) in modifiers {
            applied.push(candidate.apply_stat_modifier(actor, *kind, *op)?);
        }
        let id = StatusEffectId(raw_id);
        let mask = ControlMask::from_kinds(controls);
        let slot = candidate
            .actors
            .binary_search_by_key(&actor, |candidate| candidate.id)
            .map_err(|_| RuntimeError::UnknownActor(actor))?;
        candidate.actors[slot].status_effects.push(StatusEffect {
            id,
            shield_pool,
            expires_at,
            controls: mask,
            modifiers: applied,
        });
        candidate.actors[slot].resolve_controls();
        candidate.next_status_effect_id = raw_id.checked_add(1);
        *self = candidate;

        if mask.contains(ControlKind::Stun) {
            self.interrupt_actor(actor);
        }
        self.checkpoint_ready = false;
        self.dirty.insert(actor);
        Ok(id)
    }

    /// Remove one status effect early (a dispel or cleanse), taking its modifiers with it.
    pub fn remove_status_effect(
        &mut self,
        actor: ActorId,
        effect: StatusEffectId,
    ) -> Result<(), RuntimeError> {
        if self.phase != MatchPhase::Active {
            return Err(RuntimeError::InvalidPhase(
                "status effects require an active match",
            ));
        }
        let index = self
            .actors
            .binary_search_by_key(&actor, |candidate| candidate.id)
            .map_err(|_| RuntimeError::UnknownActor(actor))?;
        let position = self.actors[index]
            .status_effects
            .iter()
            .position(|entry| entry.id == effect)
            .ok_or(RuntimeError::UnknownStatusEffect(effect))?;
        let removed = self.actors[index].status_effects.remove(position);
        self.actors[index]
            .modifiers
            .retain(|modifier| !removed.modifiers.contains(&modifier.id));
        self.actors[index].resolve_controls();
        self.actors[index].resolve_stats();
        self.checkpoint_ready = false;
        self.dirty.insert(actor);
        Ok(())
    }

    /// Bounded read of one actor's active status effects.
    #[must_use]
    pub fn status_effects(&self, actor: ActorId) -> Option<Vec<StatusEffectView>> {
        self.actor_state(actor).map(|state| {
            state
                .status_effects
                .iter()
                .map(|effect| StatusEffectView {
                    id: effect.id,
                    expires_at: effect.expires_at,
                    controls: effect.controls,
                    modifiers: effect.modifiers.clone(),
                })
                .collect()
        })
    }

    /// Cancel whatever an actor is currently doing. Used by `Stun`; emits the same cancellation events a
    /// player-visible interrupt would, so the causal trace stays complete.
    fn interrupt_actor(&mut self, actor: ActorId) {
        let Ok(index) = self
            .actors
            .binary_search_by_key(&actor, |candidate| candidate.id)
        else {
            return;
        };
        if let Some(cast) = self.actors[index].cast.take() {
            self.pending_events.push(MatchEvent::CastCancelled {
                source: actor,
                ability: cast.ability,
                reason: CommandRejectReason::ActorStunned,
            });
            self.dirty.insert(actor);
        }
        if let Some(attack) = self.actors[index].attack.take() {
            self.pending_events.push(MatchEvent::BasicAttackCancelled {
                source: actor,
                target: attack.target,
                reason: CommandRejectReason::ActorStunned,
            });
            self.dirty.insert(actor);
        }
    }

    /// Drop every effect whose window has closed, taking its modifiers with it.
    ///
    /// Runs at the top of the tick, before any command executes, so an effect that ends this tick cannot
    /// still block the orders this tick delivers. Expiry is by ascending effect ID, which is only a
    /// tie-break for the emitted event order — removal itself is set-based and order-independent.
    fn expire_status_effects(&mut self) {
        let now = self.tick;
        for index in 0..self.actors.len() {
            if self.actors[index]
                .status_effects
                .iter()
                .all(|effect| effect.expires_at >= now)
            {
                continue;
            }
            let actor = self.actors[index].id;
            let mut expired = Vec::new();
            self.actors[index].status_effects.retain(|effect| {
                let live = effect.expires_at >= now;
                if !live {
                    expired.push(effect.clone());
                }
                live
            });
            for effect in &expired {
                self.actors[index]
                    .modifiers
                    .retain(|modifier| !effect.modifiers.contains(&modifier.id));
                self.pending_events.push(MatchEvent::StatusEffectExpired {
                    actor,
                    effect: effect.id,
                });
            }
            self.actors[index].resolve_controls();
            self.actors[index].resolve_stats();
            self.dirty.insert(actor);
        }
    }

    /// Bounded read of one actor's applied modifiers, for diagnostics and the status-effect layer above.
    #[must_use]
    pub fn modifiers(&self, actor: ActorId) -> Option<Vec<StatModifier>> {
        self.actor_state(actor).map(|state| state.modifiers.clone())
    }

    /// Authored stats before any modifier, so a caller can show base-versus-current.
    #[must_use]
    pub fn base_stats(&self, actor: ActorId) -> Option<CombatStats> {
        self.actor_state(actor).map(|state| state.base_stats)
    }

    /// Atomically spawn one authored wave batch. Failure leaves allocator, actors, events, and digests
    /// unchanged; the batch size is bounded by the configured units-per-wave ceiling.
    pub fn spawn_dynamic_batch(
        &mut self,
        spawns: &[DynamicActorSpawn],
    ) -> Result<Vec<ActorId>, RuntimeError> {
        if spawns.is_empty() {
            return Ok(Vec::new());
        }
        if spawns.len() > usize::from(self.config.max_units_per_wave) {
            return Err(RuntimeError::InvalidActor(
                "dynamic spawn batch exceeds the units-per-wave budget",
            ));
        }
        let mut candidate = self.clone();
        let mut ids = Vec::with_capacity(spawns.len());
        for spawn in spawns {
            ids.push(candidate.spawn_dynamic_actor(spawn)?);
        }
        *self = candidate;
        Ok(ids)
    }

    /// Atomically spawn several simultaneous wave batches while preserving the per-wave unit ceiling.
    pub fn spawn_dynamic_batches(
        &mut self,
        batches: &[Vec<DynamicActorSpawn>],
    ) -> Result<Vec<Vec<ActorId>>, RuntimeError> {
        if batches.is_empty() {
            return Ok(Vec::new());
        }
        if batches.len() > usize::from(self.config.max_wave_schedules) {
            return Err(RuntimeError::InvalidActor(
                "dynamic wave batch count exceeds the schedule budget",
            ));
        }
        if batches
            .iter()
            .any(|batch| batch.len() > usize::from(self.config.max_units_per_wave))
        {
            return Err(RuntimeError::InvalidActor(
                "dynamic spawn batch exceeds the units-per-wave budget",
            ));
        }
        let mut candidate = self.clone();
        let mut all_ids = Vec::with_capacity(batches.len());
        for batch in batches {
            let mut ids = Vec::with_capacity(batch.len());
            for spawn in batch {
                ids.push(candidate.spawn_dynamic_actor(spawn)?);
            }
            all_ids.push(ids);
        }
        *self = candidate;
        Ok(all_ids)
    }

    /// Start the match and atomically return the canonical tick-zero baseline frame.
    pub fn start(&mut self) -> Result<ServerFrame, RuntimeError> {
        self.require_setup("match has already started")?;
        let roster_size = u32::try_from(self.player_roster.len()).unwrap_or(u32::MAX);
        let reserved = roster_size
            .checked_mul(self.config.max_commands_per_player_per_tick)
            .ok_or(RuntimeError::CommandBudgetCannotReserveRoster)?;
        if reserved > self.config.max_commands_per_tick {
            return Err(RuntimeError::CommandBudgetCannotReserveRoster);
        }
        self.checkpoint_ready = false;
        self.phase = MatchPhase::Active;
        self.pending_events.push(MatchEvent::MatchStarted);
        Ok(self.emit_frame())
    }

    /// Complete the match and return the exactly-once terminal frame without advancing simulation time.
    pub fn finish(
        &mut self,
        outcome: MatchOutcome,
        reason: MatchEndReason,
    ) -> Result<ServerFrame, RuntimeError> {
        if self.phase != MatchPhase::Active {
            return Err(RuntimeError::InvalidPhase(
                "only an active match can be completed",
            ));
        }
        self.checkpoint_ready = false;
        self.phase = MatchPhase::Complete { outcome, reason };

        let queued = std::mem::take(&mut self.queued_commands);
        for (_, mut commands) in queued {
            commands.sort_unstable_by_key(command_order_key);
            for command in commands {
                self.record_execution_rejection(command, CommandRejectReason::MatchNotActive);
            }
        }
        let cancelled: Vec<(ActorId, AbilityId)> = self
            .actors
            .iter_mut()
            .filter_map(|actor| actor.cast.take().map(|cast| (actor.id, cast.ability)))
            .collect();
        for (source, ability) in cancelled {
            self.dirty.insert(source);
            self.pending_events.push(MatchEvent::CastCancelled {
                source,
                ability,
                reason: CommandRejectReason::MatchNotActive,
            });
        }
        let cancelled_attacks: Vec<(ActorId, ActorId)> = self
            .actors
            .iter_mut()
            .filter_map(|actor| actor.attack.take().map(|attack| (actor.id, attack.target)))
            .collect();
        for (source, target) in cancelled_attacks {
            self.dirty.insert(source);
            self.pending_events.push(MatchEvent::BasicAttackCancelled {
                source,
                target,
                reason: CommandRejectReason::MatchNotActive,
            });
        }
        let cancelled_respawns: Vec<(ActorId, Tick)> = self
            .actors
            .iter_mut()
            .filter_map(|actor| actor.respawn_at.take().map(|at_tick| (actor.id, at_tick)))
            .collect();
        for (actor, at_tick) in cancelled_respawns {
            self.dirty.insert(actor);
            self.pending_events.push(MatchEvent::RespawnCancelled {
                actor,
                at_tick,
                reason,
            });
        }
        // A completed match can produce no further kill, so no assist window can still be open. The
        // scoreboard itself is deliberately NOT cleared - a match result that erased its own scoreboard
        // would be useless.
        let stale: Vec<ActorId> = self
            .actors
            .iter()
            .filter(|actor| !actor.damage_credits.is_empty())
            .map(|actor| actor.id)
            .collect();
        for actor in stale {
            let index = self.actor_index(actor).expect("listed actor exists");
            self.actors[index].damage_credits.clear();
            self.dirty.insert(actor);
        }
        // In-flight projectiles are cancelled, never resolved: a match that has already produced its
        // terminal outcome must not have a missile land afterwards and rewrite health the result was
        // computed from.
        for projectile in std::mem::take(&mut self.projectiles) {
            self.removed_projectiles.insert(projectile.id);
            self.pending_events.push(MatchEvent::ProjectileCancelled {
                projectile: projectile.id,
                reason: CommandRejectReason::MatchNotActive,
            });
        }
        self.pending_events
            .push(MatchEvent::MatchFinished { outcome, reason });
        Ok(self.emit_frame())
    }

    #[must_use]
    pub fn actor(&self, id: ActorId) -> Option<ActorView> {
        self.actor_state(id).map(ActorState::view)
    }

    /// Bounded canonical actor snapshot for deterministic server-side rules and diagnostics.
    #[must_use]
    pub fn actors(&self) -> Vec<ActorView> {
        self.actors.iter().map(ActorState::view).collect()
    }

    #[must_use]
    pub fn ability(&self, id: AbilityId) -> Option<AbilitySpec> {
        self.ability_spec(id).copied()
    }

    #[must_use]
    pub fn live_actor_count(&self) -> usize {
        self.actors.iter().filter(|actor| actor.alive()).count()
    }

    /// Bounded read of one scoreboard line.
    #[must_use]
    pub fn score(&self, actor: ActorId) -> Option<ScoreView> {
        self.scores
            .binary_search_by_key(&actor, |entry| entry.actor)
            .ok()
            .map(|index| self.scores[index].view())
    }

    /// The whole scoreboard, in canonical actor order.
    #[must_use]
    pub fn scores(&self) -> Vec<ScoreView> {
        self.scores.iter().map(ScoreEntry::view).collect()
    }

    /// Bounded read of one actor's live assist window, for diagnostics and the layer above.
    #[must_use]
    pub fn damage_credits(&self, actor: ActorId) -> Option<Vec<(ActorId, Tick)>> {
        self.actor_state(actor).map(|state| {
            state
                .damage_credits
                .iter()
                .map(|credit| (credit.source, credit.last_tick))
                .collect()
        })
    }

    /// Current rank of one equipped ability.
    #[must_use]
    pub fn ability_rank(&self, actor: ActorId, ability: AbilityId) -> Option<u8> {
        self.actor_state(actor).and_then(|state| {
            state
                .abilities
                .binary_search_by_key(&ability, |candidate| candidate.id)
                .ok()
                .map(|index| state.abilities[index].rank)
        })
    }

    /// Grant experience to one hero and return its level afterwards.
    ///
    /// Experience saturates at `EXPERIENCE_CAP` rather than accumulating past the level cap: an unbounded
    /// counter cannot be range-checked on restore, and banked experience past max level buys nothing.
    pub fn award_experience(&mut self, actor: ActorId, amount: u32) -> Result<u8, RuntimeError> {
        if self.phase != MatchPhase::Active {
            return Err(RuntimeError::InvalidPhase(
                "experience requires an active match",
            ));
        }
        let index = self
            .actors
            .binary_search_by_key(&actor, |candidate| candidate.id)
            .map_err(|_| RuntimeError::UnknownActor(actor))?;
        if self.actors[index].kind != ActorKind::Hero {
            return Err(RuntimeError::InvalidProgression(
                "only a hero carries experience",
            ));
        }
        let before = self.actors[index].level;
        let total = self.actors[index]
            .experience
            .saturating_add(amount)
            .min(EXPERIENCE_CAP);
        let granted = total - self.actors[index].experience;
        self.actors[index].experience = total;
        self.actors[index].resolve_growth();
        let after = self.actors[index].level;
        if after > before {
            // One point per level. The unspent counter plus the spent ranks reconstruct the level exactly,
            // which is what stops a tampered payload asserting a maxed hero.
            self.actors[index].unspent_ability_points = self.actors[index]
                .unspent_ability_points
                .saturating_add(after - before);
        }
        self.checkpoint_ready = false;
        self.dirty.insert(actor);
        if granted > 0 {
            self.pending_events.push(MatchEvent::ExperienceGranted {
                actor,
                amount: granted,
                total_after: total,
            });
        }
        if after > before {
            self.pending_events.push(MatchEvent::HeroLevelUp {
                actor,
                level: after,
                unspent_ability_points: self.actors[index].unspent_ability_points,
            });
        }
        Ok(after)
    }

    /// Grant gold to one scoreboard line and return its earned total afterwards.
    pub fn grant_gold(
        &mut self,
        actor: ActorId,
        amount: u32,
        reason: GoldReason,
    ) -> Result<u32, RuntimeError> {
        if self.phase != MatchPhase::Active {
            return Err(RuntimeError::InvalidPhase("gold requires an active match"));
        }
        let index = self
            .scores
            .binary_search_by_key(&actor, |entry| entry.actor)
            .map_err(|_| RuntimeError::UnknownScoreEntry(actor))?;
        self.scores[index].earned_gold = self.scores[index].earned_gold.saturating_add(amount);
        let total_after = self.scores[index].view().gold;
        self.checkpoint_ready = false;
        self.dirty_scores.insert(actor);
        if amount > 0 {
            self.pending_events.push(MatchEvent::GoldGranted {
                actor,
                amount,
                reason,
                total_after,
            });
        }
        Ok(self.scores[index].earned_gold)
    }

    /// Accrue closed-form passive income for every scoreboard line.
    ///
    /// Runs FIRST in the tick, before the objective and time-limit early returns, because
    /// `passive_gold_paid` is compared exactly against `passive_gold_owed` by the frame audit: skipping the
    /// accrual on a deciding tick would fail the very next audit. It depends on nothing but the tick, so
    /// the earliest legal position is the correct one.
    fn accrue_passive_gold(&mut self) {
        for index in 0..self.scores.len() {
            let elapsed = self.tick.saturating_sub(self.scores[index].since_tick);
            let owed = passive_gold_owed(elapsed, self.config);
            if owed != self.scores[index].passive_gold_paid {
                self.scores[index].passive_gold_paid = owed;
                let actor = self.scores[index].actor;
                self.dirty_scores.insert(actor);
            }
        }
    }

    /// Drop assist credits whose window has closed.
    ///
    /// At the TOP of the tick, for exactly the reason status-effect expiry is: running it after command
    /// execution silently extends every assist window by one tick, and nothing that inspects only the final
    /// frame could ever see it.
    fn expire_damage_credits(&mut self) {
        let window = u64::from(self.config.assist_window_ticks);
        let horizon = self.tick.saturating_sub(window);
        for actor in &mut self.actors {
            if actor
                .damage_credits
                .iter()
                .all(|credit| credit.last_tick >= horizon)
            {
                continue;
            }
            actor
                .damage_credits
                .retain(|credit| credit.last_tick >= horizon);
            self.dirty.insert(actor.id);
        }
    }

    /// Record that `source` damaged `target` on this tick.
    ///
    /// When the list is full the entry with the lowest `(last_tick, source)` is evicted rather than the
    /// newcomer refused: refusing keeps the contributor whose window is closest to closing and drops the one
    /// who just landed a hit, which is backwards, and it would make the assist set depend on arrival order.
    fn credit_damage(&mut self, target_index: usize, source: ActorId) {
        let limit = usize::from(self.config.max_damage_credits_per_actor);
        let tick = self.tick;
        let credits = &mut self.actors[target_index].damage_credits;
        match credits.binary_search_by_key(&source, |credit| credit.source) {
            Ok(index) => credits[index].last_tick = tick,
            Err(index) => {
                if credits.len() >= limit {
                    let Some(oldest) = credits
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, credit)| (credit.last_tick, credit.source))
                        .map(|(position, _)| position)
                    else {
                        return;
                    };
                    if credits[oldest].last_tick >= tick && credits[oldest].source < source {
                        // Everything already recorded is at least as recent as this hit, so there is
                        // nothing older to give up. Refuse rather than churn a full list every tick.
                        return;
                    }
                    credits.remove(oldest);
                    let slot = credits
                        .binary_search_by_key(&source, |credit| credit.source)
                        .unwrap_or_else(|slot| slot);
                    credits.insert(
                        slot,
                        DamageCredit {
                            source,
                            last_tick: tick,
                        },
                    );
                } else {
                    credits.insert(
                        index,
                        DamageCredit {
                            source,
                            last_tick: tick,
                        },
                    );
                }
            }
        }
    }

    /// Settle every death's bounty: kill credit, streaks, shutdown gold, assists, and shared experience.
    ///
    /// Runs AFTER the whole health batch has settled, never inside it. `HEALTH_SETTLEMENT_POLICY` promises
    /// that ordering decides credit but never survival; a bounty that raised a level that raised max health
    /// mid-settlement would let ascending target order decide who survives a simultaneous exchange.
    fn settle_progression(&mut self, kills: Vec<KillRecord>) -> Result<(), RuntimeError> {
        for kill in kills {
            if let Ok(index) = self
                .scores
                .binary_search_by_key(&kill.victim, |entry| entry.actor)
            {
                self.scores[index].deaths = self.scores[index].deaths.saturating_add(1);
                self.scores[index].kill_streak = 0;
                self.dirty_scores.insert(kill.victim);
            }

            // A self-kill counts a death and nothing else: no credit, no gold, no streak.
            if kill.killer == kill.victim {
                continue;
            }

            let shutdown = streak_gold(kill.victim_streak);
            if let Ok(index) = self
                .scores
                .binary_search_by_key(&kill.killer, |entry| entry.actor)
            {
                self.scores[index].kills = self.scores[index].kills.saturating_add(1);
                let streak = self.scores[index].kill_streak.saturating_add(1);
                self.scores[index].kill_streak = streak;
                self.scores[index].best_kill_streak =
                    self.scores[index].best_kill_streak.max(streak);
                self.dirty_scores.insert(kill.killer);
                self.pending_events.push(MatchEvent::KillCredited {
                    killer: kill.killer,
                    victim: kill.victim,
                    streak_after: streak,
                });
                self.grant_gold(kill.killer, kill.bounty.gold, GoldReason::Kill)?;
                if shutdown > 0 {
                    self.grant_gold(kill.killer, shutdown, GoldReason::Shutdown)?;
                }
            }
            // A killer with no scoreboard line — a tower or a minion landing the blow — forfeits the credit
            // and the gold. Silently, and without panicking: that is an ordinary MOBA outcome.

            let assisters: Vec<ActorId> = kill
                .assisters
                .iter()
                .copied()
                .filter(|actor| *actor != kill.killer)
                .filter(|actor| {
                    self.scores
                        .binary_search_by_key(actor, |entry| entry.actor)
                        .is_ok()
                })
                .collect();
            if !assisters.is_empty() {
                // Minted, not deducted. If the assist pool came out of the killer's take, a killer's income
                // would silently depend on whether a team-mate happened to poke.
                let pool = kill.bounty.gold / 2;
                let count = u32::try_from(assisters.len()).unwrap_or(u32::MAX);
                let (base, mut remainder) = apportion(pool, count);
                for actor in &assisters {
                    let extra = u32::from(remainder > 0);
                    remainder = remainder.saturating_sub(extra);
                    if let Ok(index) = self.scores.binary_search_by_key(actor, |e| e.actor) {
                        self.scores[index].assists = self.scores[index].assists.saturating_add(1);
                        self.dirty_scores.insert(*actor);
                    }
                    self.pending_events.push(MatchEvent::AssistCredited {
                        actor: *actor,
                        victim: kill.victim,
                    });
                    self.grant_gold(*actor, base + extra, GoldReason::Assist)?;
                }
            }

            self.share_experience(&kill)?;
        }
        Ok(())
    }

    /// Split a death's experience across the killer plus every living enemy hero inside the share radius.
    ///
    /// Recipients come from `self.scores`, which is sorted by actor, so the award order is canonical for
    /// free — no spatial query's output order can ever reach the split. The remainder is CONSERVED rather
    /// than truncated, which is what makes "every unit a death mints is awarded" a testable invariant.
    fn share_experience(&mut self, kill: &KillRecord) -> Result<(), RuntimeError> {
        if kill.bounty.experience == 0 {
            return Ok(());
        }
        let radius_squared = u128::from(self.config.xp_share_radius_mm).pow(2);
        let recipients: Vec<ActorId> = self
            .scores
            .iter()
            .filter(|entry| entry.team != kill.victim_team)
            .filter(|entry| entry.actor != kill.victim)
            .filter(|entry| {
                // The killer always shares, in range or not and alive or not: a posthumous projectile kill
                // must still pay its dead caster.
                entry.actor == kill.killer
                    || self.actor_state(entry.actor).is_some_and(|actor| {
                        actor.alive()
                            && kill.victim_position.squared_distance(actor.position)
                                <= radius_squared
                    })
            })
            .map(|entry| entry.actor)
            .collect();
        let count = u32::try_from(recipients.len()).unwrap_or(u32::MAX);
        if count == 0 {
            return Ok(());
        }
        let (base, mut remainder) = apportion(kill.bounty.experience, count);
        for actor in recipients {
            let extra = u32::from(remainder > 0);
            remainder = remainder.saturating_sub(extra);
            let amount = base + extra;
            if amount > 0 && self.actor_state(actor).is_some() {
                self.award_experience(actor, amount)?;
            }
        }
        Ok(())
    }

    /// Bounded canonical snapshot of every projectile currently in flight.
    #[must_use]
    pub fn projectiles(&self) -> Vec<ProjectileView> {
        self.projectile_views()
    }

    fn projectile_views(&self) -> Vec<ProjectileView> {
        self.projectiles
            .iter()
            .map(|projectile| ProjectileView {
                id: projectile.id,
                source: projectile.source,
                team: projectile.team,
                ability: projectile.ability,
                position: projectile.position,
                end: projectile.end,
                hit_radius_mm: self
                    .ability_spec(projectile.ability)
                    .and_then(|spec| spec.projectile())
                    .map_or(0, |(_, hit_radius_mm)| hit_radius_mm),
            })
            .collect()
    }

    #[must_use]
    pub fn queued_command_count(&self) -> usize {
        self.queued_commands.values().map(Vec::len).sum()
    }

    /// Audit all state-shape invariants at a frame boundary. Soak/fuzz hosts call this after every frame;
    /// it never repairs state or hides a violation.
    pub fn check_invariants(&self) -> Result<(), InvariantViolation> {
        if self
            .ability_specs
            .windows(2)
            .any(|pair| pair[0].id >= pair[1].id)
        {
            return Err(Self::violation(
                None,
                "ability definitions are not strictly sorted",
            ));
        }
        if self.actors.windows(2).any(|pair| pair[0].id >= pair[1].id) {
            return Err(Self::violation(None, "actors are not strictly sorted"));
        }
        if self.actors.len() > usize::try_from(self.config.max_actors).unwrap_or(usize::MAX) {
            return Err(Self::violation(None, "actor budget exceeded"));
        }
        if self.player_roster.len() > usize::try_from(self.config.max_players).unwrap_or(usize::MAX)
        {
            return Err(Self::violation(None, "player roster budget exceeded"));
        }
        for actor in &self.actors {
            self.check_actor_invariants(actor)?;
        }
        self.check_command_invariants()?;
        if self.dirty.iter().any(|id| self.actor_state(*id).is_none()) {
            return Err(Self::violation(None, "dirty actor identity is not live"));
        }
        if self
            .removed
            .iter()
            .any(|id| self.actor_state(*id).is_some())
            || self.removed.iter().any(|id| self.dirty.contains(id))
        {
            return Err(Self::violation(
                None,
                "removed and live/dirty actor sets overlap",
            ));
        }
        if !self.objective_claims.is_empty() {
            return Err(Self::violation(
                None,
                "objective claims escaped the settlement boundary",
            ));
        }
        if let Some(next) = self.next_dynamic_actor_id {
            if self.actors.iter().any(|actor| actor.id.get() >= next) {
                return Err(Self::violation(
                    None,
                    "dynamic actor allocator does not exceed every allocated ID",
                ));
            }
        }
        self.check_projectile_invariants()?;
        self.check_score_invariants()?;
        // The scoreboard and per-actor progression deliberately SURVIVE `Complete`: a match result that
        // erased its own scoreboard would be useless. Assist windows do not - a completed match can produce
        // no further kill, so an open window would be state nothing can ever consume.
        if matches!(self.phase, MatchPhase::Complete { .. })
            && (!self.queued_commands.is_empty()
                || !self.projectiles.is_empty()
                || self.actors.iter().any(|actor| {
                    actor.cast.is_some()
                        || actor.attack.is_some()
                        || actor.respawn_at.is_some()
                        || !actor.damage_credits.is_empty()
                }))
        {
            return Err(Self::violation(
                None,
                "terminal state retains commands, projectiles, credits, actions, or respawn deadlines",
            ));
        }
        Ok(())
    }

    /// Audit the permanent scoreboard.
    ///
    /// The reverse referential check - that every line still has a live actor - is **deliberately absent**:
    /// a line outliving the body it was opened for is the feature, not a leak.
    fn check_score_invariants(&self) -> Result<(), InvariantViolation> {
        if self.scores.len() > usize::from(self.config.max_score_entries) {
            return Err(Self::violation(None, "scoreboard budget exceeded"));
        }
        if self
            .scores
            .windows(2)
            .any(|pair| pair[0].actor >= pair[1].actor)
        {
            return Err(Self::violation(
                None,
                "scoreboard entries are not strictly sorted by actor",
            ));
        }
        for entry in &self.scores {
            if self
                .next_dynamic_actor_id
                .is_some_and(|next| entry.actor.get() >= next)
            {
                return Err(Self::violation(
                    Some(entry.actor),
                    "scoreboard entry names an actor beyond the allocator",
                ));
            }
            if entry
                .owner
                .is_some_and(|owner| !self.player_roster.contains(&owner))
            {
                return Err(Self::violation(
                    Some(entry.actor),
                    "scoreboard owner is absent from the player roster",
                ));
            }
            if entry.since_tick > self.tick {
                return Err(Self::violation(
                    Some(entry.actor),
                    "scoreboard entry claims to predate the match",
                ));
            }
            // The whole passive economy is closed-form, so it is compared EXACTLY rather than bounded. This
            // is the check that makes half the gold supply untamperable.
            if entry.passive_gold_paid
                != passive_gold_owed(self.tick.saturating_sub(entry.since_tick), self.config)
            {
                return Err(Self::violation(
                    Some(entry.actor),
                    "passive gold does not match its closed form",
                ));
            }
            if entry.best_kill_streak < entry.kill_streak {
                return Err(Self::violation(
                    Some(entry.actor),
                    "best kill streak is below the live streak",
                ));
            }
            // Earned gold is bounded by the counters that justify it: a payload cannot claim income it has
            // no kills or assists to account for.
            let ceiling = u64::from(entry.kills)
                * (u64::from(crate::model::MAX_BOUNTY_GOLD)
                    + u64::from(crate::model::MAX_STREAK_GOLD))
                + u64::from(entry.assists) * (u64::from(crate::model::MAX_BOUNTY_GOLD) / 2);
            if u64::from(entry.earned_gold) > ceiling {
                return Err(Self::violation(
                    Some(entry.actor),
                    "earned gold exceeds what its kills and assists can account for",
                ));
            }
            if let Some(actor) = self.actor_state(entry.actor) {
                if actor.team != entry.team {
                    return Err(Self::violation(
                        Some(entry.actor),
                        "scoreboard entry disagrees with its live actor about team",
                    ));
                }
            }
        }
        for actor in &self.actors {
            if actor.kind == ActorKind::Hero
                && self
                    .scores
                    .binary_search_by_key(&actor.id, |entry| entry.actor)
                    .is_err()
            {
                return Err(Self::violation(
                    Some(actor.id),
                    "a live hero has no scoreboard entry",
                ));
            }
        }
        Ok(())
    }

    /// Projectiles are authoritative simulation state, so they are audited exactly like actors: bounded,
    /// strictly ordered, allocator-consistent, inside the map, and referentially intact against the
    /// immutable ability table their behaviour is read from.
    fn check_projectile_invariants(&self) -> Result<(), InvariantViolation> {
        if self.projectiles.len() > usize::from(self.config.max_projectiles) {
            return Err(Self::violation(None, "projectile budget exceeded"));
        }
        if self
            .projectiles
            .windows(2)
            .any(|pair| pair[0].id >= pair[1].id)
        {
            return Err(Self::violation(
                None,
                "projectiles are not strictly sorted by identity",
            ));
        }
        if self.next_projectile_id.is_some_and(|next| {
            self.projectiles
                .iter()
                .any(|projectile| projectile.id.get() >= next)
        }) {
            return Err(Self::violation(
                None,
                "projectile allocator does not exceed every allocated projectile ID",
            ));
        }
        for projectile in &self.projectiles {
            if !self.config.map_bounds.contains(projectile.position)
                || !self.config.map_bounds.contains(projectile.end)
            {
                return Err(Self::violation(
                    None,
                    "projectile position or flight terminus is outside map bounds",
                ));
            }
            // Without a projectile delivery there is no speed, so the missile would sit still forever and
            // the "every live projectile moves every tick" delta rule would silently become false.
            if self
                .ability_spec(projectile.ability)
                .and_then(|spec| spec.projectile())
                .is_none()
            {
                return Err(Self::violation(
                    None,
                    "projectile references an unknown ability or one with no projectile delivery",
                ));
            }
        }
        Ok(())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the frame-boundary actor audit keeps all relational lifecycle checks centralized"
    )]
    fn check_actor_invariants(&self, actor: &ActorState) -> Result<(), InvariantViolation> {
        if !self.config.map_bounds.contains(actor.position) {
            return Err(Self::violation(
                Some(actor.id),
                "position is outside map bounds",
            ));
        }
        // Provenance is creation evidence, so it must stay pinned to the identity it was stamped on. A
        // server-spawned ordinal is the allocated ID itself, and an owned actor can never be server-spawned.
        match actor.provenance {
            ActorProvenance::Authored => {}
            ActorProvenance::Dynamic { spawn_ordinal }
            | ActorProvenance::Wave { spawn_ordinal, .. } => {
                if spawn_ordinal != actor.id.get() || actor.owner.is_some() {
                    return Err(Self::violation(
                        Some(actor.id),
                        "server-spawn provenance does not match the actor it was stamped on",
                    ));
                }
            }
        }
        // The stat cache is derived state, so it is re-derived and compared rather than trusted. Modifiers
        // must also stay strictly sorted and unique by ID, because that order IS the aggregation order.
        if actor.modifiers.len() > usize::from(self.config.max_modifiers_per_actor) {
            return Err(Self::violation(
                Some(actor.id),
                "actor modifier budget exceeded",
            ));
        }
        if actor
            .modifiers
            .windows(2)
            .any(|pair| pair[0].id >= pair[1].id)
        {
            return Err(Self::violation(
                Some(actor.id),
                "actor modifiers are not strictly sorted by identity",
            ));
        }
        if self
            .next_modifier_id
            .is_some_and(|next| actor.modifiers.iter().any(|entry| entry.id.get() >= next))
        {
            return Err(Self::violation(
                Some(actor.id),
                "modifier allocator does not exceed every allocated modifier ID",
            ));
        }
        // With no modifiers the aggregation is the identity, so comparing against `base_stats` directly is
        // the same assertion without five `resolve_stat` passes. That matters: this audit runs for every
        // actor every frame, and the unmodified actor is overwhelmingly the common case.
        if actor.status_effects.len() > usize::from(self.config.max_status_effects_per_actor) {
            return Err(Self::violation(
                Some(actor.id),
                "actor status effect budget exceeded",
            ));
        }
        if actor
            .status_effects
            .windows(2)
            .any(|pair| pair[0].id >= pair[1].id)
        {
            return Err(Self::violation(
                Some(actor.id),
                "actor status effects are not strictly sorted by identity",
            ));
        }
        if self.next_status_effect_id.is_some_and(|next| {
            actor
                .status_effects
                .iter()
                .any(|effect| effect.id.get() >= next)
        }) {
            return Err(Self::violation(
                Some(actor.id),
                "status effect allocator does not exceed every allocated effect ID",
            ));
        }
        // An effect whose window closed must have been retired at the top of the tick.
        if actor
            .status_effects
            .iter()
            .any(|effect| effect.expires_at < self.tick)
        {
            return Err(Self::violation(
                Some(actor.id),
                "an expired status effect survived the frame boundary",
            ));
        }
        // Every modifier an effect claims to own must actually exist, or expiry would leave an orphan buff
        // that nothing can ever remove.
        if actor.status_effects.iter().any(|effect| {
            effect
                .modifiers
                .iter()
                .any(|owned| !actor.modifiers.iter().any(|entry| entry.id == *owned))
        }) {
            return Err(Self::violation(
                Some(actor.id),
                "a status effect owns a modifier the actor does not carry",
            ));
        }
        if actor.control_mask
            != actor
                .status_effects
                .iter()
                .fold(ControlMask::EMPTY, |mask, effect| {
                    mask.union(effect.controls)
                })
        {
            return Err(Self::violation(
                Some(actor.id),
                "resolved control mask does not match its status effects",
            ));
        }
        // The progression chain is three derived links, each re-derived and compared rather than trusted:
        //   level      == level_for_experience(experience)
        //   base_stats == authored_stats.with_growth(growth, level)
        //   stats      == base_stats.with_modifiers(modifiers)
        // Breaking any link is what a forged checkpoint would have to do, and each is checked here.
        if actor.experience > EXPERIENCE_CAP {
            return Err(Self::violation(
                Some(actor.id),
                "experience exceeds its hard cap",
            ));
        }
        if actor.level != level_for_experience(actor.experience) {
            return Err(Self::violation(
                Some(actor.id),
                "level does not match its experience",
            ));
        }
        let grown_matches = if actor.level == 1 {
            actor.base_stats == actor.authored_stats
        } else {
            actor.base_stats == actor.authored_stats.with_growth(actor.growth, actor.level)
        };
        if !grown_matches {
            return Err(Self::violation(
                Some(actor.id),
                "base stats do not match their authored stats and growth",
            ));
        }
        if actor.kind != ActorKind::Hero
            && (actor.level != 1 || actor.experience != 0 || actor.unspent_ability_points != 0)
        {
            return Err(Self::violation(
                Some(actor.id),
                "a non-hero carries progression",
            ));
        }
        // Ranks reconstruct the level exactly. This relation is what stops a tampered payload asserting a
        // maxed hero: spent points plus unspent points must equal the levels actually earned.
        let mut spent: u32 = 0;
        for ability in &actor.abilities {
            if ability.rank < 1 || ability.rank > MAX_ABILITY_RANK {
                return Err(Self::violation(
                    Some(actor.id),
                    "ability rank is outside its legal range",
                ));
            }
            if actor.level < rank_unlock_level(ability.rank) {
                return Err(Self::violation(
                    Some(actor.id),
                    "ability rank is not unlocked at this level",
                ));
            }
            spent += u32::from(ability.rank - 1);
        }
        if spent + u32::from(actor.unspent_ability_points) != u32::from(actor.level - 1) {
            return Err(Self::violation(
                Some(actor.id),
                "spent and unspent ability points do not reconstruct the level",
            ));
        }
        // The PRD counter is bounded by construction (it resets on a success and saturates), but a
        // decoded one is not: a payload could assert an enormous attempt count to force a guaranteed
        // critical strike on the next swing. Bound it against the largest attempt count that can still
        // matter - past `BASIS_POINTS / smallest-constant` the chance is already pinned at 100 %.
        if actor.crit_attempts > BASIS_POINTS {
            return Err(Self::violation(
                Some(actor.id),
                "critical-strike attempt counter is beyond any chance it could escalate",
            ));
        }
        if actor.damage_credits.len() > usize::from(self.config.max_damage_credits_per_actor) {
            return Err(Self::violation(
                Some(actor.id),
                "actor damage credit budget exceeded",
            ));
        }
        if actor
            .damage_credits
            .windows(2)
            .any(|pair| pair[0].source >= pair[1].source)
        {
            return Err(Self::violation(
                Some(actor.id),
                "damage credits are not strictly sorted by source",
            ));
        }
        let horizon = self
            .tick
            .saturating_sub(u64::from(self.config.assist_window_ticks));
        for credit in &actor.damage_credits {
            if credit.last_tick > self.tick {
                return Err(Self::violation(
                    Some(actor.id),
                    "damage credit is stamped in the future",
                ));
            }
            // This is the assertion that proves top-of-tick expiry actually ran.
            if credit.last_tick < horizon {
                return Err(Self::violation(
                    Some(actor.id),
                    "an expired damage credit survived the frame boundary",
                ));
            }
            if self
                .next_dynamic_actor_id
                .is_some_and(|next| credit.source.get() >= next)
            {
                return Err(Self::violation(
                    Some(actor.id),
                    "damage credit names a source beyond the allocator",
                ));
            }
        }
        if !actor.alive() && !actor.damage_credits.is_empty() {
            return Err(Self::violation(
                Some(actor.id),
                "a dead actor retains an assist window",
            ));
        }
        let resolved_matches = if actor.modifiers.is_empty() {
            actor.stats == actor.base_stats
        } else {
            actor.stats == actor.base_stats.with_modifiers(&actor.modifiers)
        };
        if !resolved_matches {
            return Err(Self::violation(
                Some(actor.id),
                "resolved stat cache does not match its base stats and modifiers",
            ));
        }
        if actor.health > actor.stats.max_health || actor.resource > actor.stats.max_resource {
            return Err(Self::violation(
                Some(actor.id),
                "health or resource exceeds its authored maximum",
            ));
        }
        if actor
            .owner
            .is_some_and(|owner| !self.player_roster.contains(&owner))
        {
            return Err(Self::violation(
                Some(actor.id),
                "actor owner is absent from the player roster",
            ));
        }
        if actor.abilities.len()
            > usize::try_from(self.config.max_abilities_per_actor).unwrap_or(usize::MAX)
            || actor
                .abilities
                .windows(2)
                .any(|pair| pair[0].id >= pair[1].id)
            || actor
                .abilities
                .iter()
                .any(|ability| self.ability_spec(ability.id).is_none())
        {
            return Err(Self::violation(
                Some(actor.id),
                "equipped abilities violate budget, order, or referential integrity",
            ));
        }
        if actor.cast.is_some() && actor.attack.is_some() {
            return Err(Self::violation(
                Some(actor.id),
                "actor has two mutually exclusive pending actions",
            ));
        }
        if actor.destination.is_some() && (actor.cast.is_some() || actor.attack.is_some()) {
            return Err(Self::violation(
                Some(actor.id),
                "actor moves while a mutually exclusive action is pending",
            ));
        }
        if actor
            .destination
            .is_some_and(|destination| !self.config.map_bounds.contains(destination))
        {
            return Err(Self::violation(
                Some(actor.id),
                "actor destination is outside map bounds",
            ));
        }
        if let DeathRule::Respawn { delay_ticks, at } = actor.death_rule {
            if delay_ticks == 0 || !self.config.map_bounds.contains(at) {
                return Err(Self::violation(
                    Some(actor.id),
                    "actor respawn rule has an invalid delay or anchor",
                ));
            }
        }
        if !actor.alive()
            && (actor.destination.is_some() || actor.cast.is_some() || actor.attack.is_some())
        {
            return Err(Self::violation(
                Some(actor.id),
                "dead actor retains navigation or an action",
            ));
        }
        if actor.alive() && actor.respawn_at.is_some() {
            return Err(Self::violation(
                Some(actor.id),
                "living actor has a respawn deadline",
            ));
        }
        if actor.respawn_at.is_some() && !matches!(actor.death_rule, DeathRule::Respawn { .. }) {
            return Err(Self::violation(
                Some(actor.id),
                "respawn deadline belongs to a non-respawning actor",
            ));
        }
        if !actor.alive()
            && matches!(actor.death_rule, DeathRule::Respawn { .. })
            && actor.respawn_at.is_none()
            && matches!(self.phase, MatchPhase::Active)
        {
            return Err(Self::violation(
                Some(actor.id),
                "dead respawning actor has no respawn deadline",
            ));
        }
        if !actor.alive() && matches!(actor.death_rule, DeathRule::Despawn) {
            return Err(Self::violation(
                Some(actor.id),
                "dead despawn actor remained in authoritative state",
            ));
        }
        if !actor.alive()
            && matches!(actor.death_rule, DeathRule::ObjectiveVictory { .. })
            && matches!(self.phase, MatchPhase::Active)
        {
            return Err(Self::violation(
                Some(actor.id),
                "dead objective actor remained in an active match",
            ));
        }
        if actor.cast.is_some_and(|cast| {
            cast.resolves_at <= self.tick || cast.resolves_at > self.config.max_match_ticks
        }) || actor.attack.is_some_and(|attack| {
            attack.resolves_at <= self.tick || attack.resolves_at > self.config.max_match_ticks
        }) {
            return Err(Self::violation(
                Some(actor.id),
                "pending action deadline is outside the remaining match",
            ));
        }
        if actor.respawn_at.is_some_and(|at_tick| at_tick <= self.tick) {
            return Err(Self::violation(
                Some(actor.id),
                "due respawn escaped the frame boundary",
            ));
        }
        Ok(())
    }

    fn check_command_invariants(&self) -> Result<(), InvariantViolation> {
        let latest_queued_tick = self
            .tick
            .saturating_add(u64::from(self.config.max_command_lead_ticks))
            .min(self.config.max_match_ticks);
        let mut queued_sequences = BTreeSet::new();
        for (tick, commands) in &self.queued_commands {
            if *tick <= self.tick || *tick > latest_queued_tick {
                return Err(Self::violation(
                    None,
                    "queued command tick is stale or beyond the match",
                ));
            }
            if commands.len()
                > usize::try_from(self.config.max_commands_per_tick).unwrap_or(usize::MAX)
            {
                return Err(Self::violation(None, "queued command tick budget exceeded"));
            }
            let mut per_player = BTreeMap::new();
            for command in commands {
                let actor = self.actor_state(command.actor);
                let cached_submission_matches = self
                    .last_submissions
                    .get(&command.player)
                    .is_some_and(|record| {
                        if record.command.sequence > command.sequence {
                            return true;
                        }
                        record.command == *command
                            && matches!(
                                record.outcome,
                                Ok(receipt)
                                    if receipt.player == command.player
                                        && receipt.sequence == command.sequence
                                        && receipt.execute_at == command.execute_at
                            )
                    });
                if command.execute_at != *tick
                    || !self.player_roster.contains(&command.player)
                    || actor
                        .is_none_or(|actor| !actor.alive() || actor.owner != Some(command.player))
                    || !cached_submission_matches
                    || !queued_sequences.insert((command.player, command.sequence))
                {
                    return Err(Self::violation(
                        Some(command.actor),
                        "queued command has invalid tick, player, or actor reference",
                    ));
                }
                let count = per_player.entry(command.player).or_insert(0_u32);
                *count = count.saturating_add(1);
                if *count > self.config.max_commands_per_player_per_tick {
                    return Err(Self::violation(
                        None,
                        "queued per-player command budget exceeded",
                    ));
                }
            }
        }
        if self.last_submissions.len()
            > usize::try_from(self.config.max_players).unwrap_or(usize::MAX)
        {
            return Err(Self::violation(None, "submission record budget exceeded"));
        }
        for (player, record) in &self.last_submissions {
            if !self.player_roster.contains(player)
                || record.command.player != *player
                || match record.outcome {
                    Ok(receipt) => receipt.player != *player,
                    Err(rejection) => rejection.player != *player,
                }
            {
                return Err(Self::violation(
                    None,
                    "last-submission record has inconsistent player identity",
                ));
            }
        }
        Ok(())
    }

    const fn violation(actor: Option<ActorId>, detail: &'static str) -> InvariantViolation {
        InvariantViolation { actor, detail }
    }

    /// Queue one player command after envelope and current-state validation.
    ///
    /// State is validated again at execution because an accepted future command can be invalidated by an
    /// earlier death, cast, or resource change. A novel active-match rejection is returned immediately and,
    /// up to the configured per-frame evidence bound, included in the next authoritative frame. An exact
    /// retry of the most recent envelope returns its cached outcome without duplicating mutation or events.
    /// Inactive-match and reordered-envelope failures are synchronous-only.
    pub fn submit(&mut self, command: PlayerCommand) -> Result<CommandReceipt, CommandRejection> {
        if self.phase != MatchPhase::Active {
            return Err(CommandRejection {
                player: command.player,
                sequence: command.sequence,
                reason: CommandRejectReason::MatchNotActive,
            });
        }
        if !self.player_roster.contains(&command.player) {
            return Err(self.record_rejection(command, CommandRejectReason::PlayerNotRegistered));
        }

        if let Some(previous) = self.last_submissions.get(&command.player).copied() {
            if command.sequence == previous.command.sequence && command == previous.command {
                return previous.outcome;
            }
            if command.sequence <= previous.command.sequence {
                return Err(CommandRejection {
                    player: command.player,
                    sequence: command.sequence,
                    reason: CommandRejectReason::DuplicateOrReorderedSequence,
                });
            }
        } else if u32::try_from(self.last_submissions.len()).unwrap_or(u32::MAX)
            >= self.config.max_players
        {
            return Err(self.record_rejection(command, CommandRejectReason::PlayerBudgetExceeded));
        }

        self.checkpoint_ready = false;
        let outcome = if let Err(reason) = self.validate_fresh_submission(&command) {
            Err(self.record_rejection(command, reason))
        } else {
            self.queued_commands
                .entry(command.execute_at)
                .or_default()
                .push(command);
            Ok(CommandReceipt {
                player: command.player,
                sequence: command.sequence,
                execute_at: command.execute_at,
            })
        };
        self.last_submissions
            .insert(command.player, SubmissionRecord { command, outcome });
        outcome
    }

    /// Advance exactly one authoritative fixed tick and emit only actors changed since the previous frame.
    pub fn step(&mut self) -> Result<ServerFrame, RuntimeError> {
        self.step_with_intents(&[])
    }

    /// Advance one tick with server-owned bot/rule intents derived from one pre-step snapshot. These intents
    /// are independently bounded and never touch player admission quotas or sequence state.
    pub fn step_with_intents(
        &mut self,
        intents: &[ActorIntent],
    ) -> Result<ServerFrame, RuntimeError> {
        self.step_with_intents_and_spawns(intents, &[])
            .map(|(frame, _)| frame)
    }

    /// Advance a tick and, only if it remains active after combat/outcome checks, publish authored dynamic
    /// spawn batches in that same frame. New actors are intentionally absent from the intent snapshot and
    /// become actionable on the following tick.
    pub fn step_with_intents_and_spawns(
        &mut self,
        intents: &[ActorIntent],
        spawn_batches: &[Vec<DynamicActorSpawn>],
    ) -> Result<(ServerFrame, Vec<Vec<ActorId>>), RuntimeError> {
        if self.phase != MatchPhase::Active {
            return Err(RuntimeError::InvalidPhase(
                "fixed ticks require an active match",
            ));
        }
        if intents.len() > usize::try_from(self.config.max_actors).unwrap_or(usize::MAX) {
            return Err(RuntimeError::InvalidInternalIntents(
                "intent count exceeds the actor budget",
            ));
        }
        let mut intents = intents.to_vec();
        intents.sort_unstable_by_key(|intent| intent.actor);
        if intents
            .windows(2)
            .any(|pair| pair[0].actor == pair[1].actor)
        {
            return Err(RuntimeError::InvalidInternalIntents(
                "an actor may produce at most one intent per tick",
            ));
        }
        if !spawn_batches.is_empty() {
            let mut preflight = self.clone();
            preflight.spawn_dynamic_batches(spawn_batches)?;
        }
        let next_tick = self.tick.checked_add(1).ok_or(RuntimeError::TickOverflow)?;
        if next_tick > self.config.max_match_ticks {
            return Err(RuntimeError::MatchDurationExceeded);
        }
        self.checkpoint_ready = false;
        self.tick = next_tick;

        // Phase 0 of the tick: closed-form passive income. It depends on nothing but the tick, and the
        // frame audit compares it exactly, so it must run before the objective and time-limit early
        // returns - otherwise the deciding tick of every match would fail its own audit.
        self.accrue_passive_gold();

        // Phase 1 of the tick: retire expired restrictions BEFORE any order is executed, so an effect whose
        // final tick was the previous one cannot block a command delivered now. Placing this after command
        // execution would silently extend every effect by exactly one tick.
        self.expire_status_effects();
        // Same argument, same placement: an assist window that closed last tick must not still be open for
        // a kill delivered now.
        self.expire_damage_credits();

        let mut commands = self.queued_commands.remove(&self.tick).unwrap_or_default();
        commands.sort_unstable_by_key(command_order_key);
        for command in commands {
            self.execute(command)?;
        }
        for intent in intents {
            let validation = self
                .actor_state(intent.actor)
                .ok_or(CommandRejectReason::ActorNotFound)
                .and_then(|actor| {
                    if actor.owner.is_some() {
                        Err(CommandRejectReason::ActorNotOwned)
                    } else {
                        self.validate_actor_intent(actor, intent.kind, self.tick)
                    }
                });
            if let Err(reason) = validation {
                self.pending_events
                    .push(MatchEvent::InternalIntentRejected {
                        actor: intent.actor,
                        reason,
                    });
            } else {
                self.apply_intent(intent.actor, intent.kind)?;
            }
        }

        self.integrate_movement();
        self.resolve_combat()?;
        if !self.objective_claims.is_empty() {
            let outcome = if self.objective_claims.len() == 1 {
                MatchOutcome::Winner(
                    *self
                        .objective_claims
                        .first()
                        .expect("non-empty objective claims"),
                )
            } else {
                MatchOutcome::Draw
            };
            self.objective_claims.clear();
            return Ok((
                self.finish(outcome, MatchEndReason::ObjectiveDestroyed)?,
                Vec::new(),
            ));
        }
        if self.tick == self.config.max_match_ticks {
            return Ok((
                self.finish(MatchOutcome::Draw, MatchEndReason::TimeLimit)?,
                Vec::new(),
            ));
        }
        self.process_respawns();
        let spawned = self.spawn_dynamic_batches(spawn_batches)?;
        Ok((self.emit_frame(), spawned))
    }

    #[must_use]
    pub fn world_digest(&self) -> WorldDigest {
        let mut hash = StableHash::new();
        hash.bytes(b"metrocalk-gameplay-mob2-v10");
        hash.u64(self.seed);
        hash.u64(self.tick);
        hash.phase(self.phase);
        hash.config(self.config);
        match self.next_dynamic_actor_id {
            Some(next) => {
                hash.bool(true);
                hash.u64(next);
            }
            None => hash.bool(false),
        }
        // The modifier allocator is authoritative checkpointed state and its value decides future aggregation
        // and override precedence, so it must be part of simulation truth — not just carried in the codec.
        match self.next_modifier_id {
            Some(next) => {
                hash.bool(true);
                hash.u64(next);
            }
            None => hash.bool(false),
        }
        match self.next_status_effect_id {
            Some(next) => {
                hash.bool(true);
                hash.u64(next);
            }
            None => hash.bool(false),
        }
        // The projectile allocator decides future resolution order, so it is simulation truth, not codec
        // bookkeeping — the same argument that put the modifier allocator here.
        match self.next_projectile_id {
            Some(next) => {
                hash.bool(true);
                hash.u64(next);
            }
            None => hash.bool(false),
        }

        hash.len(self.ability_specs.len());
        for spec in &self.ability_specs {
            hash.ability_spec(*spec);
        }

        hash.len(self.actors.len());
        for actor in &self.actors {
            hash.actor(actor);
        }

        hash.len(self.projectiles.len());
        for projectile in &self.projectiles {
            hash.projectile(*projectile);
        }

        hash.len(self.scores.len());
        for entry in &self.scores {
            hash.score_entry(*entry);
        }

        hash.len(self.player_roster.len());
        for player in &self.player_roster {
            hash.u64(player.get());
        }

        hash.len(self.last_submissions.len());
        for (player, submission) in &self.last_submissions {
            hash.u64(player.get());
            hash.command(submission.command);
            hash.submission_outcome(submission.outcome);
        }

        hash.len(self.queued_commands.len());
        for (tick, commands) in &self.queued_commands {
            hash.u64(*tick);
            let mut canonical = commands.clone();
            canonical.sort_unstable_by_key(command_order_key);
            hash.len(canonical.len());
            for command in canonical {
                hash.command(command);
            }
        }
        WorldDigest(hash.finish())
    }

    const fn require_setup(&self, reason: &'static str) -> Result<(), RuntimeError> {
        if matches!(self.phase, MatchPhase::Setup) {
            Ok(())
        } else {
            Err(RuntimeError::InvalidPhase(reason))
        }
    }

    fn validate_fresh_submission(
        &self,
        command: &PlayerCommand,
    ) -> Result<(), CommandRejectReason> {
        if command.execute_at > self.config.max_match_ticks {
            return Err(CommandRejectReason::BeyondMatchDuration);
        }
        if command.execute_at <= self.tick {
            return Err(CommandRejectReason::StaleTick);
        }
        if command.execute_at
            > self
                .tick
                .saturating_add(u64::from(self.config.max_command_lead_ticks))
        {
            return Err(CommandRejectReason::TooFarAhead);
        }
        let queued_for_tick = self.queued_commands.get(&command.execute_at);
        if u32::try_from(queued_for_tick.map_or(0, Vec::len)).unwrap_or(u32::MAX)
            >= self.config.max_commands_per_tick
        {
            return Err(CommandRejectReason::TickCommandBudgetExceeded);
        }
        if u32::try_from(queued_for_tick.map_or(0, |commands| {
            commands
                .iter()
                .filter(|queued| queued.player == command.player)
                .count()
        }))
        .unwrap_or(u32::MAX)
            >= self.config.max_commands_per_player_per_tick
        {
            return Err(CommandRejectReason::PlayerTickCommandBudgetExceeded);
        }
        self.validate_actor_command(command)
    }

    fn validate_actor_command(&self, command: &PlayerCommand) -> Result<(), CommandRejectReason> {
        let actor = self
            .actor_state(command.actor)
            .ok_or(CommandRejectReason::ActorNotFound)?;
        if actor.owner != Some(command.player) {
            return Err(CommandRejectReason::ActorNotOwned);
        }
        self.validate_actor_intent(actor, command.kind, command.execute_at)
    }

    fn validate_actor_intent(
        &self,
        actor: &ActorState,
        kind: CommandKind,
        execute_at: Tick,
    ) -> Result<(), CommandRejectReason> {
        if !actor.alive() {
            return Err(CommandRejectReason::ActorDead);
        }
        // Crowd control gates here because this is the one place BOTH player commands and server-owned bot
        // intents are validated: a stun that only stopped players would be a competitive asymmetry.
        // `Stop` is deliberately never gated — it only cancels, so refusing it would produce rejection noise
        // without preventing anything.
        let controls = actor.control_mask;
        // `UpgradeAbility` joins `Stop` in never being crowd-control gated: spending a level-up point is a
        // menu action, not something an actor performs with its body, so a stun that blocked it would be
        // punishing the player's interface rather than their character.
        if !controls.is_empty()
            && !matches!(kind, CommandKind::Stop | CommandKind::UpgradeAbility { .. })
        {
            if controls.contains(ControlKind::Stun) {
                return Err(CommandRejectReason::ActorStunned);
            }
            match kind {
                CommandKind::MoveTo { .. } if controls.contains(ControlKind::Root) => {
                    return Err(CommandRejectReason::ActorRooted);
                }
                CommandKind::Cast { .. } if controls.contains(ControlKind::Silence) => {
                    return Err(CommandRejectReason::ActorSilenced);
                }
                CommandKind::BasicAttack { .. } if controls.contains(ControlKind::Disarm) => {
                    return Err(CommandRejectReason::ActorDisarmed);
                }
                _ => {}
            }
        }

        match kind {
            CommandKind::MoveTo { destination } => {
                if actor.cast.is_some() {
                    return Err(CommandRejectReason::ActorCasting);
                }
                if actor.attack.is_some() {
                    return Err(CommandRejectReason::ActorBusy);
                }
                if !self.config.map_bounds.contains(destination) {
                    return Err(CommandRejectReason::DestinationOutOfBounds);
                }
                Ok(())
            }
            CommandKind::Stop => {
                if actor.cast.is_some() {
                    Err(CommandRejectReason::ActorCasting)
                } else if actor.attack.is_some() {
                    Err(CommandRejectReason::ActorBusy)
                } else {
                    Ok(())
                }
            }
            CommandKind::Cast { ability, target } => {
                self.validate_cast(actor, ability, target, true)?;
                let spec = self
                    .ability_spec(ability)
                    .ok_or(CommandRejectReason::AbilityNotEquipped)?;
                if execute_at
                    .checked_add(u64::from(spec.cast_ticks))
                    .is_none_or(|resolves_at| resolves_at > self.config.max_match_ticks)
                {
                    return Err(CommandRejectReason::CastExceedsMatchDuration);
                }
                Ok(())
            }
            CommandKind::BasicAttack { target } => {
                self.validate_basic_attack(actor, target, true)?;
                let spec = actor
                    .basic_attack
                    .ok_or(CommandRejectReason::BasicAttackUnavailable)?
                    .spec;
                if execute_at
                    .checked_add(u64::from(spec.windup_ticks))
                    .is_none_or(|resolves_at| resolves_at > self.config.max_match_ticks)
                {
                    return Err(CommandRejectReason::AttackExceedsMatchDuration);
                }
                Ok(())
            }
            CommandKind::UpgradeAbility { ability } => Self::validate_upgrade(actor, ability),
        }
    }

    /// Can this actor spend a point on this ability right now?
    ///
    /// Every refusal names its own cause rather than collapsing into one reason, because a client has to
    /// tell "you have no points" from "this rank is not unlocked yet" to show anything useful.
    fn validate_upgrade(actor: &ActorState, ability: AbilityId) -> Result<(), CommandRejectReason> {
        if actor.kind != ActorKind::Hero {
            return Err(CommandRejectReason::ActorHasNoProgression);
        }
        let state = actor
            .abilities
            .binary_search_by_key(&ability, |candidate| candidate.id)
            .ok()
            .map(|index| actor.abilities[index])
            .ok_or(CommandRejectReason::AbilityNotEquipped)?;
        if actor.unspent_ability_points == 0 {
            return Err(CommandRejectReason::NoAbilityPointAvailable);
        }
        if state.rank >= MAX_ABILITY_RANK {
            return Err(CommandRejectReason::AbilityRankMaxed);
        }
        // Rank N unlocks at level 2N-1, so ranks 2..5 unlock at levels 3/5/7/9 - the shipped shape, and a
        // relation the restore audit can re-derive from the level alone.
        let unlocks_at_level = rank_unlock_level(state.rank + 1);
        if actor.level < unlocks_at_level {
            return Err(CommandRejectReason::AbilityRankLocked { unlocks_at_level });
        }
        Ok(())
    }

    fn validate_cast(
        &self,
        source: &ActorState,
        ability: AbilityId,
        target: CastTarget,
        check_cost_and_cooldown: bool,
    ) -> Result<(), CommandRejectReason> {
        if source.cast.is_some() {
            return Err(CommandRejectReason::ActorCasting);
        }
        if source.attack.is_some() {
            return Err(CommandRejectReason::ActorBusy);
        }
        let state = source
            .abilities
            .binary_search_by_key(&ability, |candidate| candidate.id)
            .ok()
            .map(|index| source.abilities[index])
            .ok_or(CommandRejectReason::AbilityNotEquipped)?;
        let spec = self
            .ability_spec(ability)
            .ok_or(CommandRejectReason::AbilityNotEquipped)?;

        if check_cost_and_cooldown {
            if self.tick < state.ready_at {
                return Err(CommandRejectReason::AbilityOnCooldown {
                    ready_at: state.ready_at,
                });
            }
            if source.resource < spec.resource_cost {
                return Err(CommandRejectReason::InsufficientResource {
                    required: spec.resource_cost,
                    available: source.resource,
                });
            }
        }

        let range_squared = u128::from(spec.range_mm).pow(2);
        match (spec.aim, target) {
            (AbilityAim::Unit, CastTarget::SelfActor | CastTarget::Actor(_)) => {
                let target_id = target_actor_id(source.id, target)
                    .expect("a unit cast target names a unit or the caster");
                let target_actor = self
                    .actor_state(target_id)
                    .ok_or(CommandRejectReason::TargetNotFound)?;
                if !target_actor.alive() {
                    return Err(CommandRejectReason::TargetDead);
                }
                if !relation_matches(spec.targeting, source.id, source.team, target_actor) {
                    return Err(CommandRejectReason::TargetRelationMismatch);
                }
                if source.position.squared_distance(target_actor.position) > range_squared {
                    return Err(CommandRejectReason::TargetOutOfRange);
                }
                // A single-victim projectile needs a direction to travel, and two actors may legally share
                // a position. Refusing is fail-closed: there is no defensible flight line to invent.
                if matches!(spec.delivery, AbilityDelivery::Projectile { .. })
                    && matches!(spec.impact, ImpactShape::Single)
                    && target_actor.position == source.position
                {
                    return Err(CommandRejectReason::InvalidTargetForm);
                }
                Ok(())
            }
            (AbilityAim::Point, CastTarget::Point(point)) => {
                if !self.config.map_bounds.contains(point) {
                    return Err(CommandRejectReason::TargetPointOutOfBounds);
                }
                if source.position.squared_distance(point) > range_squared {
                    return Err(CommandRejectReason::TargetOutOfRange);
                }
                if matches!(spec.impact, ImpactShape::Single) && point == source.position {
                    return Err(CommandRejectReason::InvalidTargetForm);
                }
                Ok(())
            }
            _ => Err(CommandRejectReason::InvalidTargetForm),
        }
    }

    fn validate_basic_attack(
        &self,
        source: &ActorState,
        target: ActorId,
        check_cooldown: bool,
    ) -> Result<(), CommandRejectReason> {
        if source.cast.is_some() || source.attack.is_some() {
            return Err(CommandRejectReason::ActorBusy);
        }
        let attack = source
            .basic_attack
            .ok_or(CommandRejectReason::BasicAttackUnavailable)?;
        if check_cooldown && self.tick < attack.ready_at {
            return Err(CommandRejectReason::BasicAttackOnCooldown {
                ready_at: attack.ready_at,
            });
        }
        let target = self
            .actor_state(target)
            .ok_or(CommandRejectReason::TargetNotFound)?;
        if !target.alive() {
            return Err(CommandRejectReason::TargetDead);
        }
        if target.team == source.team || target.id == source.id {
            return Err(CommandRejectReason::TargetRelationMismatch);
        }
        if source.position.squared_distance(target.position)
            > u128::from(attack.spec.range_mm).pow(2)
        {
            return Err(CommandRejectReason::TargetOutOfRange);
        }
        Ok(())
    }

    fn execute(&mut self, command: PlayerCommand) -> Result<(), RuntimeError> {
        if let Err(reason) = self.validate_actor_command(&command) {
            self.record_execution_rejection(command, reason);
            return Ok(());
        }
        self.apply_intent(command.actor, command.kind)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one function per command verb keeps the whole applied-intent surface auditable"
    )]
    fn apply_intent(&mut self, actor_id: ActorId, kind: CommandKind) -> Result<(), RuntimeError> {
        match kind {
            CommandKind::MoveTo { destination } => {
                let index = self.actor_index(actor_id).expect("validated actor");
                if self.actors[index].position == destination {
                    if self.actors[index].destination.take().is_some() {
                        self.dirty.insert(actor_id);
                        self.pending_events
                            .push(MatchEvent::MoveStopped { actor: actor_id });
                    }
                } else if self.actors[index].destination != Some(destination) {
                    self.actors[index].destination = Some(destination);
                    self.dirty.insert(actor_id);
                    self.pending_events.push(MatchEvent::MoveStarted {
                        actor: actor_id,
                        destination,
                    });
                }
            }
            CommandKind::Stop => {
                let index = self.actor_index(actor_id).expect("validated actor");
                if self.actors[index].destination.take().is_some() {
                    self.dirty.insert(actor_id);
                    self.pending_events
                        .push(MatchEvent::MoveStopped { actor: actor_id });
                }
            }
            CommandKind::Cast { ability, target } => {
                let spec = *self.ability_spec(ability).expect("validated ability");
                let index = self.actor_index(actor_id).expect("validated actor");
                let resolves_at = self
                    .tick
                    .checked_add(u64::from(spec.cast_ticks))
                    .ok_or(RuntimeError::TickOverflow)?;
                let ready_at = self
                    .tick
                    .checked_add(u64::from(spec.cooldown_ticks))
                    .ok_or(RuntimeError::TickOverflow)?;
                let actor = &mut self.actors[index];
                actor.resource -= spec.resource_cost;
                let ability_index = actor
                    .abilities
                    .binary_search_by_key(&ability, |candidate| candidate.id)
                    .expect("validated equipped ability");
                actor.abilities[ability_index].ready_at = ready_at;
                actor.destination = None;
                actor.cast = Some(PendingCast {
                    ability,
                    target,
                    resolves_at,
                });
                self.dirty.insert(actor_id);
                self.pending_events.push(MatchEvent::CastStarted {
                    source: actor_id,
                    ability,
                    target,
                    resolves_at,
                });
            }
            CommandKind::UpgradeAbility { ability } => {
                let index = self.actor_index(actor_id).expect("validated actor");
                let slot = self.actors[index]
                    .abilities
                    .binary_search_by_key(&ability, |candidate| candidate.id)
                    .expect("validated equipped ability");
                let rank = self.actors[index].abilities[slot].rank + 1;
                self.actors[index].abilities[slot].rank = rank;
                self.actors[index].unspent_ability_points -= 1;
                self.dirty.insert(actor_id);
                self.pending_events.push(MatchEvent::AbilityRankUp {
                    actor: actor_id,
                    ability,
                    rank,
                });
            }
            CommandKind::BasicAttack { target } => {
                let index = self.actor_index(actor_id).expect("validated actor");
                let attack = self.actors[index]
                    .basic_attack
                    .expect("validated basic attack");
                let resolves_at = self
                    .tick
                    .checked_add(u64::from(attack.spec.windup_ticks))
                    .ok_or(RuntimeError::TickOverflow)?;
                let ready_at = self
                    .tick
                    .checked_add(u64::from(attack.spec.cooldown_ticks))
                    .ok_or(RuntimeError::TickOverflow)?;
                let actor = &mut self.actors[index];
                actor
                    .basic_attack
                    .as_mut()
                    .expect("validated attack")
                    .ready_at = ready_at;
                actor.destination = None;
                actor.attack = Some(PendingAttack {
                    target,
                    resolves_at,
                });
                self.dirty.insert(actor_id);
                self.pending_events.push(MatchEvent::BasicAttackStarted {
                    source: actor_id,
                    target,
                    resolves_at,
                });
            }
        }
        Ok(())
    }

    fn integrate_movement(&mut self) {
        for actor in &mut self.actors {
            if !actor.alive() || actor.cast.is_some() || actor.attack.is_some() {
                continue;
            }
            // Immobilised actors hold position. Gating only the ORDER would let an actor stunned mid-path
            // keep walking to a destination it can no longer be told to abandon.
            if actor.control_mask.contains(ControlKind::Stun)
                || actor.control_mask.contains(ControlKind::Root)
            {
                continue;
            }
            let Some(destination) = actor.destination else {
                continue;
            };
            let (next, arrived) = step_towards(
                actor.position,
                destination,
                actor.stats.move_speed_mm_per_tick,
            );
            if next != actor.position {
                actor.position = next;
                self.dirty.insert(actor.id);
            }
            if arrived {
                actor.destination = None;
                self.dirty.insert(actor.id);
                self.pending_events
                    .push(MatchEvent::MoveStopped { actor: actor.id });
            }
        }
    }

    /// One combat phase: projectiles advance, this tick's casts and attacks land, and **both** feed a single
    /// health settlement.
    ///
    /// Sharing the settlement is what preserves the kernel's order-independent same-tick truth. If impacts
    /// settled separately from casts, a missile and a spell that both reach lethal on tick T would resolve
    /// as a race decided by phase order instead of the legal mutual kill the settlement policy promises.
    fn resolve_combat(&mut self) -> Result<(), RuntimeError> {
        let mut effects = self.advance_projectiles()?;
        effects.extend(self.resolve_due_casts());
        let kills = self.settle_health_effects(effects)?;
        // Progression settles AFTER the health batch, never inside it, and still inside `resolve_combat`
        // so a bounty is not silently forfeited on the deciding tick of a match that ends by objective or
        // time limit - both of those return early, before the later phases run.
        self.settle_progression(kills)
    }

    /// Step every live projectile one tick, resolving any that strike a body or reach the end of flight.
    ///
    /// Collision is a **swept segment** test, not a point test at the new position: at 30 Hz a fast missile
    /// covers more than a body's hit radius in one tick, so a point test would let it tunnel straight
    /// through its victim. Among the bodies the swept path touches, the one nearest the projectile's
    /// pre-step position is struck, with the actor ID only as a tie-break — a projectile must hit the body
    /// in front of it, never whichever body happens to hold the lower ID.
    fn advance_projectiles(&mut self) -> Result<Vec<AdmittedHealthEffect>, RuntimeError> {
        if self.projectiles.is_empty() {
            return Ok(Vec::new());
        }
        let mut effects = Vec::new();
        let mut survivors = Vec::with_capacity(self.projectiles.len());
        for projectile in std::mem::take(&mut self.projectiles) {
            let spec = *self
                .ability_spec(projectile.ability)
                .ok_or(RuntimeError::UnknownAbility(projectile.ability))?;
            let (speed_mm_per_tick, hit_radius_mm) =
                spec.projectile().ok_or(RuntimeError::InvalidAbility(
                    projectile.ability,
                    "an in-flight projectile references an ability with no projectile delivery",
                ))?;
            let (next, arrived) =
                step_towards(projectile.position, projectile.end, speed_mm_per_tick);
            let struck = self.swept_victim(&projectile, spec, next, hit_radius_mm);

            let (impact_at, victim) = match struck {
                // The impact centres on the body that was struck, which is where a client draws it.
                Some((victim, position)) => (position, Some(victim)),
                None if arrived => (next, None),
                None => {
                    survivors.push(ProjectileState {
                        position: next,
                        ..projectile
                    });
                    continue;
                }
            };
            self.removed_projectiles.insert(projectile.id);
            self.pending_events.push(MatchEvent::ProjectileResolved {
                projectile: projectile.id,
                position: impact_at,
                victim,
            });
            self.admit_impact(
                projectile.source,
                projectile.team,
                spec,
                projectile.amount,
                impact_at,
                victim,
                &mut effects,
            );
        }
        self.projectiles = survivors;
        Ok(effects)
    }

    /// The nearest legal body the projectile's swept path touches this tick, with its position.
    ///
    /// The source is excluded outright rather than left to the relation filter: an `AnyUnit` skillshot would
    /// otherwise collide with its own caster on the first tick of flight and never leave the ground.
    fn swept_victim(
        &self,
        projectile: &ProjectileState,
        spec: AbilitySpec,
        next: Vec2Mm,
        hit_radius_mm: u32,
    ) -> Option<(ActorId, Vec2Mm)> {
        self.actors
            .iter()
            .filter(|actor| {
                actor.alive()
                    && actor.id != projectile.source
                    && relation_matches(spec.targeting, projectile.source, projectile.team, actor)
                    && segment_within(
                        projectile.position,
                        next,
                        actor.position,
                        u128::from(hit_radius_mm).pow(2),
                    )
            })
            .min_by_key(|actor| {
                (
                    projectile.position.squared_distance(actor.position),
                    actor.id,
                )
            })
            .map(|actor| (actor.id, actor.position))
    }

    /// Expand one landed ability into the health effects it admits.
    ///
    /// `Single` affects exactly the named or struck body; `Circle` sweeps every actor the footprint covers
    /// that satisfies the ability's relation filter. Unlike projectile collision, an area impact does **not**
    /// exclude the caster — an ally-targeted healing burst is supposed to heal the actor standing at its
    /// centre.
    #[expect(
        clippy::too_many_arguments,
        reason = "the impact's magnitude is passed explicitly rather than re-read, which is the point"
    )]
    fn admit_impact(
        &self,
        source: ActorId,
        source_team: TeamId,
        spec: AbilitySpec,
        amount: u32,
        centre: Vec2Mm,
        single_victim: Option<ActorId>,
        effects: &mut Vec<AdmittedHealthEffect>,
    ) {
        let victims: Vec<ActorId> = match spec.impact {
            ImpactShape::Single => single_victim.into_iter().collect(),
            ImpactShape::Circle { radius_mm } => {
                let radius_squared = u128::from(radius_mm).pow(2);
                self.actors
                    .iter()
                    .filter(|actor| {
                        actor.alive()
                            && relation_matches(spec.targeting, source, source_team, actor)
                            && centre.squared_distance(actor.position) <= radius_squared
                    })
                    .map(|actor| actor.id)
                    .collect()
            }
        };
        for target in victims {
            // The magnitude is the one resolved when this landed, not one re-read from the spec now: that
            // is what keeps a rank-up from retroactively rewriting an effect already committed.
            effects.push(match spec.effect {
                AbilityEffect::Damage { school, .. } => AdmittedHealthEffect::Damage {
                    source,
                    target,
                    cause: DamageCause::Ability(spec.id),
                    amount,
                    school,
                },
                AbilityEffect::Heal { .. } => AdmittedHealthEffect::Heal {
                    source,
                    target,
                    ability: spec.id,
                    amount,
                },
            });
        }
    }

    /// Launch one projectile for a cast that has just resolved.
    fn launch_projectile(
        &mut self,
        source: ActorId,
        source_team: TeamId,
        position: Vec2Mm,
        spec: AbilitySpec,
        amount: u32,
        target: CastTarget,
    ) -> Result<(), CommandRejectReason> {
        if self.projectiles.len() >= usize::from(self.config.max_projectiles) {
            return Err(CommandRejectReason::ProjectileBudgetExceeded);
        }
        let raw_id = self
            .next_projectile_id
            .ok_or(CommandRejectReason::ProjectileBudgetExceeded)?;
        let end = self.flight_end(source, position, spec, target)?;
        let id = ProjectileId(raw_id);
        // Monotonic IDs keep the vector sorted by construction; the frame-boundary audit proves it.
        self.projectiles.push(ProjectileState {
            id,
            source,
            team: source_team,
            ability: spec.id,
            position,
            end,
            amount,
        });
        self.next_projectile_id = raw_id.checked_add(1);
        self.pending_events.push(MatchEvent::ProjectileSpawned {
            projectile: id,
            source,
            ability: spec.id,
            position,
            end,
        });
        Ok(())
    }

    /// Where a projectile's flight ends.
    ///
    /// An **area** impact lands where it was aimed: a lobbed burst detonates at the chosen point. A
    /// **single-victim** impact travels the ability's full range along the aimed direction, because a
    /// skillshot that stopped at the finger position would fizzle harmlessly whenever the player aimed
    /// short of their opponent. The terminus is then clamped along its own line into the map bounds, so the
    /// flight direction is preserved exactly and the projectile can never leave the playable rectangle.
    fn flight_end(
        &self,
        source: ActorId,
        from: Vec2Mm,
        spec: AbilitySpec,
        target: CastTarget,
    ) -> Result<Vec2Mm, CommandRejectReason> {
        let aimed = match target {
            CastTarget::Point(point) => point,
            CastTarget::SelfActor | CastTarget::Actor(_) => {
                self.actor_state(
                    target_actor_id(source, target)
                        .ok_or(CommandRejectReason::InvalidTargetForm)?,
                )
                .ok_or(CommandRejectReason::TargetNotFound)?
                .position
            }
        };
        match spec.impact {
            ImpactShape::Circle { .. } => Ok(aimed),
            ImpactShape::Single => {
                let extended = extend_to_range(from, aimed, spec.range_mm)
                    .ok_or(CommandRejectReason::InvalidTargetForm)?;
                Ok(clamp_segment_into_bounds(
                    from,
                    extended,
                    self.config.map_bounds,
                ))
            }
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one visible admission phase makes cross-action settlement ordering auditable"
    )]
    fn resolve_due_casts(&mut self) -> Vec<AdmittedHealthEffect> {
        let due: Vec<(ActorId, DueAction)> = self
            .actors
            .iter()
            .filter_map(|actor| {
                actor
                    .cast
                    .filter(|cast| cast.resolves_at <= self.tick)
                    .map(DueAction::Cast)
                    .or_else(|| {
                        actor
                            .attack
                            .filter(|attack| attack.resolves_at <= self.tick)
                            .map(DueAction::Attack)
                    })
                    .map(|action| (actor.id, action))
            })
            .collect();

        // Clear every due action first. Ability and attack effects then share one admission snapshot and one
        // health settlement, preserving legal mutual kills without ActorId initiative.
        for (source, action) in &due {
            let Some(index) = self.actor_index(*source) else {
                continue;
            };
            match action {
                DueAction::Cast(cast) if self.actors[index].cast == Some(*cast) => {
                    self.actors[index].cast = None;
                    self.dirty.insert(*source);
                }
                DueAction::Attack(attack) if self.actors[index].attack == Some(*attack) => {
                    self.actors[index].attack = None;
                    self.dirty.insert(*source);
                }
                _ => {}
            }
        }

        let mut effects = Vec::with_capacity(due.len());
        for (source_id, action) in due {
            let Some(source_index) = self.actor_index(source_id) else {
                continue;
            };
            if !self.actors[source_index].alive() {
                self.cancel_action(source_id, action, CommandRejectReason::ActorDead);
                continue;
            }

            match action {
                DueAction::Cast(cast) => {
                    if let Err(reason) = self.validate_cast(
                        &self.actors[source_index],
                        cast.ability,
                        cast.target,
                        false,
                    ) {
                        self.cancel_action(source_id, action, reason);
                        continue;
                    }
                    let spec = *self
                        .ability_spec(cast.ability)
                        .expect("equipped ability has a registered spec");
                    let source_team = self.actors[source_index].team;
                    let source_position = self.actors[source_index].position;
                    // Resolved once, here, from the rank the caster holds at the instant the cast lands.
                    let rank = self.actors[source_index]
                        .abilities
                        .binary_search_by_key(&cast.ability, |candidate| candidate.id)
                        .map_or(1, |index| self.actors[source_index].abilities[index].rank);
                    let amount = spec.amount_at_rank(rank);
                    if matches!(spec.delivery, AbilityDelivery::Projectile { .. }) {
                        // A launch failure cancels the cast rather than silently swallowing it: the client
                        // was told a cast started, so it is owed exactly one outcome.
                        if let Err(reason) = self.launch_projectile(
                            source_id,
                            source_team,
                            source_position,
                            spec,
                            amount,
                            cast.target,
                        ) {
                            self.cancel_action(source_id, action, reason);
                            continue;
                        }
                        self.pending_events.push(MatchEvent::CastResolved {
                            source: source_id,
                            ability: cast.ability,
                            target: cast.target,
                        });
                        continue;
                    }
                    // An instant cast lands on the named unit, or centres its footprint on the aimed point.
                    let (centre, single_victim) = match cast.target {
                        CastTarget::Point(point) => (point, None),
                        CastTarget::SelfActor | CastTarget::Actor(_) => {
                            let target = target_actor_id(source_id, cast.target)
                                .expect("a unit-aimed cast names a unit");
                            let position = self
                                .actor_state(target)
                                .expect("validated cast target")
                                .position;
                            (position, Some(target))
                        }
                    };
                    self.pending_events.push(MatchEvent::CastResolved {
                        source: source_id,
                        ability: cast.ability,
                        target: cast.target,
                    });
                    self.admit_impact(
                        source_id,
                        source_team,
                        spec,
                        amount,
                        centre,
                        single_victim,
                        &mut effects,
                    );
                }
                DueAction::Attack(attack) => {
                    if let Err(reason) =
                        self.validate_basic_attack(&self.actors[source_index], attack.target, false)
                    {
                        self.cancel_action(source_id, action, reason);
                        continue;
                    }
                    let spec = self.actors[source_index]
                        .basic_attack
                        .expect("validated basic attack")
                        .spec;
                    self.pending_events.push(MatchEvent::BasicAttackResolved {
                        source: source_id,
                        target: attack.target,
                    });
                    effects.push(AdmittedHealthEffect::Damage {
                        source: source_id,
                        target: attack.target,
                        cause: DamageCause::BasicAttack,
                        amount: spec.damage,
                        school: spec.school,
                    });
                }
            }
        }
        effects
    }

    fn cancel_action(&mut self, source: ActorId, action: DueAction, reason: CommandRejectReason) {
        match action {
            DueAction::Cast(cast) => self.pending_events.push(MatchEvent::CastCancelled {
                source,
                ability: cast.ability,
                reason,
            }),
            DueAction::Attack(attack) => {
                self.pending_events.push(MatchEvent::BasicAttackCancelled {
                    source,
                    target: attack.target,
                    reason,
                });
            }
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one settlement function keeps heal, damage, and death classification order explicit"
    )]
    fn settle_health_effects(
        &mut self,
        effects: Vec<AdmittedHealthEffect>,
    ) -> Result<Vec<KillRecord>, RuntimeError> {
        let mut by_target: BTreeMap<ActorId, Vec<AdmittedHealthEffect>> = BTreeMap::new();
        for effect in effects {
            let target = match effect {
                AdmittedHealthEffect::Damage { target, .. }
                | AdmittedHealthEffect::Heal { target, .. } => target,
            };
            by_target.entry(target).or_default().push(effect);
        }

        let mut deaths = Vec::new();
        let mut kills = Vec::new();
        // Collected during the loop and applied after it: the PRD counter belongs to the ATTACKER, not the
        // target the loop is grouped by, and lifesteal heals the attacker.
        let mut crit_rolls: Vec<(ActorId, bool)> = Vec::new();
        let mut lifesteal: Vec<(ActorId, u32)> = Vec::new();
        for (target, effects) in by_target {
            let target_index = self.actor_index(target).expect("validated cast target");
            let health_before = self.actors[target_index].health;
            let max_health = self.actors[target_index].stats.max_health;
            let mut settled_health = health_before;

            for effect in &effects {
                let AdmittedHealthEffect::Heal {
                    source,
                    ability,
                    amount,
                    ..
                } = *effect
                else {
                    continue;
                };
                let health_after = settled_health.saturating_add(amount).min(max_health);
                let applied = health_after - settled_health;
                settled_health = health_after;
                self.pending_events.push(MatchEvent::HealingApplied {
                    source,
                    target,
                    ability,
                    amount: applied,
                    health_after,
                });
            }

            let mut killer = None;
            for effect in &effects {
                let AdmittedHealthEffect::Damage {
                    source,
                    cause,
                    amount,
                    school,
                    ..
                } = *effect
                else {
                    continue;
                };
                // ---- THE DAMAGE PIPELINE. This order IS the determinism contract. ----
                // 1. Critical strike, rolled under PRD and applied PRE-mitigation, so armour still
                //    matters against a crit. Only a basic attack may critical: letting every ability
                //    crit is a balance decision nobody authored, and it is trivially added later per
                //    ability rather than silently assumed here.
                // 2. Penetration scales the target's mitigation (never below zero).
                // 3. Mitigation applies to the crit-multiplied amount.
                // 4. Shields absorb what mitigation left, oldest-expiring first.
                // 5. Health takes the remainder.
                // 6. Lifesteal is computed from what was ACTUALLY applied, and lands after the batch.
                let attacker_stats = self
                    .actor_state(source)
                    .map_or_else(CombatStats::inert, |actor| actor.stats);
                let critical = matches!(cause, DamageCause::BasicAttack)
                    && attacker_stats.crit_chance_bps > 0
                    && {
                        let attempts = self
                            .actor_state(source)
                            .map_or(0, |actor| actor.crit_attempts)
                            .saturating_add(1);
                        crate::model::roll_under_bps(
                            self.seed,
                            crate::model::RngDomain::CriticalStrike,
                            self.tick,
                            source.get(),
                            target.get(),
                            u64::from(attempts),
                            crate::model::prd_chance_bps(attacker_stats.crit_chance_bps, attempts),
                        )
                    };
                if matches!(cause, DamageCause::BasicAttack) && attacker_stats.crit_chance_bps > 0 {
                    crit_rolls.push((source, critical));
                }
                let struck = if critical {
                    u32::try_from(
                        u64::from(amount) * u64::from(attacker_stats.crit_damage_bps)
                            / u64::from(BASIS_POINTS),
                    )
                    .unwrap_or(u32::MAX)
                } else {
                    amount
                };
                let reduction = self.actors[target_index]
                    .stats
                    .reduction_after_penetration(school, attacker_stats);
                let mitigated = reduce_damage(struck, reduction);
                let absorbed = self.absorb_with_shields(target_index, mitigated);
                let applied = absorbed.min(settled_health);
                if applied > 0 && attacker_stats.lifesteal_bps > 0 {
                    let healed = u64::from(applied) * u64::from(attacker_stats.lifesteal_bps)
                        / u64::from(BASIS_POINTS);
                    lifesteal.push((source, u32::try_from(healed).unwrap_or(u32::MAX)));
                }
                let health_after = settled_health - applied;
                if settled_health > 0 && health_after == 0 {
                    killer = Some(source);
                }
                settled_health = health_after;
                // Gated on APPLIED, not on the authored amount: every overkill entry in the same batch
                // still emits `DamageApplied { amount: 0 }`, and crediting those would turn one area
                // ability into a mass assist event.
                if applied > 0 && source != target {
                    self.credit_damage(target_index, source);
                }
                self.pending_events.push(MatchEvent::DamageApplied {
                    source,
                    target,
                    cause,
                    school,
                    amount: applied,
                    health_after,
                });
            }

            self.actors[target_index].health = settled_health;
            if settled_health != health_before {
                self.dirty.insert(target);
            }
            if health_before > 0 && settled_health == 0 {
                self.actors[target_index].destination = None;
                if let Some(cancelled) = self.actors[target_index].cast.take() {
                    self.pending_events.push(MatchEvent::CastCancelled {
                        source: target,
                        ability: cancelled.ability,
                        reason: CommandRejectReason::ActorDead,
                    });
                }
                if let Some(cancelled) = self.actors[target_index].attack.take() {
                    self.pending_events.push(MatchEvent::BasicAttackCancelled {
                        source: target,
                        target: cancelled.target,
                        reason: CommandRejectReason::ActorDead,
                    });
                }
                let killer =
                    killer.expect("only admitted damage can settle positive health at zero");
                self.pending_events.push(MatchEvent::ActorDied {
                    actor: target,
                    killer,
                });
                // Snapshotted here, before `apply_death_rule` can despawn the body: a despawned actor is
                // still worth a bounty, and its assist window still names who earned a share of it.
                let assisters = std::mem::take(&mut self.actors[target_index].damage_credits)
                    .into_iter()
                    .map(|credit| credit.source)
                    .collect();
                kills.push(KillRecord {
                    victim: target,
                    victim_team: self.actors[target_index].team,
                    victim_position: self.actors[target_index].position,
                    victim_streak: self
                        .scores
                        .binary_search_by_key(&target, |entry| entry.actor)
                        .map_or(0, |index| self.scores[index].kill_streak),
                    bounty: self.actors[target_index].bounty,
                    killer,
                    assisters,
                });
                deaths.push(target);
            }
        }
        // The PRD counter advances once per attempt and resets on a success. Applied after the batch so a
        // single swing cannot see its own counter move mid-settlement.
        for (actor, critical) in crit_rolls {
            let Some(index) = self.actor_index(actor) else {
                continue;
            };
            self.actors[index].crit_attempts = if critical {
                0
            } else {
                self.actors[index].crit_attempts.saturating_add(1)
            };
            self.dirty.insert(actor);
        }
        // Lifesteal lands AFTER the batch, which is a rule worth naming: it cannot rescue an attacker from
        // damage that was lethal on the same tick. That keeps the order-independent mutual kill the
        // settlement policy promises - a heal that could undo a simultaneous death would make survival
        // depend on which target the loop reached first.
        for (actor, amount) in lifesteal {
            let Some(index) = self.actor_index(actor) else {
                continue;
            };
            if !self.actors[index].alive() || amount == 0 {
                continue;
            }
            let max_health = self.actors[index].stats.max_health;
            let before = self.actors[index].health;
            let after = before.saturating_add(amount).min(max_health);
            if after == before {
                continue;
            }
            self.actors[index].health = after;
            self.dirty.insert(actor);
            self.pending_events.push(MatchEvent::LifestealApplied {
                actor,
                amount: after - before,
                health_after: after,
            });
        }
        for actor in deaths {
            self.apply_death_rule(actor)?;
        }
        Ok(kills)
    }

    /// Consume shields against `damage`, returning what reaches health.
    ///
    /// Shields are consumed **oldest-expiring first**: spending the pool that is about to vanish anyway is
    /// the player-favourable choice and it is deterministic, since expiry ties break on the monotonic
    /// effect id. A shield whose pool reaches zero is NOT removed here — it expires on its own schedule,
    /// because its other half (the restrictions and modifiers it owns) may still be doing work.
    fn absorb_with_shields(&mut self, target_index: usize, damage: u32) -> u32 {
        if damage == 0 || self.actors[target_index].status_effects.is_empty() {
            return damage;
        }
        let mut order: Vec<(Tick, StatusEffectId, usize)> = self.actors[target_index]
            .status_effects
            .iter()
            .enumerate()
            .filter(|(_, effect)| effect.shield_pool > 0)
            .map(|(index, effect)| (effect.expires_at, effect.id, index))
            .collect();
        if order.is_empty() {
            return damage;
        }
        order.sort_unstable();
        let mut remaining = damage;
        for (_, _, index) in order {
            if remaining == 0 {
                break;
            }
            let pool = self.actors[target_index].status_effects[index].shield_pool;
            let absorbed = pool.min(remaining);
            self.actors[target_index].status_effects[index].shield_pool = pool - absorbed;
            remaining -= absorbed;
            if absorbed > 0 {
                let actor = self.actors[target_index].id;
                let effect = self.actors[target_index].status_effects[index].id;
                self.dirty.insert(actor);
                self.pending_events.push(MatchEvent::ShieldAbsorbed {
                    actor,
                    effect,
                    amount: absorbed,
                    remaining: pool - absorbed,
                });
            }
        }
        remaining
    }

    fn apply_death_rule(&mut self, actor_id: ActorId) -> Result<(), RuntimeError> {
        self.reject_queued_for_actor(actor_id);
        let index = self.actor_index(actor_id).expect("classified actor exists");
        match self.actors[index].death_rule {
            DeathRule::StayDead => {}
            DeathRule::Despawn => {
                self.actors.remove(index);
                self.dirty.remove(&actor_id);
                self.removed.insert(actor_id);
                self.pending_events
                    .push(MatchEvent::ActorDespawned { actor: actor_id });
            }
            DeathRule::Respawn { delay_ticks, .. } => {
                let at_tick = self
                    .tick
                    .checked_add(u64::from(delay_ticks))
                    .ok_or(RuntimeError::TickOverflow)?;
                self.actors[index].respawn_at = Some(at_tick);
                self.dirty.insert(actor_id);
                self.pending_events.push(MatchEvent::RespawnScheduled {
                    actor: actor_id,
                    at_tick,
                });
            }
            DeathRule::ObjectiveVictory { winner } => {
                self.objective_claims.insert(winner);
            }
        }
        Ok(())
    }

    fn reject_queued_for_actor(&mut self, actor_id: ActorId) {
        let mut rejected = Vec::new();
        for commands in self.queued_commands.values_mut() {
            let mut retained = Vec::with_capacity(commands.len());
            for command in std::mem::take(commands) {
                if command.actor == actor_id {
                    rejected.push(command);
                } else {
                    retained.push(command);
                }
            }
            *commands = retained;
        }
        self.queued_commands
            .retain(|_, commands| !commands.is_empty());
        rejected.sort_unstable_by_key(|command| (command.execute_at, command_order_key(command)));
        for command in rejected {
            self.record_execution_rejection(command, CommandRejectReason::ActorDead);
        }
    }

    fn process_respawns(&mut self) {
        let due: Vec<ActorId> = self
            .actors
            .iter()
            .filter(|actor| actor.respawn_at.is_some_and(|at_tick| at_tick <= self.tick))
            .map(|actor| actor.id)
            .collect();
        for actor_id in due {
            let index = self
                .actor_index(actor_id)
                .expect("due respawn actor exists");
            let DeathRule::Respawn { at, .. } = self.actors[index].death_rule else {
                continue;
            };
            let actor = &mut self.actors[index];
            actor.position = at;
            actor.destination = None;
            actor.health = actor.stats.max_health;
            actor.resource = actor.stats.max_resource;
            actor.cast = None;
            actor.attack = None;
            actor.respawn_at = None;
            self.dirty.insert(actor_id);
            self.pending_events.push(MatchEvent::ActorRespawned {
                actor: actor_id,
                position: at,
            });
        }
    }

    fn emit_frame(&mut self) -> ServerFrame {
        let changed_ids = std::mem::take(&mut self.dirty);
        let removed_ids = std::mem::take(&mut self.removed);
        let changed = changed_ids
            .into_iter()
            .filter_map(|id| self.actor(id))
            .collect();
        let removed_projectiles = std::mem::take(&mut self.removed_projectiles);
        let changed_scores = std::mem::take(&mut self.dirty_scores);
        let scores = changed_scores
            .into_iter()
            .filter_map(|actor| self.score(actor))
            .collect();
        let mut frame = ServerFrame {
            tick: self.tick,
            phase: self.phase,
            changed,
            removed: removed_ids.into_iter().collect(),
            projectiles: self.projectile_views(),
            removed_projectiles: removed_projectiles.into_iter().collect(),
            scores,
            events: std::mem::take(&mut self.pending_events),
            world_digest: self.world_digest(),
            frame_digest: FrameDigest::default(),
        };
        self.rejection_events_in_frame = 0;
        frame.frame_digest = digest_frame(&frame);
        self.checkpoint_ready = true;
        frame
    }

    fn record_rejection(
        &mut self,
        command: PlayerCommand,
        reason: CommandRejectReason,
    ) -> CommandRejection {
        if self.rejection_events_in_frame < self.config.max_rejection_events_per_frame {
            self.checkpoint_ready = false;
            self.pending_events.push(MatchEvent::CommandRejected {
                player: command.player,
                sequence: command.sequence,
                reason,
            });
            self.rejection_events_in_frame += 1;
        }
        CommandRejection {
            player: command.player,
            sequence: command.sequence,
            reason,
        }
    }

    /// An accepted command must receive exactly one authoritative execution outcome. These events are never
    /// dropped by the bounded ingress-telemetry budget; their count is already bounded by accepted commands.
    fn record_execution_rejection(&mut self, command: PlayerCommand, reason: CommandRejectReason) {
        self.pending_events.push(MatchEvent::CommandRejected {
            player: command.player,
            sequence: command.sequence,
            reason,
        });
    }

    fn ability_spec(&self, id: AbilityId) -> Option<&AbilitySpec> {
        self.ability_specs
            .binary_search_by_key(&id, |candidate| candidate.id)
            .ok()
            .map(|index| &self.ability_specs[index])
    }

    fn actor_index(&self, id: ActorId) -> Option<usize> {
        self.actors.binary_search_by_key(&id, |actor| actor.id).ok()
    }

    fn actor_state(&self, id: ActorId) -> Option<&ActorState> {
        self.actor_index(id).map(|index| &self.actors[index])
    }
}

fn command_order_key(command: &PlayerCommand) -> (PlayerId, u32, ActorId) {
    (command.player, command.sequence, command.actor)
}

/// The hero level at which `rank` becomes available: `2N − 1`, so ranks 2–5 unlock at 3/5/7/9.
///
/// A relation rather than a table, because the restore audit re-derives it from the level alone.
pub(crate) const fn rank_unlock_level(rank: u8) -> u8 {
    if rank <= 1 {
        return 1;
    }
    (rank * 2).saturating_sub(1)
}

const fn target_actor_id(source: ActorId, target: CastTarget) -> Option<ActorId> {
    match target {
        CastTarget::SelfActor => Some(source),
        CastTarget::Actor(id) => Some(id),
        CastTarget::Point(_) => None,
    }
}

/// Does `target` satisfy the relation an ability filters by?
///
/// Deliberately keyed on the source's *identity and team* rather than a live `ActorState`: a projectile's
/// caster may already be dead or despawned when its missile lands, and the relation it launched under must
/// not change because of that.
fn relation_matches(
    targeting: AbilityTargeting,
    source: ActorId,
    source_team: TeamId,
    target: &ActorState,
) -> bool {
    match targeting {
        AbilityTargeting::SelfOnly => source == target.id,
        AbilityTargeting::Ally => source_team == target.team,
        AbilityTargeting::Enemy => source_team != target.team,
        AbilityTargeting::AnyUnit => true,
    }
}

/// Is `point` within `radius_squared` of the segment `from`→`to`?
///
/// Pure integer arithmetic with no division: the perpendicular case compares `cross²` against
/// `radius² × |segment|²` instead of dividing to recover the distance. With map coordinates bounded to
/// ±500,000 mm and the projectile speed/radius ceilings in `model`, every intermediate stays several orders
/// of magnitude inside `i128`.
fn segment_within(from: Vec2Mm, to: Vec2Mm, point: Vec2Mm, radius_squared: u128) -> bool {
    let segment_x = i128::from(to.x) - i128::from(from.x);
    let segment_y = i128::from(to.y) - i128::from(from.y);
    let to_point_x = i128::from(point.x) - i128::from(from.x);
    let to_point_y = i128::from(point.y) - i128::from(from.y);
    let segment_squared = segment_x * segment_x + segment_y * segment_y;
    if segment_squared == 0 {
        return from.squared_distance(point) <= radius_squared;
    }
    let projection = to_point_x * segment_x + to_point_y * segment_y;
    if projection <= 0 {
        return from.squared_distance(point) <= radius_squared;
    }
    if projection >= segment_squared {
        return to.squared_distance(point) <= radius_squared;
    }
    let cross = to_point_x * segment_y - to_point_y * segment_x;
    let Ok(radius_squared) = i128::try_from(radius_squared) else {
        return true;
    };
    cross.saturating_mul(cross) <= radius_squared.saturating_mul(segment_squared)
}

/// The point `range_mm` away from `from` along the direction `from`→`towards`.
///
/// `None` when the two points coincide: there is no direction, and inventing one would make a skillshot's
/// flight line depend on an arbitrary convention.
fn extend_to_range(from: Vec2Mm, towards: Vec2Mm, range_mm: u32) -> Option<Vec2Mm> {
    if from == towards {
        return None;
    }
    let delta_x = i128::from(towards.x) - i128::from(from.x);
    let delta_y = i128::from(towards.y) - i128::from(from.y);
    let distance =
        i128::try_from(integer_sqrt_ceil(from.squared_distance(towards))).unwrap_or(i128::MAX);
    if distance == 0 {
        return None;
    }
    let range = i128::from(range_mm);
    let x = i128::from(from.x) + delta_x * range / distance;
    let y = i128::from(from.y) + delta_y * range / distance;
    Some(Vec2Mm::new(
        i32::try_from(x).unwrap_or(if x.is_negative() { i32::MIN } else { i32::MAX }),
        i32::try_from(y).unwrap_or(if y.is_negative() { i32::MIN } else { i32::MAX }),
    ))
}

/// Shorten `from`→`to` along its own line until it stays inside `bounds`.
///
/// Truncating the scaled offset always moves the result **toward** `from`, which is inside the rectangle, so
/// the clamp can only ever land inside. Clamping the coordinates independently would have been simpler and
/// wrong: it bends the flight line, so a skillshot aimed at a corner would travel somewhere the player did
/// not aim.
fn clamp_segment_into_bounds(from: Vec2Mm, to: Vec2Mm, bounds: RectMm) -> Vec2Mm {
    if bounds.contains(to) {
        return to;
    }
    let delta_x = i128::from(to.x) - i128::from(from.x);
    let delta_y = i128::from(to.y) - i128::from(from.y);
    let mut numerator = 1_i128;
    let mut denominator = 1_i128;
    for (delta, headroom) in [
        (
            delta_x,
            axis_headroom(from.x, delta_x, bounds.min.x, bounds.max.x),
        ),
        (
            delta_y,
            axis_headroom(from.y, delta_y, bounds.min.y, bounds.max.y),
        ),
    ] {
        if delta == 0 {
            continue;
        }
        let magnitude = delta.abs();
        if headroom * denominator < numerator * magnitude {
            numerator = headroom;
            denominator = magnitude;
        }
    }
    let x = i128::from(from.x) + delta_x * numerator / denominator;
    let y = i128::from(from.y) + delta_y * numerator / denominator;
    Vec2Mm::new(
        i32::try_from(x).unwrap_or(from.x),
        i32::try_from(y).unwrap_or(from.y),
    )
}

/// How far the coordinate may still travel in the direction of `delta` before leaving `[minimum, maximum]`.
fn axis_headroom(from: i32, delta: i128, minimum: i32, maximum: i32) -> i128 {
    if delta > 0 {
        i128::from(maximum) - i128::from(from)
    } else {
        i128::from(from) - i128::from(minimum)
    }
    .max(0)
}

fn reduce_damage(amount: u32, reduction_bps: u16) -> u32 {
    let remaining = u64::from(BASIS_POINTS - reduction_bps);
    let scaled = u64::from(amount) * remaining / u64::from(BASIS_POINTS);
    let result = u32::try_from(scaled).expect("basis-point reduction cannot increase damage");
    if amount > 0 && result == 0 {
        1
    } else {
        result
    }
}

fn step_towards(current: Vec2Mm, target: Vec2Mm, max_step: u32) -> (Vec2Mm, bool) {
    if current == target {
        return (target, true);
    }
    if max_step == 0 {
        return (current, false);
    }
    let squared = current.squared_distance(target);
    let distance = integer_sqrt_ceil(squared);
    if u128::from(max_step) >= distance {
        return (target, true);
    }

    let dx = i128::from(target.x) - i128::from(current.x);
    let dy = i128::from(target.y) - i128::from(current.y);
    let distance_i128 = i128::try_from(distance).expect("distance between i32 points fits i128");
    let step = i128::from(max_step);
    let mut step_x = dx * step / distance_i128;
    let mut step_y = dy * step / distance_i128;
    if step_x == 0 && step_y == 0 {
        if dx.abs() >= dy.abs() {
            step_x = dx.signum();
        } else {
            step_y = dy.signum();
        }
    }
    let next_x = i128::from(current.x) + step_x;
    let next_y = i128::from(current.y) + step_y;
    (
        Vec2Mm::new(
            i32::try_from(next_x).expect("interpolation remains between i32 endpoints"),
            i32::try_from(next_y).expect("interpolation remains between i32 endpoints"),
        ),
        false,
    )
}

/// The stable one-byte discriminant for one authoritative event.
///
/// It lives apart from the payload encoding so it is a single table that a test can walk. That matters:
/// `StatusEffectExpired` and `RespawnCancelled` were both emitting tag **19**, and a duplicate tag is a hole
/// in an encoding whose entire job is that two different causal traces cannot hash the same. The table is
/// append-only — a shipped tag is never reused for a different event.
pub(crate) const fn event_tag(event: MatchEvent) -> u8 {
    match event {
        MatchEvent::MatchStarted => 0,
        MatchEvent::CommandRejected { .. } => 1,
        MatchEvent::MoveStarted { .. } => 2,
        MatchEvent::MoveStopped { .. } => 3,
        MatchEvent::CastStarted { .. } => 4,
        MatchEvent::CastCancelled { .. } => 5,
        MatchEvent::CastResolved { .. } => 6,
        MatchEvent::DamageApplied { .. } => 7,
        MatchEvent::HealingApplied { .. } => 8,
        MatchEvent::ActorDied { .. } => 9,
        MatchEvent::MatchFinished { .. } => 10,
        MatchEvent::BasicAttackStarted { .. } => 11,
        MatchEvent::BasicAttackCancelled { .. } => 12,
        MatchEvent::BasicAttackResolved { .. } => 13,
        MatchEvent::ActorDespawned { .. } => 14,
        MatchEvent::RespawnScheduled { .. } => 15,
        MatchEvent::ActorRespawned { .. } => 16,
        MatchEvent::InternalIntentRejected { .. } => 17,
        MatchEvent::ActorSpawned { .. } => 18,
        MatchEvent::RespawnCancelled { .. } => 19,
        MatchEvent::StatusEffectExpired { .. } => 20,
        MatchEvent::ProjectileSpawned { .. } => 21,
        MatchEvent::ProjectileResolved { .. } => 22,
        MatchEvent::ProjectileCancelled { .. } => 23,
        MatchEvent::KillCredited { .. } => 24,
        MatchEvent::AssistCredited { .. } => 25,
        MatchEvent::GoldGranted { .. } => 26,
        MatchEvent::ExperienceGranted { .. } => 27,
        MatchEvent::HeroLevelUp { .. } => 28,
        MatchEvent::AbilityRankUp { .. } => 29,
        MatchEvent::ShieldAbsorbed { .. } => 30,
        MatchEvent::LifestealApplied { .. } => 31,
    }
}

pub(crate) fn digest_frame(frame: &ServerFrame) -> FrameDigest {
    let mut hash = StableHash::new();
    hash.bytes(b"metrocalk-gameplay-frame-v8");
    hash.u64(frame.tick);
    hash.phase(frame.phase);
    hash.len(frame.changed.len());
    for actor in &frame.changed {
        hash.actor_view(actor);
    }
    hash.len(frame.removed.len());
    for actor in &frame.removed {
        hash.u64(actor.get());
    }
    hash.len(frame.projectiles.len());
    for projectile in &frame.projectiles {
        hash.projectile_view(*projectile);
    }
    hash.len(frame.removed_projectiles.len());
    for projectile in &frame.removed_projectiles {
        hash.u64(projectile.get());
    }
    hash.len(frame.scores.len());
    for entry in &frame.scores {
        hash.score_view(*entry);
    }
    hash.len(frame.events.len());
    for event in &frame.events {
        hash.match_event(*event);
    }
    hash.u64(frame.world_digest.0);
    FrameDigest(hash.finish())
}

struct StableHash(u64);

impl StableHash {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    const fn new() -> Self {
        Self(Self::OFFSET)
    }

    const fn finish(self) -> u64 {
        self.0
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    fn bool(&mut self, value: bool) {
        self.u8(u8::from(value));
    }

    fn u8(&mut self, value: u8) {
        self.bytes(&[value]);
    }

    fn u16(&mut self, value: u16) {
        self.bytes(&value.to_le_bytes());
    }

    fn u32(&mut self, value: u32) {
        self.bytes(&value.to_le_bytes());
    }

    fn i32(&mut self, value: i32) {
        self.bytes(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn len(&mut self, value: usize) {
        self.u64(u64::try_from(value).unwrap_or(u64::MAX));
    }

    fn point(&mut self, point: Vec2Mm) {
        self.i32(point.x);
        self.i32(point.y);
    }

    fn modifier(&mut self, modifier: StatModifier) {
        self.u64(modifier.id.get());
        self.u8(match modifier.kind {
            StatKind::MaxHealth => 0,
            StatKind::MaxResource => 1,
            StatKind::MoveSpeed => 2,
            StatKind::PhysicalReduction => 3,
            StatKind::MagicReduction => 4,
            StatKind::CritChance => 5,
            StatKind::CritDamage => 6,
            StatKind::PhysicalPenetration => 7,
            StatKind::MagicPenetration => 8,
            StatKind::Lifesteal => 9,
        });
        match modifier.op {
            ModifierOp::Flat { magnitude } => {
                self.u8(0);
                self.i32(magnitude);
            }
            ModifierOp::PercentAdd { bps } => {
                self.u8(1);
                self.i32(bps);
            }
            ModifierOp::PercentMult { bps } => {
                self.u8(2);
                self.i32(bps);
            }
            ModifierOp::Override { value } => {
                self.u8(3);
                self.u32(value);
            }
        }
    }

    fn stats(&mut self, stats: CombatStats) {
        self.u32(stats.max_health);
        self.u32(stats.max_resource);
        self.u32(stats.move_speed_mm_per_tick);
        self.u16(stats.physical_reduction_bps);
        self.u16(stats.magic_reduction_bps);
        self.u16(stats.crit_chance_bps);
        self.u16(stats.crit_damage_bps);
        self.u16(stats.physical_penetration_bps);
        self.u16(stats.magic_penetration_bps);
        self.u16(stats.lifesteal_bps);
    }

    fn provenance(&mut self, provenance: ActorProvenance) {
        match provenance {
            ActorProvenance::Authored => self.u8(0),
            ActorProvenance::Dynamic { spawn_ordinal } => {
                self.u8(1);
                self.u64(spawn_ordinal);
            }
            ActorProvenance::Wave {
                wave_id,
                unit_slot,
                spawn_ordinal,
            } => {
                self.u8(2);
                self.u32(wave_id);
                self.u16(unit_slot);
                self.u64(spawn_ordinal);
            }
        }
    }

    fn phase(&mut self, phase: MatchPhase) {
        match phase {
            MatchPhase::Setup => self.u8(0),
            MatchPhase::Active => self.u8(1),
            MatchPhase::Complete { outcome, reason } => {
                self.u8(2);
                self.outcome(outcome);
                self.end_reason(reason);
            }
        }
    }

    fn outcome(&mut self, outcome: MatchOutcome) {
        match outcome {
            MatchOutcome::Winner(team) => {
                self.u8(0);
                self.u8(team.get());
            }
            MatchOutcome::Draw => self.u8(1),
        }
    }

    fn end_reason(&mut self, reason: MatchEndReason) {
        self.u8(match reason {
            MatchEndReason::ObjectiveDestroyed => 0,
            MatchEndReason::TimeLimit => 1,
            MatchEndReason::Host => 2,
        });
    }

    fn config(&mut self, config: MatchConfig) {
        self.u16(config.tick_rate_hz);
        self.point(config.map_bounds.min);
        self.point(config.map_bounds.max);
        self.u32(config.max_players);
        self.u32(config.max_actors);
        self.u32(config.max_abilities_per_actor);
        self.u32(config.max_commands_per_tick);
        self.u32(config.max_commands_per_player_per_tick);
        self.u32(config.max_rejection_events_per_frame);
        self.u16(config.max_command_lead_ticks);
        self.u64(config.max_match_ticks);
        self.u16(config.max_lanes);
        self.u16(config.max_lane_points_per_lane);
        self.u16(config.max_wave_schedules);
        self.u16(config.max_units_per_wave);
        self.u16(config.max_modifiers_per_actor);
        self.u16(config.max_status_effects_per_actor);
        self.u16(config.max_projectiles);
        self.u16(config.max_score_entries);
        self.u16(config.max_damage_credits_per_actor);
        self.u32(config.assist_window_ticks);
        self.u32(config.xp_share_radius_mm);
        self.u32(config.passive_gold_per_100s);
        self.u32(config.max_checkpoint_bytes);
    }

    fn projectile(&mut self, projectile: ProjectileState) {
        let ProjectileState {
            id,
            source,
            team,
            ability,
            position,
            end,
            amount,
        } = projectile;
        self.u64(id.get());
        self.u64(source.get());
        self.u8(team.get());
        self.u32(ability.get());
        self.point(position);
        self.point(end);
        self.u32(amount);
    }

    fn bounty(&mut self, bounty: Bounty) {
        self.u32(bounty.gold);
        self.u32(bounty.experience);
    }

    fn growth(&mut self, growth: StatGrowth) {
        self.u32(growth.max_health_per_level);
        self.u32(growth.max_resource_per_level);
        self.u32(growth.move_speed_mm_per_tick_per_level);
        self.u16(growth.physical_reduction_bps_per_level);
        self.u16(growth.magic_reduction_bps_per_level);
    }

    fn gold_reason(&mut self, reason: GoldReason) {
        self.u8(match reason {
            GoldReason::Kill => 0,
            GoldReason::Shutdown => 1,
            GoldReason::Assist => 2,
        });
    }

    fn score_entry(&mut self, entry: ScoreEntry) {
        let ScoreEntry {
            actor,
            owner,
            team,
            since_tick,
            earned_gold,
            passive_gold_paid,
            kills,
            deaths,
            assists,
            kill_streak,
            best_kill_streak,
        } = entry;
        self.u64(actor.get());
        match owner {
            Some(owner) => {
                self.bool(true);
                self.u64(owner.get());
            }
            None => self.bool(false),
        }
        self.u8(team.get());
        self.u64(since_tick);
        self.u32(earned_gold);
        // The derived passive half is hashed alongside its inputs, exactly as the stat cache is: a drifted
        // cache must not be invisible to a digest comparison between two runtimes.
        self.u32(passive_gold_paid);
        self.u32(kills);
        self.u32(deaths);
        self.u32(assists);
        self.u16(kill_streak);
        self.u16(best_kill_streak);
    }

    fn score_view(&mut self, view: ScoreView) {
        let ScoreView {
            actor,
            owner,
            team,
            gold,
            kills,
            deaths,
            assists,
            kill_streak,
            best_kill_streak,
        } = view;
        self.u64(actor.get());
        match owner {
            Some(owner) => {
                self.bool(true);
                self.u64(owner.get());
            }
            None => self.bool(false),
        }
        self.u8(team.get());
        self.u32(gold);
        self.u32(kills);
        self.u32(deaths);
        self.u32(assists);
        self.u16(kill_streak);
        self.u16(best_kill_streak);
    }

    fn projectile_view(&mut self, projectile: ProjectileView) {
        self.u64(projectile.id.get());
        self.u64(projectile.source.get());
        self.u8(projectile.team.get());
        self.u32(projectile.ability.get());
        self.point(projectile.position);
        self.point(projectile.end);
        self.u32(projectile.hit_radius_mm);
    }

    fn ability_spec(&mut self, spec: AbilitySpec) {
        self.u32(spec.id.get());
        self.u8(match spec.aim {
            AbilityAim::Unit => 0,
            AbilityAim::Point => 1,
        });
        self.u8(match spec.targeting {
            AbilityTargeting::SelfOnly => 0,
            AbilityTargeting::Ally => 1,
            AbilityTargeting::Enemy => 2,
            AbilityTargeting::AnyUnit => 3,
        });
        self.u32(spec.range_mm);
        self.u32(spec.resource_cost);
        self.u32(spec.cooldown_ticks);
        self.u16(spec.cast_ticks);
        self.u32(spec.per_rank_amount);
        match spec.delivery {
            AbilityDelivery::Instant => self.u8(0),
            AbilityDelivery::Projectile {
                speed_mm_per_tick,
                hit_radius_mm,
            } => {
                self.u8(1);
                self.u32(speed_mm_per_tick);
                self.u32(hit_radius_mm);
            }
        }
        match spec.impact {
            ImpactShape::Single => self.u8(0),
            ImpactShape::Circle { radius_mm } => {
                self.u8(1);
                self.u32(radius_mm);
            }
        }
        match spec.effect {
            AbilityEffect::Damage { amount, school } => {
                self.u8(0);
                self.u32(amount);
                self.u8(match school {
                    DamageSchool::Physical => 0,
                    DamageSchool::Magic => 1,
                    DamageSchool::True => 2,
                });
            }
            AbilityEffect::Heal { amount } => {
                self.u8(1);
                self.u32(amount);
            }
        }
    }

    /// Destructured on purpose: omitting a field here is the one failure mode in the whole state layer
    /// that nothing else would catch - the digest would simply stop covering it, silently. Binding every
    /// field by name turns that into a compile error the next time one is added.
    #[expect(
        clippy::too_many_lines,
        reason = "the exhaustive destructured actor encoding is intentionally one visible block"
    )]
    fn actor(&mut self, actor: &ActorState) {
        let ActorState {
            id: _,
            owner: _,
            team: _,
            kind: _,
            position: _,
            destination: _,
            authored_stats,
            growth,
            bounty,
            experience,
            level,
            unspent_ability_points,
            crit_attempts,
            damage_credits,
            base_stats: _,
            modifiers: _,
            status_effects: _,
            control_mask: _,
            stats: _,
            health: _,
            resource: _,
            abilities: _,
            cast: _,
            basic_attack: _,
            attack: _,
            death_rule: _,
            respawn_at: _,
            provenance: _,
        } = actor;
        self.u64(actor.id.get());
        self.provenance(actor.provenance);
        match actor.owner {
            Some(owner) => {
                self.bool(true);
                self.u64(owner.get());
            }
            None => self.bool(false),
        }
        self.u8(actor.team.get());
        self.u8(match actor.kind {
            ActorKind::Hero => 0,
            ActorKind::Minion => 1,
            ActorKind::Structure => 2,
            ActorKind::Objective => 3,
            ActorKind::Pet => 4,
            ActorKind::Projectile => 5,
        });
        self.point(actor.position);
        match actor.destination {
            Some(destination) => {
                self.bool(true);
                self.point(destination);
            }
            None => self.bool(false),
        }
        self.stats(*authored_stats);
        self.growth(*growth);
        self.bounty(*bounty);
        self.u32(*experience);
        self.u8(*unspent_ability_points);
        self.u16(*crit_attempts);
        self.len(damage_credits.len());
        for credit in damage_credits {
            self.u64(credit.source.get());
            self.u64(credit.last_tick);
        }
        // Derived caches are hashed alongside their inputs, per the rule the stat cache already follows.
        self.u8(*level);
        self.stats(actor.base_stats);
        self.len(actor.modifiers.len());
        for modifier in &actor.modifiers {
            self.modifier(*modifier);
        }
        self.len(actor.status_effects.len());
        for effect in &actor.status_effects {
            self.u64(effect.id.get());
            self.u64(effect.expires_at);
            self.u32(effect.shield_pool);
            self.u8(effect.controls.bits());
            self.len(effect.modifiers.len());
            for owned in &effect.modifiers {
                self.u64(owned.get());
            }
        }
        // The resolved caches are hashed as well as their inputs: a drifted cache would otherwise be
        // invisible to a digest comparison between two runtimes.
        self.u8(actor.control_mask.bits());
        self.stats(actor.stats);
        self.death_rule(actor.death_rule);
        self.u32(actor.health);
        self.u32(actor.resource);
        self.len(actor.abilities.len());
        for ability in &actor.abilities {
            self.u32(ability.id.get());
            self.u64(ability.ready_at);
            self.u8(ability.rank);
        }
        match actor.cast {
            Some(cast) => {
                self.bool(true);
                self.u32(cast.ability.get());
                self.cast_target(cast.target);
                self.u64(cast.resolves_at);
            }
            None => self.bool(false),
        }
        match actor.basic_attack {
            Some(attack) => {
                self.bool(true);
                self.u32(attack.spec.range_mm);
                self.u32(attack.spec.damage);
                self.damage_school(attack.spec.school);
                self.u16(attack.spec.windup_ticks);
                self.u32(attack.spec.cooldown_ticks);
                self.u64(attack.ready_at);
            }
            None => self.bool(false),
        }
        match actor.attack {
            Some(attack) => {
                self.bool(true);
                self.u64(attack.target.get());
                self.u64(attack.resolves_at);
            }
            None => self.bool(false),
        }
        match actor.respawn_at {
            Some(at_tick) => {
                self.bool(true);
                self.u64(at_tick);
            }
            None => self.bool(false),
        }
    }

    fn cast_target(&mut self, target: CastTarget) {
        match target {
            CastTarget::SelfActor => self.u8(0),
            CastTarget::Actor(actor) => {
                self.u8(1);
                self.u64(actor.get());
            }
            CastTarget::Point(point) => {
                self.u8(2);
                self.point(point);
            }
        }
    }

    fn actor_view(&mut self, actor: &ActorView) {
        self.u64(actor.id.get());
        match actor.owner {
            Some(owner) => {
                self.bool(true);
                self.u64(owner.get());
            }
            None => self.bool(false),
        }
        self.u8(actor.team.get());
        self.u8(match actor.kind {
            ActorKind::Hero => 0,
            ActorKind::Minion => 1,
            ActorKind::Structure => 2,
            ActorKind::Objective => 3,
            ActorKind::Pet => 4,
            ActorKind::Projectile => 5,
        });
        self.point(actor.position);
        match actor.destination {
            Some(destination) => {
                self.bool(true);
                self.point(destination);
            }
            None => self.bool(false),
        }
        self.u32(actor.health);
        self.u32(actor.max_health);
        self.u32(actor.resource);
        self.u32(actor.max_resource);
        self.u32(actor.move_speed_mm_per_tick);
        self.u16(actor.physical_reduction_bps);
        self.u16(actor.magic_reduction_bps);
        self.u8(actor.controls.bits());
        self.bool(actor.alive);
        match actor.cast {
            Some(cast) => {
                self.bool(true);
                self.u32(cast.ability.get());
                self.cast_target(cast.target);
                self.u64(cast.resolves_at);
            }
            None => self.bool(false),
        }
        match actor.basic_attack {
            Some(attack) => {
                self.bool(true);
                self.u64(attack.target.get());
                self.u64(attack.resolves_at);
            }
            None => self.bool(false),
        }
        match actor.basic_attack_ready_at {
            Some(ready_at) => {
                self.bool(true);
                self.u64(ready_at);
            }
            None => self.bool(false),
        }
        match actor.basic_attack_range_mm {
            Some(range) => {
                self.bool(true);
                self.u32(range);
            }
            None => self.bool(false),
        }
        match actor.respawn_at {
            Some(at_tick) => {
                self.bool(true);
                self.u64(at_tick);
            }
            None => self.bool(false),
        }
        self.u8(actor.level);
        self.u32(actor.experience);
        self.u8(actor.unspent_ability_points);
        self.len(actor.cooldowns.len());
        for cooldown in &actor.cooldowns {
            self.u32(cooldown.ability.get());
            self.u64(cooldown.ready_at);
            self.u8(cooldown.rank);
        }
    }

    fn rejection_reason(&mut self, reason: CommandRejectReason) {
        match reason {
            CommandRejectReason::MatchNotActive => self.u8(0),
            CommandRejectReason::StaleTick => self.u8(1),
            CommandRejectReason::TooFarAhead => self.u8(2),
            CommandRejectReason::DuplicateOrReorderedSequence => self.u8(3),
            CommandRejectReason::PlayerBudgetExceeded => self.u8(4),
            CommandRejectReason::PlayerNotRegistered => self.u8(5),
            CommandRejectReason::TickCommandBudgetExceeded => self.u8(6),
            CommandRejectReason::PlayerTickCommandBudgetExceeded => self.u8(7),
            CommandRejectReason::ActorNotFound => self.u8(8),
            CommandRejectReason::ActorNotOwned => self.u8(9),
            CommandRejectReason::ActorDead => self.u8(10),
            CommandRejectReason::DestinationOutOfBounds => self.u8(11),
            CommandRejectReason::ActorCasting => self.u8(12),
            CommandRejectReason::AbilityNotEquipped => self.u8(13),
            CommandRejectReason::AbilityOnCooldown { ready_at } => {
                self.u8(14);
                self.u64(ready_at);
            }
            CommandRejectReason::InsufficientResource {
                required,
                available,
            } => {
                self.u8(15);
                self.u32(required);
                self.u32(available);
            }
            CommandRejectReason::InvalidTarget => self.u8(16),
            CommandRejectReason::TargetNotFound => self.u8(17),
            CommandRejectReason::TargetDead => self.u8(18),
            CommandRejectReason::TargetRelationMismatch => self.u8(19),
            CommandRejectReason::TargetOutOfRange => self.u8(20),
            CommandRejectReason::BeyondMatchDuration => self.u8(21),
            CommandRejectReason::CastExceedsMatchDuration => self.u8(22),
            CommandRejectReason::ActorBusy => self.u8(23),
            CommandRejectReason::BasicAttackUnavailable => self.u8(24),
            CommandRejectReason::BasicAttackOnCooldown { ready_at } => {
                self.u8(25);
                self.u64(ready_at);
            }
            CommandRejectReason::AttackExceedsMatchDuration => self.u8(26),
            CommandRejectReason::ActorStunned => self.u8(27),
            CommandRejectReason::ActorRooted => self.u8(28),
            CommandRejectReason::ActorSilenced => self.u8(29),
            CommandRejectReason::ActorDisarmed => self.u8(30),
            CommandRejectReason::InvalidTargetForm => self.u8(31),
            CommandRejectReason::TargetPointOutOfBounds => self.u8(32),
            CommandRejectReason::ProjectileBudgetExceeded => self.u8(33),
            CommandRejectReason::NoAbilityPointAvailable => self.u8(34),
            CommandRejectReason::AbilityRankMaxed => self.u8(35),
            CommandRejectReason::AbilityRankLocked { unlocks_at_level } => {
                self.u8(36);
                self.u8(unlocks_at_level);
            }
            CommandRejectReason::ActorHasNoProgression => self.u8(37),
        }
    }

    fn submission_outcome(&mut self, outcome: Result<CommandReceipt, CommandRejection>) {
        match outcome {
            Ok(receipt) => {
                self.u8(0);
                self.u64(receipt.player.get());
                self.u32(receipt.sequence);
                self.u64(receipt.execute_at);
            }
            Err(rejection) => {
                self.u8(1);
                self.u64(rejection.player.get());
                self.u32(rejection.sequence);
                self.rejection_reason(rejection.reason);
            }
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the stable exhaustive event encoding is intentionally centralized"
    )]
    fn match_event(&mut self, event: MatchEvent) {
        self.u8(event_tag(event));
        match event {
            MatchEvent::MatchStarted => {}
            MatchEvent::CommandRejected {
                player,
                sequence,
                reason,
            } => {
                self.u64(player.get());
                self.u32(sequence);
                self.rejection_reason(reason);
            }
            MatchEvent::MoveStarted { actor, destination } => {
                self.u64(actor.get());
                self.point(destination);
            }
            // One shared payload: the discriminant emitted above is what tells these three apart.
            MatchEvent::MoveStopped { actor }
            | MatchEvent::ActorDespawned { actor }
            | MatchEvent::ActorSpawned { actor } => {
                self.u64(actor.get());
            }
            MatchEvent::CastStarted {
                source,
                ability,
                target,
                resolves_at,
            } => {
                self.u64(source.get());
                self.u32(ability.get());
                self.cast_target(target);
                self.u64(resolves_at);
            }
            MatchEvent::CastCancelled {
                source,
                ability,
                reason,
            } => {
                self.u64(source.get());
                self.u32(ability.get());
                self.rejection_reason(reason);
            }
            MatchEvent::CastResolved {
                source,
                ability,
                target,
            } => {
                self.u64(source.get());
                self.u32(ability.get());
                self.cast_target(target);
            }
            MatchEvent::DamageApplied {
                source,
                target,
                cause,
                school,
                amount,
                health_after,
            } => {
                self.u64(source.get());
                self.u64(target.get());
                self.damage_cause(cause);
                self.damage_school(school);
                self.u32(amount);
                self.u32(health_after);
            }
            MatchEvent::HealingApplied {
                source,
                target,
                ability,
                amount,
                health_after,
            } => {
                self.u64(source.get());
                self.u64(target.get());
                self.u32(ability.get());
                self.u32(amount);
                self.u32(health_after);
            }
            MatchEvent::ActorDied { actor, killer } => {
                self.u64(actor.get());
                self.u64(killer.get());
            }
            MatchEvent::MatchFinished { outcome, reason } => {
                self.outcome(outcome);
                self.end_reason(reason);
            }
            MatchEvent::BasicAttackStarted {
                source,
                target,
                resolves_at,
            } => {
                self.u64(source.get());
                self.u64(target.get());
                self.u64(resolves_at);
            }
            MatchEvent::BasicAttackCancelled {
                source,
                target,
                reason,
            } => {
                self.u64(source.get());
                self.u64(target.get());
                self.rejection_reason(reason);
            }
            MatchEvent::BasicAttackResolved { source, target } => {
                self.u64(source.get());
                self.u64(target.get());
            }
            MatchEvent::RespawnScheduled { actor, at_tick } => {
                self.u64(actor.get());
                self.u64(at_tick);
            }
            MatchEvent::RespawnCancelled {
                actor,
                at_tick,
                reason,
            } => {
                self.u64(actor.get());
                self.u64(at_tick);
                self.end_reason(reason);
            }
            MatchEvent::ActorRespawned { actor, position } => {
                self.u64(actor.get());
                self.point(position);
            }
            MatchEvent::InternalIntentRejected { actor, reason } => {
                self.u64(actor.get());
                self.rejection_reason(reason);
            }
            MatchEvent::StatusEffectExpired { actor, effect } => {
                self.u64(actor.get());
                self.u64(effect.get());
            }
            MatchEvent::ProjectileSpawned {
                projectile,
                source,
                ability,
                position,
                end,
            } => {
                self.u64(projectile.get());
                self.u64(source.get());
                self.u32(ability.get());
                self.point(position);
                self.point(end);
            }
            MatchEvent::ProjectileResolved {
                projectile,
                position,
                victim,
            } => {
                self.u64(projectile.get());
                self.point(position);
                match victim {
                    Some(victim) => {
                        self.bool(true);
                        self.u64(victim.get());
                    }
                    None => self.bool(false),
                }
            }
            MatchEvent::ProjectileCancelled { projectile, reason } => {
                self.u64(projectile.get());
                self.rejection_reason(reason);
            }
            MatchEvent::KillCredited {
                killer,
                victim,
                streak_after,
            } => {
                self.u64(killer.get());
                self.u64(victim.get());
                self.u16(streak_after);
            }
            MatchEvent::AssistCredited { actor, victim } => {
                self.u64(actor.get());
                self.u64(victim.get());
            }
            MatchEvent::GoldGranted {
                actor,
                amount,
                reason,
                total_after,
            } => {
                self.u64(actor.get());
                self.u32(amount);
                self.gold_reason(reason);
                self.u32(total_after);
            }
            MatchEvent::ExperienceGranted {
                actor,
                amount,
                total_after,
            } => {
                self.u64(actor.get());
                self.u32(amount);
                self.u32(total_after);
            }
            MatchEvent::HeroLevelUp {
                actor,
                level,
                unspent_ability_points,
            } => {
                self.u64(actor.get());
                self.u8(level);
                self.u8(unspent_ability_points);
            }
            MatchEvent::AbilityRankUp {
                actor,
                ability,
                rank,
            } => {
                self.u64(actor.get());
                self.u32(ability.get());
                self.u8(rank);
            }
            MatchEvent::ShieldAbsorbed {
                actor,
                effect,
                amount,
                remaining,
            } => {
                self.u64(actor.get());
                self.u64(effect.get());
                self.u32(amount);
                self.u32(remaining);
            }
            MatchEvent::LifestealApplied {
                actor,
                amount,
                health_after,
            } => {
                self.u64(actor.get());
                self.u32(amount);
                self.u32(health_after);
            }
        }
    }

    fn command(&mut self, command: PlayerCommand) {
        self.u64(command.player.get());
        self.u32(command.sequence);
        self.u64(command.execute_at);
        self.u64(command.actor.get());
        match command.kind {
            CommandKind::MoveTo { destination } => {
                self.u8(0);
                self.point(destination);
            }
            CommandKind::Stop => self.u8(1),
            CommandKind::Cast { ability, target } => {
                self.u8(2);
                self.u32(ability.get());
                self.cast_target(target);
            }
            CommandKind::BasicAttack { target } => {
                self.u8(3);
                self.u64(target.get());
            }
            CommandKind::UpgradeAbility { ability } => {
                self.u8(4);
                self.u32(ability.get());
            }
        }
    }

    fn damage_school(&mut self, school: DamageSchool) {
        self.u8(match school {
            DamageSchool::Physical => 0,
            DamageSchool::Magic => 1,
            DamageSchool::True => 2,
        });
    }

    fn damage_cause(&mut self, cause: DamageCause) {
        match cause {
            DamageCause::Ability(ability) => {
                self.u8(0);
                self.u32(ability.get());
            }
            DamageCause::BasicAttack => self.u8(1),
        }
    }

    fn death_rule(&mut self, rule: DeathRule) {
        match rule {
            DeathRule::StayDead => self.u8(0),
            DeathRule::Despawn => self.u8(1),
            DeathRule::Respawn { delay_ticks, at } => {
                self.u8(2);
                self.u32(delay_ticks);
                self.point(at);
            }
            DeathRule::ObjectiveVictory { winner } => {
                self.u8(3);
                self.u8(winner.get());
            }
        }
    }
}
