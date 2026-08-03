use std::error::Error;
use std::fmt::{self, Display, Formatter};

/// The authoritative fixed-step index.
pub type Tick = u64;

/// One whole in basis-point arithmetic.
pub const BASIS_POINTS: u16 = 10_000;

/// Hard safety ceiling for an in-memory MOB-1 match/replay. Longer evidence moves to checkpointed MOB-2
/// artifacts instead of allocating an unbounded frame history.
pub const MAX_MATCH_TICKS: Tick = 250_000;

/// Absolute allocation ceiling for one core-runtime checkpoint, independent of authored target profiles.
pub const MAX_RUNTIME_CHECKPOINT_BYTES: u32 = 16 * 1024 * 1024;

/// Hard decoded-state ceilings. Target profiles are normally much smaller; these prevent a valid-sized
/// checkpoint from amplifying attacker-controlled counts into unbounded heap structures.
pub const MAX_PLAYER_BUDGET: u32 = 64;
pub const MAX_ACTOR_BUDGET: u32 = 4_096;
pub const MAX_ABILITY_DEFINITIONS: u32 = 4_096;
pub const MAX_ABILITIES_PER_ACTOR_BUDGET: u32 = 64;
pub const MAX_COMMANDS_PER_TICK_BUDGET: u32 = 2_048;
pub const MAX_COMMANDS_PER_PLAYER_PER_TICK_BUDGET: u32 = 128;
pub const MAX_REJECTION_EVENTS_PER_FRAME_BUDGET: u32 = 2_048;
pub const MAX_COMMAND_LEAD_TICKS: u16 = 128;
pub const MAX_LANE_BUDGET: u16 = 16;
pub const MAX_LANE_POINTS_PER_LANE_BUDGET: u16 = 1_024;
pub const MAX_WAVE_SCHEDULE_BUDGET: u16 = 256;
pub const MAX_UNITS_PER_WAVE_BUDGET: u16 = 256;
pub const MAX_MODIFIERS_PER_ACTOR_BUDGET: u16 = 64;
pub const MAX_STATUS_EFFECTS_PER_ACTOR_BUDGET: u16 = 32;

macro_rules! id_type {
    ($name:ident, $inner:ty, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub $inner);

        impl $name {
            #[must_use]
            pub const fn get(self) -> $inner {
                self.0
            }
        }
    };
}

id_type!(
    ActorId,
    u64,
    "Stable runtime identity for one replicated actor."
);
id_type!(
    PlayerId,
    u64,
    "Authenticated player identity inside one match."
);
id_type!(TeamId, u8, "Team identity inside one match.");
id_type!(
    AbilityId,
    u32,
    "Dense-build-compatible identity for an ability definition."
);

/// A deterministic two-dimensional gameplay position in integer millimetres.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Vec2Mm {
    pub x: i32,
    pub y: i32,
}

impl Vec2Mm {
    #[must_use]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    pub(crate) fn squared_distance(self, other: Self) -> u128 {
        let dx = i64::from(other.x) - i64::from(self.x);
        let dy = i64::from(other.y) - i64::from(self.y);
        u128::from(dx.unsigned_abs()).pow(2) + u128::from(dy.unsigned_abs()).pow(2)
    }
}

/// Inclusive map bounds in integer millimetres.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RectMm {
    pub min: Vec2Mm,
    pub max: Vec2Mm,
}

impl RectMm {
    #[must_use]
    pub const fn new(min: Vec2Mm, max: Vec2Mm) -> Self {
        Self { min, max }
    }

    #[must_use]
    pub const fn contains(self, point: Vec2Mm) -> bool {
        point.x >= self.min.x
            && point.x <= self.max.x
            && point.y >= self.min.y
            && point.y <= self.max.y
    }

    pub(crate) const fn is_valid(self) -> bool {
        self.min.x < self.max.x && self.min.y < self.max.y
    }
}

/// Broad runtime category. Domain-specific behavior is data, not an inheritance hierarchy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActorKind {
    Hero,
    Minion,
    Structure,
    Objective,
    Pet,
    Projectile,
}

/// Authoritative combat and movement values already compiled for the match tick rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CombatStats {
    pub max_health: u32,
    pub max_resource: u32,
    pub move_speed_mm_per_tick: u32,
    pub physical_reduction_bps: u16,
    pub magic_reduction_bps: u16,
}

impl CombatStats {
    pub(crate) fn validate(self) -> Result<(), RuntimeError> {
        if self.max_health == 0 {
            return Err(RuntimeError::InvalidActor("max health must be positive"));
        }
        if self.physical_reduction_bps >= BASIS_POINTS || self.magic_reduction_bps >= BASIS_POINTS {
            return Err(RuntimeError::InvalidActor(
                "damage reduction must be below 10,000 basis points",
            ));
        }
        Ok(())
    }

    /// Resolve authored base stats plus a set of modifiers into the stats the simulation actually uses.
    ///
    /// **This function is the determinism contract.** With integer arithmetic the evaluation order decides
    /// the result, so the order is fixed here, documented, and tested — never left to insertion order:
    ///
    /// 1. **Flat** magnitudes are summed. Integer addition is associative and commutative, so this stage is
    ///    order-independent by construction.
    /// 2. **PercentAdd** magnitudes are summed into ONE multiplier applied once. Summing before dividing
    ///    means +10% and +20% compose to exactly +30%, and only one truncation happens.
    /// 3. **PercentMult** magnitudes apply sequentially, ordered **by magnitude** (ties by ID). Truncating
    ///    multiplication is not commutative — `7 × 1.5 → 10 × 0.5 → 5` but `7 × 0.5 → 3 × 1.5 → 4` — so a
    ///    total order is mandatory. Ordering by magnitude rather than by ID makes the result a function of
    ///    the modifier SET, so applying the same buffs in a different sequence cannot yield different stats.
    ///    Equal magnitudes commute exactly, so the ID tiebreak never changes the value.
    /// 4. **Override** replaces the value outright, and the **highest modifier ID wins**. This stage is
    ///    deliberately order-DEPENDENT: "most recently applied wins" is the intended semantics for an
    ///    override, and there is no set-determined key that expresses it. It is still fully deterministic —
    ///    the same operation sequence always produces the same IDs — but two different sequences applying
    ///    the same overrides can legitimately differ.
    /// 5. The result is clamped into the stat's legal domain. Clamping is a safety property, not a detail:
    ///    it is what stops a stacked reduction modifier from reaching 100% and making an actor immortal.
    ///
    /// Division truncates toward zero. Every intermediate is `i64`, which cannot overflow for `u32` bases
    /// and basis-point multipliers, and saturating arithmetic backstops the impossible case.
    #[must_use]
    pub fn with_modifiers(self, modifiers: &[StatModifier]) -> Self {
        Self {
            max_health: resolve_stat(self.max_health, StatKind::MaxHealth, modifiers),
            max_resource: resolve_stat(self.max_resource, StatKind::MaxResource, modifiers),
            move_speed_mm_per_tick: resolve_stat(
                self.move_speed_mm_per_tick,
                StatKind::MoveSpeed,
                modifiers,
            ),
            physical_reduction_bps: u16::try_from(resolve_stat(
                u32::from(self.physical_reduction_bps),
                StatKind::PhysicalReduction,
                modifiers,
            ))
            .unwrap_or(BASIS_POINTS - 1),
            magic_reduction_bps: u16::try_from(resolve_stat(
                u32::from(self.magic_reduction_bps),
                StatKind::MagicReduction,
                modifiers,
            ))
            .unwrap_or(BASIS_POINTS - 1),
        }
    }
}

/// One resolvable actor attribute. Every variant maps to exactly one [`CombatStats`] field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StatKind {
    MaxHealth,
    MaxResource,
    MoveSpeed,
    PhysicalReduction,
    MagicReduction,
}

impl StatKind {
    /// The legal domain for a resolved value. Reductions stay strictly below 100% so no stack of modifiers
    /// can produce an immortal actor, and max health stays positive so `validate` can never be violated by
    /// a modifier.
    const fn domain(self) -> (i64, i64) {
        match self {
            Self::MaxHealth => (1, u32::MAX as i64),
            Self::MaxResource | Self::MoveSpeed => (0, u32::MAX as i64),
            Self::PhysicalReduction | Self::MagicReduction => (0, BASIS_POINTS as i64 - 1),
        }
    }
}

/// How one modifier changes its stat. Magnitudes are signed: a slow is a negative `PercentAdd`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierOp {
    Flat { magnitude: i32 },
    PercentAdd { bps: i32 },
    PercentMult { bps: i32 },
    Override { value: u32 },
}

/// Monotonic never-reused identity for one applied modifier. Allocation order defines both the sequential
/// `PercentMult` order and override precedence, which is why it must never be reused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModifierId(pub u64);

impl ModifierId {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// One crowd-control restriction. These are the genre's load-bearing interaction verbs: they decide what an
/// actor may *do*, independently of what its stats are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ControlKind {
    /// Blocks movement, casting, and attacking, and interrupts anything already in progress.
    Stun,
    /// Blocks movement only.
    Root,
    /// Blocks casting only. Does not interrupt a cast already resolving.
    Silence,
    /// Blocks basic attacks only.
    Disarm,
}

impl ControlKind {
    const fn bit(self) -> u8 {
        match self {
            Self::Stun => 1,
            Self::Root => 1 << 1,
            Self::Silence => 1 << 2,
            Self::Disarm => 1 << 3,
        }
    }

    pub(crate) const ALL: [Self; 4] = [Self::Stun, Self::Root, Self::Silence, Self::Disarm];
}

/// A set of active restrictions, held as a bitmask.
///
/// A mask rather than a list on purpose: union is order-free and idempotent, so combining the restrictions of
/// several overlapping effects cannot depend on iteration order — the same reasoning that made stat stage 3
/// order by magnitude.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ControlMask(u8);

impl ControlMask {
    pub const EMPTY: Self = Self(0);
    const VALID_BITS: u8 = 0b1111;

    #[must_use]
    pub fn from_kinds(kinds: &[ControlKind]) -> Self {
        Self(kinds.iter().fold(0, |bits, kind| bits | kind.bit()))
    }

    #[must_use]
    pub const fn contains(self, kind: ControlKind) -> bool {
        self.0 & kind.bit() != 0
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Reject unknown bits rather than silently masking them: an undefined restriction decoded from a
    /// checkpoint is corruption, not a future feature to tolerate.
    pub(crate) const fn from_bits(bits: u8) -> Option<Self> {
        if bits & !Self::VALID_BITS == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    #[must_use]
    pub fn kinds(self) -> Vec<ControlKind> {
        ControlKind::ALL
            .into_iter()
            .filter(|kind| self.contains(*kind))
            .collect()
    }
}

/// Monotonic never-reused identity for one applied status effect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StatusEffectId(pub u64);

impl StatusEffectId {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Read-only projection of one active status effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEffectView {
    pub id: StatusEffectId,
    pub expires_at: Tick,
    pub controls: ControlMask,
    pub modifiers: Vec<ModifierId>,
}

/// One modifier bound to one actor. Lifetime is deliberately absent: this layer resolves *how* stats
/// combine. Timed expiry, stacking rules, and dispel belong to the status-effect layer above it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatModifier {
    pub id: ModifierId,
    pub kind: StatKind,
    pub op: ModifierOp,
}

fn resolve_stat(base: u32, kind: StatKind, modifiers: &[StatModifier]) -> u32 {
    let (minimum, maximum) = kind.domain();
    let mut flat_sum: i64 = 0;
    let mut percent_add: i64 = 0;
    let mut override_value: Option<(ModifierId, u32)> = None;

    // Stage 1 and 2: commutative sums, plus the highest-ID override.
    for modifier in modifiers.iter().filter(|entry| entry.kind == kind) {
        match modifier.op {
            ModifierOp::Flat { magnitude } => {
                flat_sum = flat_sum.saturating_add(i64::from(magnitude));
            }
            ModifierOp::PercentAdd { bps } => {
                percent_add = percent_add.saturating_add(i64::from(bps));
            }
            ModifierOp::PercentMult { .. } => {}
            ModifierOp::Override { value } => {
                if override_value.is_none_or(|(current, _)| modifier.id > current) {
                    override_value = Some((modifier.id, value));
                }
            }
        }
    }

    let mut value = i64::from(base).saturating_add(flat_sum);
    let combined = i64::from(BASIS_POINTS).saturating_add(percent_add).max(0);
    value = value.saturating_mul(combined) / i64::from(BASIS_POINTS);

    // Stage 3: sequential, so it needs a total order — and the order must be derived from the modifier SET,
    // not from when each was applied, or the same buffs in a different sequence would resolve differently.
    // Sorting by magnitude achieves that; the ID tiebreak only makes the sort total, and equal magnitudes
    // commute exactly so it can never change the result.
    let mut multiplicative: Vec<(i32, ModifierId)> = modifiers
        .iter()
        .filter(|entry| entry.kind == kind)
        .filter_map(|entry| match entry.op {
            ModifierOp::PercentMult { bps } => Some((bps, entry.id)),
            _ => None,
        })
        .collect();
    multiplicative.sort_unstable();
    for (bps, _) in multiplicative {
        let factor = i64::from(BASIS_POINTS)
            .saturating_add(i64::from(bps))
            .max(0);
        value = value.saturating_mul(factor) / i64::from(BASIS_POINTS);
    }

    // Stage 4 then 5.
    if let Some((_, replacement)) = override_value {
        value = i64::from(replacement);
    }
    u32::try_from(value.clamp(minimum, maximum)).unwrap_or(0)
}

/// Damage channel used to select a target's compiled mitigation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DamageSchool {
    Physical,
    Magic,
    True,
}

/// Legal relationship between caster and unit target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbilityTargeting {
    SelfOnly,
    Ally,
    Enemy,
    AnyUnit,
}

/// MOB-1's bounded effect set. Later gates extend this centrally with statuses, areas, and projectiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbilityEffect {
    Damage { amount: u32, school: DamageSchool },
    Heal { amount: u32 },
}

/// Immutable compiled ability definition shared by server, client prediction, and replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AbilitySpec {
    pub id: AbilityId,
    pub targeting: AbilityTargeting,
    pub range_mm: u32,
    pub resource_cost: u32,
    pub cooldown_ticks: u32,
    pub cast_ticks: u16,
    pub effect: AbilityEffect,
}

impl AbilitySpec {
    pub(crate) fn validate(self) -> Result<(), RuntimeError> {
        let amount = match self.effect {
            AbilityEffect::Damage { amount, .. } | AbilityEffect::Heal { amount } => amount,
        };
        if amount == 0 {
            return Err(RuntimeError::InvalidAbility(
                self.id,
                "effect amount must be positive",
            ));
        }
        Ok(())
    }
}

/// Cooked one-shot basic attack. Repeated attacks are explicit intents rather than an implicit auto-loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BasicAttackSpec {
    pub range_mm: u32,
    pub damage: u32,
    pub school: DamageSchool,
    pub windup_ticks: u16,
    pub cooldown_ticks: u32,
}

impl BasicAttackSpec {
    pub(crate) const fn validate(self) -> Result<(), RuntimeError> {
        if self.range_mm == 0 {
            return Err(RuntimeError::InvalidActor(
                "basic-attack range must be positive",
            ));
        }
        if self.damage == 0 {
            return Err(RuntimeError::InvalidActor(
                "basic-attack damage must be positive",
            ));
        }
        Ok(())
    }
}

/// Cooked lifecycle behavior after authoritative death classification.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeathRule {
    #[default]
    StayDead,
    Despawn,
    Respawn {
        delay_ticks: u32,
        at: Vec2Mm,
    },
    ObjectiveVictory {
        winner: TeamId,
    },
}

/// Immutable creation provenance carried by authoritative actor state and deterministic evidence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActorProvenance {
    #[default]
    Authored,
    Dynamic {
        spawn_ordinal: u64,
    },
    Wave {
        wave_id: u32,
        unit_slot: u16,
        spawn_ordinal: u64,
    },
}

/// Provenance request for a server-spawned actor. The runtime supplies the immutable monotonic ordinal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DynamicActorProvenance {
    #[default]
    Generic,
    Wave {
        wave_id: u32,
        unit_slot: u16,
    },
}

/// One actor in the cooked initial match state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActorSpawn {
    pub id: ActorId,
    pub owner: Option<PlayerId>,
    pub team: TeamId,
    pub kind: ActorKind,
    pub position: Vec2Mm,
    pub stats: CombatStats,
    pub abilities: Vec<AbilityId>,
    pub basic_attack: Option<BasicAttackSpec>,
    pub death_rule: DeathRule,
}

/// Runtime-spawned unowned actor template. The authoritative allocator supplies its never-reused ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DynamicActorSpawn {
    pub provenance: DynamicActorProvenance,
    pub team: TeamId,
    pub kind: ActorKind,
    pub position: Vec2Mm,
    pub stats: CombatStats,
    pub abilities: Vec<AbilityId>,
    pub basic_attack: Option<BasicAttackSpec>,
    pub death_rule: DeathRule,
}

/// Bounded match settings. Content may override these only through a validated target profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchConfig {
    pub tick_rate_hz: u16,
    pub map_bounds: RectMm,
    pub max_players: u32,
    pub max_actors: u32,
    pub max_abilities_per_actor: u32,
    pub max_commands_per_tick: u32,
    pub max_commands_per_player_per_tick: u32,
    pub max_rejection_events_per_frame: u32,
    pub max_command_lead_ticks: u16,
    pub max_match_ticks: Tick,
    pub max_lanes: u16,
    pub max_lane_points_per_lane: u16,
    pub max_wave_schedules: u16,
    pub max_units_per_wave: u16,
    pub max_modifiers_per_actor: u16,
    pub max_status_effects_per_actor: u16,
    pub max_checkpoint_bytes: u32,
}

impl MatchConfig {
    /// Reference profile for the first mobile competitive slice. It is a validation target, not a measured
    /// performance claim.
    #[must_use]
    pub const fn competitive_mobile() -> Self {
        Self {
            tick_rate_hz: 30,
            map_bounds: RectMm::new(
                Vec2Mm::new(-500_000, -500_000),
                Vec2Mm::new(500_000, 500_000),
            ),
            max_players: 16,
            max_actors: 512,
            max_abilities_per_actor: 8,
            max_commands_per_tick: 128,
            max_commands_per_player_per_tick: 8,
            max_rejection_events_per_frame: 128,
            max_command_lead_ticks: 8,
            max_match_ticks: 30 * 60 * 60,
            max_lanes: 1,
            max_lane_points_per_lane: 64,
            max_wave_schedules: 8,
            max_units_per_wave: 16,
            max_modifiers_per_actor: 16,
            max_status_effects_per_actor: 8,
            max_checkpoint_bytes: 4 * 1024 * 1024,
        }
    }

    pub(crate) fn validate(self) -> Result<(), RuntimeError> {
        if self.tick_rate_hz == 0 {
            return Err(RuntimeError::InvalidConfig("tick rate must be positive"));
        }
        if !self.map_bounds.is_valid() {
            return Err(RuntimeError::InvalidConfig(
                "map bounds must have positive area",
            ));
        }
        if self.max_players == 0
            || self.max_actors == 0
            || self.max_abilities_per_actor == 0
            || self.max_commands_per_tick == 0
            || self.max_commands_per_player_per_tick == 0
            || self.max_rejection_events_per_frame == 0
            || self.max_lanes == 0
            || self.max_lane_points_per_lane < 2
            || self.max_wave_schedules == 0
            || self.max_units_per_wave == 0
            || self.max_modifiers_per_actor == 0
            || self.max_status_effects_per_actor == 0
            || self.max_checkpoint_bytes == 0
        {
            return Err(RuntimeError::InvalidConfig(
                "runtime and authored-content budgets must be positive",
            ));
        }
        if self.max_commands_per_player_per_tick > self.max_commands_per_tick {
            return Err(RuntimeError::InvalidConfig(
                "per-player command budget cannot exceed the global tick budget",
            ));
        }
        if self.max_players > MAX_PLAYER_BUDGET
            || self.max_actors > MAX_ACTOR_BUDGET
            || self.max_abilities_per_actor > MAX_ABILITIES_PER_ACTOR_BUDGET
            || self.max_commands_per_tick > MAX_COMMANDS_PER_TICK_BUDGET
            || self.max_commands_per_player_per_tick > MAX_COMMANDS_PER_PLAYER_PER_TICK_BUDGET
            || self.max_rejection_events_per_frame > MAX_REJECTION_EVENTS_PER_FRAME_BUDGET
            || self.max_command_lead_ticks > MAX_COMMAND_LEAD_TICKS
            || self.max_lanes > MAX_LANE_BUDGET
            || self.max_lane_points_per_lane > MAX_LANE_POINTS_PER_LANE_BUDGET
            || self.max_wave_schedules > MAX_WAVE_SCHEDULE_BUDGET
            || self.max_units_per_wave > MAX_UNITS_PER_WAVE_BUDGET
            || self.max_modifiers_per_actor > MAX_MODIFIERS_PER_ACTOR_BUDGET
            || self.max_status_effects_per_actor > MAX_STATUS_EFFECTS_PER_ACTOR_BUDGET
        {
            return Err(RuntimeError::InvalidConfig(
                "target profile exceeds a hard decoded-state budget",
            ));
        }
        if self.max_command_lead_ticks == 0 {
            return Err(RuntimeError::InvalidConfig(
                "command lead window must be positive",
            ));
        }
        if self.max_match_ticks == 0 {
            return Err(RuntimeError::InvalidConfig(
                "maximum match duration must be positive",
            ));
        }
        if self.max_match_ticks > MAX_MATCH_TICKS {
            return Err(RuntimeError::InvalidConfig(
                "maximum match duration exceeds the hard runtime ceiling",
            ));
        }
        if self.max_checkpoint_bytes > MAX_RUNTIME_CHECKPOINT_BYTES {
            return Err(RuntimeError::InvalidConfig(
                "checkpoint budget exceeds the 16 MiB hard ceiling",
            ));
        }
        Ok(())
    }
}

impl Default for MatchConfig {
    fn default() -> Self {
        Self::competitive_mobile()
    }
}

/// High-level authoritative match lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchPhase {
    Setup,
    Active,
    Complete {
        outcome: MatchOutcome,
        reason: MatchEndReason,
    },
}

/// Legal terminal result. A draw is first-class so simultaneous objectives and the time limit never require
/// the host to invent a winner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchOutcome {
    Winner(TeamId),
    Draw,
}

/// Stable reason for an authoritative terminal result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchEndReason {
    ObjectiveDestroyed,
    TimeLimit,
    Host,
}

/// Invalid authored/runtime setup. Player command failures use `CommandRejectReason` instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeError {
    InvalidConfig(&'static str),
    InvalidAbility(AbilityId, &'static str),
    DuplicateAbility(AbilityId),
    InvalidActor(&'static str),
    DuplicateActor(ActorId),
    PlayerBudgetExceeded,
    ActorBudgetExceeded,
    ActorIdExhausted,
    ModifierIdExhausted,
    StatusEffectIdExhausted,
    UnknownStatusEffect(StatusEffectId),
    UnknownActor(ActorId),
    UnknownModifier(ModifierId),
    UnknownAbility(AbilityId),
    ActorOutOfBounds(ActorId),
    InvalidPhase(&'static str),
    CommandBudgetCannotReserveRoster,
    InvalidInternalIntents(&'static str),
    MatchDurationExceeded,
    TickOverflow,
}

impl Display for RuntimeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(reason) => write!(formatter, "invalid match config: {reason}"),
            Self::InvalidAbility(id, reason) => {
                write!(formatter, "invalid ability {}: {reason}", id.get())
            }
            Self::DuplicateAbility(id) => write!(formatter, "duplicate ability {}", id.get()),
            Self::InvalidActor(reason) => write!(formatter, "invalid actor: {reason}"),
            Self::DuplicateActor(id) => write!(formatter, "duplicate actor {}", id.get()),
            Self::PlayerBudgetExceeded => formatter.write_str("player roster budget exceeded"),
            Self::ActorBudgetExceeded => formatter.write_str("actor budget exceeded"),
            Self::ActorIdExhausted => formatter.write_str("dynamic actor ID space exhausted"),
            Self::ModifierIdExhausted => formatter.write_str("stat modifier ID space exhausted"),
            Self::StatusEffectIdExhausted => {
                formatter.write_str("status effect ID space exhausted")
            }
            Self::UnknownStatusEffect(id) => {
                write!(formatter, "unknown status effect {}", id.get())
            }
            Self::UnknownActor(id) => write!(formatter, "unknown actor {}", id.get()),
            Self::UnknownModifier(id) => write!(formatter, "unknown stat modifier {}", id.get()),
            Self::UnknownAbility(id) => write!(formatter, "unknown ability {}", id.get()),
            Self::ActorOutOfBounds(id) => {
                write!(formatter, "actor {} is outside map bounds", id.get())
            }
            Self::InvalidPhase(reason) => write!(formatter, "invalid match phase: {reason}"),
            Self::CommandBudgetCannotReserveRoster => formatter.write_str(
                "global command budget cannot reserve the per-player quota for the authored roster",
            ),
            Self::InvalidInternalIntents(reason) => {
                write!(formatter, "invalid internal actor intents: {reason}")
            }
            Self::MatchDurationExceeded => {
                formatter.write_str("maximum authoritative match duration reached")
            }
            Self::TickOverflow => formatter.write_str("authoritative tick overflow"),
        }
    }
}

impl Error for RuntimeError {}
