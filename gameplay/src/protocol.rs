use crate::model::{
    AbilityId, ActorId, ActorKind, ActorProvenance, DamageSchool, MatchEndReason, MatchOutcome,
    MatchPhase, PlayerId, TeamId, Tick, Vec2Mm,
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

/// Unit target carried by a cast command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastTarget {
    SelfActor,
    Actor(ActorId),
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
    AbilityOnCooldown { ready_at: Tick },
    InsufficientResource { required: u32, available: u32 },
    InvalidTarget,
    TargetNotFound,
    TargetDead,
    TargetRelationMismatch,
    TargetOutOfRange,
    BeyondMatchDuration,
    CastExceedsMatchDuration,
    ActorBusy,
    BasicAttackUnavailable,
    BasicAttackOnCooldown { ready_at: Tick },
    AttackExceedsMatchDuration,
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
    pub alive: bool,
    pub cast: Option<CastProgress>,
    pub basic_attack: Option<BasicAttackProgress>,
    pub basic_attack_ready_at: Option<Tick>,
    pub basic_attack_range_mm: Option<u32>,
    pub respawn_at: Option<Tick>,
    /// Immutable creation record. Controllers above the runtime use this to re-derive which schedule owns an
    /// actor instead of trusting a separately stored membership list.
    pub provenance: ActorProvenance,
    pub cooldowns: Vec<CooldownView>,
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
    ActorSpawned {
        actor: ActorId,
    },
    MatchFinished {
        outcome: MatchOutcome,
        reason: MatchEndReason,
    },
}

/// One actor-level authoritative delta frame. `changed` contains only actors dirtied since the previous
/// frame. A transport can split reliable events from sequenced state while retaining this causal identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerFrame {
    pub tick: Tick,
    pub phase: MatchPhase,
    pub changed: Vec<ActorView>,
    pub removed: Vec<ActorId>,
    pub events: Vec<MatchEvent>,
    pub world_digest: WorldDigest,
    pub frame_digest: FrameDigest,
}
