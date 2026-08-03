use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Display, Formatter};

use crate::{
    AbilityId, AbilityTargeting, ActorId, ActorIntent, ActorKind, ActorView, BasicAttackSpec,
    CastTarget, CombatStats, CommandKind, CommandReceipt, CommandRejection, CompiledLane,
    DeathRule, DynamicActorProvenance, DynamicActorSpawn, LaneError, LanePosition, LaneSpec,
    MatchPhase, MatchRuntime, PlayerCommand, PlayerId, RuntimeError, ServerFrame, TeamId, Tick,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WaveId(pub u32);

impl WaveId {
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Deterministic server-side controller for one actor. Owned player actors cannot also be bots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaneBotSpec {
    pub actor: ActorId,
    pub goal: LanePosition,
    pub aggro_range_mm: u32,
    pub target_priority: Vec<ActorKind>,
    pub cast_ability: Option<AbilityId>,
}

/// One semantic unit slot in an authored wave. Unit order is preserved within every spawn batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaveUnitSpec {
    pub progress_offset_mm: i64,
    pub stats: CombatStats,
    pub abilities: Vec<AbilityId>,
    pub basic_attack: Option<BasicAttackSpec>,
}

/// Bounded repeating wave schedule for the MOB-2 one-route recipe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaveSpec {
    pub id: WaveId,
    pub team: TeamId,
    pub spawn: LanePosition,
    pub goal: LanePosition,
    pub aggro_range_mm: u32,
    pub target_priority: Vec<ActorKind>,
    pub cast_ability: Option<AbilityId>,
    pub first_tick: Tick,
    pub interval_ticks: u32,
    pub max_alive: u32,
    pub units: Vec<WaveUnitSpec>,
}

/// Why one owned-player submission did not reach the composed match.
///
/// The lane layer adds exactly one refusal of its own and otherwise forwards the core taxonomy unchanged, so
/// the core's checkpoint-encoded reject reasons stay a closed set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneCommandRejection {
    Runtime(CommandRejection),
    OutsideCorridor { player: PlayerId, sequence: u32 },
}

impl Display for LaneCommandRejection {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime(rejection) => write!(formatter, "{rejection:?}"),
            Self::OutsideCorridor { player, sequence } => write!(
                formatter,
                "player {} sequence {sequence} targeted a destination outside the lane corridor",
                player.get()
            ),
        }
    }
}

impl Error for LaneCommandRejection {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaneMatchEvent {
    WaveSpawned { wave: WaveId, actors: Vec<ActorId> },
    WaveSkippedAliveCap { wave: WaveId, alive: u32, cap: u32 },
}

/// Digest for authoritative state owned by the lane controller in addition to the core runtime world.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LaneMatchDigest(pub u64);

/// Versioned composite evidence for one emitted core frame plus its lane-controller events and state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LaneFrameDigest(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OneLaneFrame {
    pub server: ServerFrame,
    pub lane_events: Vec<LaneMatchEvent>,
    pub lane_digest: LaneMatchDigest,
    pub lane_frame_digest: LaneFrameDigest,
}

/// Constant-memory deterministic run evidence for soak tests and the headless host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LaneMatchSummary {
    pub frames: u64,
    pub rolling_frame_digest: u64,
    pub final_phase: MatchPhase,
    pub final_world_digest: crate::WorldDigest,
    pub final_lane_digest: LaneMatchDigest,
    pub peak_actor_count: u32,
    pub peak_live_actor_count: u32,
    pub peak_queued_commands: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct WaveState {
    pub(crate) spec: WaveSpec,
    pub(crate) next_tick: Tick,
    pub(crate) live: BTreeSet<ActorId>,
}

struct DueWavePlan {
    index: usize,
    following_tick: Tick,
    batch: Option<Vec<DynamicActorSpawn>>,
}

/// MOB-2 controller that composes lane-space bot decisions and atomic waves around the shared runtime.
pub struct OneLaneMatch {
    pub(crate) runtime: MatchRuntime,
    pub(crate) lane: CompiledLane,
    pub(crate) bots: Vec<LaneBotSpec>,
    pub(crate) waves: Vec<WaveState>,
    pub(crate) definition_digest: u64,
}

impl OneLaneMatch {
    pub fn attach(
        runtime: MatchRuntime,
        lane: &LaneSpec,
        mut bots: Vec<LaneBotSpec>,
        mut waves: Vec<WaveSpec>,
    ) -> Result<Self, LaneMatchError> {
        if runtime.phase() != MatchPhase::Active {
            return Err(LaneMatchError::InvalidDefinition(
                "one-lane controller requires an active runtime",
            ));
        }
        if runtime.config().max_lanes != 1 {
            return Err(LaneMatchError::InvalidDefinition(
                "MOB-2 one-lane profile must declare exactly one lane",
            ));
        }
        let compiled = CompiledLane::compile(lane, runtime.config())?;
        for actor in &runtime.actors {
            compiled.project(actor.position)?;
            if let DeathRule::Respawn { at, .. } = actor.death_rule {
                compiled.project(at)?;
            }
        }
        bots.sort_unstable_by_key(|bot| bot.actor);
        if bots.windows(2).any(|pair| pair[0].actor == pair[1].actor) {
            return Err(LaneMatchError::InvalidDefinition(
                "duplicate bot actor identity",
            ));
        }
        for bot in &bots {
            validate_bot(&runtime, &compiled, bot)?;
        }

        if waves.len() > usize::from(runtime.config().max_wave_schedules) {
            return Err(LaneMatchError::InvalidDefinition(
                "wave schedule budget exceeded",
            ));
        }
        waves.sort_unstable_by_key(|wave| wave.id);
        if waves.windows(2).any(|pair| pair[0].id == pair[1].id) {
            return Err(LaneMatchError::InvalidDefinition("duplicate wave identity"));
        }
        let mut reserved = u32::try_from(runtime.actors().len()).unwrap_or(u32::MAX);
        for wave in &waves {
            validate_wave(&runtime, &compiled, wave)?;
            reserved =
                reserved
                    .checked_add(wave.max_alive)
                    .ok_or(LaneMatchError::InvalidDefinition(
                        "wave live-cap reservation overflowed",
                    ))?;
        }
        if reserved > runtime.config().max_actors {
            return Err(LaneMatchError::InvalidDefinition(
                "initial actors plus wave live caps exceed the actor budget",
            ));
        }

        let definition_digest = one_lane_definition_digest(&compiled, &bots, &waves);

        let waves = waves
            .into_iter()
            .map(|spec| WaveState {
                next_tick: spec.first_tick,
                spec,
                live: BTreeSet::new(),
            })
            .collect();
        Ok(Self {
            runtime,
            lane: compiled,
            bots,
            waves,
            definition_digest,
        })
    }

    #[must_use]
    pub const fn runtime(&self) -> &MatchRuntime {
        &self.runtime
    }

    /// Apply a server-owned status effect to an actor inside the composed match.
    ///
    /// Delegated deliberately rather than exposing `&mut MatchRuntime`: a caller with mutable access to the
    /// runtime could spawn or remove actors behind the controller's back and desync the wave bookkeeping
    /// that `capture_checkpoint` insists must stay reconstructible. Status effects are a server rule effect,
    /// which the controller already owns, so this widening is safe where a raw handle would not be.
    pub fn apply_status_effect(
        &mut self,
        actor: ActorId,
        duration_ticks: u32,
        controls: &[crate::ControlKind],
        modifiers: &[(crate::StatKind, crate::ModifierOp)],
    ) -> Result<crate::StatusEffectId, RuntimeError> {
        self.runtime
            .apply_status_effect(actor, duration_ticks, controls, modifiers)
    }

    /// Bounded count of commands still queued in the core runtime. Exposed so a caller can prove a rejected
    /// submission left no residue.
    #[must_use]
    pub fn queued_command_count(&self) -> usize {
        self.runtime.queued_command_count()
    }

    /// Rewrite one owned-player command into the exact envelope this controller will accept.
    ///
    /// A `MoveTo` destination is snapped to the nearest point ON the authored corridor; every other command
    /// passes through byte-identical. This is deliberately ANCHORING, not corridor-following: the core
    /// runtime integrates a destination in a straight line, so a player crossing a bend still cuts the
    /// corner. Making an owned actor track the polyline would need a per-tick server-issued movement channel,
    /// which the runtime correctly forbids today (server intents cannot drive owned actors).
    ///
    /// This is public because it is a prediction contract: a client can call it to compute the exact command
    /// the server will queue, without guessing at the corridor math.
    pub fn anchor_command(&self, command: PlayerCommand) -> Result<PlayerCommand, LaneError> {
        match command.kind {
            CommandKind::MoveTo { destination } => {
                let anchored = self.lane.point_at(self.lane.project(destination)?)?;
                Ok(PlayerCommand {
                    kind: CommandKind::MoveTo {
                        destination: anchored,
                    },
                    ..command
                })
            }
            CommandKind::Stop | CommandKind::Cast { .. } | CommandKind::BasicAttack { .. } => {
                Ok(command)
            }
        }
    }

    /// Submit one owned-player command into the composed match after attach.
    ///
    /// An off-corridor destination is refused HERE, before delegation, so a lane refusal never enters the
    /// core runtime's submission cache or its checkpoint encoding. That keeps the core's rejection taxonomy —
    /// which is tag-encoded byte-by-byte into the checkpoint format — untouched by a lane-layer concern.
    ///
    /// A lane-refused command leaves no trace, so the player's sequence cursor does not advance; a later
    /// higher sequence is still accepted, exactly as it would be had the command never been sent.
    pub fn submit(
        &mut self,
        command: PlayerCommand,
    ) -> Result<CommandReceipt, LaneCommandRejection> {
        let anchored =
            self.anchor_command(command)
                .map_err(|_| LaneCommandRejection::OutsideCorridor {
                    player: command.player,
                    sequence: command.sequence,
                })?;
        self.runtime
            .submit(anchored)
            .map_err(LaneCommandRejection::Runtime)
    }

    #[must_use]
    pub fn lane_digest(&self) -> LaneMatchDigest {
        let mut hash = LaneHash::new();
        hash.bytes(b"metrocalk-one-lane-mob2-v2");
        hash.u64(self.runtime.world_digest().0);
        hash.u64(self.definition_digest);
        hash.u16(self.lane.id().get());
        hash.u64(self.lane.length_mm());
        hash.u32(self.lane.half_width_mm());
        hash.len(self.lane.centerline().len());
        for point in self.lane.centerline() {
            hash.bytes(&point.x.to_le_bytes());
            hash.bytes(&point.y.to_le_bytes());
        }
        hash.len(self.bots.len());
        for bot in &self.bots {
            hash.bot(bot);
        }
        hash.len(self.waves.len());
        for wave in &self.waves {
            hash.wave(&wave.spec);
            hash.u64(wave.next_tick);
            hash.len(wave.live.len());
            for actor in &wave.live {
                hash.u64(actor.get());
            }
        }
        LaneMatchDigest(hash.finish())
    }

    pub fn step(&mut self) -> Result<OneLaneFrame, LaneMatchError> {
        if self.runtime.phase() != MatchPhase::Active {
            return Err(LaneMatchError::Runtime(RuntimeError::InvalidPhase(
                "one-lane match has completed",
            )));
        }
        self.prune_removed_actors();
        let snapshot = self.runtime.actors();
        let intents = self.derive_bot_intents(&snapshot)?;
        let next_tick = self
            .runtime
            .tick()
            .checked_add(1)
            .ok_or(RuntimeError::TickOverflow)?;
        let plans = self.prepare_due_waves(next_tick)?;
        let batches: Vec<Vec<DynamicActorSpawn>> =
            plans.iter().filter_map(|plan| plan.batch.clone()).collect();
        let (server, spawned) = self
            .runtime
            .step_with_intents_and_spawns(&intents, &batches)?;
        let lane_events = if matches!(server.phase, MatchPhase::Active) {
            self.commit_due_waves(plans, &spawned)
        } else {
            Vec::new()
        };
        self.prune_removed_actors();
        let lane_digest = self.lane_digest();
        let lane_frame_digest = compose_lane_frame_digest(&server, &lane_events, lane_digest);
        Ok(OneLaneFrame {
            server,
            lane_events,
            lane_digest,
            lane_frame_digest,
        })
    }

    /// Run without retaining frames. This is the checkpoint/soak observation seam, not a wall-clock pacer.
    pub fn run_streaming(&mut self, max_steps: u64) -> Result<LaneMatchSummary, LaneMatchError> {
        if max_steps == 0 {
            return Err(LaneMatchError::InvalidDefinition(
                "streaming step budget must be positive",
            ));
        }
        let mut rolling = LaneHash::new();
        rolling.bytes(b"metrocalk-one-lane-frame-stream-v2");
        let mut frames = 0_u64;
        let mut peak_actors = u32::try_from(self.runtime.actors().len()).unwrap_or(u32::MAX);
        let mut peak_live = u32::try_from(self.runtime.live_actor_count()).unwrap_or(u32::MAX);
        let mut peak_queued =
            u32::try_from(self.runtime.queued_command_count()).unwrap_or(u32::MAX);

        while self.runtime.phase() == MatchPhase::Active && frames < max_steps {
            let frame = self.step()?;
            self.runtime.check_invariants().map_err(|_| {
                LaneMatchError::InvalidDefinition("runtime invariant failed during streaming run")
            })?;
            rolling.u64(frame.lane_frame_digest.0);
            frames += 1;
            peak_actors =
                peak_actors.max(u32::try_from(self.runtime.actors().len()).unwrap_or(u32::MAX));
            peak_live =
                peak_live.max(u32::try_from(self.runtime.live_actor_count()).unwrap_or(u32::MAX));
            peak_queued = peak_queued
                .max(u32::try_from(self.runtime.queued_command_count()).unwrap_or(u32::MAX));
        }
        if self.runtime.phase() == MatchPhase::Active {
            return Err(LaneMatchError::StepLimitReached);
        }
        Ok(LaneMatchSummary {
            frames,
            rolling_frame_digest: rolling.finish(),
            final_phase: self.runtime.phase(),
            final_world_digest: self.runtime.world_digest(),
            final_lane_digest: self.lane_digest(),
            peak_actor_count: peak_actors,
            peak_live_actor_count: peak_live,
            peak_queued_commands: peak_queued,
        })
    }

    fn derive_bot_intents(
        &self,
        snapshot: &[ActorView],
    ) -> Result<Vec<ActorIntent>, LaneMatchError> {
        let actors: BTreeMap<ActorId, &ActorView> =
            snapshot.iter().map(|actor| (actor.id, actor)).collect();
        let positions: BTreeMap<ActorId, LanePosition> = snapshot
            .iter()
            .filter(|actor| actor.alive)
            .filter_map(|actor| {
                self.lane
                    .project(actor.position)
                    .ok()
                    .map(|position| (actor.id, position))
            })
            .collect();

        let next_tick = self
            .runtime
            .tick()
            .checked_add(1)
            .ok_or(RuntimeError::TickOverflow)?;
        let mut intents = Vec::with_capacity(self.bots.len());
        for bot in &self.bots {
            let Some(source) = actors.get(&bot.actor).copied() else {
                continue;
            };
            if !source.alive || source.cast.is_some() || source.basic_attack.is_some() {
                continue;
            }
            let source_lane =
                positions
                    .get(&source.id)
                    .copied()
                    .ok_or(LaneMatchError::InvalidDefinition(
                        "living bot is outside its cooked lane",
                    ))?;
            let target = select_target(source, source_lane, bot, snapshot, &positions);
            if let Some((target, target_lane, distance)) = target {
                if let Some(ability) = self.ready_bot_cast(source, bot, distance, next_tick) {
                    intents.push(ActorIntent {
                        actor: source.id,
                        kind: CommandKind::Cast {
                            ability,
                            target: CastTarget::Actor(target.id),
                        },
                    });
                    continue;
                }
                if source
                    .basic_attack_ready_at
                    .is_some_and(|ready| ready <= next_tick)
                    && source
                        .basic_attack_range_mm
                        .is_some_and(|range| distance <= u64::from(range))
                {
                    intents.push(ActorIntent {
                        actor: source.id,
                        kind: CommandKind::BasicAttack { target: target.id },
                    });
                    continue;
                }
                if let Some(intent) = self.move_intent(source, source_lane, target_lane)? {
                    intents.push(intent);
                }
            } else if let Some(intent) = self.move_intent(source, source_lane, bot.goal)? {
                intents.push(intent);
            }
        }
        Ok(intents)
    }

    fn ready_bot_cast(
        &self,
        source: &ActorView,
        bot: &LaneBotSpec,
        distance: u64,
        next_tick: Tick,
    ) -> Option<AbilityId> {
        let ability = bot.cast_ability?;
        let spec = self.runtime.ability(ability)?;
        let ready = source
            .cooldowns
            .iter()
            .find(|cooldown| cooldown.ability == ability)?
            .ready_at;
        (ready <= next_tick
            && source.resource >= spec.resource_cost
            && distance <= u64::from(spec.range_mm))
        .then_some(ability)
    }

    fn move_intent(
        &self,
        source: &ActorView,
        current: LanePosition,
        target: LanePosition,
    ) -> Result<Option<ActorIntent>, LaneMatchError> {
        if source.move_speed_mm_per_tick == 0 || current == target {
            return Ok(None);
        }
        let (next, _) =
            self.lane
                .advance_towards(current, target, source.move_speed_mm_per_tick)?;
        Ok(Some(ActorIntent {
            actor: source.id,
            kind: CommandKind::MoveTo {
                destination: self.lane.point_at(next)?,
            },
        }))
    }

    fn prepare_due_waves(&self, next_tick: Tick) -> Result<Vec<DueWavePlan>, LaneMatchError> {
        let mut plans = Vec::new();
        for (index, wave) in self.waves.iter().enumerate() {
            if wave.next_tick != next_tick {
                continue;
            }
            let alive = u32::try_from(wave.live.len()).unwrap_or(u32::MAX);
            let unit_count = u32::try_from(wave.spec.units.len()).unwrap_or(u32::MAX);
            let following_tick = wave
                .next_tick
                .checked_add(u64::from(wave.spec.interval_ticks))
                .ok_or(RuntimeError::TickOverflow)?;
            if alive.saturating_add(unit_count) > wave.spec.max_alive {
                plans.push(DueWavePlan {
                    index,
                    following_tick,
                    batch: None,
                });
                continue;
            }
            let mut spawns = Vec::with_capacity(wave.spec.units.len());
            for (slot, unit) in wave.spec.units.iter().enumerate() {
                let progress = add_signed_progress(
                    wave.spec.spawn.progress_mm,
                    unit.progress_offset_mm,
                    self.lane.length_mm(),
                )?;
                // The authored slot always fits: `validate_wave` bounds the batch by the u16
                // units-per-wave budget, so this is a total conversion, not a silent clamp.
                let unit_slot = u16::try_from(slot).map_err(|_| {
                    LaneMatchError::InvalidDefinition(
                        "wave unit slot exceeds the authored units-per-wave budget",
                    )
                })?;
                spawns.push(DynamicActorSpawn {
                    provenance: DynamicActorProvenance::Wave {
                        wave_id: wave.spec.id.get(),
                        unit_slot,
                    },
                    team: wave.spec.team,
                    kind: ActorKind::Minion,
                    position: self.lane.point_at(LanePosition {
                        lane: self.lane.id(),
                        progress_mm: progress,
                    })?,
                    stats: unit.stats,
                    abilities: unit.abilities.clone(),
                    basic_attack: unit.basic_attack,
                    death_rule: DeathRule::Despawn,
                });
            }
            plans.push(DueWavePlan {
                index,
                following_tick,
                batch: Some(spawns),
            });
        }
        Ok(plans)
    }

    fn commit_due_waves(
        &mut self,
        plans: Vec<DueWavePlan>,
        spawned: &[Vec<ActorId>],
    ) -> Vec<LaneMatchEvent> {
        let mut spawned_cursor = 0;
        let mut events = Vec::with_capacity(plans.len());
        for plan in plans {
            let wave = &mut self.waves[plan.index];
            wave.next_tick = plan.following_tick;
            if plan.batch.is_some() {
                let ids = spawned[spawned_cursor].clone();
                spawned_cursor += 1;
                for id in &ids {
                    wave.live.insert(*id);
                    self.bots.push(LaneBotSpec {
                        actor: *id,
                        goal: wave.spec.goal,
                        aggro_range_mm: wave.spec.aggro_range_mm,
                        target_priority: wave.spec.target_priority.clone(),
                        cast_ability: wave.spec.cast_ability,
                    });
                }
                events.push(LaneMatchEvent::WaveSpawned {
                    wave: wave.spec.id,
                    actors: ids,
                });
            } else {
                events.push(LaneMatchEvent::WaveSkippedAliveCap {
                    wave: wave.spec.id,
                    alive: u32::try_from(wave.live.len()).unwrap_or(u32::MAX),
                    cap: wave.spec.max_alive,
                });
            }
        }
        self.bots.sort_unstable_by_key(|bot| bot.actor);
        events
    }

    fn prune_removed_actors(&mut self) {
        let existing: BTreeSet<ActorId> = self
            .runtime
            .actors()
            .into_iter()
            .map(|actor| actor.id)
            .collect();
        self.bots.retain(|bot| existing.contains(&bot.actor));
        for wave in &mut self.waves {
            wave.live.retain(|actor| existing.contains(actor));
        }
    }
}

pub(crate) fn one_lane_definition_digest(
    lane: &CompiledLane,
    bots: &[LaneBotSpec],
    waves: &[WaveSpec],
) -> u64 {
    let mut hash = LaneHash::new();
    hash.bytes(b"metrocalk-one-lane-definitions-v1");
    hash.u16(lane.id().get());
    hash.u64(lane.length_mm());
    hash.u32(lane.half_width_mm());
    hash.len(lane.centerline().len());
    for point in lane.centerline() {
        hash.bytes(&point.x.to_le_bytes());
        hash.bytes(&point.y.to_le_bytes());
    }
    hash.len(bots.len());
    for bot in bots {
        hash.bot(bot);
    }
    hash.len(waves.len());
    for wave in waves {
        hash.wave(wave);
    }
    hash.finish()
}

fn compose_lane_frame_digest(
    server: &ServerFrame,
    lane_events: &[LaneMatchEvent],
    lane_digest: LaneMatchDigest,
) -> LaneFrameDigest {
    let mut hash = LaneHash::new();
    hash.bytes(b"metrocalk-one-lane-composite-frame-v1");
    hash.u64(server.frame_digest.0);
    hash.u64(lane_digest.0);
    hash.len(lane_events.len());
    for event in lane_events {
        hash.lane_event(event);
    }
    LaneFrameDigest(hash.finish())
}

pub(crate) fn validate_bot(
    runtime: &MatchRuntime,
    lane: &CompiledLane,
    bot: &LaneBotSpec,
) -> Result<(), LaneMatchError> {
    let actor = runtime
        .actor(bot.actor)
        .ok_or(LaneMatchError::InvalidDefinition(
            "bot actor does not exist",
        ))?;
    if actor.owner.is_some() {
        return Err(LaneMatchError::InvalidDefinition(
            "an owned actor cannot also have a bot controller",
        ));
    }
    lane.project(actor.position)?;
    validate_bot_policy(
        runtime,
        lane,
        bot.goal,
        bot.aggro_range_mm,
        &bot.target_priority,
        bot.cast_ability,
    )?;
    if bot.cast_ability.is_some_and(|ability| {
        !actor
            .cooldowns
            .iter()
            .any(|cooldown| cooldown.ability == ability)
    }) {
        return Err(LaneMatchError::InvalidDefinition(
            "bot cast ability is not equipped by its actor",
        ));
    }
    Ok(())
}

pub(crate) fn validate_bot_policy(
    runtime: &MatchRuntime,
    lane: &CompiledLane,
    goal: LanePosition,
    aggro_range_mm: u32,
    target_priority: &[ActorKind],
    cast_ability: Option<AbilityId>,
) -> Result<(), LaneMatchError> {
    lane.point_at(goal)?;
    if aggro_range_mm == 0 || target_priority.is_empty() {
        return Err(LaneMatchError::InvalidDefinition(
            "bot aggro and target priority must be non-empty",
        ));
    }
    for (index, kind) in target_priority.iter().enumerate() {
        if target_priority[..index].contains(kind) {
            return Err(LaneMatchError::InvalidDefinition(
                "bot target priority contains duplicates",
            ));
        }
    }
    if let Some(ability) = cast_ability {
        let spec = runtime
            .ability(ability)
            .ok_or(LaneMatchError::InvalidDefinition(
                "bot ability is not registered",
            ))?;
        if !matches!(
            spec.targeting,
            AbilityTargeting::Enemy | AbilityTargeting::AnyUnit
        ) {
            return Err(LaneMatchError::InvalidDefinition(
                "bot cast ability must accept an enemy target",
            ));
        }
    }
    Ok(())
}

fn validate_wave(
    runtime: &MatchRuntime,
    lane: &CompiledLane,
    wave: &WaveSpec,
) -> Result<(), LaneMatchError> {
    validate_bot_policy(
        runtime,
        lane,
        wave.goal,
        wave.aggro_range_mm,
        &wave.target_priority,
        wave.cast_ability,
    )?;
    lane.point_at(wave.spawn)?;
    if wave.first_tick <= runtime.tick() || wave.interval_ticks == 0 || wave.max_alive == 0 {
        return Err(LaneMatchError::InvalidDefinition(
            "wave ticks, interval, and live cap must be positive and future",
        ));
    }
    if wave.units.is_empty()
        || wave.units.len() > usize::from(runtime.config().max_units_per_wave)
        || u32::try_from(wave.units.len()).unwrap_or(u32::MAX) > wave.max_alive
    {
        return Err(LaneMatchError::InvalidDefinition(
            "wave unit batch is empty or exceeds its configured cap",
        ));
    }
    for unit in &wave.units {
        unit.stats.validate()?;
        if let Some(attack) = unit.basic_attack {
            attack.validate()?;
        }
        add_signed_progress(
            wave.spawn.progress_mm,
            unit.progress_offset_mm,
            lane.length_mm(),
        )?;
        if unit.abilities.len()
            > usize::try_from(runtime.config().max_abilities_per_actor).unwrap_or(usize::MAX)
        {
            return Err(LaneMatchError::InvalidDefinition(
                "wave unit ability budget exceeded",
            ));
        }
        let mut abilities = unit.abilities.clone();
        abilities.sort_unstable();
        if abilities.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(LaneMatchError::InvalidDefinition(
                "wave unit contains duplicate abilities",
            ));
        }
        for ability in &abilities {
            if runtime.ability(*ability).is_none() {
                return Err(LaneMatchError::InvalidDefinition(
                    "wave unit references an unknown ability",
                ));
            }
        }
        if wave
            .cast_ability
            .is_some_and(|ability| !abilities.contains(&ability))
        {
            return Err(LaneMatchError::InvalidDefinition(
                "wave cast ability must be equipped by every unit",
            ));
        }
    }
    Ok(())
}

fn select_target<'a>(
    source: &ActorView,
    source_lane: LanePosition,
    bot: &LaneBotSpec,
    actors: &'a [ActorView],
    positions: &BTreeMap<ActorId, LanePosition>,
) -> Option<(&'a ActorView, LanePosition, u64)> {
    actors
        .iter()
        .filter(|target| target.alive && target.team != source.team)
        .filter_map(|target| {
            let priority = bot
                .target_priority
                .iter()
                .position(|kind| *kind == target.kind)?;
            let lane = *positions.get(&target.id)?;
            let distance = source_lane.progress_mm.abs_diff(lane.progress_mm);
            (distance <= u64::from(bot.aggro_range_mm))
                .then_some((priority, distance, target.id, target, lane))
        })
        .min_by_key(|(priority, distance, id, _, _)| (*priority, *distance, *id))
        .map(|(_, distance, _, target, lane)| (target, lane, distance))
}

pub(crate) fn add_signed_progress(
    base: u64,
    offset: i64,
    lane_length: u64,
) -> Result<u64, LaneMatchError> {
    let progress = if offset >= 0 {
        base.checked_add(offset.unsigned_abs())
    } else {
        base.checked_sub(offset.unsigned_abs())
    }
    .ok_or(LaneMatchError::InvalidDefinition(
        "wave progress offset overflows the lane",
    ))?;
    if progress > lane_length {
        return Err(LaneMatchError::InvalidDefinition(
            "wave progress offset is outside the lane",
        ));
    }
    Ok(progress)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaneMatchError {
    Runtime(RuntimeError),
    Lane(LaneError),
    InvalidDefinition(&'static str),
    StepLimitReached,
}

impl Display for LaneMatchError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime(error) => Display::fmt(error, formatter),
            Self::Lane(error) => Display::fmt(error, formatter),
            Self::InvalidDefinition(reason) => {
                write!(formatter, "invalid one-lane match: {reason}")
            }
            Self::StepLimitReached => {
                formatter.write_str("one-lane streaming run reached its explicit step limit")
            }
        }
    }
}

impl Error for LaneMatchError {}

impl From<RuntimeError> for LaneMatchError {
    fn from(value: RuntimeError) -> Self {
        Self::Runtime(value)
    }
}

impl From<LaneError> for LaneMatchError {
    fn from(value: LaneError) -> Self {
        Self::Lane(value)
    }
}

struct LaneHash(u64);

impl LaneHash {
    const fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    const fn finish(self) -> u64 {
        self.0
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn u8(&mut self, value: u8) {
        self.bytes(&value.to_le_bytes());
    }

    fn u16(&mut self, value: u16) {
        self.bytes(&value.to_le_bytes());
    }

    fn u32(&mut self, value: u32) {
        self.bytes(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn len(&mut self, value: usize) {
        self.u64(u64::try_from(value).unwrap_or(u64::MAX));
    }

    fn bot(&mut self, bot: &LaneBotSpec) {
        self.u64(bot.actor.get());
        self.u16(bot.goal.lane.get());
        self.u64(bot.goal.progress_mm);
        self.u32(bot.aggro_range_mm);
        self.len(bot.target_priority.len());
        for kind in &bot.target_priority {
            self.u8(actor_kind_tag(*kind));
        }
        match bot.cast_ability {
            Some(ability) => {
                self.u8(1);
                self.u32(ability.get());
            }
            None => self.u8(0),
        }
    }

    fn wave(&mut self, wave: &WaveSpec) {
        self.u32(wave.id.get());
        self.u8(wave.team.get());
        self.u16(wave.spawn.lane.get());
        self.u64(wave.spawn.progress_mm);
        self.u16(wave.goal.lane.get());
        self.u64(wave.goal.progress_mm);
        self.u32(wave.aggro_range_mm);
        self.len(wave.target_priority.len());
        for kind in &wave.target_priority {
            self.u8(actor_kind_tag(*kind));
        }
        match wave.cast_ability {
            Some(ability) => {
                self.u8(1);
                self.u32(ability.get());
            }
            None => self.u8(0),
        }
        self.u64(wave.first_tick);
        self.u32(wave.interval_ticks);
        self.u32(wave.max_alive);
        self.len(wave.units.len());
        for unit in &wave.units {
            self.bytes(&unit.progress_offset_mm.to_le_bytes());
            self.u32(unit.stats.max_health);
            self.u32(unit.stats.max_resource);
            self.u32(unit.stats.move_speed_mm_per_tick);
            self.u16(unit.stats.physical_reduction_bps);
            self.u16(unit.stats.magic_reduction_bps);
            self.len(unit.abilities.len());
            for ability in &unit.abilities {
                self.u32(ability.get());
            }
            match unit.basic_attack {
                Some(attack) => {
                    self.u8(1);
                    self.u32(attack.range_mm);
                    self.u32(attack.damage);
                    self.u8(match attack.school {
                        crate::DamageSchool::Physical => 0,
                        crate::DamageSchool::Magic => 1,
                        crate::DamageSchool::True => 2,
                    });
                    self.u16(attack.windup_ticks);
                    self.u32(attack.cooldown_ticks);
                }
                None => self.u8(0),
            }
        }
    }

    fn lane_event(&mut self, event: &LaneMatchEvent) {
        match event {
            LaneMatchEvent::WaveSpawned { wave, actors } => {
                self.u8(0);
                self.u32(wave.get());
                self.len(actors.len());
                for actor in actors {
                    self.u64(actor.get());
                }
            }
            LaneMatchEvent::WaveSkippedAliveCap { wave, alive, cap } => {
                self.u8(1);
                self.u32(wave.get());
                self.u32(*alive);
                self.u32(*cap);
            }
        }
    }
}

const fn actor_kind_tag(kind: ActorKind) -> u8 {
    match kind {
        ActorKind::Hero => 0,
        ActorKind::Minion => 1,
        ActorKind::Structure => 2,
        ActorKind::Objective => 3,
        ActorKind::Pet => 4,
        ActorKind::Projectile => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AbilityEffect, AbilitySpec, ActorSpawn, DamageSchool, MatchConfig, MatchEndReason,
        MatchEvent, MatchOutcome, PlayerCommand, PlayerId, RectMm, Vec2Mm,
    };

    const fn stats(health: u32, speed: u32) -> CombatStats {
        CombatStats {
            max_health: health,
            max_resource: 0,
            move_speed_mm_per_tick: speed,
            physical_reduction_bps: 0,
            magic_reduction_bps: 0,
        }
    }

    fn lane() -> LaneSpec {
        LaneSpec {
            id: crate::LaneId(0),
            centerline: vec![Vec2Mm::new(0, 0), Vec2Mm::new(10_000, 0)],
            half_width_mm: 500,
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the test fixture keeps each authored actor property explicit at call sites"
    )]
    fn spawn(
        id: u64,
        team: u8,
        kind: ActorKind,
        x: i32,
        health: u32,
        speed: u32,
        attack: Option<BasicAttackSpec>,
        death_rule: DeathRule,
    ) -> ActorSpawn {
        ActorSpawn {
            id: ActorId(id),
            owner: None,
            team: TeamId(team),
            kind,
            position: Vec2Mm::new(x, 0),
            stats: stats(health, speed),
            abilities: vec![],
            basic_attack: attack,
            death_rule,
        }
    }

    fn objective_match() -> OneLaneMatch {
        let config = MatchConfig {
            map_bounds: RectMm::new(Vec2Mm::new(-1_000, -1_000), Vec2Mm::new(11_000, 1_000)),
            max_match_ticks: 100,
            ..MatchConfig::default()
        };
        let attack = BasicAttackSpec {
            range_mm: 1_000,
            damage: 100,
            school: DamageSchool::True,
            windup_ticks: 0,
            cooldown_ticks: 5,
        };
        let mut runtime = MatchRuntime::new(config, 0x2a).unwrap();
        runtime
            .spawn_actor(&spawn(
                10,
                0,
                ActorKind::Structure,
                0,
                100,
                0,
                None,
                DeathRule::ObjectiveVictory { winner: TeamId(1) },
            ))
            .unwrap();
        runtime
            .spawn_actor(&spawn(
                20,
                1,
                ActorKind::Structure,
                10_000,
                100,
                0,
                None,
                DeathRule::ObjectiveVictory { winner: TeamId(0) },
            ))
            .unwrap();
        runtime
            .spawn_actor(&spawn(
                101,
                0,
                ActorKind::Hero,
                1_000,
                500,
                1_000,
                Some(attack),
                DeathRule::StayDead,
            ))
            .unwrap();
        runtime
            .spawn_actor(&spawn(
                202,
                1,
                ActorKind::Hero,
                9_000,
                500,
                1_000,
                Some(attack),
                DeathRule::StayDead,
            ))
            .unwrap();
        runtime.start().unwrap();
        OneLaneMatch::attach(
            runtime,
            &lane(),
            vec![
                LaneBotSpec {
                    actor: ActorId(202),
                    goal: LanePosition {
                        lane: crate::LaneId(0),
                        progress_mm: 0,
                    },
                    aggro_range_mm: 10_000,
                    target_priority: vec![ActorKind::Structure],
                    cast_ability: None,
                },
                LaneBotSpec {
                    actor: ActorId(101),
                    goal: LanePosition {
                        lane: crate::LaneId(0),
                        progress_mm: 10_000,
                    },
                    aggro_range_mm: 10_000,
                    target_priority: vec![ActorKind::Structure],
                    cast_ability: None,
                },
            ],
            vec![],
        )
        .unwrap()
    }

    const HUMAN: PlayerId = PlayerId(1);
    const HUMAN_HERO: ActorId = ActorId(101);

    /// The same objective fixture, except hero 101 belongs to a human player instead of a bot. `validate_bot`
    /// forbids an owned actor from also carrying a bot controller, so the two populations are disjoint by
    /// construction and this is the only legal shape for a mixed match.
    fn mixed_match() -> OneLaneMatch {
        let config = MatchConfig {
            map_bounds: RectMm::new(Vec2Mm::new(-1_000, -1_000), Vec2Mm::new(11_000, 1_000)),
            max_match_ticks: 100,
            ..MatchConfig::default()
        };
        let attack = BasicAttackSpec {
            range_mm: 1_000,
            damage: 100,
            school: DamageSchool::True,
            windup_ticks: 0,
            cooldown_ticks: 5,
        };
        let mut runtime = MatchRuntime::new(config, 0x2a).unwrap();
        runtime
            .spawn_actor(&spawn(
                10,
                0,
                ActorKind::Structure,
                0,
                100,
                0,
                None,
                DeathRule::ObjectiveVictory { winner: TeamId(1) },
            ))
            .unwrap();
        runtime
            .spawn_actor(&spawn(
                20,
                1,
                ActorKind::Structure,
                10_000,
                100,
                0,
                None,
                DeathRule::ObjectiveVictory { winner: TeamId(0) },
            ))
            .unwrap();
        runtime
            .spawn_actor(&ActorSpawn {
                owner: Some(HUMAN),
                ..spawn(
                    101,
                    0,
                    ActorKind::Hero,
                    1_000,
                    500,
                    1_000,
                    Some(attack),
                    DeathRule::StayDead,
                )
            })
            .unwrap();
        runtime
            .spawn_actor(&spawn(
                202,
                1,
                ActorKind::Hero,
                9_000,
                500,
                1_000,
                Some(attack),
                DeathRule::StayDead,
            ))
            .unwrap();
        runtime.start().unwrap();
        OneLaneMatch::attach(
            runtime,
            &lane(),
            vec![LaneBotSpec {
                actor: ActorId(202),
                goal: LanePosition {
                    lane: crate::LaneId(0),
                    progress_mm: 0,
                },
                aggro_range_mm: 10_000,
                target_priority: vec![ActorKind::Structure],
                cast_ability: None,
            }],
            vec![],
        )
        .unwrap()
    }

    fn move_to(sequence: u32, execute_at: Tick, destination: Vec2Mm) -> PlayerCommand {
        PlayerCommand {
            player: HUMAN,
            sequence,
            execute_at,
            actor: HUMAN_HERO,
            kind: CommandKind::MoveTo { destination },
        }
    }

    #[test]
    fn owned_player_command_reaches_the_composed_match_after_attach() {
        let mut game = mixed_match();
        // Submitted AFTER attach — the ingress that did not exist before this slice.
        let receipt = game.submit(move_to(1, 1, Vec2Mm::new(5_000, 0))).unwrap();
        assert_eq!(receipt.execute_at, 1);
        assert_eq!(game.queued_command_count(), 1);

        let start = game.runtime().actor(HUMAN_HERO).unwrap().position.x;
        let frame = game.step().unwrap();
        let moved = game.runtime().actor(HUMAN_HERO).unwrap().position.x;
        assert!(
            moved > start,
            "the human hero should advance toward its commanded destination"
        );
        assert!(frame.server.events.iter().any(
            |event| matches!(event, MatchEvent::MoveStarted { actor, .. } if *actor == HUMAN_HERO)
        ));
    }

    #[test]
    fn off_corridor_destination_is_rejected_without_side_effects() {
        let mut game = mixed_match();
        let before_digest = game.lane_digest();

        // 5 000 mm off a 500 mm-half-width corridor.
        let rejection = game
            .submit(move_to(1, 1, Vec2Mm::new(5_000, 5_000)))
            .unwrap_err();
        assert_eq!(
            rejection,
            LaneCommandRejection::OutsideCorridor {
                player: HUMAN,
                sequence: 1,
            }
        );
        assert_eq!(game.queued_command_count(), 0);
        assert_eq!(game.lane_digest(), before_digest);

        // The refusal left no cursor behind, so an ordinary later sequence is still accepted.
        assert!(game.submit(move_to(2, 1, Vec2Mm::new(5_000, 0))).is_ok());
    }

    #[test]
    fn anchoring_is_idempotent_and_preserves_the_retry_cache() {
        let mut game = mixed_match();
        // Inside the corridor but off the centreline, so anchoring genuinely rewrites the envelope.
        let original = move_to(1, 1, Vec2Mm::new(5_000, 300));
        let anchored = game.anchor_command(original).unwrap();
        assert_ne!(
            anchored.kind, original.kind,
            "this destination must actually be rewritten or the test proves nothing"
        );

        let first = game.submit(original).unwrap();
        assert_eq!(game.queued_command_count(), 1);
        // A retrying client resends the ORIGINAL, but the cache holds the ANCHORED envelope. The retry is
        // only idempotent because anchoring is deterministic and applied before the cache comparison.
        let retry = game.submit(original).unwrap();
        assert_eq!(first, retry);
        assert_eq!(
            game.queued_command_count(),
            1,
            "an exact retry must not queue a second command"
        );
    }

    #[test]
    fn mixed_human_and_bot_lane_run_is_reproducible() {
        fn run() -> (Vec<LaneFrameDigest>, MatchPhase) {
            let mut game = mixed_match();
            let mut digests = Vec::new();
            // The scripted human commands are interleaved with bot decisions until the match ends on its own
            // terms; the run is driven by the real lifecycle rather than a fixed tick count.
            for tick in 0..12_u64 {
                if game.runtime().phase() != MatchPhase::Active {
                    break;
                }
                if tick == 0 {
                    game.submit(move_to(1, 1, Vec2Mm::new(9_000, 200))).unwrap();
                }
                if tick == 2 {
                    game.submit(move_to(2, 4, Vec2Mm::new(2_000, -100)))
                        .unwrap();
                }
                digests.push(game.step().unwrap().lane_frame_digest);
            }
            (digests, game.runtime().phase())
        }

        let (first, first_phase) = run();
        let (second, second_phase) = run();
        assert!(
            first.len() >= 3,
            "the scripted run must outlive both human commands"
        );
        assert_eq!(first, second);
        assert_eq!(first_phase, second_phase);
        assert!(
            matches!(first_phase, MatchPhase::Complete { .. }),
            "a mixed human/bot match must still reach a legal terminal state"
        );
    }

    #[test]
    fn stat_modifiers_hold_lane_determinism_and_survive_the_composite_checkpoint() {
        // GP-07 landed in the core runtime; this proves the COMPOSED match still reproduces bit-identically
        // with modifier state live, and that the one-lane envelope carries it. Without this, modifiers are
        // only ever exercised by bare-runtime tests.
        const CONTENT: crate::ContentId = crate::ContentId::new([0x9e; 32]);

        fn run_with_modifiers() -> (Vec<LaneFrameDigest>, OneLaneMatch) {
            let mut game = mixed_match();
            game.runtime
                .apply_stat_modifier(
                    HUMAN_HERO,
                    crate::StatKind::MoveSpeed,
                    crate::ModifierOp::PercentAdd { bps: 2_500 },
                )
                .unwrap();
            game.runtime
                .apply_stat_modifier(
                    ActorId(202),
                    crate::StatKind::MaxHealth,
                    crate::ModifierOp::Flat { magnitude: 120 },
                )
                .unwrap();
            let mut digests = Vec::new();
            for _ in 0..4 {
                digests.push(game.step().unwrap().lane_frame_digest);
            }
            (digests, game)
        }

        let bots = vec![LaneBotSpec {
            actor: ActorId(202),
            goal: LanePosition {
                lane: crate::LaneId(0),
                progress_mm: 0,
            },
            aggro_range_mm: 10_000,
            target_priority: vec![ActorKind::Structure],
            cast_ability: None,
        }];
        let (first, live) = run_with_modifiers();
        let (second, _) = run_with_modifiers();
        assert_eq!(
            first, second,
            "modifier state must not break lane determinism"
        );
        assert_eq!(
            live.runtime()
                .actor(HUMAN_HERO)
                .unwrap()
                .move_speed_mm_per_tick,
            1_250
        );

        let checkpoint = live
            .capture_checkpoint(CONTENT, &lane(), &bots, &[])
            .unwrap();
        let restored =
            OneLaneMatch::restore_checkpoint(&checkpoint, CONTENT, &lane(), &bots, &[]).unwrap();
        assert_eq!(restored.lane_digest(), live.lane_digest());
        assert_eq!(
            restored.runtime().modifiers(HUMAN_HERO).unwrap(),
            live.runtime().modifiers(HUMAN_HERO).unwrap()
        );
        assert_eq!(
            restored
                .runtime()
                .actor(HUMAN_HERO)
                .unwrap()
                .move_speed_mm_per_tick,
            1_250
        );
    }

    #[test]
    fn owned_player_run_survives_checkpoint_round_trip() {
        const CONTENT: crate::ContentId = crate::ContentId::new([0x7c; 32]);
        let bots = vec![LaneBotSpec {
            actor: ActorId(202),
            goal: LanePosition {
                lane: crate::LaneId(0),
                progress_mm: 0,
            },
            aggro_range_mm: 10_000,
            target_priority: vec![ActorKind::Structure],
            cast_ability: None,
        }];

        let mut uninterrupted = mixed_match();
        uninterrupted
            .submit(move_to(1, 1, Vec2Mm::new(6_000, 0)))
            .unwrap();
        for _ in 0..3 {
            uninterrupted.step().unwrap();
        }

        // No new controller state was introduced by this slice, so the existing envelope must already carry a
        // commanded human. This test proves that rather than assuming it.
        let checkpoint = uninterrupted
            .capture_checkpoint(CONTENT, &lane(), &bots, &[])
            .unwrap();
        let mut restored =
            OneLaneMatch::restore_checkpoint(&checkpoint, CONTENT, &lane(), &bots, &[]).unwrap();
        assert_eq!(restored.lane_digest(), uninterrupted.lane_digest());

        for _ in 0..6 {
            assert_eq!(
                restored.step().unwrap().lane_frame_digest,
                uninterrupted.step().unwrap().lane_frame_digest
            );
        }
    }

    #[test]
    fn bots_reach_a_reproducible_legal_objective_draw() {
        fn run() -> Vec<OneLaneFrame> {
            let mut game = objective_match();
            let mut frames = Vec::new();
            while game.runtime().phase() == MatchPhase::Active {
                frames.push(game.step().unwrap());
                game.runtime().check_invariants().unwrap();
                assert!(frames.len() < 20);
            }
            frames
        }

        let expected = run();
        let actual = run();
        assert_eq!(actual, expected);
        let terminal = &actual.last().unwrap().server;
        assert_eq!(
            terminal.phase,
            MatchPhase::Complete {
                outcome: MatchOutcome::Draw,
                reason: MatchEndReason::ObjectiveDestroyed,
            }
        );
    }

    #[test]
    fn waves_spawn_atomically_and_advance_when_alive_cap_skips() {
        let mut runtime = MatchRuntime::new(MatchConfig::default(), 7).unwrap();
        runtime.start().unwrap();
        let wave = WaveSpec {
            id: WaveId(3),
            team: TeamId(0),
            spawn: LanePosition {
                lane: crate::LaneId(0),
                progress_mm: 1_000,
            },
            goal: LanePosition {
                lane: crate::LaneId(0),
                progress_mm: 9_000,
            },
            aggro_range_mm: 1_000,
            target_priority: vec![ActorKind::Structure],
            cast_ability: None,
            first_tick: 1,
            interval_ticks: 2,
            max_alive: 2,
            units: vec![
                WaveUnitSpec {
                    progress_offset_mm: 0,
                    stats: stats(100, 0),
                    abilities: vec![],
                    basic_attack: None,
                },
                WaveUnitSpec {
                    progress_offset_mm: 100,
                    stats: stats(100, 0),
                    abilities: vec![],
                    basic_attack: None,
                },
            ],
        };
        let mut game = OneLaneMatch::attach(runtime, &lane(), vec![], vec![wave]).unwrap();
        let first = game.step().unwrap();
        let LaneMatchEvent::WaveSpawned { actors, .. } = &first.lane_events[0] else {
            panic!("expected wave spawn")
        };
        assert_ne!(
            first.lane_frame_digest,
            compose_lane_frame_digest(&first.server, &[], first.lane_digest),
            "lane events must contribute to composite frame evidence"
        );
        assert_eq!(actors.len(), 2);
        assert_eq!(game.runtime().actors().len(), 2);
        assert!(game.step().unwrap().lane_events.is_empty());
        let skipped = game.step().unwrap();
        assert!(matches!(
            skipped.lane_events.as_slice(),
            [LaneMatchEvent::WaveSkippedAliveCap {
                wave: WaveId(3),
                alive: 2,
                cap: 2,
            }]
        ));
        assert_eq!(game.runtime().actors().len(), 2);
    }

    #[test]
    fn attach_rejects_initial_actors_and_respawn_anchors_outside_the_lane() {
        let mut off_lane_runtime = MatchRuntime::new(MatchConfig::default(), 80).unwrap();
        let mut off_lane_actor = spawn(
            1,
            0,
            ActorKind::Hero,
            1_000,
            100,
            100,
            None,
            DeathRule::StayDead,
        );
        off_lane_actor.position = Vec2Mm::new(1_000, 501);
        off_lane_runtime.spawn_actor(&off_lane_actor).unwrap();
        off_lane_runtime.start().unwrap();
        assert!(OneLaneMatch::attach(off_lane_runtime, &lane(), vec![], vec![]).is_err());

        let mut off_anchor_runtime = MatchRuntime::new(MatchConfig::default(), 81).unwrap();
        off_anchor_runtime
            .spawn_actor(&spawn(
                2,
                0,
                ActorKind::Hero,
                1_000,
                100,
                100,
                None,
                DeathRule::Respawn {
                    delay_ticks: 2,
                    at: Vec2Mm::new(1_000, 501),
                },
            ))
            .unwrap();
        off_anchor_runtime.start().unwrap();
        assert!(OneLaneMatch::attach(off_anchor_runtime, &lane(), vec![], vec![]).is_err());
    }

    #[test]
    fn off_lane_non_bot_target_does_not_halt_bot_decision_ticks() {
        let mut runtime = MatchRuntime::new(MatchConfig::default(), 82).unwrap();
        let mut player = spawn(
            1,
            0,
            ActorKind::Hero,
            1_000,
            100,
            2_000,
            None,
            DeathRule::StayDead,
        );
        player.owner = Some(PlayerId(7));
        runtime.spawn_actor(&player).unwrap();
        runtime
            .spawn_actor(&spawn(
                2,
                1,
                ActorKind::Hero,
                9_000,
                100,
                0,
                None,
                DeathRule::StayDead,
            ))
            .unwrap();
        runtime.start().unwrap();
        runtime
            .submit(PlayerCommand {
                player: PlayerId(7),
                sequence: 1,
                execute_at: 1,
                actor: ActorId(1),
                kind: CommandKind::MoveTo {
                    destination: Vec2Mm::new(1_000, 2_000),
                },
            })
            .unwrap();
        let mut game = OneLaneMatch::attach(
            runtime,
            &lane(),
            vec![LaneBotSpec {
                actor: ActorId(2),
                goal: LanePosition {
                    lane: crate::LaneId(0),
                    progress_mm: 0,
                },
                aggro_range_mm: 10_000,
                target_priority: vec![ActorKind::Hero],
                cast_ability: None,
            }],
            vec![],
        )
        .unwrap();

        game.step().unwrap();
        assert_eq!(game.runtime().actor(ActorId(1)).unwrap().position.y, 2_000);
        game.step().unwrap();
        game.runtime().check_invariants().unwrap();
    }

    #[test]
    fn owned_actor_cannot_be_attached_as_a_bot() {
        let mut runtime = MatchRuntime::new(MatchConfig::default(), 8).unwrap();
        let mut actor = spawn(
            1,
            0,
            ActorKind::Hero,
            1_000,
            100,
            100,
            None,
            DeathRule::StayDead,
        );
        actor.owner = Some(crate::PlayerId(1));
        runtime.spawn_actor(&actor).unwrap();
        runtime.start().unwrap();
        let result = OneLaneMatch::attach(
            runtime,
            &lane(),
            vec![LaneBotSpec {
                actor: ActorId(1),
                goal: LanePosition {
                    lane: crate::LaneId(0),
                    progress_mm: 10_000,
                },
                aggro_range_mm: 1_000,
                target_priority: vec![ActorKind::Structure],
                cast_ability: None,
            }],
            vec![],
        );
        assert!(matches!(
            result,
            Err(LaneMatchError::InvalidDefinition(
                "an owned actor cannot also have a bot controller"
            ))
        ));
    }

    #[test]
    fn initial_bot_cast_must_be_equipped_by_its_actor() {
        let ability = AbilityId(77);
        let mut runtime = MatchRuntime::new(MatchConfig::default(), 90).unwrap();
        runtime
            .register_ability(AbilitySpec {
                id: ability,
                targeting: AbilityTargeting::Enemy,
                range_mm: 1_000,
                resource_cost: 0,
                cooldown_ticks: 1,
                cast_ticks: 0,
                effect: AbilityEffect::Damage {
                    amount: 1,
                    school: DamageSchool::True,
                },
            })
            .unwrap();
        runtime
            .spawn_actor(&spawn(
                1,
                0,
                ActorKind::Hero,
                1_000,
                100,
                100,
                None,
                DeathRule::StayDead,
            ))
            .unwrap();
        runtime.start().unwrap();
        let result = OneLaneMatch::attach(
            runtime,
            &lane(),
            vec![LaneBotSpec {
                actor: ActorId(1),
                goal: LanePosition {
                    lane: crate::LaneId(0),
                    progress_mm: 9_000,
                },
                aggro_range_mm: 1_000,
                target_priority: vec![ActorKind::Minion],
                cast_ability: Some(ability),
            }],
            vec![],
        );
        assert!(matches!(
            result,
            Err(LaneMatchError::InvalidDefinition(
                "bot cast ability is not equipped by its actor"
            ))
        ));
    }

    #[test]
    fn wave_unit_abilities_are_bounded_unique_and_include_bot_cast() {
        let ability_one = AbilityId(77);
        let ability_two = AbilityId(88);
        let make_runtime = |max_abilities_per_actor| {
            let config = MatchConfig {
                max_abilities_per_actor,
                ..MatchConfig::default()
            };
            let mut runtime = MatchRuntime::new(config, 91).unwrap();
            for id in [ability_one, ability_two] {
                runtime
                    .register_ability(AbilitySpec {
                        id,
                        targeting: AbilityTargeting::Enemy,
                        range_mm: 1_000,
                        resource_cost: 0,
                        cooldown_ticks: 1,
                        cast_ticks: 0,
                        effect: AbilityEffect::Damage {
                            amount: 1,
                            school: DamageSchool::True,
                        },
                    })
                    .unwrap();
            }
            runtime.start().unwrap();
            runtime
        };
        let make_wave = |abilities: Vec<AbilityId>, cast_ability| WaveSpec {
            id: WaveId(1),
            team: TeamId(0),
            spawn: LanePosition {
                lane: crate::LaneId(0),
                progress_mm: 1_000,
            },
            goal: LanePosition {
                lane: crate::LaneId(0),
                progress_mm: 9_000,
            },
            aggro_range_mm: 1_000,
            target_priority: vec![ActorKind::Minion],
            cast_ability,
            first_tick: 1,
            interval_ticks: 2,
            max_alive: 1,
            units: vec![WaveUnitSpec {
                progress_offset_mm: 0,
                stats: stats(100, 100),
                abilities,
                basic_attack: None,
            }],
        };

        let duplicate = OneLaneMatch::attach(
            make_runtime(2),
            &lane(),
            vec![],
            vec![make_wave(vec![ability_one, ability_one], None)],
        );
        assert!(matches!(
            duplicate,
            Err(LaneMatchError::InvalidDefinition(
                "wave unit contains duplicate abilities"
            ))
        ));

        let over_budget = OneLaneMatch::attach(
            make_runtime(1),
            &lane(),
            vec![],
            vec![make_wave(vec![ability_one, ability_two], None)],
        );
        assert!(matches!(
            over_budget,
            Err(LaneMatchError::InvalidDefinition(
                "wave unit ability budget exceeded"
            ))
        ));

        let missing_cast = OneLaneMatch::attach(
            make_runtime(2),
            &lane(),
            vec![],
            vec![make_wave(vec![ability_two], Some(ability_one))],
        );
        assert!(matches!(
            missing_cast,
            Err(LaneMatchError::InvalidDefinition(
                "wave cast ability must be equipped by every unit"
            ))
        ));
    }

    #[test]
    fn streaming_time_limit_soak_is_constant_memory_and_reproducible() {
        fn run() -> LaneMatchSummary {
            let config = MatchConfig::default();
            let mut runtime = MatchRuntime::new(config, 0x50a4).unwrap();
            runtime.start().unwrap();
            let mut game = OneLaneMatch::attach(runtime, &lane(), vec![], vec![]).unwrap();
            game.run_streaming(config.max_match_ticks).unwrap()
        }

        let expected = run();
        let actual = run();
        assert_eq!(actual, expected);
        assert_eq!(actual.frames, MatchConfig::default().max_match_ticks);
        assert_eq!(
            actual.final_phase,
            MatchPhase::Complete {
                outcome: MatchOutcome::Draw,
                reason: MatchEndReason::TimeLimit,
            }
        );
        assert_eq!(actual.peak_actor_count, 0);
        assert_eq!(actual.peak_live_actor_count, 0);
        assert_eq!(actual.peak_queued_commands, 0);
    }
}
