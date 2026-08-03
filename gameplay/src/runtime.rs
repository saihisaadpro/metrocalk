use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Display, Formatter};

use crate::model::{
    AbilityEffect, AbilityId, AbilitySpec, AbilityTargeting, ActorId, ActorKind, ActorProvenance,
    ActorSpawn, BasicAttackSpec, CombatStats, DamageSchool, DeathRule, DynamicActorProvenance,
    DynamicActorSpawn, MatchConfig, MatchEndReason, MatchOutcome, MatchPhase, ModifierId,
    ModifierOp, PlayerId, RuntimeError, StatKind, StatModifier, TeamId, Tick, Vec2Mm, BASIS_POINTS,
    MAX_ABILITY_DEFINITIONS,
};
use crate::protocol::{
    ActorIntent, ActorView, BasicAttackProgress, CastProgress, CastTarget, CommandKind,
    CommandReceipt, CommandRejectReason, CommandRejection, CooldownView, DamageCause, FrameDigest,
    MatchEvent, PlayerCommand, ServerFrame, WorldDigest,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AbilityState {
    pub(crate) id: AbilityId,
    pub(crate) ready_at: Tick,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActorState {
    pub(crate) id: ActorId,
    pub(crate) owner: Option<PlayerId>,
    pub(crate) team: TeamId,
    pub(crate) kind: ActorKind,
    pub(crate) position: Vec2Mm,
    pub(crate) destination: Option<Vec2Mm>,
    /// Authored stats, never mutated after spawn. The durable truth a modifier resolves against.
    pub(crate) base_stats: CombatStats,
    /// Applied modifiers, kept sorted by ID and unique. This plus `base_stats` is the whole stat state.
    pub(crate) modifiers: Vec<StatModifier>,
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
            base_stats: spawn.stats,
            modifiers: Vec::new(),
            stats: spawn.stats,
            health: spawn.stats.max_health,
            resource: spawn.stats.max_resource,
            abilities: abilities
                .into_iter()
                .map(|id| AbilityState { id, ready_at: 0 })
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
    fn resolve_stats(&mut self) {
        self.stats = self.base_stats.with_modifiers(&self.modifiers);
        self.health = self.health.min(self.stats.max_health);
        self.resource = self.resource.min(self.stats.max_resource);
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
            provenance: self.provenance,
            cooldowns: self
                .abilities
                .iter()
                .map(|ability| CooldownView {
                    ability: ability.id,
                    ready_at: ability.ready_at,
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
    pub(crate) player_roster: BTreeSet<PlayerId>,
    pub(crate) queued_commands: BTreeMap<Tick, Vec<PlayerCommand>>,
    pub(crate) last_submissions: BTreeMap<PlayerId, SubmissionRecord>,
    pub(crate) dirty: BTreeSet<ActorId>,
    pub(crate) removed: BTreeSet<ActorId>,
    pub(crate) pending_events: Vec<MatchEvent>,
    pub(crate) rejection_events_in_frame: u32,
    pub(crate) objective_claims: BTreeSet<TeamId>,
    pub(crate) next_dynamic_actor_id: Option<u64>,
    pub(crate) next_modifier_id: Option<u64>,
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
            player_roster: BTreeSet::new(),
            queued_commands: BTreeMap::new(),
            last_submissions: BTreeMap::new(),
            dirty: BTreeSet::new(),
            removed: BTreeSet::new(),
            pending_events: Vec::new(),
            rejection_events_in_frame: 0,
            objective_claims: BTreeSet::new(),
            next_dynamic_actor_id: Some(1),
            next_modifier_id: Some(1),
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
        if let Some(basic_attack) = spawn.basic_attack {
            basic_attack.validate()?;
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
        if matches!(self.phase, MatchPhase::Complete { .. })
            && (!self.queued_commands.is_empty()
                || self.actors.iter().any(|actor| {
                    actor.cast.is_some() || actor.attack.is_some() || actor.respawn_at.is_some()
                }))
        {
            return Err(Self::violation(
                None,
                "terminal state retains commands, pending actions, or respawn deadlines",
            ));
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
        self.resolve_due_casts()?;
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
        hash.bytes(b"metrocalk-gameplay-mob2-v6");
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

        hash.len(self.ability_specs.len());
        for spec in &self.ability_specs {
            hash.ability_spec(*spec);
        }

        hash.len(self.actors.len());
        for actor in &self.actors {
            hash.actor(actor);
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
        }
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

        let target_id = target_actor_id(source.id, target);
        let target_actor = self
            .actor_state(target_id)
            .ok_or(CommandRejectReason::TargetNotFound)?;
        if !target_actor.alive() {
            return Err(CommandRejectReason::TargetDead);
        }
        if !target_matches(spec.targeting, source, target_actor) {
            return Err(CommandRejectReason::TargetRelationMismatch);
        }
        let range_squared = u128::from(spec.range_mm).pow(2);
        if source.position.squared_distance(target_actor.position) > range_squared {
            return Err(CommandRejectReason::TargetOutOfRange);
        }
        Ok(())
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

    #[expect(
        clippy::too_many_lines,
        reason = "one visible admission phase makes cross-action settlement ordering auditable"
    )]
    fn resolve_due_casts(&mut self) -> Result<(), RuntimeError> {
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
                    let target = target_actor_id(source_id, cast.target);
                    self.pending_events.push(MatchEvent::CastResolved {
                        source: source_id,
                        ability: cast.ability,
                        target: cast.target,
                    });
                    match spec.effect {
                        AbilityEffect::Damage { amount, school } => {
                            effects.push(AdmittedHealthEffect::Damage {
                                source: source_id,
                                target,
                                cause: DamageCause::Ability(cast.ability),
                                amount,
                                school,
                            });
                        }
                        AbilityEffect::Heal { amount } => {
                            effects.push(AdmittedHealthEffect::Heal {
                                source: source_id,
                                target,
                                ability: cast.ability,
                                amount,
                            });
                        }
                    }
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
        self.settle_health_effects(effects)?;
        Ok(())
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
    ) -> Result<(), RuntimeError> {
        let mut by_target: BTreeMap<ActorId, Vec<AdmittedHealthEffect>> = BTreeMap::new();
        for effect in effects {
            let target = match effect {
                AdmittedHealthEffect::Damage { target, .. }
                | AdmittedHealthEffect::Heal { target, .. } => target,
            };
            by_target.entry(target).or_default().push(effect);
        }

        let mut deaths = Vec::new();
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
                let reduction = match school {
                    DamageSchool::Physical => {
                        self.actors[target_index].stats.physical_reduction_bps
                    }
                    DamageSchool::Magic => self.actors[target_index].stats.magic_reduction_bps,
                    DamageSchool::True => 0,
                };
                let applied = reduce_damage(amount, reduction).min(settled_health);
                let health_after = settled_health - applied;
                if settled_health > 0 && health_after == 0 {
                    killer = Some(source);
                }
                settled_health = health_after;
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
                self.pending_events.push(MatchEvent::ActorDied {
                    actor: target,
                    killer: killer
                        .expect("only admitted damage can settle positive health at zero"),
                });
                deaths.push(target);
            }
        }
        for actor in deaths {
            self.apply_death_rule(actor)?;
        }
        Ok(())
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
        let mut frame = ServerFrame {
            tick: self.tick,
            phase: self.phase,
            changed,
            removed: removed_ids.into_iter().collect(),
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

const fn target_actor_id(source: ActorId, target: CastTarget) -> ActorId {
    match target {
        CastTarget::SelfActor => source,
        CastTarget::Actor(id) => id,
    }
}

fn target_matches(targeting: AbilityTargeting, source: &ActorState, target: &ActorState) -> bool {
    match targeting {
        AbilityTargeting::SelfOnly => source.id == target.id,
        AbilityTargeting::Ally => source.team == target.team,
        AbilityTargeting::Enemy => source.team != target.team,
        AbilityTargeting::AnyUnit => true,
    }
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

fn integer_sqrt(value: u128) -> u128 {
    if value < 2 {
        return value;
    }
    let mut estimate = value;
    let mut next = estimate.div_ceil(2);
    while next < estimate {
        estimate = next;
        next = u128::midpoint(estimate, value / estimate);
    }
    estimate
}

fn integer_sqrt_ceil(value: u128) -> u128 {
    let floor = integer_sqrt(value);
    if floor * floor == value {
        floor
    } else {
        floor + 1
    }
}

fn digest_frame(frame: &ServerFrame) -> FrameDigest {
    let mut hash = StableHash::new();
    hash.bytes(b"metrocalk-gameplay-frame-v4");
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
        self.u32(config.max_checkpoint_bytes);
    }

    fn ability_spec(&mut self, spec: AbilitySpec) {
        self.u32(spec.id.get());
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

    fn actor(&mut self, actor: &ActorState) {
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
        self.stats(actor.base_stats);
        self.len(actor.modifiers.len());
        for modifier in &actor.modifiers {
            self.modifier(*modifier);
        }
        // The resolved cache is hashed as well as its inputs: a drifted cache would otherwise be invisible
        // to a digest comparison between two runtimes.
        self.stats(actor.stats);
        self.death_rule(actor.death_rule);
        self.u32(actor.health);
        self.u32(actor.resource);
        self.len(actor.abilities.len());
        for ability in &actor.abilities {
            self.u32(ability.id.get());
            self.u64(ability.ready_at);
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
        self.len(actor.cooldowns.len());
        for cooldown in &actor.cooldowns {
            self.u32(cooldown.ability.get());
            self.u64(cooldown.ready_at);
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
        match event {
            MatchEvent::MatchStarted => self.u8(0),
            MatchEvent::CommandRejected {
                player,
                sequence,
                reason,
            } => {
                self.u8(1);
                self.u64(player.get());
                self.u32(sequence);
                self.rejection_reason(reason);
            }
            MatchEvent::MoveStarted { actor, destination } => {
                self.u8(2);
                self.u64(actor.get());
                self.point(destination);
            }
            MatchEvent::MoveStopped { actor } => {
                self.u8(3);
                self.u64(actor.get());
            }
            MatchEvent::CastStarted {
                source,
                ability,
                target,
                resolves_at,
            } => {
                self.u8(4);
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
                self.u8(5);
                self.u64(source.get());
                self.u32(ability.get());
                self.rejection_reason(reason);
            }
            MatchEvent::CastResolved {
                source,
                ability,
                target,
            } => {
                self.u8(6);
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
                self.u8(7);
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
                self.u8(8);
                self.u64(source.get());
                self.u64(target.get());
                self.u32(ability.get());
                self.u32(amount);
                self.u32(health_after);
            }
            MatchEvent::ActorDied { actor, killer } => {
                self.u8(9);
                self.u64(actor.get());
                self.u64(killer.get());
            }
            MatchEvent::MatchFinished { outcome, reason } => {
                self.u8(10);
                self.outcome(outcome);
                self.end_reason(reason);
            }
            MatchEvent::BasicAttackStarted {
                source,
                target,
                resolves_at,
            } => {
                self.u8(11);
                self.u64(source.get());
                self.u64(target.get());
                self.u64(resolves_at);
            }
            MatchEvent::BasicAttackCancelled {
                source,
                target,
                reason,
            } => {
                self.u8(12);
                self.u64(source.get());
                self.u64(target.get());
                self.rejection_reason(reason);
            }
            MatchEvent::BasicAttackResolved { source, target } => {
                self.u8(13);
                self.u64(source.get());
                self.u64(target.get());
            }
            MatchEvent::ActorDespawned { actor } => {
                self.u8(14);
                self.u64(actor.get());
            }
            MatchEvent::RespawnScheduled { actor, at_tick } => {
                self.u8(15);
                self.u64(actor.get());
                self.u64(at_tick);
            }
            MatchEvent::RespawnCancelled {
                actor,
                at_tick,
                reason,
            } => {
                self.u8(19);
                self.u64(actor.get());
                self.u64(at_tick);
                self.end_reason(reason);
            }
            MatchEvent::ActorRespawned { actor, position } => {
                self.u8(16);
                self.u64(actor.get());
                self.point(position);
            }
            MatchEvent::InternalIntentRejected { actor, reason } => {
                self.u8(17);
                self.u64(actor.get());
                self.rejection_reason(reason);
            }
            MatchEvent::ActorSpawned { actor } => {
                self.u8(18);
                self.u64(actor.get());
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
