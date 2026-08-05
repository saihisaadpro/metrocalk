//! Versioned, bounded persistence for the deterministic core match runtime.
//!
//! The wire format is deliberately private to this crate's compatibility contract: every integer is
//! little-endian, every enum/option has an explicit one-byte tag, and every collection has a `u32` count.
//! It contains core [`MatchRuntime`] truth only. The one-lane controller composes this payload with its wave
//! scheduler through the separate atomic `OneLaneCheckpoint` envelope.
//! FNV checksums and deterministic digests provide accidental-corruption evidence only. The host must load
//! checkpoints from trusted storage or authenticate an outer storage/transport envelope before decoding.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Display, Formatter};

use crate::model::{
    level_for_experience, passive_gold_owed, AbilityAim, AbilityDelivery, AbilityEffect, AbilityId,
    AbilitySpec, AbilityTargeting, ActorId, ActorKind, ActorProvenance, BasicAttackSpec, Bounty,
    CombatStats, ControlMask, DamageSchool, DeathRule, ImpactShape, MatchConfig, MatchEndReason,
    MatchOutcome, MatchPhase, ModifierId, ModifierOp, PlayerId, ProjectileId, RectMm, StatGrowth,
    StatKind, StatModifier, StatusEffectId, TeamId, Tick, Vec2Mm, MAX_ABILITY_DEFINITIONS,
    MAX_RUNTIME_CHECKPOINT_BYTES,
};
use crate::protocol::{
    CastTarget, CommandKind, CommandReceipt, CommandRejectReason, CommandRejection, PlayerCommand,
    WorldDigest,
};
use crate::runtime::{
    AbilityState, ActorState, BasicAttackState, DamageCredit, MatchRuntime, PendingAttack,
    PendingCast, ProjectileState, ScoreEntry, StatusEffect, SubmissionRecord,
};

const MAGIC: [u8; 8] = *b"MTCKPT01";
// v7 adds the pseudo-random-distribution attempt counter, per-status-effect shield pools, and the five
// offensive combat stats (crit chance/damage, physical and magic penetration, lifesteal). The RNG itself
// adds NO state: rolls are keyed rather than streamed, so there is no cursor to persist.
// v6 adds authored stat growth, kill bounties, experience, unspent ability points, the per-actor assist
// window, per-ability ranks, and the permanent scoreboard. It deliberately does NOT encode `level`,
// `base_stats`, `stats`, `control_mask`, or `passive_gold_paid`: every one of those is re-derived on decode
// from inputs the payload cannot forge, which is what makes a self-consistent forged checkpoint fail the
// restore audit rather than pass it.
// v5 adds in-flight projectiles plus their allocator, and the projectile budget inside the config. Speed,
// hit radius, footprint, and effect are NOT encoded per projectile: they are read from the immutable ability
// spec, so a tampered payload cannot give one missile different physics from the ability that launched it.
// v4 adds per-actor timed status effects; the resolved control mask is NOT encoded (same reasoning).
// v3 added per-actor base stats plus the applied stat-modifier list; the resolved stat cache is deliberately
// NOT encoded, because it is derivable and a stored copy could disagree with its own inputs.
// v2 added the immutable per-actor creation provenance record. The format is not yet persisted by any
// production host, so the version is bumped rather than carrying a compatibility shim: a v1 payload has no
// provenance to reconstruct, and inferring one would fabricate creation evidence.
const FORMAT_VERSION: u16 = 7;
const HEADER_BYTES: usize = 64;
const PAYLOAD_LENGTH_OFFSET: usize = 44;
const PAYLOAD_CHECKSUM_OFFSET: usize = 48;
const WORLD_DIGEST_OFFSET: usize = 56;
const MIN_ABILITY_BYTES: usize = 24;
const MIN_COMMAND_BYTES: usize = 29;
// 8 id + 1 kind tag + 1 op tag + 4 smallest payload.
const MIN_MODIFIER_BYTES: usize = 14;
// 8 id + 1 provenance tag + 1 owner tag + 1 team + 1 kind + 8 position + 1 destination tag
// + 26 authored stats + 16 growth + 8 bounty + 4 experience + 1 unspent point + 2 crit attempts
// + 4 credit count
// + 4 modifier count + 4 status-effect count + 4 health + 4 resource + 4 ability count + 5 option tags.
const MIN_ACTOR_BYTES: usize = 107;
// 8 id + 8 expiry + 4 shield pool + 1 controls + 4 owned-modifier count.
const MIN_STATUS_EFFECT_BYTES: usize = 25;
// 8 id + 8 source + 1 team + 4 ability + 8 position + 8 flight terminus + 4 launch magnitude.
const MIN_PROJECTILE_BYTES: usize = 41;
// 8 actor + 1 owner tag + 1 team + 8 since + 4 earned + 4 kills + 4 deaths + 4 assists + 2 + 2 streaks.
const MIN_SCORE_ENTRY_BYTES: usize = 38;
// 8 source + 8 last tick.
const MIN_DAMAGE_CREDIT_BYTES: usize = 16;

/// Identity of the exact cooked rules/content bundle required to interpret a checkpoint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentId([u8; 32]);

impl ContentId {
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Opaque checkpoint bytes. Constructing this wrapper applies the absolute allocation ceiling only;
/// compatibility, content identity, accidental-corruption evidence, and runtime invariants are checked by
/// restore. Host authentication is a required outer-envelope responsibility.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeCheckpoint {
    bytes: Vec<u8>,
}

impl RuntimeCheckpoint {
    /// Copy opaque bytes into a bounded envelope. The host must authenticate them before restore.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CheckpointError> {
        enforce_size(
            bytes.len(),
            usize::try_from(MAX_RUNTIME_CHECKPOINT_BYTES).unwrap_or(usize::MAX),
        )?;
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(bytes.len())
            .map_err(|_| CheckpointError::AllocationFailed)?;
        owned.extend_from_slice(bytes);
        Ok(Self { bytes: owned })
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// Fail-closed checkpoint capture/restore error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckpointError {
    NotFrameBoundary,
    SizeLimitExceeded {
        size: u64,
        limit: u64,
    },
    CountOverflow,
    InvalidMagic,
    UnsupportedVersion(u16),
    UnsupportedFlags(u16),
    ContentMismatch {
        expected: ContentId,
        actual: ContentId,
    },
    Truncated,
    LengthMismatch,
    CorruptPayload,
    InvalidTag {
        field: &'static str,
        tag: u8,
    },
    InvalidData(&'static str),
    AllocationFailed,
    RuntimeInvariant {
        actor: Option<ActorId>,
        detail: &'static str,
    },
    DigestMismatch {
        encoded: WorldDigest,
        recomputed: WorldDigest,
    },
}

impl Display for CheckpointError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFrameBoundary => formatter.write_str(
                "checkpoint capture requires empty outbound buffers at a frame boundary",
            ),
            Self::SizeLimitExceeded { size, limit } => {
                write!(formatter, "checkpoint size {size} exceeds limit {limit}")
            }
            Self::CountOverflow => formatter.write_str("checkpoint collection count exceeds u32"),
            Self::InvalidMagic => formatter.write_str("invalid checkpoint magic"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported checkpoint version {version}")
            }
            Self::UnsupportedFlags(flags) => {
                write!(formatter, "unsupported checkpoint flags {flags}")
            }
            Self::ContentMismatch { .. } => {
                formatter.write_str("checkpoint cooked-content identity does not match")
            }
            Self::Truncated => formatter.write_str("checkpoint is truncated"),
            Self::LengthMismatch => formatter.write_str("checkpoint payload length does not match"),
            Self::CorruptPayload => {
                formatter.write_str("checkpoint payload checksum does not match")
            }
            Self::InvalidTag { field, tag } => {
                write!(formatter, "invalid checkpoint tag {tag} for {field}")
            }
            Self::InvalidData(detail) => write!(formatter, "invalid checkpoint data: {detail}"),
            Self::AllocationFailed => formatter.write_str("checkpoint allocation failed"),
            Self::RuntimeInvariant { actor, detail } => {
                if let Some(actor) = actor {
                    write!(
                        formatter,
                        "restored runtime invariant failed for actor {}: {detail}",
                        actor.get()
                    )
                } else {
                    write!(formatter, "restored runtime invariant failed: {detail}")
                }
            }
            Self::DigestMismatch { .. } => {
                formatter.write_str("restored runtime digest does not match checkpoint")
            }
        }
    }
}

impl Error for CheckpointError {}

impl MatchRuntime {
    /// Capture core runtime truth at an emitted-frame boundary.
    ///
    /// Dirty/removed actor sets, pending delivery events, rejection counters, and tick-local objective claims
    /// are outbound or transient state and are not serialized. Capture refuses to proceed unless all are empty.
    pub fn capture_checkpoint(
        &self,
        content_id: ContentId,
    ) -> Result<RuntimeCheckpoint, CheckpointError> {
        self.require_checkpoint_boundary()?;
        self.check_invariants()
            .map_err(|violation| CheckpointError::RuntimeInvariant {
                actor: violation.actor,
                detail: violation.detail,
            })?;

        let configured_limit = usize::try_from(self.config.max_checkpoint_bytes)
            .unwrap_or(usize::MAX)
            .min(usize::try_from(MAX_RUNTIME_CHECKPOINT_BYTES).unwrap_or(usize::MAX));
        let mut encoder = Encoder::new(configured_limit)?;
        encoder.bytes(&MAGIC)?;
        encoder.u16(FORMAT_VERSION)?;
        encoder.u16(0)?;
        encoder.bytes(content_id.as_bytes())?;
        encoder.u32(0)?;
        encoder.u64(0)?;
        encoder.u64(0)?;

        encode_config(&mut encoder, self.config)?;
        encoder.u64(self.seed)?;
        encoder.u64(self.tick)?;
        encode_phase(&mut encoder, self.phase)?;
        encode_option_u64(&mut encoder, self.next_dynamic_actor_id)?;
        // Encoded, not derived from the live modifiers: removing every modifier must NOT let the allocator
        // regress and reissue an ID, because ID order defines aggregation order and override precedence.
        encode_option_u64(&mut encoder, self.next_modifier_id)?;
        encode_option_u64(&mut encoder, self.next_status_effect_id)?;
        encode_option_u64(&mut encoder, self.next_projectile_id)?;

        if self.ability_specs.len() > usize::try_from(MAX_ABILITY_DEFINITIONS).unwrap_or(usize::MAX)
        {
            return Err(CheckpointError::InvalidData(
                "ability definition count exceeds the hard budget",
            ));
        }
        encoder.count(self.ability_specs.len())?;
        for ability in &self.ability_specs {
            encode_ability(&mut encoder, *ability)?;
        }

        encoder.count(self.actors.len())?;
        for actor in &self.actors {
            encode_actor(&mut encoder, actor)?;
        }

        encoder.count(self.projectiles.len())?;
        for projectile in &self.projectiles {
            encode_projectile(&mut encoder, *projectile)?;
        }

        encoder.count(self.scores.len())?;
        for entry in &self.scores {
            encode_score_entry(&mut encoder, *entry)?;
        }

        encoder.count(self.player_roster.len())?;
        for player in &self.player_roster {
            encoder.u64(player.get())?;
        }

        encoder.count(self.queued_commands.len())?;
        for (tick, commands) in &self.queued_commands {
            encoder.u64(*tick)?;
            let mut canonical = commands.clone();
            canonical.sort_unstable_by_key(command_order_key);
            encoder.count(canonical.len())?;
            for command in canonical {
                encode_command(&mut encoder, command)?;
            }
        }

        encoder.count(self.last_submissions.len())?;
        for (player, submission) in &self.last_submissions {
            encoder.u64(player.get())?;
            encode_command(&mut encoder, submission.command)?;
            encode_submission_outcome(&mut encoder, submission.outcome)?;
        }

        let mut bytes = encoder.finish();
        let payload_length = bytes
            .len()
            .checked_sub(HEADER_BYTES)
            .ok_or(CheckpointError::Truncated)?;
        let payload_length_u32 =
            u32::try_from(payload_length).map_err(|_| CheckpointError::CountOverflow)?;
        let checksum = checksum(&bytes[HEADER_BYTES..]);
        let digest = self.world_digest();
        bytes[PAYLOAD_LENGTH_OFFSET..PAYLOAD_LENGTH_OFFSET + 4]
            .copy_from_slice(&payload_length_u32.to_le_bytes());
        bytes[PAYLOAD_CHECKSUM_OFFSET..PAYLOAD_CHECKSUM_OFFSET + 8]
            .copy_from_slice(&checksum.to_le_bytes());
        bytes[WORLD_DIGEST_OFFSET..WORLD_DIGEST_OFFSET + 8]
            .copy_from_slice(&digest.0.to_le_bytes());
        Ok(RuntimeCheckpoint { bytes })
    }

    /// Restore a new core runtime after host authentication and complete structural validation.
    ///
    /// The caller's live runtime, if any, is never mutated. The returned runtime starts with empty outbound
    /// buffers. A `OneLaneMatch` controller cannot yet be restored by this API because its wave and bot state
    /// is intentionally outside the core checkpoint.
    #[expect(
        clippy::too_many_lines,
        reason = "fail-closed restore keeps the complete ordered validation pipeline visible"
    )]
    pub fn restore_checkpoint(
        checkpoint: &RuntimeCheckpoint,
        expected_content_id: ContentId,
    ) -> Result<Self, CheckpointError> {
        let bytes = checkpoint.as_bytes();
        enforce_size(
            bytes.len(),
            usize::try_from(MAX_RUNTIME_CHECKPOINT_BYTES).unwrap_or(usize::MAX),
        )?;
        if bytes.len() < HEADER_BYTES {
            return Err(CheckpointError::Truncated);
        }
        if bytes[..8] != MAGIC {
            return Err(CheckpointError::InvalidMagic);
        }
        let version = read_header_u16(bytes, 8)?;
        if version != FORMAT_VERSION {
            return Err(CheckpointError::UnsupportedVersion(version));
        }
        let flags = read_header_u16(bytes, 10)?;
        if flags != 0 {
            return Err(CheckpointError::UnsupportedFlags(flags));
        }
        let actual_content_id = ContentId(
            bytes[12..44]
                .try_into()
                .map_err(|_| CheckpointError::Truncated)?,
        );
        if actual_content_id != expected_content_id {
            return Err(CheckpointError::ContentMismatch {
                expected: expected_content_id,
                actual: actual_content_id,
            });
        }
        let payload_length = usize::try_from(read_header_u32(bytes, PAYLOAD_LENGTH_OFFSET)?)
            .map_err(|_| CheckpointError::LengthMismatch)?;
        if payload_length != bytes.len() - HEADER_BYTES {
            return Err(CheckpointError::LengthMismatch);
        }
        let encoded_checksum = read_header_u64(bytes, PAYLOAD_CHECKSUM_OFFSET)?;
        if checksum(&bytes[HEADER_BYTES..]) != encoded_checksum {
            return Err(CheckpointError::CorruptPayload);
        }
        let encoded_digest = WorldDigest(read_header_u64(bytes, WORLD_DIGEST_OFFSET)?);

        let mut decoder = Decoder::new(&bytes[HEADER_BYTES..]);
        let config = decode_config(&mut decoder)?;
        config
            .validate()
            .map_err(|_| CheckpointError::InvalidData("match config failed validation"))?;
        enforce_size(
            bytes.len(),
            usize::try_from(config.max_checkpoint_bytes).unwrap_or(usize::MAX),
        )?;
        let seed = decoder.u64()?;
        let tick = decoder.u64()?;
        let phase = decode_phase(&mut decoder)?;
        let next_dynamic_actor_id = decode_option_u64(&mut decoder, "dynamic actor allocator")?;
        let next_modifier_id = decode_option_u64(&mut decoder, "stat modifier allocator")?;
        let next_status_effect_id = decode_option_u64(&mut decoder, "status effect allocator")?;
        let next_projectile_id = decode_option_u64(&mut decoder, "projectile allocator")?;

        let ability_count = decoder.count(
            usize::try_from(MAX_ABILITY_DEFINITIONS).unwrap_or(usize::MAX),
            "ability definition count exceeds the hard budget",
        )?;
        decoder.require_minimum(ability_count, MIN_ABILITY_BYTES)?;
        let mut ability_specs = allocate_vec(ability_count)?;
        for _ in 0..ability_count {
            ability_specs.push(decode_ability(&mut decoder)?);
        }

        let actor_limit = usize::try_from(config.max_actors).unwrap_or(usize::MAX);
        let actor_count = decoder.count(actor_limit, "actor count exceeds configured budget")?;
        // The authoritative breakdown lives at `MIN_ACTOR_BYTES`; recompute it there, from the encoder.
        decoder.require_minimum(actor_count, MIN_ACTOR_BYTES)?;
        let mut actors = allocate_vec(actor_count)?;
        for _ in 0..actor_count {
            actors.push(decode_actor(&mut decoder, config)?);
        }

        let projectile_count = decoder.count(
            usize::from(config.max_projectiles),
            "projectile count exceeds configured budget",
        )?;
        decoder.require_minimum(projectile_count, MIN_PROJECTILE_BYTES)?;
        let mut projectiles = allocate_vec(projectile_count)?;
        for _ in 0..projectile_count {
            projectiles.push(decode_projectile(&mut decoder, config)?);
        }
        if projectiles
            .windows(2)
            .any(|pair: &[ProjectileState]| pair[0].id >= pair[1].id)
        {
            return Err(CheckpointError::InvalidData(
                "projectiles are not strictly sorted by identity",
            ));
        }

        let score_count = decoder.count(
            usize::from(config.max_score_entries),
            "scoreboard count exceeds configured budget",
        )?;
        decoder.require_minimum(score_count, MIN_SCORE_ENTRY_BYTES)?;
        let mut scores = allocate_vec(score_count)?;
        for _ in 0..score_count {
            scores.push(decode_score_entry(&mut decoder, tick, config)?);
        }
        if scores
            .windows(2)
            .any(|pair: &[ScoreEntry]| pair[0].actor >= pair[1].actor)
        {
            return Err(CheckpointError::InvalidData(
                "scoreboard entries are not strictly sorted by actor",
            ));
        }

        let roster_limit = usize::try_from(config.max_players).unwrap_or(usize::MAX);
        let roster_count =
            decoder.count(roster_limit, "player roster exceeds configured budget")?;
        decoder.require_minimum(roster_count, 8)?;
        let mut player_roster = BTreeSet::new();
        for _ in 0..roster_count {
            if !player_roster.insert(PlayerId(decoder.u64()?)) {
                return Err(CheckpointError::InvalidData("duplicate player in roster"));
            }
        }

        let queue_bucket_limit = usize::from(config.max_command_lead_ticks);
        let queue_bucket_count =
            decoder.count(queue_bucket_limit, "queued tick bucket count is impossible")?;
        decoder.require_minimum(queue_bucket_count, 12)?;
        let mut queued_commands = BTreeMap::new();
        for _ in 0..queue_bucket_count {
            let queued_tick = decoder.u64()?;
            let command_limit = usize::try_from(config.max_commands_per_tick).unwrap_or(usize::MAX);
            let command_count =
                decoder.count(command_limit, "queued command count exceeds tick budget")?;
            decoder.require_minimum(command_count, MIN_COMMAND_BYTES)?;
            let mut commands = allocate_vec(command_count)?;
            for _ in 0..command_count {
                commands.push(decode_command(&mut decoder)?);
            }
            if commands
                .windows(2)
                .any(|pair| command_order_key(&pair[0]) >= command_order_key(&pair[1]))
            {
                return Err(CheckpointError::InvalidData(
                    "queued commands are not in canonical order",
                ));
            }
            if queued_commands.insert(queued_tick, commands).is_some() {
                return Err(CheckpointError::InvalidData("duplicate queued tick bucket"));
            }
        }

        let submission_count =
            decoder.count(roster_limit, "last-submission count exceeds player budget")?;
        decoder.require_minimum(submission_count, 8 + MIN_COMMAND_BYTES + 1)?;
        let mut last_submissions = BTreeMap::new();
        for _ in 0..submission_count {
            let player = PlayerId(decoder.u64()?);
            let command = decode_command(&mut decoder)?;
            let outcome = decode_submission_outcome(&mut decoder)?;
            if last_submissions
                .insert(player, SubmissionRecord { command, outcome })
                .is_some()
            {
                return Err(CheckpointError::InvalidData(
                    "duplicate player in last-submission map",
                ));
            }
        }
        decoder.finish()?;

        let runtime = Self {
            config,
            seed,
            tick,
            phase,
            ability_specs,
            actors,
            projectiles,
            scores,
            player_roster,
            queued_commands,
            last_submissions,
            dirty: BTreeSet::new(),
            removed: BTreeSet::new(),
            removed_projectiles: BTreeSet::new(),
            dirty_scores: BTreeSet::new(),
            pending_events: Vec::new(),
            rejection_events_in_frame: 0,
            objective_claims: BTreeSet::new(),
            next_dynamic_actor_id,
            next_modifier_id,
            next_status_effect_id,
            next_projectile_id,
            checkpoint_ready: true,
        };
        runtime.validate_restored_state()?;
        let recomputed = runtime.world_digest();
        if recomputed != encoded_digest {
            return Err(CheckpointError::DigestMismatch {
                encoded: encoded_digest,
                recomputed,
            });
        }
        Ok(runtime)
    }

    fn require_checkpoint_boundary(&self) -> Result<(), CheckpointError> {
        if self.checkpoint_ready
            && self.dirty.is_empty()
            && self.removed.is_empty()
            && self.removed_projectiles.is_empty()
            && self.dirty_scores.is_empty()
            && self.pending_events.is_empty()
            && self.rejection_events_in_frame == 0
            && self.objective_claims.is_empty()
        {
            Ok(())
        } else {
            Err(CheckpointError::NotFrameBoundary)
        }
    }

    fn validate_restored_state(&self) -> Result<(), CheckpointError> {
        if self.tick > self.config.max_match_ticks {
            return Err(CheckpointError::InvalidData(
                "authoritative tick exceeds configured duration",
            ));
        }
        match self.phase {
            MatchPhase::Setup => {
                return Err(CheckpointError::InvalidData(
                    "setup phase has no emitted frame boundary and cannot be restored",
                ));
            }
            MatchPhase::Active if self.tick >= self.config.max_match_ticks => {
                return Err(CheckpointError::InvalidData(
                    "active phase reached the terminal tick",
                ));
            }
            _ => {}
        }
        for ability in &self.ability_specs {
            ability.validate().map_err(|_| {
                CheckpointError::InvalidData("ability definition failed validation")
            })?;
        }
        for actor in &self.actors {
            validate_restored_actor(actor, self.tick, self.config)?;
        }
        for record in self.last_submissions.values() {
            validate_submission_record(*record)?;
        }
        self.check_invariants()
            .map_err(|violation| CheckpointError::RuntimeInvariant {
                actor: violation.actor,
                detail: violation.detail,
            })
    }
}

fn validate_restored_actor(
    actor: &ActorState,
    tick: Tick,
    config: MatchConfig,
) -> Result<(), CheckpointError> {
    // Validate the AUTHORED base, not the derived cache: the cache is recomputed from base+modifiers on
    // decode, so validating it would be a tautology and would leave base_stats checked nowhere.
    actor
        .authored_stats
        .validate()
        .map_err(|_| CheckpointError::InvalidData("actor authored stats failed validation"))?;
    actor
        .growth
        .validate()
        .map_err(|_| CheckpointError::InvalidData("actor stat growth failed validation"))?;
    actor
        .bounty
        .validate()
        .map_err(|_| CheckpointError::InvalidData("actor bounty failed validation"))?;
    if let Some(basic_attack) = actor.basic_attack {
        basic_attack
            .spec
            .validate()
            .map_err(|_| CheckpointError::InvalidData("basic attack failed validation"))?;
    }
    if let DeathRule::Respawn { delay_ticks: 0, .. } = actor.death_rule {
        return Err(CheckpointError::InvalidData(
            "respawn delay must be positive",
        ));
    }
    if actor
        .destination
        .is_some_and(|destination| !config.map_bounds.contains(destination))
    {
        return Err(CheckpointError::InvalidData(
            "actor destination is outside map bounds",
        ));
    }
    if let Some(cast) = actor.cast {
        if !actor
            .abilities
            .iter()
            .any(|ability| ability.id == cast.ability)
        {
            return Err(CheckpointError::InvalidData(
                "pending cast ability is not equipped",
            ));
        }
        if cast.resolves_at <= tick || cast.resolves_at > config.max_match_ticks {
            return Err(CheckpointError::InvalidData(
                "pending cast deadline is outside the remaining match",
            ));
        }
    }
    if let Some(attack) = actor.attack {
        if actor.basic_attack.is_none() {
            return Err(CheckpointError::InvalidData(
                "pending basic attack has no authored attack",
            ));
        }
        if attack.resolves_at <= tick || attack.resolves_at > config.max_match_ticks {
            return Err(CheckpointError::InvalidData(
                "pending attack deadline is outside the remaining match",
            ));
        }
    }
    Ok(())
}

fn validate_submission_record(record: SubmissionRecord) -> Result<(), CheckpointError> {
    match record.outcome {
        Ok(receipt)
            if receipt.player == record.command.player
                && receipt.sequence == record.command.sequence
                && receipt.execute_at == record.command.execute_at =>
        {
            Ok(())
        }
        Err(rejection)
            if rejection.player == record.command.player
                && rejection.sequence == record.command.sequence =>
        {
            Ok(())
        }
        _ => Err(CheckpointError::InvalidData(
            "last-submission outcome does not match its command",
        )),
    }
}

struct Encoder {
    bytes: Vec<u8>,
    limit: usize,
}

impl Encoder {
    fn new(limit: usize) -> Result<Self, CheckpointError> {
        if limit < HEADER_BYTES {
            return Err(CheckpointError::SizeLimitExceeded {
                size: u64::try_from(HEADER_BYTES).unwrap_or(u64::MAX),
                limit: u64::try_from(limit).unwrap_or(u64::MAX),
            });
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(HEADER_BYTES.min(limit))
            .map_err(|_| CheckpointError::AllocationFailed)?;
        Ok(Self { bytes, limit })
    }

    fn bytes(&mut self, value: &[u8]) -> Result<(), CheckpointError> {
        let next = self.bytes.len().checked_add(value.len()).ok_or(
            CheckpointError::SizeLimitExceeded {
                size: u64::MAX,
                limit: u64::try_from(self.limit).unwrap_or(u64::MAX),
            },
        )?;
        if next > self.limit {
            return Err(CheckpointError::SizeLimitExceeded {
                size: u64::try_from(next).unwrap_or(u64::MAX),
                limit: u64::try_from(self.limit).unwrap_or(u64::MAX),
            });
        }
        self.bytes
            .try_reserve_exact(value.len())
            .map_err(|_| CheckpointError::AllocationFailed)?;
        self.bytes.extend_from_slice(value);
        Ok(())
    }

    fn u8(&mut self, value: u8) -> Result<(), CheckpointError> {
        self.bytes(&[value])
    }

    fn u16(&mut self, value: u16) -> Result<(), CheckpointError> {
        self.bytes(&value.to_le_bytes())
    }

    fn u32(&mut self, value: u32) -> Result<(), CheckpointError> {
        self.bytes(&value.to_le_bytes())
    }

    fn i32(&mut self, value: i32) -> Result<(), CheckpointError> {
        self.bytes(&value.to_le_bytes())
    }

    fn u64(&mut self, value: u64) -> Result<(), CheckpointError> {
        self.bytes(&value.to_le_bytes())
    }

    fn count(&mut self, value: usize) -> Result<(), CheckpointError> {
        self.u32(u32::try_from(value).map_err(|_| CheckpointError::CountOverflow)?)
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

struct Decoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Decoder<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], CheckpointError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(CheckpointError::Truncated)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(CheckpointError::Truncated)?;
        self.offset = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, CheckpointError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, CheckpointError> {
        Ok(u16::from_le_bytes(
            self.take(2)?
                .try_into()
                .map_err(|_| CheckpointError::Truncated)?,
        ))
    }

    fn u32(&mut self) -> Result<u32, CheckpointError> {
        Ok(u32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| CheckpointError::Truncated)?,
        ))
    }

    fn i32(&mut self) -> Result<i32, CheckpointError> {
        Ok(i32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| CheckpointError::Truncated)?,
        ))
    }

    fn u64(&mut self) -> Result<u64, CheckpointError> {
        Ok(u64::from_le_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| CheckpointError::Truncated)?,
        ))
    }

    fn count(&mut self, limit: usize, detail: &'static str) -> Result<usize, CheckpointError> {
        let count = usize::try_from(self.u32()?).map_err(|_| CheckpointError::CountOverflow)?;
        if count > limit {
            return Err(CheckpointError::InvalidData(detail));
        }
        Ok(count)
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    fn require_minimum(
        &self,
        count: usize,
        minimum_item_bytes: usize,
    ) -> Result<(), CheckpointError> {
        let required = count
            .checked_mul(minimum_item_bytes)
            .ok_or(CheckpointError::Truncated)?;
        if required > self.remaining() {
            return Err(CheckpointError::Truncated);
        }
        Ok(())
    }

    fn finish(self) -> Result<(), CheckpointError> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(CheckpointError::LengthMismatch)
        }
    }
}

fn allocate_vec<T>(capacity: usize) -> Result<Vec<T>, CheckpointError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| CheckpointError::AllocationFailed)?;
    Ok(values)
}

fn enforce_size(size: usize, limit: usize) -> Result<(), CheckpointError> {
    if size > limit {
        Err(CheckpointError::SizeLimitExceeded {
            size: u64::try_from(size).unwrap_or(u64::MAX),
            limit: u64::try_from(limit).unwrap_or(u64::MAX),
        })
    } else {
        Ok(())
    }
}

fn read_header_u16(bytes: &[u8], offset: usize) -> Result<u16, CheckpointError> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or(CheckpointError::Truncated)?
            .try_into()
            .map_err(|_| CheckpointError::Truncated)?,
    ))
}

fn read_header_u32(bytes: &[u8], offset: usize) -> Result<u32, CheckpointError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(CheckpointError::Truncated)?
            .try_into()
            .map_err(|_| CheckpointError::Truncated)?,
    ))
}

fn read_header_u64(bytes: &[u8], offset: usize) -> Result<u64, CheckpointError> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(CheckpointError::Truncated)?
            .try_into()
            .map_err(|_| CheckpointError::Truncated)?,
    ))
}

// FNV is bounded accidental-corruption evidence, not authentication.
fn checksum(bytes: &[u8]) -> u64 {
    let mut value = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        value ^= u64::from(*byte);
        value = value.wrapping_mul(0x0000_0100_0000_01b3);
    }
    value
}

fn command_order_key(command: &PlayerCommand) -> (PlayerId, u32, ActorId) {
    (command.player, command.sequence, command.actor)
}

fn encode_config(encoder: &mut Encoder, config: MatchConfig) -> Result<(), CheckpointError> {
    encoder.u16(config.tick_rate_hz)?;
    encode_point(encoder, config.map_bounds.min)?;
    encode_point(encoder, config.map_bounds.max)?;
    encoder.u32(config.max_players)?;
    encoder.u32(config.max_actors)?;
    encoder.u32(config.max_abilities_per_actor)?;
    encoder.u32(config.max_commands_per_tick)?;
    encoder.u32(config.max_commands_per_player_per_tick)?;
    encoder.u32(config.max_rejection_events_per_frame)?;
    encoder.u16(config.max_command_lead_ticks)?;
    encoder.u64(config.max_match_ticks)?;
    encoder.u16(config.max_lanes)?;
    encoder.u16(config.max_lane_points_per_lane)?;
    encoder.u16(config.max_wave_schedules)?;
    encoder.u16(config.max_units_per_wave)?;
    encoder.u16(config.max_modifiers_per_actor)?;
    encoder.u16(config.max_status_effects_per_actor)?;
    encoder.u16(config.max_projectiles)?;
    encoder.u16(config.max_score_entries)?;
    encoder.u16(config.max_damage_credits_per_actor)?;
    encoder.u32(config.assist_window_ticks)?;
    encoder.u32(config.xp_share_radius_mm)?;
    encoder.u32(config.passive_gold_per_100s)?;
    encoder.u32(config.max_checkpoint_bytes)
}

fn decode_config(decoder: &mut Decoder<'_>) -> Result<MatchConfig, CheckpointError> {
    Ok(MatchConfig {
        tick_rate_hz: decoder.u16()?,
        map_bounds: RectMm::new(decode_point(decoder)?, decode_point(decoder)?),
        max_players: decoder.u32()?,
        max_actors: decoder.u32()?,
        max_abilities_per_actor: decoder.u32()?,
        max_commands_per_tick: decoder.u32()?,
        max_commands_per_player_per_tick: decoder.u32()?,
        max_rejection_events_per_frame: decoder.u32()?,
        max_command_lead_ticks: decoder.u16()?,
        max_match_ticks: decoder.u64()?,
        max_lanes: decoder.u16()?,
        max_lane_points_per_lane: decoder.u16()?,
        max_wave_schedules: decoder.u16()?,
        max_units_per_wave: decoder.u16()?,
        max_modifiers_per_actor: decoder.u16()?,
        max_status_effects_per_actor: decoder.u16()?,
        max_projectiles: decoder.u16()?,
        max_score_entries: decoder.u16()?,
        max_damage_credits_per_actor: decoder.u16()?,
        assist_window_ticks: decoder.u32()?,
        xp_share_radius_mm: decoder.u32()?,
        passive_gold_per_100s: decoder.u32()?,
        max_checkpoint_bytes: decoder.u32()?,
    })
}

fn encode_point(encoder: &mut Encoder, point: Vec2Mm) -> Result<(), CheckpointError> {
    encoder.i32(point.x)?;
    encoder.i32(point.y)
}

fn decode_point(decoder: &mut Decoder<'_>) -> Result<Vec2Mm, CheckpointError> {
    Ok(Vec2Mm::new(decoder.i32()?, decoder.i32()?))
}

fn encode_phase(encoder: &mut Encoder, phase: MatchPhase) -> Result<(), CheckpointError> {
    match phase {
        MatchPhase::Setup => encoder.u8(0),
        MatchPhase::Active => encoder.u8(1),
        MatchPhase::Complete { outcome, reason } => {
            encoder.u8(2)?;
            encode_outcome(encoder, outcome)?;
            encode_end_reason(encoder, reason)
        }
    }
}

fn decode_phase(decoder: &mut Decoder<'_>) -> Result<MatchPhase, CheckpointError> {
    match decoder.u8()? {
        0 => Ok(MatchPhase::Setup),
        1 => Ok(MatchPhase::Active),
        2 => Ok(MatchPhase::Complete {
            outcome: decode_outcome(decoder)?,
            reason: decode_end_reason(decoder)?,
        }),
        tag => Err(CheckpointError::InvalidTag {
            field: "match phase",
            tag,
        }),
    }
}

fn encode_outcome(encoder: &mut Encoder, outcome: MatchOutcome) -> Result<(), CheckpointError> {
    match outcome {
        MatchOutcome::Winner(team) => {
            encoder.u8(0)?;
            encoder.u8(team.get())
        }
        MatchOutcome::Draw => encoder.u8(1),
    }
}

fn decode_outcome(decoder: &mut Decoder<'_>) -> Result<MatchOutcome, CheckpointError> {
    match decoder.u8()? {
        0 => Ok(MatchOutcome::Winner(TeamId(decoder.u8()?))),
        1 => Ok(MatchOutcome::Draw),
        tag => Err(CheckpointError::InvalidTag {
            field: "match outcome",
            tag,
        }),
    }
}

fn encode_end_reason(encoder: &mut Encoder, reason: MatchEndReason) -> Result<(), CheckpointError> {
    encoder.u8(match reason {
        MatchEndReason::ObjectiveDestroyed => 0,
        MatchEndReason::TimeLimit => 1,
        MatchEndReason::Host => 2,
    })
}

fn decode_end_reason(decoder: &mut Decoder<'_>) -> Result<MatchEndReason, CheckpointError> {
    match decoder.u8()? {
        0 => Ok(MatchEndReason::ObjectiveDestroyed),
        1 => Ok(MatchEndReason::TimeLimit),
        2 => Ok(MatchEndReason::Host),
        tag => Err(CheckpointError::InvalidTag {
            field: "match end reason",
            tag,
        }),
    }
}

fn encode_option_u64(encoder: &mut Encoder, value: Option<u64>) -> Result<(), CheckpointError> {
    match value {
        Some(value) => {
            encoder.u8(1)?;
            encoder.u64(value)
        }
        None => encoder.u8(0),
    }
}

fn decode_option_u64(
    decoder: &mut Decoder<'_>,
    field: &'static str,
) -> Result<Option<u64>, CheckpointError> {
    match decoder.u8()? {
        0 => Ok(None),
        1 => Ok(Some(decoder.u64()?)),
        tag => Err(CheckpointError::InvalidTag { field, tag }),
    }
}

fn encode_ability(encoder: &mut Encoder, ability: AbilitySpec) -> Result<(), CheckpointError> {
    encoder.u32(ability.id.get())?;
    encoder.u8(match ability.aim {
        AbilityAim::Unit => 0,
        AbilityAim::Point => 1,
    })?;
    encoder.u8(match ability.targeting {
        AbilityTargeting::SelfOnly => 0,
        AbilityTargeting::Ally => 1,
        AbilityTargeting::Enemy => 2,
        AbilityTargeting::AnyUnit => 3,
    })?;
    encoder.u32(ability.range_mm)?;
    encoder.u32(ability.resource_cost)?;
    encoder.u32(ability.cooldown_ticks)?;
    encoder.u16(ability.cast_ticks)?;
    encoder.u32(ability.per_rank_amount)?;
    match ability.delivery {
        AbilityDelivery::Instant => encoder.u8(0)?,
        AbilityDelivery::Projectile {
            speed_mm_per_tick,
            hit_radius_mm,
        } => {
            encoder.u8(1)?;
            encoder.u32(speed_mm_per_tick)?;
            encoder.u32(hit_radius_mm)?;
        }
    }
    match ability.impact {
        ImpactShape::Single => encoder.u8(0)?,
        ImpactShape::Circle { radius_mm } => {
            encoder.u8(1)?;
            encoder.u32(radius_mm)?;
        }
    }
    match ability.effect {
        AbilityEffect::Damage { amount, school } => {
            encoder.u8(0)?;
            encoder.u32(amount)?;
            encode_damage_school(encoder, school)
        }
        AbilityEffect::Heal { amount } => {
            encoder.u8(1)?;
            encoder.u32(amount)
        }
    }
}

fn decode_ability(decoder: &mut Decoder<'_>) -> Result<AbilitySpec, CheckpointError> {
    let id = AbilityId(decoder.u32()?);
    let aim = match decoder.u8()? {
        0 => AbilityAim::Unit,
        1 => AbilityAim::Point,
        tag => {
            return Err(CheckpointError::InvalidTag {
                field: "ability aim",
                tag,
            });
        }
    };
    let targeting = match decoder.u8()? {
        0 => AbilityTargeting::SelfOnly,
        1 => AbilityTargeting::Ally,
        2 => AbilityTargeting::Enemy,
        3 => AbilityTargeting::AnyUnit,
        tag => {
            return Err(CheckpointError::InvalidTag {
                field: "ability targeting",
                tag,
            });
        }
    };
    let range_mm = decoder.u32()?;
    let resource_cost = decoder.u32()?;
    let cooldown_ticks = decoder.u32()?;
    let cast_ticks = decoder.u16()?;
    let per_rank_amount = decoder.u32()?;
    let delivery = match decoder.u8()? {
        0 => AbilityDelivery::Instant,
        1 => AbilityDelivery::Projectile {
            speed_mm_per_tick: decoder.u32()?,
            hit_radius_mm: decoder.u32()?,
        },
        tag => {
            return Err(CheckpointError::InvalidTag {
                field: "ability delivery",
                tag,
            });
        }
    };
    let impact = match decoder.u8()? {
        0 => ImpactShape::Single,
        1 => ImpactShape::Circle {
            radius_mm: decoder.u32()?,
        },
        tag => {
            return Err(CheckpointError::InvalidTag {
                field: "ability impact",
                tag,
            });
        }
    };
    let effect = match decoder.u8()? {
        0 => AbilityEffect::Damage {
            amount: decoder.u32()?,
            school: decode_damage_school(decoder)?,
        },
        1 => AbilityEffect::Heal {
            amount: decoder.u32()?,
        },
        tag => {
            return Err(CheckpointError::InvalidTag {
                field: "ability effect",
                tag,
            });
        }
    };
    Ok(AbilitySpec {
        id,
        aim,
        targeting,
        range_mm,
        resource_cost,
        cooldown_ticks,
        cast_ticks,
        delivery,
        impact,
        effect,
        per_rank_amount,
    })
}

fn encode_growth(encoder: &mut Encoder, growth: StatGrowth) -> Result<(), CheckpointError> {
    encoder.u32(growth.max_health_per_level)?;
    encoder.u32(growth.max_resource_per_level)?;
    encoder.u32(growth.move_speed_mm_per_tick_per_level)?;
    encoder.u16(growth.physical_reduction_bps_per_level)?;
    encoder.u16(growth.magic_reduction_bps_per_level)
}

fn decode_growth(decoder: &mut Decoder<'_>) -> Result<StatGrowth, CheckpointError> {
    Ok(StatGrowth {
        max_health_per_level: decoder.u32()?,
        max_resource_per_level: decoder.u32()?,
        move_speed_mm_per_tick_per_level: decoder.u32()?,
        physical_reduction_bps_per_level: decoder.u16()?,
        magic_reduction_bps_per_level: decoder.u16()?,
    })
}

fn encode_bounty(encoder: &mut Encoder, bounty: Bounty) -> Result<(), CheckpointError> {
    encoder.u32(bounty.gold)?;
    encoder.u32(bounty.experience)
}

fn decode_bounty(decoder: &mut Decoder<'_>) -> Result<Bounty, CheckpointError> {
    Ok(Bounty {
        gold: decoder.u32()?,
        experience: decoder.u32()?,
    })
}

/// Decode one actor's assist window. Strict source ordering is enforced here because that order is what
/// makes the assist set a function of the SET of contributors rather than of decode order.
fn decode_damage_credits(
    decoder: &mut Decoder<'_>,
    config: MatchConfig,
) -> Result<Vec<DamageCredit>, CheckpointError> {
    let count = decoder.count(
        usize::from(config.max_damage_credits_per_actor),
        "actor damage credit count exceeds configured budget",
    )?;
    decoder.require_minimum(count, MIN_DAMAGE_CREDIT_BYTES)?;
    let mut credits = allocate_vec(count)?;
    for _ in 0..count {
        credits.push(DamageCredit {
            source: ActorId(decoder.u64()?),
            last_tick: decoder.u64()?,
        });
    }
    if credits
        .windows(2)
        .any(|pair: &[DamageCredit]| pair[0].source >= pair[1].source)
    {
        return Err(CheckpointError::InvalidData(
            "damage credits are not strictly sorted by source",
        ));
    }
    Ok(credits)
}

fn encode_score_entry(encoder: &mut Encoder, entry: ScoreEntry) -> Result<(), CheckpointError> {
    encoder.u64(entry.actor.get())?;
    encode_option_player(encoder, entry.owner)?;
    encoder.u8(entry.team.get())?;
    encoder.u64(entry.since_tick)?;
    encoder.u32(entry.earned_gold)?;
    // `passive_gold_paid` is deliberately NOT encoded: it is a closed form of the tick, so re-deriving it
    // on decode makes the entire passive half of the economy impossible to forge.
    encoder.u32(entry.kills)?;
    encoder.u32(entry.deaths)?;
    encoder.u32(entry.assists)?;
    encoder.u16(entry.kill_streak)?;
    encoder.u16(entry.best_kill_streak)
}

fn decode_score_entry(
    decoder: &mut Decoder<'_>,
    tick: Tick,
    config: MatchConfig,
) -> Result<ScoreEntry, CheckpointError> {
    let actor = ActorId(decoder.u64()?);
    let owner = decode_option_player(decoder)?;
    let team = TeamId(decoder.u8()?);
    let since_tick = decoder.u64()?;
    if since_tick > tick {
        return Err(CheckpointError::InvalidData(
            "scoreboard entry claims to predate the match",
        ));
    }
    let earned_gold = decoder.u32()?;
    let kills = decoder.u32()?;
    let deaths = decoder.u32()?;
    let assists = decoder.u32()?;
    let kill_streak = decoder.u16()?;
    let best_kill_streak = decoder.u16()?;
    Ok(ScoreEntry {
        actor,
        owner,
        team,
        since_tick,
        earned_gold,
        passive_gold_paid: passive_gold_owed(tick.saturating_sub(since_tick), config),
        kills,
        deaths,
        assists,
        kill_streak,
        best_kill_streak,
    })
}

fn encode_projectile(
    encoder: &mut Encoder,
    projectile: ProjectileState,
) -> Result<(), CheckpointError> {
    encoder.u64(projectile.id.get())?;
    encoder.u64(projectile.source.get())?;
    encoder.u8(projectile.team.get())?;
    encoder.u32(projectile.ability.get())?;
    encode_point(encoder, projectile.position)?;
    encode_point(encoder, projectile.end)?;
    encoder.u32(projectile.amount)
}

/// Decode one in-flight projectile.
///
/// Bounds are checked here rather than left to the frame audit so a hostile payload cannot place a missile
/// off the map and have it swept against every actor before anything rejects it. Its ability reference is
/// validated later, by the same frame-boundary audit that guards a live match, because the ability table has
/// not finished decoding at this point.
fn decode_projectile(
    decoder: &mut Decoder<'_>,
    config: MatchConfig,
) -> Result<ProjectileState, CheckpointError> {
    let id = ProjectileId(decoder.u64()?);
    let source = ActorId(decoder.u64()?);
    let team = TeamId(decoder.u8()?);
    let ability = AbilityId(decoder.u32()?);
    let position = decode_point(decoder)?;
    let end = decode_point(decoder)?;
    let amount = decoder.u32()?;
    if !config.map_bounds.contains(position) || !config.map_bounds.contains(end) {
        return Err(CheckpointError::InvalidData(
            "projectile position or flight terminus is outside map bounds",
        ));
    }
    Ok(ProjectileState {
        id,
        source,
        team,
        ability,
        position,
        end,
        amount,
    })
}

fn encode_actor(encoder: &mut Encoder, actor: &ActorState) -> Result<(), CheckpointError> {
    encoder.u64(actor.id.get())?;
    encode_provenance(encoder, actor.provenance)?;
    encode_option_player(encoder, actor.owner)?;
    encoder.u8(actor.team.get())?;
    encode_actor_kind(encoder, actor.kind)?;
    encode_point(encoder, actor.position)?;
    encode_option_point(encoder, actor.destination)?;
    // AUTHORED stats plus authored growth plus experience. `level` and `base_stats` are both re-derived on
    // decode and then re-asserted by the frame-boundary audit, so a tampered checkpoint cannot assert a
    // level, a growth curve, or stats its own inputs do not produce.
    encode_stats(encoder, actor.authored_stats)?;
    encode_growth(encoder, actor.growth)?;
    encode_bounty(encoder, actor.bounty)?;
    encoder.u32(actor.experience)?;
    encoder.u8(actor.unspent_ability_points)?;
    encoder.u16(actor.crit_attempts)?;
    encoder.count(actor.damage_credits.len())?;
    for credit in &actor.damage_credits {
        encoder.u64(credit.source.get())?;
        encoder.u64(credit.last_tick)?;
    }
    encoder.count(actor.modifiers.len())?;
    for modifier in &actor.modifiers {
        encoder.u64(modifier.id.get())?;
        encode_stat_kind(encoder, modifier.kind)?;
        encode_modifier_op(encoder, modifier.op)?;
    }
    encoder.count(actor.status_effects.len())?;
    for effect in &actor.status_effects {
        encoder.u64(effect.id.get())?;
        encoder.u64(effect.expires_at)?;
        encoder.u32(effect.shield_pool)?;
        encoder.u8(effect.controls.bits())?;
        encoder.count(effect.modifiers.len())?;
        for owned in &effect.modifiers {
            encoder.u64(owned.get())?;
        }
    }
    encoder.u32(actor.health)?;
    encoder.u32(actor.resource)?;
    encoder.count(actor.abilities.len())?;
    for ability in &actor.abilities {
        encoder.u32(ability.id.get())?;
        encoder.u64(ability.ready_at)?;
        encoder.u8(ability.rank)?;
    }
    match actor.cast {
        Some(cast) => {
            encoder.u8(1)?;
            encoder.u32(cast.ability.get())?;
            encode_cast_target(encoder, cast.target)?;
            encoder.u64(cast.resolves_at)?;
        }
        None => encoder.u8(0)?,
    }
    match actor.basic_attack {
        Some(attack) => {
            encoder.u8(1)?;
            encode_basic_attack(encoder, attack.spec)?;
            encoder.u64(attack.ready_at)?;
        }
        None => encoder.u8(0)?,
    }
    match actor.attack {
        Some(attack) => {
            encoder.u8(1)?;
            encoder.u64(attack.target.get())?;
            encoder.u64(attack.resolves_at)?;
        }
        None => encoder.u8(0)?,
    }
    encode_death_rule(encoder, actor.death_rule)?;
    encode_option_u64(encoder, actor.respawn_at)
}

#[expect(
    clippy::too_many_lines,
    reason = "the decoder must mirror the encoder field for field, in one readable sequence"
)]
fn decode_actor(
    decoder: &mut Decoder<'_>,
    config: MatchConfig,
) -> Result<ActorState, CheckpointError> {
    let id = ActorId(decoder.u64()?);
    let provenance = decode_provenance(decoder)?;
    let owner = decode_option_player(decoder)?;
    let team = TeamId(decoder.u8()?);
    let kind = decode_actor_kind(decoder)?;
    let position = decode_point(decoder)?;
    let destination = decode_option_point(decoder)?;
    let authored_stats = decode_stats(decoder)?;
    let growth = decode_growth(decoder)?;
    let bounty = decode_bounty(decoder)?;
    let experience = decoder.u32()?;
    let unspent_ability_points = decoder.u8()?;
    let crit_attempts = decoder.u16()?;
    let damage_credits = decode_damage_credits(decoder, config)?;
    // Re-derived, never trusted from the payload - the same rule the stat and control caches follow.
    let level = level_for_experience(experience);
    let base_stats = authored_stats.with_growth(growth, level);
    let modifiers = decode_modifiers(decoder, config)?;
    let stats = base_stats.with_modifiers(&modifiers);
    let status_effects = decode_status_effects(decoder, config)?;
    // Re-derived, never trusted from the payload - same rule as the stat cache.
    let control_mask = status_effects
        .iter()
        .fold(ControlMask::EMPTY, |mask, effect| {
            mask.union(effect.controls)
        });
    let health = decoder.u32()?;
    let resource = decoder.u32()?;
    let ability_limit = usize::try_from(config.max_abilities_per_actor).unwrap_or(usize::MAX);
    let ability_count = decoder.count(ability_limit, "actor ability count exceeds budget")?;
    decoder.require_minimum(ability_count, 13)?;
    let mut abilities = allocate_vec(ability_count)?;
    for _ in 0..ability_count {
        abilities.push(AbilityState {
            id: AbilityId(decoder.u32()?),
            ready_at: decoder.u64()?,
            rank: decoder.u8()?,
        });
    }
    let cast = match decoder.u8()? {
        0 => None,
        1 => Some(PendingCast {
            ability: AbilityId(decoder.u32()?),
            target: decode_cast_target(decoder)?,
            resolves_at: decoder.u64()?,
        }),
        tag => {
            return Err(CheckpointError::InvalidTag {
                field: "pending cast option",
                tag,
            });
        }
    };
    let basic_attack = match decoder.u8()? {
        0 => None,
        1 => Some(BasicAttackState {
            spec: decode_basic_attack(decoder)?,
            ready_at: decoder.u64()?,
        }),
        tag => {
            return Err(CheckpointError::InvalidTag {
                field: "basic attack option",
                tag,
            });
        }
    };
    let attack = match decoder.u8()? {
        0 => None,
        1 => Some(PendingAttack {
            target: ActorId(decoder.u64()?),
            resolves_at: decoder.u64()?,
        }),
        tag => {
            return Err(CheckpointError::InvalidTag {
                field: "pending attack option",
                tag,
            });
        }
    };
    let death_rule = decode_death_rule(decoder)?;
    if let DeathRule::Respawn { at, .. } = death_rule {
        if !config.map_bounds.contains(at) {
            return Err(CheckpointError::InvalidData(
                "respawn point is outside map bounds",
            ));
        }
    }
    let respawn_at = decode_option_u64(decoder, "respawn deadline option")?;
    Ok(ActorState {
        id,
        owner,
        team,
        kind,
        position,
        destination,
        authored_stats,
        growth,
        bounty,
        experience,
        level,
        unspent_ability_points,
        crit_attempts,
        damage_credits,
        base_stats,
        modifiers,
        status_effects,
        control_mask,
        stats,
        health,
        resource,
        abilities,
        cast,
        basic_attack,
        attack,
        death_rule,
        respawn_at,
        provenance,
    })
}

/// Decode one actor's modifier list. Strict ID ordering is enforced here because that order defines the
/// aggregation order, so an unsorted payload would resolve to different stats than the one that wrote it.
fn decode_modifiers(
    decoder: &mut Decoder<'_>,
    config: MatchConfig,
) -> Result<Vec<StatModifier>, CheckpointError> {
    let limit = usize::from(config.max_modifiers_per_actor);
    let count = decoder.count(limit, "actor modifier count exceeds budget")?;
    decoder.require_minimum(count, MIN_MODIFIER_BYTES)?;
    let mut modifiers = allocate_vec(count)?;
    for _ in 0..count {
        modifiers.push(StatModifier {
            id: ModifierId(decoder.u64()?),
            kind: decode_stat_kind(decoder)?,
            op: decode_modifier_op(decoder)?,
        });
    }
    if modifiers.windows(2).any(|pair| pair[0].id >= pair[1].id) {
        return Err(CheckpointError::InvalidData(
            "actor modifiers are not strictly sorted by identity",
        ));
    }
    Ok(modifiers)
}

/// Decode one actor's timed status effects.
///
/// Split out of `decode_actor` so that function stays inside its line budget, and because the ownership rule
/// is worth stating on its own: each effect names the modifiers it granted, and strict ID ordering is
/// enforced so a payload cannot reorder effects into an expiry sequence different from the one that wrote it.
fn decode_status_effects(
    decoder: &mut Decoder<'_>,
    config: MatchConfig,
) -> Result<Vec<StatusEffect>, CheckpointError> {
    let limit = usize::from(config.max_status_effects_per_actor);
    let count = decoder.count(limit, "actor status effect count exceeds budget")?;
    decoder.require_minimum(count, MIN_STATUS_EFFECT_BYTES)?;
    let mut effects = allocate_vec(count)?;
    for _ in 0..count {
        let id = StatusEffectId(decoder.u64()?);
        let expires_at = decoder.u64()?;
        let shield_pool = decoder.u32()?;
        let controls = ControlMask::from_bits(decoder.u8()?).ok_or(
            CheckpointError::InvalidData("status effect carries an undefined control bit"),
        )?;
        let granted_count = decoder.count(
            usize::from(config.max_modifiers_per_actor),
            "status effect modifier count exceeds budget",
        )?;
        decoder.require_minimum(granted_count, 8)?;
        let mut granted = allocate_vec(granted_count)?;
        for _ in 0..granted_count {
            granted.push(ModifierId(decoder.u64()?));
        }
        effects.push(StatusEffect {
            id,
            shield_pool,
            expires_at,
            controls,
            modifiers: granted,
        });
    }
    if effects.windows(2).any(|pair| pair[0].id >= pair[1].id) {
        return Err(CheckpointError::InvalidData(
            "actor status effects are not strictly sorted by identity",
        ));
    }
    Ok(effects)
}

fn encode_stat_kind(encoder: &mut Encoder, kind: StatKind) -> Result<(), CheckpointError> {
    encoder.u8(match kind {
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
    })
}

fn decode_stat_kind(decoder: &mut Decoder<'_>) -> Result<StatKind, CheckpointError> {
    match decoder.u8()? {
        0 => Ok(StatKind::MaxHealth),
        1 => Ok(StatKind::MaxResource),
        2 => Ok(StatKind::MoveSpeed),
        3 => Ok(StatKind::PhysicalReduction),
        4 => Ok(StatKind::MagicReduction),
        5 => Ok(StatKind::CritChance),
        6 => Ok(StatKind::CritDamage),
        7 => Ok(StatKind::PhysicalPenetration),
        8 => Ok(StatKind::MagicPenetration),
        9 => Ok(StatKind::Lifesteal),
        tag => Err(CheckpointError::InvalidTag {
            field: "stat kind",
            tag,
        }),
    }
}

fn encode_modifier_op(encoder: &mut Encoder, op: ModifierOp) -> Result<(), CheckpointError> {
    match op {
        ModifierOp::Flat { magnitude } => {
            encoder.u8(0)?;
            encoder.i32(magnitude)
        }
        ModifierOp::PercentAdd { bps } => {
            encoder.u8(1)?;
            encoder.i32(bps)
        }
        ModifierOp::PercentMult { bps } => {
            encoder.u8(2)?;
            encoder.i32(bps)
        }
        ModifierOp::Override { value } => {
            encoder.u8(3)?;
            encoder.u32(value)
        }
    }
}

fn decode_modifier_op(decoder: &mut Decoder<'_>) -> Result<ModifierOp, CheckpointError> {
    match decoder.u8()? {
        0 => Ok(ModifierOp::Flat {
            magnitude: decoder.i32()?,
        }),
        1 => Ok(ModifierOp::PercentAdd {
            bps: decoder.i32()?,
        }),
        2 => Ok(ModifierOp::PercentMult {
            bps: decoder.i32()?,
        }),
        3 => Ok(ModifierOp::Override {
            value: decoder.u32()?,
        }),
        tag => Err(CheckpointError::InvalidTag {
            field: "stat modifier operation",
            tag,
        }),
    }
}

fn encode_provenance(
    encoder: &mut Encoder,
    provenance: ActorProvenance,
) -> Result<(), CheckpointError> {
    match provenance {
        ActorProvenance::Authored => encoder.u8(0),
        ActorProvenance::Dynamic { spawn_ordinal } => {
            encoder.u8(1)?;
            encoder.u64(spawn_ordinal)
        }
        ActorProvenance::Wave {
            wave_id,
            unit_slot,
            spawn_ordinal,
        } => {
            encoder.u8(2)?;
            encoder.u32(wave_id)?;
            encoder.u16(unit_slot)?;
            encoder.u64(spawn_ordinal)
        }
    }
}

fn decode_provenance(decoder: &mut Decoder<'_>) -> Result<ActorProvenance, CheckpointError> {
    match decoder.u8()? {
        0 => Ok(ActorProvenance::Authored),
        1 => Ok(ActorProvenance::Dynamic {
            spawn_ordinal: decoder.u64()?,
        }),
        2 => Ok(ActorProvenance::Wave {
            wave_id: decoder.u32()?,
            unit_slot: decoder.u16()?,
            spawn_ordinal: decoder.u64()?,
        }),
        tag => Err(CheckpointError::InvalidTag {
            field: "actor provenance",
            tag,
        }),
    }
}

fn encode_option_player(
    encoder: &mut Encoder,
    player: Option<PlayerId>,
) -> Result<(), CheckpointError> {
    match player {
        Some(player) => {
            encoder.u8(1)?;
            encoder.u64(player.get())
        }
        None => encoder.u8(0),
    }
}

fn decode_option_player(decoder: &mut Decoder<'_>) -> Result<Option<PlayerId>, CheckpointError> {
    match decoder.u8()? {
        0 => Ok(None),
        1 => Ok(Some(PlayerId(decoder.u64()?))),
        tag => Err(CheckpointError::InvalidTag {
            field: "actor owner option",
            tag,
        }),
    }
}

fn encode_option_point(
    encoder: &mut Encoder,
    point: Option<Vec2Mm>,
) -> Result<(), CheckpointError> {
    match point {
        Some(point) => {
            encoder.u8(1)?;
            encode_point(encoder, point)
        }
        None => encoder.u8(0),
    }
}

fn decode_option_point(decoder: &mut Decoder<'_>) -> Result<Option<Vec2Mm>, CheckpointError> {
    match decoder.u8()? {
        0 => Ok(None),
        1 => Ok(Some(decode_point(decoder)?)),
        tag => Err(CheckpointError::InvalidTag {
            field: "destination option",
            tag,
        }),
    }
}

fn encode_actor_kind(encoder: &mut Encoder, kind: ActorKind) -> Result<(), CheckpointError> {
    encoder.u8(match kind {
        ActorKind::Hero => 0,
        ActorKind::Minion => 1,
        ActorKind::Structure => 2,
        ActorKind::Objective => 3,
        ActorKind::Pet => 4,
        ActorKind::Projectile => 5,
    })
}

fn decode_actor_kind(decoder: &mut Decoder<'_>) -> Result<ActorKind, CheckpointError> {
    match decoder.u8()? {
        0 => Ok(ActorKind::Hero),
        1 => Ok(ActorKind::Minion),
        2 => Ok(ActorKind::Structure),
        3 => Ok(ActorKind::Objective),
        4 => Ok(ActorKind::Pet),
        5 => Ok(ActorKind::Projectile),
        tag => Err(CheckpointError::InvalidTag {
            field: "actor kind",
            tag,
        }),
    }
}

fn encode_stats(encoder: &mut Encoder, stats: CombatStats) -> Result<(), CheckpointError> {
    encoder.u32(stats.max_health)?;
    encoder.u32(stats.max_resource)?;
    encoder.u32(stats.move_speed_mm_per_tick)?;
    encoder.u16(stats.physical_reduction_bps)?;
    encoder.u16(stats.magic_reduction_bps)?;
    encoder.u16(stats.crit_chance_bps)?;
    encoder.u16(stats.crit_damage_bps)?;
    encoder.u16(stats.physical_penetration_bps)?;
    encoder.u16(stats.magic_penetration_bps)?;
    encoder.u16(stats.lifesteal_bps)
}

fn decode_stats(decoder: &mut Decoder<'_>) -> Result<CombatStats, CheckpointError> {
    Ok(CombatStats {
        max_health: decoder.u32()?,
        max_resource: decoder.u32()?,
        move_speed_mm_per_tick: decoder.u32()?,
        physical_reduction_bps: decoder.u16()?,
        magic_reduction_bps: decoder.u16()?,
        crit_chance_bps: decoder.u16()?,
        crit_damage_bps: decoder.u16()?,
        physical_penetration_bps: decoder.u16()?,
        magic_penetration_bps: decoder.u16()?,
        lifesteal_bps: decoder.u16()?,
    })
}

fn encode_basic_attack(
    encoder: &mut Encoder,
    attack: BasicAttackSpec,
) -> Result<(), CheckpointError> {
    encoder.u32(attack.range_mm)?;
    encoder.u32(attack.damage)?;
    encode_damage_school(encoder, attack.school)?;
    encoder.u16(attack.windup_ticks)?;
    encoder.u32(attack.cooldown_ticks)
}

fn decode_basic_attack(decoder: &mut Decoder<'_>) -> Result<BasicAttackSpec, CheckpointError> {
    Ok(BasicAttackSpec {
        range_mm: decoder.u32()?,
        damage: decoder.u32()?,
        school: decode_damage_school(decoder)?,
        windup_ticks: decoder.u16()?,
        cooldown_ticks: decoder.u32()?,
    })
}

fn encode_damage_school(
    encoder: &mut Encoder,
    school: DamageSchool,
) -> Result<(), CheckpointError> {
    encoder.u8(match school {
        DamageSchool::Physical => 0,
        DamageSchool::Magic => 1,
        DamageSchool::True => 2,
    })
}

fn decode_damage_school(decoder: &mut Decoder<'_>) -> Result<DamageSchool, CheckpointError> {
    match decoder.u8()? {
        0 => Ok(DamageSchool::Physical),
        1 => Ok(DamageSchool::Magic),
        2 => Ok(DamageSchool::True),
        tag => Err(CheckpointError::InvalidTag {
            field: "damage school",
            tag,
        }),
    }
}

fn encode_death_rule(encoder: &mut Encoder, rule: DeathRule) -> Result<(), CheckpointError> {
    match rule {
        DeathRule::StayDead => encoder.u8(0),
        DeathRule::Despawn => encoder.u8(1),
        DeathRule::Respawn { delay_ticks, at } => {
            encoder.u8(2)?;
            encoder.u32(delay_ticks)?;
            encode_point(encoder, at)
        }
        DeathRule::ObjectiveVictory { winner } => {
            encoder.u8(3)?;
            encoder.u8(winner.get())
        }
    }
}

fn decode_death_rule(decoder: &mut Decoder<'_>) -> Result<DeathRule, CheckpointError> {
    match decoder.u8()? {
        0 => Ok(DeathRule::StayDead),
        1 => Ok(DeathRule::Despawn),
        2 => Ok(DeathRule::Respawn {
            delay_ticks: decoder.u32()?,
            at: decode_point(decoder)?,
        }),
        3 => Ok(DeathRule::ObjectiveVictory {
            winner: TeamId(decoder.u8()?),
        }),
        tag => Err(CheckpointError::InvalidTag {
            field: "death rule",
            tag,
        }),
    }
}

fn encode_cast_target(encoder: &mut Encoder, target: CastTarget) -> Result<(), CheckpointError> {
    match target {
        CastTarget::SelfActor => encoder.u8(0),
        CastTarget::Actor(actor) => {
            encoder.u8(1)?;
            encoder.u64(actor.get())
        }
        CastTarget::Point(point) => {
            encoder.u8(2)?;
            encode_point(encoder, point)
        }
    }
}

fn decode_cast_target(decoder: &mut Decoder<'_>) -> Result<CastTarget, CheckpointError> {
    match decoder.u8()? {
        0 => Ok(CastTarget::SelfActor),
        1 => Ok(CastTarget::Actor(ActorId(decoder.u64()?))),
        2 => Ok(CastTarget::Point(decode_point(decoder)?)),
        tag => Err(CheckpointError::InvalidTag {
            field: "cast target",
            tag,
        }),
    }
}

fn encode_command(encoder: &mut Encoder, command: PlayerCommand) -> Result<(), CheckpointError> {
    encoder.u64(command.player.get())?;
    encoder.u32(command.sequence)?;
    encoder.u64(command.execute_at)?;
    encoder.u64(command.actor.get())?;
    match command.kind {
        CommandKind::MoveTo { destination } => {
            encoder.u8(0)?;
            encode_point(encoder, destination)
        }
        CommandKind::Stop => encoder.u8(1),
        CommandKind::Cast { ability, target } => {
            encoder.u8(2)?;
            encoder.u32(ability.get())?;
            encode_cast_target(encoder, target)
        }
        CommandKind::BasicAttack { target } => {
            encoder.u8(3)?;
            encoder.u64(target.get())
        }
        CommandKind::UpgradeAbility { ability } => {
            encoder.u8(4)?;
            encoder.u32(ability.get())
        }
    }
}

fn decode_command(decoder: &mut Decoder<'_>) -> Result<PlayerCommand, CheckpointError> {
    let player = PlayerId(decoder.u64()?);
    let sequence = decoder.u32()?;
    let execute_at = decoder.u64()?;
    let actor = ActorId(decoder.u64()?);
    let kind = match decoder.u8()? {
        0 => CommandKind::MoveTo {
            destination: decode_point(decoder)?,
        },
        1 => CommandKind::Stop,
        2 => CommandKind::Cast {
            ability: AbilityId(decoder.u32()?),
            target: decode_cast_target(decoder)?,
        },
        3 => CommandKind::BasicAttack {
            target: ActorId(decoder.u64()?),
        },
        tag => {
            return Err(CheckpointError::InvalidTag {
                field: "command kind",
                tag,
            });
        }
    };
    Ok(PlayerCommand {
        player,
        sequence,
        execute_at,
        actor,
        kind,
    })
}

fn encode_submission_outcome(
    encoder: &mut Encoder,
    outcome: Result<CommandReceipt, CommandRejection>,
) -> Result<(), CheckpointError> {
    match outcome {
        Ok(receipt) => {
            encoder.u8(0)?;
            encoder.u64(receipt.player.get())?;
            encoder.u32(receipt.sequence)?;
            encoder.u64(receipt.execute_at)
        }
        Err(rejection) => {
            encoder.u8(1)?;
            encoder.u64(rejection.player.get())?;
            encoder.u32(rejection.sequence)?;
            encode_rejection_reason(encoder, rejection.reason)
        }
    }
}

fn decode_submission_outcome(
    decoder: &mut Decoder<'_>,
) -> Result<Result<CommandReceipt, CommandRejection>, CheckpointError> {
    match decoder.u8()? {
        0 => Ok(Ok(CommandReceipt {
            player: PlayerId(decoder.u64()?),
            sequence: decoder.u32()?,
            execute_at: decoder.u64()?,
        })),
        1 => Ok(Err(CommandRejection {
            player: PlayerId(decoder.u64()?),
            sequence: decoder.u32()?,
            reason: decode_rejection_reason(decoder)?,
        })),
        tag => Err(CheckpointError::InvalidTag {
            field: "submission outcome",
            tag,
        }),
    }
}

fn encode_rejection_reason(
    encoder: &mut Encoder,
    reason: CommandRejectReason,
) -> Result<(), CheckpointError> {
    match reason {
        CommandRejectReason::MatchNotActive => encoder.u8(0),
        CommandRejectReason::StaleTick => encoder.u8(1),
        CommandRejectReason::TooFarAhead => encoder.u8(2),
        CommandRejectReason::DuplicateOrReorderedSequence => encoder.u8(3),
        CommandRejectReason::PlayerBudgetExceeded => encoder.u8(4),
        CommandRejectReason::PlayerNotRegistered => encoder.u8(5),
        CommandRejectReason::TickCommandBudgetExceeded => encoder.u8(6),
        CommandRejectReason::PlayerTickCommandBudgetExceeded => encoder.u8(7),
        CommandRejectReason::ActorNotFound => encoder.u8(8),
        CommandRejectReason::ActorNotOwned => encoder.u8(9),
        CommandRejectReason::ActorDead => encoder.u8(10),
        CommandRejectReason::DestinationOutOfBounds => encoder.u8(11),
        CommandRejectReason::ActorCasting => encoder.u8(12),
        CommandRejectReason::AbilityNotEquipped => encoder.u8(13),
        CommandRejectReason::AbilityOnCooldown { ready_at } => {
            encoder.u8(14)?;
            encoder.u64(ready_at)
        }
        CommandRejectReason::InsufficientResource {
            required,
            available,
        } => {
            encoder.u8(15)?;
            encoder.u32(required)?;
            encoder.u32(available)
        }
        CommandRejectReason::InvalidTarget => encoder.u8(16),
        CommandRejectReason::TargetNotFound => encoder.u8(17),
        CommandRejectReason::TargetDead => encoder.u8(18),
        CommandRejectReason::TargetRelationMismatch => encoder.u8(19),
        CommandRejectReason::TargetOutOfRange => encoder.u8(20),
        CommandRejectReason::BeyondMatchDuration => encoder.u8(21),
        CommandRejectReason::CastExceedsMatchDuration => encoder.u8(22),
        CommandRejectReason::ActorBusy => encoder.u8(23),
        CommandRejectReason::BasicAttackUnavailable => encoder.u8(24),
        CommandRejectReason::BasicAttackOnCooldown { ready_at } => {
            encoder.u8(25)?;
            encoder.u64(ready_at)
        }
        CommandRejectReason::AttackExceedsMatchDuration => encoder.u8(26),
        CommandRejectReason::ActorStunned => encoder.u8(27),
        CommandRejectReason::ActorRooted => encoder.u8(28),
        CommandRejectReason::ActorSilenced => encoder.u8(29),
        CommandRejectReason::ActorDisarmed => encoder.u8(30),
        CommandRejectReason::InvalidTargetForm => encoder.u8(31),
        CommandRejectReason::TargetPointOutOfBounds => encoder.u8(32),
        CommandRejectReason::ProjectileBudgetExceeded => encoder.u8(33),
        CommandRejectReason::NoAbilityPointAvailable => encoder.u8(34),
        CommandRejectReason::AbilityRankMaxed => encoder.u8(35),
        CommandRejectReason::AbilityRankLocked { unlocks_at_level } => {
            encoder.u8(36)?;
            encoder.u8(unlocks_at_level)
        }
        CommandRejectReason::ActorHasNoProgression => encoder.u8(37),
    }
}

fn decode_rejection_reason(
    decoder: &mut Decoder<'_>,
) -> Result<CommandRejectReason, CheckpointError> {
    match decoder.u8()? {
        0 => Ok(CommandRejectReason::MatchNotActive),
        1 => Ok(CommandRejectReason::StaleTick),
        2 => Ok(CommandRejectReason::TooFarAhead),
        3 => Ok(CommandRejectReason::DuplicateOrReorderedSequence),
        4 => Ok(CommandRejectReason::PlayerBudgetExceeded),
        5 => Ok(CommandRejectReason::PlayerNotRegistered),
        6 => Ok(CommandRejectReason::TickCommandBudgetExceeded),
        7 => Ok(CommandRejectReason::PlayerTickCommandBudgetExceeded),
        8 => Ok(CommandRejectReason::ActorNotFound),
        9 => Ok(CommandRejectReason::ActorNotOwned),
        10 => Ok(CommandRejectReason::ActorDead),
        11 => Ok(CommandRejectReason::DestinationOutOfBounds),
        12 => Ok(CommandRejectReason::ActorCasting),
        13 => Ok(CommandRejectReason::AbilityNotEquipped),
        14 => Ok(CommandRejectReason::AbilityOnCooldown {
            ready_at: decoder.u64()?,
        }),
        15 => Ok(CommandRejectReason::InsufficientResource {
            required: decoder.u32()?,
            available: decoder.u32()?,
        }),
        16 => Ok(CommandRejectReason::InvalidTarget),
        17 => Ok(CommandRejectReason::TargetNotFound),
        18 => Ok(CommandRejectReason::TargetDead),
        19 => Ok(CommandRejectReason::TargetRelationMismatch),
        20 => Ok(CommandRejectReason::TargetOutOfRange),
        21 => Ok(CommandRejectReason::BeyondMatchDuration),
        22 => Ok(CommandRejectReason::CastExceedsMatchDuration),
        23 => Ok(CommandRejectReason::ActorBusy),
        24 => Ok(CommandRejectReason::BasicAttackUnavailable),
        25 => Ok(CommandRejectReason::BasicAttackOnCooldown {
            ready_at: decoder.u64()?,
        }),
        26 => Ok(CommandRejectReason::AttackExceedsMatchDuration),
        27 => Ok(CommandRejectReason::ActorStunned),
        28 => Ok(CommandRejectReason::ActorRooted),
        29 => Ok(CommandRejectReason::ActorSilenced),
        30 => Ok(CommandRejectReason::ActorDisarmed),
        31 => Ok(CommandRejectReason::InvalidTargetForm),
        32 => Ok(CommandRejectReason::TargetPointOutOfBounds),
        33 => Ok(CommandRejectReason::ProjectileBudgetExceeded),
        34 => Ok(CommandRejectReason::NoAbilityPointAvailable),
        35 => Ok(CommandRejectReason::AbilityRankMaxed),
        36 => Ok(CommandRejectReason::AbilityRankLocked {
            unlocks_at_level: decoder.u8()?,
        }),
        37 => Ok(CommandRejectReason::ActorHasNoProgression),
        tag => Err(CheckpointError::InvalidTag {
            field: "command rejection reason",
            tag,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ActorSpawn;
    use crate::protocol::MatchEvent;

    const CONTENT: ContentId = ContentId::new([0x5a; 32]);
    const OTHER_CONTENT: ContentId = ContentId::new([0xa5; 32]);
    const STRIKE: AbilityId = AbilityId(1);
    const HEAL: AbilityId = AbilityId(2);
    const LANCE: AbilityId = AbilityId(3);
    const PLAYER_ONE: PlayerId = PlayerId(10);
    const PLAYER_TWO: PlayerId = PlayerId(20);
    const RETAINED_DESPAWNED_PLAYER: PlayerId = PlayerId(30);

    const fn stats() -> CombatStats {
        CombatStats {
            max_health: 1_000,
            max_resource: 200,
            move_speed_mm_per_tick: 50,
            physical_reduction_bps: 1_000,
            magic_reduction_bps: 500,
            crit_chance_bps: 0,
            crit_damage_bps: crate::BASIS_POINTS,
            physical_penetration_bps: 0,
            magic_penetration_bps: 0,
            lifesteal_bps: 0,
        }
    }

    const fn attack() -> BasicAttackSpec {
        BasicAttackSpec {
            range_mm: 2_000,
            damage: 75,
            school: DamageSchool::Physical,
            windup_ticks: 2,
            cooldown_ticks: 5,
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the fixture intentionally exercises every core checkpoint state family"
    )]
    fn active_fixture() -> MatchRuntime {
        // Non-default progression throughout: a field left at its default encodes as zeros, and the whole
        // seeded mutation corpus goes blind to it.
        const HERO_GROWTH: StatGrowth = StatGrowth {
            max_health_per_level: 90,
            max_resource_per_level: 25,
            move_speed_mm_per_tick_per_level: 1,
            physical_reduction_bps_per_level: 40,
            magic_reduction_bps_per_level: 20,
        };
        const HERO_BOUNTY: Bounty = Bounty {
            gold: 300,
            experience: 175,
        };
        // Level 5 costs 1,720 cumulative experience, leaving four points to account for: two on STRIKE
        // (rank 3, which unlocks at level 5), one on HEAL (rank 2), and one unspent.
        const HERO_EXPERIENCE: u32 = 1_720;
        let config = MatchConfig {
            max_match_ticks: 100,
            ..MatchConfig::default()
        };
        let ability_specs = vec![
            AbilitySpec {
                id: STRIKE,
                aim: AbilityAim::Unit,
                delivery: AbilityDelivery::Instant,
                impact: ImpactShape::Single,
                targeting: AbilityTargeting::Enemy,
                range_mm: 2_000,
                resource_cost: 20,
                cooldown_ticks: 10,
                cast_ticks: 2,
                per_rank_amount: 0,
                effect: AbilityEffect::Damage {
                    amount: 120,
                    school: DamageSchool::Magic,
                },
            },
            AbilitySpec {
                id: HEAL,
                aim: AbilityAim::Unit,
                delivery: AbilityDelivery::Instant,
                impact: ImpactShape::Single,
                targeting: AbilityTargeting::SelfOnly,
                range_mm: 0,
                resource_cost: 10,
                cooldown_ticks: 8,
                cast_ticks: 1,
                per_rank_amount: 0,
                effect: AbilityEffect::Heal { amount: 80 },
            },
            AbilitySpec {
                id: LANCE,
                aim: AbilityAim::Point,
                delivery: AbilityDelivery::Projectile {
                    speed_mm_per_tick: 400,
                    hit_radius_mm: 250,
                },
                impact: ImpactShape::Circle { radius_mm: 600 },
                targeting: AbilityTargeting::Enemy,
                range_mm: 5_000,
                resource_cost: 15,
                cooldown_ticks: 12,
                cast_ticks: 1,
                per_rank_amount: 35,
                effect: AbilityEffect::Damage {
                    amount: 90,
                    school: DamageSchool::Physical,
                },
            },
        ];
        // The first actor carries a real modifier set so the round-trip corpus exercises every encoded
        // field, and so a decoded cache that disagreed with its inputs would fail the restore audit.
        let hero_modifiers = vec![
            StatModifier {
                id: ModifierId(3),
                kind: StatKind::MaxHealth,
                op: ModifierOp::Flat { magnitude: 150 },
            },
            StatModifier {
                id: ModifierId(7),
                kind: StatKind::MoveSpeed,
                op: ModifierOp::PercentAdd { bps: 2_000 },
            },
            StatModifier {
                id: ModifierId(9),
                kind: StatKind::PhysicalReduction,
                op: ModifierOp::PercentMult { bps: -5_000 },
            },
        ];
        let actors = vec![
            ActorState {
                id: ActorId(1),
                owner: Some(PLAYER_ONE),
                team: TeamId(0),
                kind: ActorKind::Hero,
                position: Vec2Mm::new(0, 0),
                destination: None,
                authored_stats: stats(),
                growth: HERO_GROWTH,
                bounty: HERO_BOUNTY,
                experience: HERO_EXPERIENCE,
                level: 5,
                unspent_ability_points: 1,
                crit_attempts: 0,
                damage_credits: vec![DamageCredit {
                    source: ActorId(2),
                    last_tick: 4,
                }],
                base_stats: stats().with_growth(HERO_GROWTH, 5),
                modifiers: hero_modifiers.clone(),
                status_effects: Vec::new(),
                control_mask: ControlMask::EMPTY,
                stats: stats()
                    .with_growth(HERO_GROWTH, 5)
                    .with_modifiers(&hero_modifiers),
                health: 850,
                resource: 150,
                abilities: vec![
                    AbilityState {
                        id: STRIKE,
                        ready_at: 12,
                        rank: 3,
                    },
                    AbilityState {
                        id: HEAL,
                        ready_at: 0,
                        rank: 2,
                    },
                ],
                cast: Some(PendingCast {
                    ability: STRIKE,
                    target: CastTarget::Actor(ActorId(2)),
                    resolves_at: 7,
                }),
                basic_attack: Some(BasicAttackState {
                    spec: attack(),
                    ready_at: 4,
                }),
                attack: None,
                death_rule: DeathRule::Respawn {
                    delay_ticks: 10,
                    at: Vec2Mm::new(0, 0),
                },
                respawn_at: None,
                provenance: ActorProvenance::Authored,
            },
            ActorState {
                id: ActorId(2),
                owner: Some(PLAYER_TWO),
                team: TeamId(1),
                kind: ActorKind::Hero,
                position: Vec2Mm::new(1_000, 0),
                destination: None,
                authored_stats: stats(),
                growth: HERO_GROWTH,
                bounty: HERO_BOUNTY,
                experience: 0,
                level: 1,
                unspent_ability_points: 0,
                crit_attempts: 0,
                damage_credits: Vec::new(),
                base_stats: stats(),
                modifiers: Vec::new(),
                status_effects: Vec::new(),
                control_mask: ControlMask::EMPTY,
                stats: stats(),
                health: 700,
                resource: 125,
                abilities: vec![AbilityState {
                    id: STRIKE,
                    ready_at: 0,
                    rank: 1,
                }],
                cast: None,
                basic_attack: Some(BasicAttackState {
                    spec: attack(),
                    ready_at: 10,
                }),
                attack: Some(PendingAttack {
                    target: ActorId(1),
                    resolves_at: 7,
                }),
                death_rule: DeathRule::StayDead,
                respawn_at: None,
                provenance: ActorProvenance::Authored,
            },
            ActorState {
                id: ActorId(3),
                owner: None,
                team: TeamId(0),
                kind: ActorKind::Minion,
                position: Vec2Mm::new(250, 0),
                destination: None,
                authored_stats: stats(),
                growth: StatGrowth::NONE,
                bounty: Bounty {
                    gold: 21,
                    experience: 60,
                },
                experience: 0,
                level: 1,
                unspent_ability_points: 0,
                crit_attempts: 0,
                damage_credits: Vec::new(),
                base_stats: stats(),
                modifiers: Vec::new(),
                status_effects: Vec::new(),
                control_mask: ControlMask::EMPTY,
                stats: stats(),
                health: 0,
                resource: 0,
                abilities: Vec::new(),
                cast: None,
                basic_attack: None,
                attack: None,
                death_rule: DeathRule::Respawn {
                    delay_ticks: 3,
                    at: Vec2Mm::new(300, 0),
                },
                respawn_at: Some(8),
                // The richest provenance variant is carried by the fixture so the round-trip corpus
                // exercises every encoded field, not just the default tag.
                provenance: ActorProvenance::Wave {
                    wave_id: 2,
                    unit_slot: 1,
                    spawn_ordinal: 3,
                },
            },
        ];
        let queued = PlayerCommand {
            player: PLAYER_ONE,
            sequence: 4,
            execute_at: 6,
            actor: ActorId(1),
            kind: CommandKind::Stop,
        };
        let rejected = PlayerCommand {
            player: PLAYER_TWO,
            sequence: 9,
            execute_at: 6,
            actor: ActorId(2),
            kind: CommandKind::MoveTo {
                destination: Vec2Mm::new(i32::MAX, i32::MAX),
            },
        };
        MatchRuntime {
            config,
            seed: 0x1234_5678_9abc_def0,
            tick: 5,
            phase: MatchPhase::Active,
            ability_specs,
            actors,
            // One missile is in flight so the round-trip corpus exercises the v5 projectile section, and so
            // a decoded projectile that referenced an ability with no projectile delivery would fail the
            // restore audit rather than sit still forever.
            projectiles: vec![ProjectileState {
                id: ProjectileId(7),
                source: ActorId(1),
                team: TeamId(0),
                ability: LANCE,
                position: Vec2Mm::new(1_200, -400),
                end: Vec2Mm::new(6_000, -400),
                // Launched at LANCE rank 3: 90 base + 2 x 35 per rank.
                amount: 160,
            }],
            scores: vec![
                ScoreEntry {
                    actor: ActorId(1),
                    owner: Some(PLAYER_ONE),
                    team: TeamId(0),
                    since_tick: 0,
                    earned_gold: 640,
                    passive_gold_paid: passive_gold_owed(5, config),
                    kills: 2,
                    deaths: 1,
                    assists: 1,
                    kill_streak: 2,
                    best_kill_streak: 2,
                },
                ScoreEntry {
                    actor: ActorId(2),
                    owner: Some(PLAYER_TWO),
                    team: TeamId(1),
                    since_tick: 0,
                    earned_gold: 150,
                    passive_gold_paid: passive_gold_owed(5, config),
                    kills: 1,
                    deaths: 2,
                    assists: 0,
                    kill_streak: 0,
                    best_kill_streak: 3,
                },
            ],
            player_roster: [PLAYER_ONE, PLAYER_TWO, RETAINED_DESPAWNED_PLAYER]
                .into_iter()
                .collect(),
            queued_commands: [(6, vec![queued])].into_iter().collect(),
            last_submissions: [
                (
                    PLAYER_ONE,
                    SubmissionRecord {
                        command: queued,
                        outcome: Ok(CommandReceipt {
                            player: PLAYER_ONE,
                            sequence: 4,
                            execute_at: 6,
                        }),
                    },
                ),
                (
                    PLAYER_TWO,
                    SubmissionRecord {
                        command: rejected,
                        outcome: Err(CommandRejection {
                            player: PLAYER_TWO,
                            sequence: 9,
                            reason: CommandRejectReason::DestinationOutOfBounds,
                        }),
                    },
                ),
            ]
            .into_iter()
            .collect(),
            dirty: BTreeSet::new(),
            removed: BTreeSet::new(),
            removed_projectiles: BTreeSet::new(),
            dirty_scores: BTreeSet::new(),
            pending_events: Vec::new(),
            rejection_events_in_frame: 0,
            objective_claims: BTreeSet::new(),
            next_dynamic_actor_id: Some(44),
            next_modifier_id: Some(12),
            next_status_effect_id: Some(1),
            next_projectile_id: Some(8),
            checkpoint_ready: true,
        }
    }

    #[test]
    fn round_trip_preserves_all_core_runtime_truth_and_continuation() {
        let runtime = active_fixture();
        runtime.check_invariants().unwrap();
        let checkpoint = runtime.capture_checkpoint(CONTENT).unwrap();
        let mut restored = MatchRuntime::restore_checkpoint(&checkpoint, CONTENT).unwrap();

        assert_eq!(restored.config, runtime.config);
        assert_eq!(restored.seed, runtime.seed);
        assert_eq!(restored.tick, runtime.tick);
        assert_eq!(restored.phase, runtime.phase);
        assert_eq!(restored.ability_specs, runtime.ability_specs);
        assert_eq!(restored.actors, runtime.actors);
        assert_eq!(restored.player_roster, runtime.player_roster);
        assert_eq!(restored.queued_commands, runtime.queued_commands);
        assert_eq!(restored.last_submissions, runtime.last_submissions);
        assert_eq!(
            restored.next_dynamic_actor_id,
            runtime.next_dynamic_actor_id
        );
        assert!(restored.dirty.is_empty());
        assert!(restored.removed.is_empty());
        assert!(restored.pending_events.is_empty());
        assert_eq!(restored.rejection_events_in_frame, 0);
        assert!(restored.objective_claims.is_empty());
        assert!(restored.checkpoint_ready);
        assert_eq!(restored.world_digest(), runtime.world_digest());

        let mut uninterrupted = runtime;
        for _ in 0..3 {
            assert_eq!(restored.step().unwrap(), uninterrupted.step().unwrap());
        }
    }

    #[test]
    fn terminal_outcome_round_trips() {
        let mut runtime = active_fixture();
        runtime
            .finish(MatchOutcome::Winner(TeamId(7)), MatchEndReason::Host)
            .unwrap();
        let checkpoint = runtime.capture_checkpoint(CONTENT).unwrap();
        let restored = MatchRuntime::restore_checkpoint(&checkpoint, CONTENT).unwrap();
        assert_eq!(restored.phase, runtime.phase);
        assert_eq!(restored.world_digest(), runtime.world_digest());
    }

    #[test]
    fn exhausted_dynamic_allocator_round_trips_after_terminal_id_despawn() {
        let mut runtime = active_fixture();
        runtime.next_dynamic_actor_id = None;
        let checkpoint = runtime.capture_checkpoint(CONTENT).unwrap();
        let restored = MatchRuntime::restore_checkpoint(&checkpoint, CONTENT).unwrap();
        assert_eq!(restored.next_dynamic_actor_id, None);
    }

    #[test]
    fn retained_owned_despawn_roster_changes_world_digest() {
        let mut runtime = MatchRuntime::new(MatchConfig::default(), 99).unwrap();
        runtime
            .register_ability(AbilitySpec {
                id: STRIKE,
                aim: AbilityAim::Unit,
                delivery: AbilityDelivery::Instant,
                impact: ImpactShape::Single,
                targeting: AbilityTargeting::Enemy,
                range_mm: 2_000,
                resource_cost: 0,
                cooldown_ticks: 1,
                cast_ticks: 0,
                per_rank_amount: 0,
                effect: AbilityEffect::Damage {
                    amount: 2_000,
                    school: DamageSchool::True,
                },
            })
            .unwrap();
        runtime
            .spawn_actor(&ActorSpawn {
                id: ActorId(1),
                owner: Some(PLAYER_ONE),
                team: TeamId(0),
                kind: ActorKind::Hero,
                position: Vec2Mm::new(0, 0),
                growth: StatGrowth::NONE,
                bounty: Bounty::NONE,
                stats: stats(),
                abilities: vec![STRIKE],
                basic_attack: None,
                death_rule: DeathRule::StayDead,
            })
            .unwrap();
        runtime
            .spawn_actor(&ActorSpawn {
                id: ActorId(2),
                owner: Some(RETAINED_DESPAWNED_PLAYER),
                team: TeamId(1),
                kind: ActorKind::Minion,
                position: Vec2Mm::new(100, 0),
                growth: StatGrowth::NONE,
                bounty: Bounty::NONE,
                stats: stats(),
                abilities: Vec::new(),
                basic_attack: None,
                death_rule: DeathRule::Despawn,
            })
            .unwrap();
        runtime.start().unwrap();
        runtime
            .submit(PlayerCommand {
                player: PLAYER_ONE,
                sequence: 1,
                execute_at: 1,
                actor: ActorId(1),
                kind: CommandKind::Cast {
                    ability: STRIKE,
                    target: CastTarget::Actor(ActorId(2)),
                },
            })
            .unwrap();
        runtime.step().unwrap();
        assert!(runtime.actor(ActorId(2)).is_none());
        assert!(runtime.player_roster.contains(&RETAINED_DESPAWNED_PLAYER));

        let mut without_retained_player = runtime.clone();
        assert!(without_retained_player
            .player_roster
            .remove(&RETAINED_DESPAWNED_PLAYER));
        assert_ne!(
            runtime.world_digest(),
            without_retained_player.world_digest()
        );
    }

    #[test]
    fn capture_requires_empty_outbound_buffers() {
        let mut runtime = active_fixture();
        runtime
            .pending_events
            .push(MatchEvent::MoveStopped { actor: ActorId(1) });
        assert_eq!(
            runtime.capture_checkpoint(CONTENT),
            Err(CheckpointError::NotFrameBoundary)
        );
    }

    #[test]
    fn accepted_submission_clears_frame_boundary_readiness() {
        let mut runtime = MatchRuntime::new(MatchConfig::default(), 7).unwrap();
        runtime
            .spawn_actor(&ActorSpawn {
                id: ActorId(1),
                owner: Some(PLAYER_ONE),
                team: TeamId(0),
                kind: ActorKind::Hero,
                position: Vec2Mm::new(0, 0),
                growth: StatGrowth::NONE,
                bounty: Bounty::NONE,
                stats: stats(),
                abilities: Vec::new(),
                basic_attack: None,
                death_rule: DeathRule::StayDead,
            })
            .unwrap();
        runtime.start().unwrap();
        runtime
            .submit(PlayerCommand {
                player: PLAYER_ONE,
                sequence: 1,
                execute_at: 1,
                actor: ActorId(1),
                kind: CommandKind::Stop,
            })
            .unwrap();
        assert_eq!(
            runtime.capture_checkpoint(CONTENT),
            Err(CheckpointError::NotFrameBoundary)
        );
        runtime.step().unwrap();
        runtime.capture_checkpoint(CONTENT).unwrap();
    }

    #[test]
    fn reverse_command_arrival_produces_identical_checkpoint_bytes() {
        fn run(reverse: bool) -> RuntimeCheckpoint {
            let mut runtime = MatchRuntime::new(MatchConfig::default(), 71).unwrap();
            for (id, player, team) in [
                (ActorId(1), PLAYER_ONE, TeamId(0)),
                (ActorId(2), PLAYER_TWO, TeamId(1)),
            ] {
                runtime
                    .spawn_actor(&ActorSpawn {
                        id,
                        owner: Some(player),
                        team,
                        kind: ActorKind::Hero,
                        position: Vec2Mm::new(i32::try_from(id.get()).unwrap() * 100, 0),
                        growth: StatGrowth::NONE,
                        bounty: Bounty::NONE,
                        stats: stats(),
                        abilities: Vec::new(),
                        basic_attack: None,
                        death_rule: DeathRule::StayDead,
                    })
                    .unwrap();
            }
            runtime.start().unwrap();
            let commands = [
                PlayerCommand {
                    player: PLAYER_ONE,
                    sequence: 1,
                    execute_at: 2,
                    actor: ActorId(1),
                    kind: CommandKind::Stop,
                },
                PlayerCommand {
                    player: PLAYER_TWO,
                    sequence: 1,
                    execute_at: 2,
                    actor: ActorId(2),
                    kind: CommandKind::Stop,
                },
            ];
            if reverse {
                for command in commands.into_iter().rev() {
                    runtime.submit(command).unwrap();
                }
            } else {
                for command in commands {
                    runtime.submit(command).unwrap();
                }
            }
            runtime.step().unwrap();
            runtime.capture_checkpoint(CONTENT).unwrap()
        }

        assert_eq!(run(false).as_bytes(), run(true).as_bytes());
    }

    #[test]
    fn content_identity_is_mandatory() {
        let checkpoint = active_fixture().capture_checkpoint(CONTENT).unwrap();
        assert!(matches!(
            MatchRuntime::restore_checkpoint(&checkpoint, OTHER_CONTENT),
            Err(CheckpointError::ContentMismatch { .. })
        ));
    }

    #[test]
    fn corruption_and_digest_tampering_fail_closed() {
        let checkpoint = active_fixture().capture_checkpoint(CONTENT).unwrap();
        let mut corrupt = checkpoint.as_bytes().to_vec();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 0x80;
        let corrupt = RuntimeCheckpoint::from_bytes(&corrupt).unwrap();
        assert!(matches!(
            MatchRuntime::restore_checkpoint(&corrupt, CONTENT),
            Err(CheckpointError::CorruptPayload)
        ));

        let mut digest_tamper = checkpoint.as_bytes().to_vec();
        // The seed follows the config block; derived so the tamper keeps hitting the seed as config grows.
        let mut seed_probe = Decoder::new(&checkpoint.as_bytes()[HEADER_BYTES..]);
        decode_config(&mut seed_probe).unwrap();
        let seed_offset = HEADER_BYTES + seed_probe.offset;
        digest_tamper[seed_offset] ^= 1;
        let payload_checksum = checksum(&digest_tamper[HEADER_BYTES..]);
        digest_tamper[PAYLOAD_CHECKSUM_OFFSET..PAYLOAD_CHECKSUM_OFFSET + 8]
            .copy_from_slice(&payload_checksum.to_le_bytes());
        let digest_tamper = RuntimeCheckpoint::from_bytes(&digest_tamper).unwrap();
        assert!(matches!(
            MatchRuntime::restore_checkpoint(&digest_tamper, CONTENT),
            Err(CheckpointError::DigestMismatch { .. })
        ));
    }

    #[test]
    fn semantically_invalid_state_fails_even_with_matching_checksum_and_world_digest() {
        let runtime = active_fixture();
        let checkpoint = runtime.capture_checkpoint(CONTENT).unwrap();
        let mut bytes = checkpoint.as_bytes().to_vec();

        let mut decoder = Decoder::new(&bytes[HEADER_BYTES..]);
        decode_config(&mut decoder).unwrap();
        decoder.u64().unwrap();
        decoder.u64().unwrap();
        decode_phase(&mut decoder).unwrap();
        decode_option_u64(&mut decoder, "actor allocator").unwrap();
        decode_option_u64(&mut decoder, "modifier allocator").unwrap();
        decode_option_u64(&mut decoder, "status effect allocator").unwrap();
        decode_option_u64(&mut decoder, "projectile allocator").unwrap();
        let ability_count = decoder.count(usize::MAX, "test ability count").unwrap();
        for _ in 0..ability_count {
            decode_ability(&mut decoder).unwrap();
        }
        assert!(decoder.count(usize::MAX, "test actor count").unwrap() > 0);
        // Walk the first actor's leading fields with the real decoder instead of a hand-counted byte
        // offset, so this adversarial fixture keeps targeting the destination tag when the actor layout
        // gains a field.
        decoder.u64().unwrap();
        decode_provenance(&mut decoder).unwrap();
        decode_option_player(&mut decoder).unwrap();
        decoder.u8().unwrap();
        decode_actor_kind(&mut decoder).unwrap();
        decode_point(&mut decoder).unwrap();
        let destination_tag_offset = HEADER_BYTES + decoder.offset;
        assert_eq!(bytes[destination_tag_offset], 0);
        bytes[destination_tag_offset] = 1;
        let destination = Vec2Mm::new(500, 0);
        let mut encoded_destination = Vec::new();
        encoded_destination.extend_from_slice(&destination.x.to_le_bytes());
        encoded_destination.extend_from_slice(&destination.y.to_le_bytes());
        let insertion = destination_tag_offset + 1;
        for (offset, byte) in encoded_destination.into_iter().enumerate() {
            bytes.insert(insertion + offset, byte);
        }

        let mut invalid_runtime = runtime;
        invalid_runtime.actors[0].destination = Some(destination);
        let invalid_digest = invalid_runtime.world_digest();
        let payload_length = u32::try_from(bytes.len() - HEADER_BYTES).unwrap();
        bytes[PAYLOAD_LENGTH_OFFSET..PAYLOAD_LENGTH_OFFSET + 4]
            .copy_from_slice(&payload_length.to_le_bytes());
        let payload_checksum = checksum(&bytes[HEADER_BYTES..]);
        bytes[PAYLOAD_CHECKSUM_OFFSET..PAYLOAD_CHECKSUM_OFFSET + 8]
            .copy_from_slice(&payload_checksum.to_le_bytes());
        bytes[WORLD_DIGEST_OFFSET..WORLD_DIGEST_OFFSET + 8]
            .copy_from_slice(&invalid_digest.0.to_le_bytes());

        let invalid = RuntimeCheckpoint::from_bytes(&bytes).unwrap();
        assert!(matches!(
            MatchRuntime::restore_checkpoint(&invalid, CONTENT),
            Err(CheckpointError::RuntimeInvariant {
                actor: Some(ActorId(1)),
                detail: "actor moves while a mutually exclusive action is pending",
            })
        ));
    }

    #[test]
    fn seeded_arbitrary_and_valid_mutation_decoder_corpus_fails_closed() {
        let mut state = 0x4348_4b50_5446_555a_u64;
        for case in 0..512_usize {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let length = if case < 66 {
                case
            } else {
                usize::try_from(state % 8_193).expect("bounded corpus length fits usize")
            };
            let mut bytes = vec![0_u8; length];
            for byte in &mut bytes {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                *byte = state.to_le_bytes()[4];
            }
            let checkpoint = RuntimeCheckpoint::from_bytes(&bytes).unwrap();
            assert!(
                MatchRuntime::restore_checkpoint(&checkpoint, CONTENT).is_err(),
                "arbitrary decoder corpus case {case} unexpectedly restored"
            );
        }

        let valid = active_fixture().capture_checkpoint(CONTENT).unwrap();
        for case in 0..512_usize {
            let mut bytes = valid.as_bytes().to_vec();
            let offset = case % bytes.len();
            bytes[offset] ^= 1_u8 << (case % 8);
            let checkpoint = RuntimeCheckpoint::from_bytes(&bytes).unwrap();
            assert!(
                MatchRuntime::restore_checkpoint(&checkpoint, CONTENT).is_err(),
                "mutated valid checkpoint case {case} unexpectedly restored"
            );
        }
    }

    #[test]
    fn truncation_and_both_size_ceilings_fail_closed() {
        let checkpoint = active_fixture().capture_checkpoint(CONTENT).unwrap();
        let truncated = RuntimeCheckpoint::from_bytes(&checkpoint.as_bytes()[..16]).unwrap();
        assert!(matches!(
            MatchRuntime::restore_checkpoint(&truncated, CONTENT),
            Err(CheckpointError::Truncated)
        ));

        let hard_oversize = vec![0; usize::try_from(MAX_RUNTIME_CHECKPOINT_BYTES).unwrap() + 1];
        assert!(matches!(
            RuntimeCheckpoint::from_bytes(&hard_oversize),
            Err(CheckpointError::SizeLimitExceeded { .. })
        ));

        let hard_config = MatchConfig {
            max_checkpoint_bytes: MAX_RUNTIME_CHECKPOINT_BYTES + 1,
            ..MatchConfig::default()
        };
        assert!(MatchRuntime::new(hard_config, 0).is_err());

        let mut configured_too_small = active_fixture();
        configured_too_small.config.max_checkpoint_bytes = 128;
        assert!(matches!(
            configured_too_small.capture_checkpoint(CONTENT),
            Err(CheckpointError::SizeLimitExceeded { .. })
        ));

        let mut encoded_config_too_small = checkpoint.as_bytes().to_vec();
        // `max_checkpoint_bytes` is the last field `encode_config` writes, so its offset is derived from the
        // real decoded config length rather than hand-counted — the config gains fields over time.
        let mut config_probe = Decoder::new(&checkpoint.as_bytes()[HEADER_BYTES..]);
        decode_config(&mut config_probe).unwrap();
        let config_ceiling_offset = HEADER_BYTES + config_probe.offset - 4;
        let below_envelope = u32::try_from(encoded_config_too_small.len() - 1).unwrap();
        encoded_config_too_small[config_ceiling_offset..config_ceiling_offset + 4]
            .copy_from_slice(&below_envelope.to_le_bytes());
        let payload_checksum = checksum(&encoded_config_too_small[HEADER_BYTES..]);
        encoded_config_too_small[PAYLOAD_CHECKSUM_OFFSET..PAYLOAD_CHECKSUM_OFFSET + 8]
            .copy_from_slice(&payload_checksum.to_le_bytes());
        let encoded_config_too_small =
            RuntimeCheckpoint::from_bytes(&encoded_config_too_small).unwrap();
        assert!(matches!(
            MatchRuntime::restore_checkpoint(&encoded_config_too_small, CONTENT),
            Err(CheckpointError::SizeLimitExceeded { .. })
        ));
    }

    #[test]
    fn hostile_definition_count_is_rejected_before_allocation() {
        let checkpoint = active_fixture().capture_checkpoint(CONTENT).unwrap();
        let mut hostile = checkpoint.as_bytes().to_vec();
        // Walk the real prefix (config, seed, tick, phase, both allocators) so this stays aimed at the
        // ability count when any of those encodings change size.
        let mut probe = Decoder::new(&checkpoint.as_bytes()[HEADER_BYTES..]);
        decode_config(&mut probe).unwrap();
        probe.u64().unwrap();
        probe.u64().unwrap();
        decode_phase(&mut probe).unwrap();
        decode_option_u64(&mut probe, "actor allocator").unwrap();
        decode_option_u64(&mut probe, "modifier allocator").unwrap();
        decode_option_u64(&mut probe, "status effect allocator").unwrap();
        decode_option_u64(&mut probe, "projectile allocator").unwrap();
        let ability_count_offset = HEADER_BYTES + probe.offset;
        hostile[ability_count_offset..ability_count_offset + 4]
            .copy_from_slice(&u32::MAX.to_le_bytes());
        let payload_checksum = checksum(&hostile[HEADER_BYTES..]);
        hostile[PAYLOAD_CHECKSUM_OFFSET..PAYLOAD_CHECKSUM_OFFSET + 8]
            .copy_from_slice(&payload_checksum.to_le_bytes());
        let hostile = RuntimeCheckpoint::from_bytes(&hostile).unwrap();
        assert!(matches!(
            MatchRuntime::restore_checkpoint(&hostile, CONTENT),
            Err(CheckpointError::InvalidData(
                "ability definition count exceeds the hard budget"
            ))
        ));

        let hostile_config = MatchConfig {
            max_actors: crate::MAX_ACTOR_BUDGET + 1,
            ..MatchConfig::default()
        };
        assert!(MatchRuntime::new(hostile_config, 0).is_err());

        let mut setup_phase = checkpoint.as_bytes().to_vec();
        // The phase tag sits immediately after config, seed, and tick — derived, not hand-counted.
        let mut phase_probe = Decoder::new(&checkpoint.as_bytes()[HEADER_BYTES..]);
        decode_config(&mut phase_probe).unwrap();
        phase_probe.u64().unwrap();
        phase_probe.u64().unwrap();
        let phase_offset = HEADER_BYTES + phase_probe.offset;
        setup_phase[phase_offset] = 0;
        let payload_checksum = checksum(&setup_phase[HEADER_BYTES..]);
        setup_phase[PAYLOAD_CHECKSUM_OFFSET..PAYLOAD_CHECKSUM_OFFSET + 8]
            .copy_from_slice(&payload_checksum.to_le_bytes());
        let setup_phase = RuntimeCheckpoint::from_bytes(&setup_phase).unwrap();
        assert!(matches!(
            MatchRuntime::restore_checkpoint(&setup_phase, CONTENT),
            Err(CheckpointError::InvalidData(
                "setup phase has no emitted frame boundary and cannot be restored"
            ))
        ));
    }

    #[test]
    fn header_is_fixed_and_little_endian() {
        let checkpoint = active_fixture().capture_checkpoint(CONTENT).unwrap();
        let bytes = checkpoint.as_bytes();
        assert_eq!(&bytes[..8], &MAGIC);
        assert_eq!(&bytes[8..10], &FORMAT_VERSION.to_le_bytes());
        assert_eq!(&bytes[10..12], &[0, 0]);
        assert_eq!(&bytes[12..44], CONTENT.as_bytes());
        assert_eq!(
            read_header_u32(bytes, PAYLOAD_LENGTH_OFFSET).unwrap() as usize,
            bytes.len() - HEADER_BYTES
        );

        let mut future_version = bytes.to_vec();
        future_version[8..10].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        let future_version = RuntimeCheckpoint::from_bytes(&future_version).unwrap();
        assert!(matches!(
            MatchRuntime::restore_checkpoint(&future_version, CONTENT),
            Err(CheckpointError::UnsupportedVersion(version))
                if version == FORMAT_VERSION + 1
        ));
    }
}
