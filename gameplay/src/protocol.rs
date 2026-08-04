use crate::model::{
    AbilityId, ActorId, ActorKind, ActorProvenance, ControlMask, DamageSchool, GoldReason,
    MatchEndReason, MatchOutcome, MatchPhase, PlayerId, ProjectileId, ScoreView, StatusEffectId,
    TeamId, Tick, Vec2Mm,
};

/// Deterministic equality key for authoritative gameplay simulation state.
///
/// Outbound delivery buffers are intentionally excluded. Compare `ServerFrame::frame_digest` when validating
/// actor deltas and causal events as well as simulation truth.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorldDigest(pub u64);

/// Deterministic equality key for one complete authoritative frame, including deltas and causal events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameDigest(pub u64);

/// Target carried by a cast command.
///
/// The variant must match the ability's declared [`AbilityAim`](crate::AbilityAim): a unit-aimed ability
/// refuses a `Point` and a point-aimed ability refuses a unit, both with `InvalidTarget`. Mismatch is a
/// protocol error rather than a silent coercion, because coercing one into the other would let a client
/// choose which targeting rules its ability obeys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastTarget {
    SelfActor,
    Actor(ActorId),
    /// A map point, in the same integer millimetres as actor positions.
    Point(Vec2Mm),
}

/// Player intent accepted by the authoritative runtime. Presentation coordinates must be converted and
/// validated before this boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandKind {
    MoveTo {
        destination: Vec2Mm,
    },
    Stop,
    Cast {
        ability: AbilityId,
        target: CastTarget,
    },
    BasicAttack {
        target: ActorId,
    },
    /// Spend one unspent ability point to raise one equipped ability by a rank.
    ///
    /// A player command rather than a server-owned mutator because choosing *which* ability to raise is
    /// the decision this genre puts in the player's hands. It routes through the same single validation
    /// choke point as every other command, so a bot intent expresses it identically.
    UpgradeAbility {
        ability: AbilityId,
    },
}

/// Authenticated-session-ready command envelope. Authentication itself belongs to the network adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerCommand {
    pub player: PlayerId,
    pub sequence: u32,
    pub execute_at: Tick,
    pub actor: ActorId,
    pub kind: CommandKind,
}

/// Server-owned intent for bots and deterministic game rules. It has no player identity or sequence and
/// therefore cannot consume authenticated ingress quotas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActorIntent {
    pub actor: ActorId,
    pub kind: CommandKind,
}

/// Explicit, stable rejection taxonomy suitable for UI recovery and anti-cheat telemetry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandRejectReason {
    MatchNotActive,
    StaleTick,
    TooFarAhead,
    DuplicateOrReorderedSequence,
    PlayerBudgetExceeded,
    PlayerNotRegistered,
    TickCommandBudgetExceeded,
    PlayerTickCommandBudgetExceeded,
    ActorNotFound,
    ActorNotOwned,
    ActorDead,
    DestinationOutOfBounds,
    ActorCasting,
    AbilityNotEquipped,
    AbilityOnCooldown {
        ready_at: Tick,
    },
    InsufficientResource {
        required: u32,
        available: u32,
    },
    InvalidTarget,
    TargetNotFound,
    TargetDead,
    TargetRelationMismatch,
    TargetOutOfRange,
    BeyondMatchDuration,
    CastExceedsMatchDuration,
    ActorBusy,
    BasicAttackUnavailable,
    BasicAttackOnCooldown {
        ready_at: Tick,
    },
    AttackExceedsMatchDuration,
    ActorStunned,
    ActorRooted,
    ActorSilenced,
    ActorDisarmed,
    /// The cast's target form does not match the ability's declared aim, or a point-aimed skillshot was
    /// aimed exactly at the caster and so has no direction to travel.
    InvalidTargetForm,
    /// The aimed point lies outside the authoritative map bounds.
    TargetPointOutOfBounds,
    /// The match already carries its configured maximum of in-flight projectiles. Fail-closed: the cast is
    /// cancelled rather than silently dropping a projectile the client was told to expect.
    ProjectileBudgetExceeded,
    /// No unspent ability point is available.
    NoAbilityPointAvailable,
    /// The ability is already at its maximum rank.
    AbilityRankMaxed,
    /// The actor's level does not yet unlock this rank.
    AbilityRankLocked {
        unlocks_at_level: u8,
    },
    /// Only heroes carry progression, so only a hero can spend a point.
    ActorHasNoProgression,
}

/// Accepted queue position. A later state change can still make the command invalid at execution time; that
/// produces an authoritative rejection event using the same player/sequence identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandReceipt {
    pub player: PlayerId,
    pub sequence: u32,
    pub execute_at: Tick,
}

/// Rejected command receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandRejection {
    pub player: PlayerId,
    pub sequence: u32,
    pub reason: CommandRejectReason,
}

/// Compact cooldown projection required by the local HUD and prediction layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CooldownView {
    pub ability: AbilityId,
    pub ready_at: Tick,
    /// Current rank, always at least 1. A client needs it to draw the ability's real magnitude.
    pub rank: u8,
}

/// Read-only cast progress in an authoritative actor delta.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CastProgress {
    pub ability: AbilityId,
    pub target: CastTarget,
    pub resolves_at: Tick,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BasicAttackProgress {
    pub target: ActorId,
    pub resolves_at: Tick,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DamageCause {
    Ability(AbilityId),
    BasicAttack,
}

/// Network-facing authoritative actor state. MOB-1 sends a full compact actor payload only when that actor is
/// dirty; field-level quantized deltas are a measured protocol-layer optimization in MOB-3.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActorView {
    pub id: ActorId,
    pub owner: Option<PlayerId>,
    pub team: TeamId,
    pub kind: ActorKind,
    pub position: Vec2Mm,
    pub destination: Option<Vec2Mm>,
    pub health: u32,
    pub max_health: u32,
    pub resource: u32,
    pub max_resource: u32,
    pub move_speed_mm_per_tick: u32,
    /// Resolved mitigation. Exposed because a modifier that only changes reduction would otherwise mark the
    /// actor dirty while emitting a byte-identical view — a delta that claims a change it cannot show.
    pub physical_reduction_bps: u16,
    pub magic_reduction_bps: u16,
    /// Active crowd-control restrictions. A client needs these to grey out abilities and show CC bars.
    pub controls: ControlMask,
    pub alive: bool,
    pub cast: Option<CastProgress>,
    pub basic_attack: Option<BasicAttackProgress>,
    pub basic_attack_ready_at: Option<Tick>,
    pub basic_attack_range_mm: Option<u32>,
    pub respawn_at: Option<Tick>,
    /// Progression. `level` is DERIVED from `experience` and is projected rather than stored, so a client
    /// and the server can never disagree about which one is authoritative.
    pub level: u8,
    pub experience: u32,
    pub unspent_ability_points: u8,
    /// Immutable creation record. Controllers above the runtime use this to re-derive which schedule owns an
    /// actor instead of trusting a separately stored membership list.
    pub provenance: ActorProvenance,
    pub cooldowns: Vec<CooldownView>,
}

/// Network-facing authoritative state for one in-flight projectile.
///
/// `hit_radius_mm` is a projection of the launching ability's spec, carried so a client can draw and
/// predict the missile without holding the ability table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectileView {
    pub id: ProjectileId,
    pub source: ActorId,
    pub team: TeamId,
    pub ability: AbilityId,
    pub position: Vec2Mm,
    /// Fixed flight terminus, decided at launch. Exposed because it is what makes the missile predictable:
    /// a client can interpolate the whole flight from one frame.
    pub end: Vec2Mm,
    pub hit_radius_mm: u32,
}

/// Authoritative causal trace. Presentation systems consume these events; cosmetic feedback never writes
/// damage or match truth back into the runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchEvent {
    MatchStarted,
    CommandRejected {
        player: PlayerId,
        sequence: u32,
        reason: CommandRejectReason,
    },
    MoveStarted {
        actor: ActorId,
        destination: Vec2Mm,
    },
    MoveStopped {
        actor: ActorId,
    },
    CastStarted {
        source: ActorId,
        ability: AbilityId,
        target: CastTarget,
        resolves_at: Tick,
    },
    CastCancelled {
        source: ActorId,
        ability: AbilityId,
        reason: CommandRejectReason,
    },
    CastResolved {
        source: ActorId,
        ability: AbilityId,
        target: CastTarget,
    },
    DamageApplied {
        source: ActorId,
        target: ActorId,
        cause: DamageCause,
        school: DamageSchool,
        amount: u32,
        health_after: u32,
    },
    HealingApplied {
        source: ActorId,
        target: ActorId,
        ability: AbilityId,
        amount: u32,
        health_after: u32,
    },
    ActorDied {
        actor: ActorId,
        killer: ActorId,
    },
    BasicAttackStarted {
        source: ActorId,
        target: ActorId,
        resolves_at: Tick,
    },
    BasicAttackCancelled {
        source: ActorId,
        target: ActorId,
        reason: CommandRejectReason,
    },
    BasicAttackResolved {
        source: ActorId,
        target: ActorId,
    },
    ActorDespawned {
        actor: ActorId,
    },
    RespawnScheduled {
        actor: ActorId,
        at_tick: Tick,
    },
    RespawnCancelled {
        actor: ActorId,
        at_tick: Tick,
        reason: MatchEndReason,
    },
    ActorRespawned {
        actor: ActorId,
        position: Vec2Mm,
    },
    InternalIntentRejected {
        actor: ActorId,
        reason: CommandRejectReason,
    },
    StatusEffectExpired {
        actor: ActorId,
        effect: StatusEffectId,
    },
    ActorSpawned {
        actor: ActorId,
    },
    ProjectileSpawned {
        projectile: ProjectileId,
        source: ActorId,
        ability: AbilityId,
        position: Vec2Mm,
        end: Vec2Mm,
    },
    /// The projectile left the world. `victim` is `Some` when it struck a body and `None` when it reached
    /// the end of its flight. One event covers both because a client draws the same thing either way — an
    /// impact at a position — and splitting them would let the two drift apart.
    ProjectileResolved {
        projectile: ProjectileId,
        position: Vec2Mm,
        victim: Option<ActorId>,
    },
    /// The projectile was removed without resolving its effect — today only by match completion.
    ProjectileCancelled {
        projectile: ProjectileId,
        reason: CommandRejectReason,
    },
    /// A kill was credited to a scoreboard line. Distinct from `ActorDied`, which stays byte-stable: this
    /// carries the scoreboard-grade detail without rewriting a shipped tag's payload.
    KillCredited {
        killer: ActorId,
        victim: ActorId,
        streak_after: u16,
    },
    /// One assist, emitted once per assister so the event stays `Copy` rather than carrying a list.
    AssistCredited {
        actor: ActorId,
        victim: ActorId,
    },
    GoldGranted {
        actor: ActorId,
        amount: u32,
        reason: GoldReason,
        total_after: u32,
    },
    ExperienceGranted {
        actor: ActorId,
        amount: u32,
        total_after: u32,
    },
    HeroLevelUp {
        actor: ActorId,
        level: u8,
        unspent_ability_points: u8,
    },
    AbilityRankUp {
        actor: ActorId,
        ability: AbilityId,
        rank: u8,
    },
    MatchFinished {
        outcome: MatchOutcome,
        reason: MatchEndReason,
    },
}

/// One actor-level authoritative delta frame. `changed` contains only actors dirtied since the previous
/// frame. A transport can split reliable events from sequenced state while retaining this causal identity.
///
/// `projectiles` carries **every** live projectile rather than a dirty subset. That is not a departure from
/// the delta rule: a projectile whose speed is zero is refused at authoring time, so every live projectile
/// moves on every tick and the full set *is* the delta. Tracking a dirty subset would cost state and
/// produce the same bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerFrame {
    pub tick: Tick,
    pub phase: MatchPhase,
    pub changed: Vec<ActorView>,
    pub removed: Vec<ActorId>,
    pub projectiles: Vec<ProjectileView>,
    pub removed_projectiles: Vec<ProjectileId>,
    /// Scoreboard lines changed since the previous frame — a genuine dirty subset, unlike `projectiles`.
    /// The contrast is the point: a projectile always moves, so its full set IS the delta, whereas a score
    /// line changes only on a kill or on the roughly one tick in fifteen where closed-form passive income
    /// crosses a whole gold unit. There is no `removed_scores`, because a line is never removed.
    pub scores: Vec<ScoreView>,
    pub events: Vec<MatchEvent>,
    pub world_digest: WorldDigest,
    pub frame_digest: FrameDigest,
}
