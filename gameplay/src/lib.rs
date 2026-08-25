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
    apportion, experience_for_level, level_for_experience, passive_gold_owed, prd_chance_bps,
    prd_constant_bps, roll_u64, roll_under_bps, streak_gold, AbilityAim, AbilityDelivery,
    AbilityEffect, AbilityId, AbilitySpec, AbilityTargeting, ActorId, ActorKind, ActorProvenance,
    ActorSpawn, AttackOrder, BasicAttackSpec, Bounty, CombatStats, ControlKind, ControlMask,
    DamageSchool, DeathRule, DynamicActorProvenance, DynamicActorSpawn, GoldReason, ImpactShape,
    MatchConfig, MatchEndReason, MatchOutcome, MatchPhase, ModifierId, ModifierOp, PlayerId,
    ProjectileId, RectMm, RngDomain, RuntimeError, ScoreView, StatGrowth, StatKind, StatModifier,
    StatusEffectId, StatusEffectView, TeamId, Tick, Vec2Mm, BASIS_POINTS, EXPERIENCE_CAP,
    MAX_ABILITIES_PER_ACTOR_BUDGET, MAX_ABILITY_DEFINITIONS, MAX_ABILITY_PER_RANK_AMOUNT,
    MAX_ABILITY_RANK, MAX_ACTOR_BUDGET, MAX_ASSIST_WINDOW_TICKS, MAX_BOUNTY_EXPERIENCE,
    MAX_BOUNTY_GOLD, MAX_COMMANDS_PER_PLAYER_PER_TICK_BUDGET, MAX_COMMANDS_PER_TICK_BUDGET,
    MAX_COMMAND_LEAD_TICKS, MAX_DAMAGE_CREDITS_PER_ACTOR_BUDGET, MAX_HERO_LEVEL,
    MAX_IMPACT_RADIUS_MM, MAX_LANE_BUDGET, MAX_LANE_POINTS_PER_LANE_BUDGET, MAX_MATCH_TICKS,
    MAX_MODIFIERS_PER_ACTOR_BUDGET, MAX_PASSIVE_GOLD_PER_100S, MAX_PLAYER_BUDGET,
    MAX_PROJECTILE_BUDGET, MAX_PROJECTILE_RADIUS_MM, MAX_PROJECTILE_SPEED_MM_PER_TICK,
    MAX_REJECTION_EVENTS_PER_FRAME_BUDGET, MAX_RUNTIME_CHECKPOINT_BYTES, MAX_SCORE_ENTRY_BUDGET,
    MAX_STATUS_EFFECTS_PER_ACTOR_BUDGET, MAX_STAT_GROWTH_PER_LEVEL, MAX_STREAK_GOLD,
    MAX_UNITS_PER_WAVE_BUDGET, MAX_WAVE_SCHEDULE_BUDGET, MAX_XP_SHARE_RADIUS_MM,
};
pub use protocol::{
    ActorIntent, ActorView, BasicAttackProgress, CastProgress, CastTarget, CommandKind,
    CommandReceipt, CommandRejectReason, CommandRejection, CooldownView, DamageCause, FrameDigest,
    MatchEvent, PlayerCommand, ProjectileView, ServerFrame, WorldDigest,
};
pub use replay::{
    ReplayError, ReplayOutcome, Scenario, ScenarioFinish, ScenarioSubmission, SubmissionOutcome,
};
pub use runtime::{
    InvariantViolation, MatchRuntime, FRAME_DIGEST_DOMAIN, HEALTH_SETTLEMENT_POLICY,
    WORLD_DIGEST_DOMAIN,
};

#[cfg(test)]
mod tests;
pub use lane::{CompiledLane, LaneError, LaneId, LanePosition, LaneSpec};
pub use moba::{
    LaneBotSpec, LaneCommandRejection, LaneFrameDigest, LaneMatchDigest, LaneMatchError,
    LaneMatchEvent, LaneMatchSummary, OneLaneFrame, OneLaneMatch, WaveId, WaveSpec, WaveUnitSpec,
};
pub use moba_checkpoint::{OneLaneCheckpoint, OneLaneCheckpointError};
