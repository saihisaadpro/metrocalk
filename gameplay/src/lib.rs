//! Portable, deterministic match-runtime foundation for competitive mobile games.
//!
//! The authoring ECS/Loro document remains the durable collaborative source of truth. A build step will cook
//! that data into the compact types in this crate. A live match then has one authoritative fixed-step owner;
//! it never runs over editor JSON, CRDT merge rules, Tauri IPC, or renderer state.
//!
//! MOB-1 proves the shared spine: typed actors/abilities, validated commands, integer combat, deltas, hashing,
//! and exact scenario replay. The MOB-2 candidate adds a bounded one-route lane coordinate, one-shot attacks,
//! server-owned bot intents, waves, lifecycle rules, legal objective/time-limit outcomes, a bounded core
//! checkpoint, and an atomic one-lane controller checkpoint. Host-authenticated persistence, competitive
//! sockets/prediction, status systems, and production mobile hosts remain explicit gated work. This crate does
//! not imply those production gates have passed.

mod checkpoint;
mod lane;
mod moba;
mod moba_checkpoint;
mod model;
mod protocol;
mod replay;
mod runtime;

pub use checkpoint::{CheckpointError, ContentId, RuntimeCheckpoint};
pub use model::{
    AbilityEffect, AbilityId, AbilitySpec, AbilityTargeting, ActorId, ActorKind, ActorProvenance,
    ActorSpawn, BasicAttackSpec, CombatStats, DamageSchool, DeathRule, DynamicActorProvenance,
    DynamicActorSpawn, MatchConfig, MatchEndReason, MatchOutcome, MatchPhase, ModifierId,
    ModifierOp, PlayerId, RectMm, RuntimeError, StatKind, StatModifier, TeamId, Tick, Vec2Mm,
    BASIS_POINTS, MAX_ABILITIES_PER_ACTOR_BUDGET, MAX_ABILITY_DEFINITIONS, MAX_ACTOR_BUDGET,
    MAX_COMMANDS_PER_PLAYER_PER_TICK_BUDGET, MAX_COMMANDS_PER_TICK_BUDGET, MAX_COMMAND_LEAD_TICKS,
    MAX_LANE_BUDGET, MAX_LANE_POINTS_PER_LANE_BUDGET, MAX_MATCH_TICKS,
    MAX_MODIFIERS_PER_ACTOR_BUDGET, MAX_PLAYER_BUDGET, MAX_REJECTION_EVENTS_PER_FRAME_BUDGET,
    MAX_RUNTIME_CHECKPOINT_BYTES, MAX_UNITS_PER_WAVE_BUDGET, MAX_WAVE_SCHEDULE_BUDGET,
};
pub use protocol::{
    ActorIntent, ActorView, BasicAttackProgress, CastProgress, CastTarget, CommandKind,
    CommandReceipt, CommandRejectReason, CommandRejection, CooldownView, DamageCause, FrameDigest,
    MatchEvent, PlayerCommand, ServerFrame, WorldDigest,
};
pub use replay::{
    ReplayError, ReplayOutcome, Scenario, ScenarioFinish, ScenarioSubmission, SubmissionOutcome,
};
pub use runtime::{InvariantViolation, MatchRuntime, HEALTH_SETTLEMENT_POLICY};

#[cfg(test)]
mod tests;
pub use lane::{CompiledLane, LaneError, LaneId, LanePosition, LaneSpec};
pub use moba::{
    LaneBotSpec, LaneCommandRejection, LaneFrameDigest, LaneMatchDigest, LaneMatchError,
    LaneMatchEvent, LaneMatchSummary, OneLaneFrame, OneLaneMatch, WaveId, WaveSpec, WaveUnitSpec,
};
pub use moba_checkpoint::{OneLaneCheckpoint, OneLaneCheckpointError};
