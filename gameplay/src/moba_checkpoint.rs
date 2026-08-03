//! Atomic frame-boundary checkpoints for [`OneLaneMatch`].
//!
//! This envelope composes the core runtime checkpoint with the lane controller's wave scheduler. Immutable
//! cooked lane, initial-bot, and wave definitions remain caller-owned and are identity-checked on both capture
//! and restore. Owned-player lane navigation is intentionally outside this server-controller API.
//! FNV checksums and deterministic digests provide accidental-corruption evidence only. The host must use
//! trusted storage or authenticate an outer storage/transport envelope before decoding.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Display, Formatter};

use crate::moba::{
    add_signed_progress, one_lane_definition_digest, validate_bot, validate_bot_policy,
    LaneBotSpec, LaneMatchDigest, LaneMatchError, OneLaneMatch, WaveSpec, WaveState,
};
use crate::{
    ActorId, ActorKind, ActorProvenance, CheckpointError, CompiledLane, ContentId, LaneError,
    LaneSpec, MatchPhase, MatchRuntime, RuntimeCheckpoint, Tick, MAX_RUNTIME_CHECKPOINT_BYTES,
};

const MAGIC: [u8; 8] = *b"MTLNCP01";
const FORMAT_VERSION: u16 = 1;
const HEADER_BYTES: usize = 72;
const PAYLOAD_LENGTH_OFFSET: usize = 44;
const PAYLOAD_CHECKSUM_OFFSET: usize = 48;
const LANE_DIGEST_OFFSET: usize = 56;
const DEFINITION_DIGEST_OFFSET: usize = 64;
const CORE_SECTION_TAG: u8 = 1;
const WAVES_SECTION_TAG: u8 = 2;
const MIN_WAVE_STATE_BYTES: usize = 16;

/// Opaque, bounded one-lane match checkpoint bytes. Host authentication remains an outer-envelope concern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OneLaneCheckpoint {
    bytes: Vec<u8>,
}

impl OneLaneCheckpoint {
    /// Copy opaque bytes after applying only the absolute ceiling; authenticate before restore.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, OneLaneCheckpointError> {
        enforce_size(
            bytes.len(),
            usize::try_from(MAX_RUNTIME_CHECKPOINT_BYTES).unwrap_or(usize::MAX),
        )?;
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(bytes.len())
            .map_err(|_| OneLaneCheckpointError::AllocationFailed)?;
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

/// Fail-closed one-lane checkpoint error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OneLaneCheckpointError {
    Core(CheckpointError),
    Lane(LaneMatchError),
    SizeLimitExceeded {
        size: u64,
        limit: u64,
    },
    CountOverflow,
    AllocationFailed,
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
        expected: u8,
        actual: u8,
    },
    DefinitionMismatch,
    InvalidState(&'static str),
    LaneDigestMismatch {
        encoded: LaneMatchDigest,
        recomputed: LaneMatchDigest,
    },
}

impl Display for OneLaneCheckpointError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core(error) => write!(formatter, "core checkpoint failed: {error}"),
            Self::Lane(error) => Display::fmt(error, formatter),
            Self::SizeLimitExceeded { size, limit } => {
                write!(
                    formatter,
                    "one-lane checkpoint size {size} exceeds limit {limit}"
                )
            }
            Self::CountOverflow => {
                formatter.write_str("one-lane checkpoint collection count exceeds u32")
            }
            Self::AllocationFailed => formatter.write_str("one-lane checkpoint allocation failed"),
            Self::InvalidMagic => formatter.write_str("invalid one-lane checkpoint magic"),
            Self::UnsupportedVersion(version) => {
                write!(
                    formatter,
                    "unsupported one-lane checkpoint version {version}"
                )
            }
            Self::UnsupportedFlags(flags) => {
                write!(formatter, "unsupported one-lane checkpoint flags {flags}")
            }
            Self::ContentMismatch { .. } => {
                formatter.write_str("one-lane checkpoint cooked-content identity does not match")
            }
            Self::Truncated => formatter.write_str("one-lane checkpoint is truncated"),
            Self::LengthMismatch => {
                formatter.write_str("one-lane checkpoint payload length does not match")
            }
            Self::CorruptPayload => {
                formatter.write_str("one-lane checkpoint payload checksum does not match")
            }
            Self::InvalidTag { expected, actual } => write!(
                formatter,
                "invalid one-lane checkpoint section tag {actual}; expected {expected}"
            ),
            Self::DefinitionMismatch => formatter.write_str(
                "one-lane checkpoint definitions do not match the supplied cooked definitions",
            ),
            Self::InvalidState(detail) => {
                write!(formatter, "invalid one-lane checkpoint state: {detail}")
            }
            Self::LaneDigestMismatch { .. } => {
                formatter.write_str("restored one-lane digest does not match checkpoint")
            }
        }
    }
}

impl Error for OneLaneCheckpointError {}

impl From<CheckpointError> for OneLaneCheckpointError {
    fn from(value: CheckpointError) -> Self {
        Self::Core(value)
    }
}

impl From<LaneMatchError> for OneLaneCheckpointError {
    fn from(value: LaneMatchError) -> Self {
        Self::Lane(value)
    }
}

impl From<LaneError> for OneLaneCheckpointError {
    fn from(value: LaneError) -> Self {
        Self::Lane(LaneMatchError::Lane(value))
    }
}

struct PreparedDefinitions {
    lane: CompiledLane,
    initial_bots: Vec<LaneBotSpec>,
    waves: Vec<WaveSpec>,
    digest: u64,
}

#[derive(Clone, Debug)]
struct EncodedWaveState {
    id: crate::WaveId,
    next_tick: Tick,
    live: BTreeSet<ActorId>,
}

impl OneLaneMatch {
    /// Capture one atomic core-runtime plus lane-controller checkpoint at an emitted-frame boundary.
    pub fn capture_checkpoint(
        &self,
        content_id: ContentId,
        lane: &LaneSpec,
        initial_bots: &[LaneBotSpec],
        waves: &[WaveSpec],
    ) -> Result<OneLaneCheckpoint, OneLaneCheckpointError> {
        if self.runtime.phase() != MatchPhase::Active {
            return Err(OneLaneCheckpointError::InvalidState(
                "only an active one-lane match can be resumed",
            ));
        }
        let definitions = prepare_definitions(&self.runtime, lane, initial_bots, waves)?;
        if definitions.digest != self.definition_digest
            || definitions.waves.len() != self.waves.len()
            || !definitions
                .waves
                .iter()
                .zip(&self.waves)
                .all(|(definition, state)| definition == &state.spec)
            || !same_lane(&definitions.lane, &self.lane)
        {
            return Err(OneLaneCheckpointError::DefinitionMismatch);
        }
        validate_wave_states(&self.runtime, &self.waves)?;
        let expected_bots = reconstruct_bots(
            &self.runtime,
            &definitions.lane,
            &definitions.initial_bots,
            &self.waves,
        )?;
        if expected_bots != self.bots {
            return Err(OneLaneCheckpointError::InvalidState(
                "bot registry is not reconstructible from immutable definitions and live waves",
            ));
        }

        let core = self.runtime.capture_checkpoint(content_id)?;
        let configured_limit = usize::try_from(self.runtime.config().max_checkpoint_bytes)
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
        encoder.u64(0)?;

        encoder.u8(CORE_SECTION_TAG)?;
        encoder.count(core.len())?;
        encoder.bytes(core.as_bytes())?;
        encoder.u8(WAVES_SECTION_TAG)?;
        encoder.count(self.waves.len())?;
        for wave in &self.waves {
            encoder.u32(wave.spec.id.get())?;
            encoder.u64(wave.next_tick)?;
            encoder.count(wave.live.len())?;
            for actor in &wave.live {
                encoder.u64(actor.get())?;
            }
        }

        let mut bytes = encoder.finish();
        let payload_length = bytes
            .len()
            .checked_sub(HEADER_BYTES)
            .ok_or(OneLaneCheckpointError::Truncated)?;
        let payload_length =
            u32::try_from(payload_length).map_err(|_| OneLaneCheckpointError::CountOverflow)?;
        let payload_checksum = checksum(&bytes[HEADER_BYTES..]);
        let lane_digest = self.lane_digest();
        bytes[PAYLOAD_LENGTH_OFFSET..PAYLOAD_LENGTH_OFFSET + 4]
            .copy_from_slice(&payload_length.to_le_bytes());
        bytes[PAYLOAD_CHECKSUM_OFFSET..PAYLOAD_CHECKSUM_OFFSET + 8]
            .copy_from_slice(&payload_checksum.to_le_bytes());
        bytes[LANE_DIGEST_OFFSET..LANE_DIGEST_OFFSET + 8]
            .copy_from_slice(&lane_digest.0.to_le_bytes());
        bytes[DEFINITION_DIGEST_OFFSET..DEFINITION_DIGEST_OFFSET + 8]
            .copy_from_slice(&definitions.digest.to_le_bytes());
        Ok(OneLaneCheckpoint { bytes })
    }

    /// Restore an active one-lane match from caller-supplied immutable cooked definitions.
    #[expect(
        clippy::too_many_lines,
        reason = "the ordered fail-closed composite restore pipeline is intentionally visible"
    )]
    pub fn restore_checkpoint(
        checkpoint: &OneLaneCheckpoint,
        expected_content_id: ContentId,
        lane: &LaneSpec,
        initial_bots: &[LaneBotSpec],
        waves: &[WaveSpec],
    ) -> Result<Self, OneLaneCheckpointError> {
        let bytes = checkpoint.as_bytes();
        enforce_size(
            bytes.len(),
            usize::try_from(MAX_RUNTIME_CHECKPOINT_BYTES).unwrap_or(usize::MAX),
        )?;
        if bytes.len() < HEADER_BYTES {
            return Err(OneLaneCheckpointError::Truncated);
        }
        if bytes[..8] != MAGIC {
            return Err(OneLaneCheckpointError::InvalidMagic);
        }
        let version = read_header_u16(bytes, 8)?;
        if version != FORMAT_VERSION {
            return Err(OneLaneCheckpointError::UnsupportedVersion(version));
        }
        let flags = read_header_u16(bytes, 10)?;
        if flags != 0 {
            return Err(OneLaneCheckpointError::UnsupportedFlags(flags));
        }
        let actual_content_id = ContentId::new(
            bytes[12..44]
                .try_into()
                .map_err(|_| OneLaneCheckpointError::Truncated)?,
        );
        if actual_content_id != expected_content_id {
            return Err(OneLaneCheckpointError::ContentMismatch {
                expected: expected_content_id,
                actual: actual_content_id,
            });
        }
        let payload_length = usize::try_from(read_header_u32(bytes, PAYLOAD_LENGTH_OFFSET)?)
            .map_err(|_| OneLaneCheckpointError::LengthMismatch)?;
        if payload_length != bytes.len() - HEADER_BYTES {
            return Err(OneLaneCheckpointError::LengthMismatch);
        }
        let encoded_checksum = read_header_u64(bytes, PAYLOAD_CHECKSUM_OFFSET)?;
        if checksum(&bytes[HEADER_BYTES..]) != encoded_checksum {
            return Err(OneLaneCheckpointError::CorruptPayload);
        }
        let encoded_lane_digest = LaneMatchDigest(read_header_u64(bytes, LANE_DIGEST_OFFSET)?);
        let encoded_definition_digest = read_header_u64(bytes, DEFINITION_DIGEST_OFFSET)?;

        let mut decoder = Decoder::new(&bytes[HEADER_BYTES..]);
        decoder.expect_tag(CORE_SECTION_TAG)?;
        let core_length = decoder.count(
            usize::try_from(MAX_RUNTIME_CHECKPOINT_BYTES).unwrap_or(usize::MAX),
            "embedded core checkpoint exceeds the hard ceiling",
        )?;
        let core_bytes = decoder.take(core_length)?;
        let core_checkpoint = RuntimeCheckpoint::from_bytes(core_bytes)?;
        let runtime = MatchRuntime::restore_checkpoint(&core_checkpoint, expected_content_id)?;
        enforce_size(
            bytes.len(),
            usize::try_from(runtime.config().max_checkpoint_bytes).unwrap_or(usize::MAX),
        )?;
        if runtime.phase() != MatchPhase::Active {
            return Err(OneLaneCheckpointError::InvalidState(
                "only an active one-lane match can be resumed",
            ));
        }

        let definitions = prepare_definitions(&runtime, lane, initial_bots, waves)?;
        if definitions.digest != encoded_definition_digest {
            return Err(OneLaneCheckpointError::DefinitionMismatch);
        }

        decoder.expect_tag(WAVES_SECTION_TAG)?;
        let wave_count = decoder.count(
            usize::from(runtime.config().max_wave_schedules),
            "wave-state count exceeds configured schedule budget",
        )?;
        decoder.require_minimum(wave_count, MIN_WAVE_STATE_BYTES)?;
        let mut encoded_states = BTreeMap::new();
        let mut total_live = 0_usize;
        for _ in 0..wave_count {
            let id = crate::WaveId(decoder.u32()?);
            let next_tick = decoder.u64()?;
            let live_count = decoder.count(
                usize::try_from(runtime.config().max_actors).unwrap_or(usize::MAX),
                "wave live set exceeds actor budget",
            )?;
            total_live = total_live
                .checked_add(live_count)
                .ok_or(OneLaneCheckpointError::CountOverflow)?;
            if total_live > usize::try_from(runtime.config().max_actors).unwrap_or(usize::MAX) {
                return Err(OneLaneCheckpointError::InvalidState(
                    "combined wave live sets exceed the actor budget",
                ));
            }
            decoder.require_minimum(live_count, 8)?;
            let mut live = BTreeSet::new();
            for _ in 0..live_count {
                if !live.insert(ActorId(decoder.u64()?)) {
                    return Err(OneLaneCheckpointError::InvalidState(
                        "duplicate actor in one wave live set",
                    ));
                }
            }
            if encoded_states
                .insert(
                    id,
                    EncodedWaveState {
                        id,
                        next_tick,
                        live,
                    },
                )
                .is_some()
            {
                return Err(OneLaneCheckpointError::InvalidState(
                    "duplicate wave identity",
                ));
            }
        }
        decoder.finish()?;

        if encoded_states.len() != definitions.waves.len() {
            return Err(OneLaneCheckpointError::InvalidState(
                "checkpoint is missing or adds a wave identity",
            ));
        }
        let mut restored_waves = Vec::new();
        restored_waves
            .try_reserve_exact(definitions.waves.len())
            .map_err(|_| OneLaneCheckpointError::AllocationFailed)?;
        for spec in definitions.waves {
            let state =
                encoded_states
                    .remove(&spec.id)
                    .ok_or(OneLaneCheckpointError::InvalidState(
                        "checkpoint is missing a known wave identity",
                    ))?;
            if state.id != spec.id {
                return Err(OneLaneCheckpointError::InvalidState(
                    "wave-state identity is inconsistent",
                ));
            }
            restored_waves.push(WaveState {
                spec,
                next_tick: state.next_tick,
                live: state.live,
            });
        }
        if !encoded_states.is_empty() {
            return Err(OneLaneCheckpointError::InvalidState(
                "checkpoint references an unknown wave identity",
            ));
        }
        validate_wave_states(&runtime, &restored_waves)?;
        let bots = reconstruct_bots(
            &runtime,
            &definitions.lane,
            &definitions.initial_bots,
            &restored_waves,
        )?;
        let restored = Self {
            runtime,
            lane: definitions.lane,
            bots,
            waves: restored_waves,
            definition_digest: encoded_definition_digest,
        };
        let recomputed = restored.lane_digest();
        if recomputed != encoded_lane_digest {
            return Err(OneLaneCheckpointError::LaneDigestMismatch {
                encoded: encoded_lane_digest,
                recomputed,
            });
        }
        Ok(restored)
    }
}

fn prepare_definitions(
    runtime: &MatchRuntime,
    lane: &LaneSpec,
    initial_bots: &[LaneBotSpec],
    waves: &[WaveSpec],
) -> Result<PreparedDefinitions, OneLaneCheckpointError> {
    if runtime.config().max_lanes != 1 {
        return Err(OneLaneCheckpointError::InvalidState(
            "one-lane profile must declare exactly one lane",
        ));
    }
    let lane = CompiledLane::compile(lane, runtime.config())?;

    if initial_bots.len() > usize::try_from(runtime.config().max_actors).unwrap_or(usize::MAX) {
        return Err(OneLaneCheckpointError::InvalidState(
            "initial bot count exceeds configured actor budget",
        ));
    }
    let mut initial_bots = initial_bots.to_vec();
    initial_bots.sort_unstable_by_key(|bot| bot.actor);
    if initial_bots
        .windows(2)
        .any(|pair| pair[0].actor == pair[1].actor)
    {
        return Err(OneLaneCheckpointError::InvalidState(
            "duplicate initial bot actor identity",
        ));
    }
    for bot in &initial_bots {
        if runtime.actor(bot.actor).is_some() {
            validate_bot(runtime, &lane, bot)?;
        } else {
            validate_bot_policy(
                runtime,
                &lane,
                bot.goal,
                bot.aggro_range_mm,
                &bot.target_priority,
                bot.cast_ability,
            )?;
        }
    }

    if waves.len() > usize::from(runtime.config().max_wave_schedules) {
        return Err(OneLaneCheckpointError::InvalidState(
            "wave definition count exceeds configured schedule budget",
        ));
    }
    let mut waves = waves.to_vec();
    waves.sort_unstable_by_key(|wave| wave.id);
    if waves.windows(2).any(|pair| pair[0].id == pair[1].id) {
        return Err(OneLaneCheckpointError::InvalidState(
            "duplicate wave definition identity",
        ));
    }
    for wave in &waves {
        validate_static_wave(runtime, &lane, wave)?;
    }
    let digest = one_lane_definition_digest(&lane, &initial_bots, &waves);
    Ok(PreparedDefinitions {
        lane,
        initial_bots,
        waves,
        digest,
    })
}

fn validate_static_wave(
    runtime: &MatchRuntime,
    lane: &CompiledLane,
    wave: &WaveSpec,
) -> Result<(), OneLaneCheckpointError> {
    validate_bot_policy(
        runtime,
        lane,
        wave.goal,
        wave.aggro_range_mm,
        &wave.target_priority,
        wave.cast_ability,
    )?;
    lane.point_at(wave.spawn)?;
    if wave.first_tick == 0 || wave.interval_ticks == 0 || wave.max_alive == 0 {
        return Err(OneLaneCheckpointError::InvalidState(
            "wave ticks, interval, and live cap must be positive",
        ));
    }
    if wave.units.is_empty()
        || wave.units.len() > usize::from(runtime.config().max_units_per_wave)
        || u32::try_from(wave.units.len()).unwrap_or(u32::MAX) > wave.max_alive
    {
        return Err(OneLaneCheckpointError::InvalidState(
            "wave unit batch is empty or exceeds its configured cap",
        ));
    }
    for unit in &wave.units {
        unit.stats.validate().map_err(LaneMatchError::Runtime)?;
        if let Some(attack) = unit.basic_attack {
            attack.validate().map_err(LaneMatchError::Runtime)?;
        }
        add_signed_progress(
            wave.spawn.progress_mm,
            unit.progress_offset_mm,
            lane.length_mm(),
        )?;
        if unit.abilities.len()
            > usize::try_from(runtime.config().max_abilities_per_actor).unwrap_or(usize::MAX)
        {
            return Err(OneLaneCheckpointError::InvalidState(
                "wave unit ability count exceeds configured actor budget",
            ));
        }
        let mut abilities = unit.abilities.clone();
        abilities.sort_unstable();
        if abilities.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(OneLaneCheckpointError::InvalidState(
                "wave unit ability list contains duplicates",
            ));
        }
        for ability in &abilities {
            if runtime.ability(*ability).is_none() {
                return Err(OneLaneCheckpointError::InvalidState(
                    "wave unit references an unknown ability",
                ));
            }
        }
        if wave
            .cast_ability
            .is_some_and(|ability| !abilities.contains(&ability))
        {
            return Err(OneLaneCheckpointError::InvalidState(
                "wave bot cast ability is not equipped by every unit",
            ));
        }
    }
    Ok(())
}

/// The one scheduled tick a wave must be waiting on at a frame boundary.
///
/// A wave fires when the tick being entered equals its cursor and then advances by exactly one interval,
/// whether it spawned or was skipped by its live cap. So at any settled boundary the cursor is the smallest
/// authored occurrence strictly greater than the current tick — a single legal value, not merely an
/// interval-aligned one. Deriving it means an interval-aligned forward or backward jump is rejected instead
/// of silently resuming a schedule that never would have occurred.
fn expected_next_tick(spec: &WaveSpec, tick: Tick) -> Result<Tick, OneLaneCheckpointError> {
    let interval = u64::from(spec.interval_ticks);
    if interval == 0 {
        return Err(OneLaneCheckpointError::InvalidState(
            "wave interval must be positive",
        ));
    }
    if spec.first_tick > tick {
        return Ok(spec.first_tick);
    }
    let elapsed = tick.saturating_sub(spec.first_tick);
    let occurrences = (elapsed / interval)
        .checked_add(1)
        .ok_or(OneLaneCheckpointError::CountOverflow)?;
    occurrences
        .checked_mul(interval)
        .and_then(|offset| spec.first_tick.checked_add(offset))
        .ok_or(OneLaneCheckpointError::CountOverflow)
}

/// Group every authoritative actor that carries wave-spawn provenance by the wave that created it.
///
/// This is the tamper-resistant half of wave membership: the wave ID and unit slot are stamped by the
/// allocator, pinned to the actor's own never-reused ID by the runtime's frame-boundary audit, and hashed
/// into the world digest. Membership is therefore re-derived from actor truth rather than believed from a
/// separately encoded list.
fn derive_wave_membership(runtime: &MatchRuntime) -> BTreeMap<u32, BTreeSet<ActorId>> {
    let mut membership: BTreeMap<u32, BTreeSet<ActorId>> = BTreeMap::new();
    for actor in runtime.actors() {
        if let ActorProvenance::Wave { wave_id, .. } = actor.provenance {
            membership.entry(wave_id).or_default().insert(actor.id);
        }
    }
    membership
}

fn validate_wave_states(
    runtime: &MatchRuntime,
    waves: &[WaveState],
) -> Result<(), OneLaneCheckpointError> {
    let mut derived = derive_wave_membership(runtime);
    for wave in waves {
        if wave.next_tick <= runtime.tick() {
            return Err(OneLaneCheckpointError::InvalidState(
                "wave scheduler tick is stale",
            ));
        }
        if wave.next_tick != expected_next_tick(&wave.spec, runtime.tick())? {
            return Err(OneLaneCheckpointError::InvalidState(
                "wave scheduler tick is not the exact next authored occurrence",
            ));
        }
        if wave.live.len() > usize::try_from(wave.spec.max_alive).unwrap_or(usize::MAX) {
            return Err(OneLaneCheckpointError::InvalidState(
                "wave live set exceeds its authored cap",
            ));
        }
        for actor_id in &wave.live {
            let actor = runtime
                .actor(*actor_id)
                .ok_or(OneLaneCheckpointError::InvalidState(
                    "wave live set references an unknown actor",
                ))?;
            if !actor.alive
                || actor.owner.is_some()
                || actor.kind != ActorKind::Minion
                || actor.team != wave.spec.team
            {
                return Err(OneLaneCheckpointError::InvalidState(
                    "wave live actor has invalid life, ownership, kind, or team",
                ));
            }
            match actor.provenance {
                ActorProvenance::Wave {
                    wave_id, unit_slot, ..
                } if wave_id == wave.spec.id.get()
                    && usize::from(unit_slot) < wave.spec.units.len() => {}
                _ => {
                    return Err(OneLaneCheckpointError::InvalidState(
                        "wave live actor was not spawned by this wave's authored schedule",
                    ));
                }
            }
        }
        // Exact set equality in both directions: an actor cannot be smuggled into a wave it was not
        // spawned by, and a live wave actor cannot be hidden from the cap and bot registry by omission.
        if derived.remove(&wave.spec.id.get()).unwrap_or_default() != wave.live {
            return Err(OneLaneCheckpointError::InvalidState(
                "wave live set does not match the spawn provenance recorded on authoritative actors",
            ));
        }
    }
    if !derived.is_empty() {
        return Err(OneLaneCheckpointError::InvalidState(
            "an authoritative actor carries provenance for a wave that is not scheduled",
        ));
    }
    Ok(())
}

fn reconstruct_bots(
    runtime: &MatchRuntime,
    lane: &CompiledLane,
    initial_bots: &[LaneBotSpec],
    waves: &[WaveState],
) -> Result<Vec<LaneBotSpec>, OneLaneCheckpointError> {
    let mut bots = Vec::new();
    bots.try_reserve_exact(
        initial_bots
            .len()
            .checked_add(waves.iter().map(|wave| wave.live.len()).sum::<usize>())
            .ok_or(OneLaneCheckpointError::CountOverflow)?,
    )
    .map_err(|_| OneLaneCheckpointError::AllocationFailed)?;
    for bot in initial_bots {
        if runtime.actor(bot.actor).is_some() {
            validate_bot(runtime, lane, bot)?;
            bots.push(bot.clone());
        }
    }
    for wave in waves {
        for actor in &wave.live {
            let bot = LaneBotSpec {
                actor: *actor,
                goal: wave.spec.goal,
                aggro_range_mm: wave.spec.aggro_range_mm,
                target_priority: wave.spec.target_priority.clone(),
                cast_ability: wave.spec.cast_ability,
            };
            validate_bot(runtime, lane, &bot)?;
            bots.push(bot);
        }
    }
    bots.sort_unstable_by_key(|bot| bot.actor);
    if bots.windows(2).any(|pair| pair[0].actor == pair[1].actor) {
        return Err(OneLaneCheckpointError::InvalidState(
            "reconstructed bot registry contains duplicate actors",
        ));
    }
    Ok(bots)
}

fn same_lane(left: &CompiledLane, right: &CompiledLane) -> bool {
    left.id() == right.id()
        && left.length_mm() == right.length_mm()
        && left.half_width_mm() == right.half_width_mm()
        && left.centerline() == right.centerline()
}

struct Encoder {
    bytes: Vec<u8>,
    limit: usize,
}

impl Encoder {
    fn new(limit: usize) -> Result<Self, OneLaneCheckpointError> {
        if limit < HEADER_BYTES {
            return Err(OneLaneCheckpointError::SizeLimitExceeded {
                size: u64::try_from(HEADER_BYTES).unwrap_or(u64::MAX),
                limit: u64::try_from(limit).unwrap_or(u64::MAX),
            });
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(HEADER_BYTES)
            .map_err(|_| OneLaneCheckpointError::AllocationFailed)?;
        Ok(Self { bytes, limit })
    }

    fn bytes(&mut self, value: &[u8]) -> Result<(), OneLaneCheckpointError> {
        let next = self.bytes.len().checked_add(value.len()).ok_or(
            OneLaneCheckpointError::SizeLimitExceeded {
                size: u64::MAX,
                limit: u64::try_from(self.limit).unwrap_or(u64::MAX),
            },
        )?;
        if next > self.limit {
            return Err(OneLaneCheckpointError::SizeLimitExceeded {
                size: u64::try_from(next).unwrap_or(u64::MAX),
                limit: u64::try_from(self.limit).unwrap_or(u64::MAX),
            });
        }
        self.bytes
            .try_reserve_exact(value.len())
            .map_err(|_| OneLaneCheckpointError::AllocationFailed)?;
        self.bytes.extend_from_slice(value);
        Ok(())
    }

    fn u8(&mut self, value: u8) -> Result<(), OneLaneCheckpointError> {
        self.bytes(&[value])
    }

    fn u16(&mut self, value: u16) -> Result<(), OneLaneCheckpointError> {
        self.bytes(&value.to_le_bytes())
    }

    fn u32(&mut self, value: u32) -> Result<(), OneLaneCheckpointError> {
        self.bytes(&value.to_le_bytes())
    }

    fn u64(&mut self, value: u64) -> Result<(), OneLaneCheckpointError> {
        self.bytes(&value.to_le_bytes())
    }

    fn count(&mut self, value: usize) -> Result<(), OneLaneCheckpointError> {
        self.u32(u32::try_from(value).map_err(|_| OneLaneCheckpointError::CountOverflow)?)
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

    fn take(&mut self, length: usize) -> Result<&'a [u8], OneLaneCheckpointError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(OneLaneCheckpointError::Truncated)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(OneLaneCheckpointError::Truncated)?;
        self.offset = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, OneLaneCheckpointError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, OneLaneCheckpointError> {
        Ok(u32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| OneLaneCheckpointError::Truncated)?,
        ))
    }

    fn u64(&mut self) -> Result<u64, OneLaneCheckpointError> {
        Ok(u64::from_le_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| OneLaneCheckpointError::Truncated)?,
        ))
    }

    fn expect_tag(&mut self, expected: u8) -> Result<(), OneLaneCheckpointError> {
        let actual = self.u8()?;
        if actual == expected {
            Ok(())
        } else {
            Err(OneLaneCheckpointError::InvalidTag { expected, actual })
        }
    }

    fn count(
        &mut self,
        limit: usize,
        detail: &'static str,
    ) -> Result<usize, OneLaneCheckpointError> {
        let count =
            usize::try_from(self.u32()?).map_err(|_| OneLaneCheckpointError::CountOverflow)?;
        if count > limit {
            return Err(OneLaneCheckpointError::InvalidState(detail));
        }
        Ok(count)
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    fn require_minimum(
        &self,
        count: usize,
        item_bytes: usize,
    ) -> Result<(), OneLaneCheckpointError> {
        let required = count
            .checked_mul(item_bytes)
            .ok_or(OneLaneCheckpointError::Truncated)?;
        if required > self.remaining() {
            return Err(OneLaneCheckpointError::Truncated);
        }
        Ok(())
    }

    fn finish(self) -> Result<(), OneLaneCheckpointError> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(OneLaneCheckpointError::LengthMismatch)
        }
    }
}

fn enforce_size(size: usize, limit: usize) -> Result<(), OneLaneCheckpointError> {
    if size > limit {
        Err(OneLaneCheckpointError::SizeLimitExceeded {
            size: u64::try_from(size).unwrap_or(u64::MAX),
            limit: u64::try_from(limit).unwrap_or(u64::MAX),
        })
    } else {
        Ok(())
    }
}

fn read_header_u16(bytes: &[u8], offset: usize) -> Result<u16, OneLaneCheckpointError> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or(OneLaneCheckpointError::Truncated)?
            .try_into()
            .map_err(|_| OneLaneCheckpointError::Truncated)?,
    ))
}

fn read_header_u32(bytes: &[u8], offset: usize) -> Result<u32, OneLaneCheckpointError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(OneLaneCheckpointError::Truncated)?
            .try_into()
            .map_err(|_| OneLaneCheckpointError::Truncated)?,
    ))
}

fn read_header_u64(bytes: &[u8], offset: usize) -> Result<u64, OneLaneCheckpointError> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(OneLaneCheckpointError::Truncated)?
            .try_into()
            .map_err(|_| OneLaneCheckpointError::Truncated)?,
    ))
}

// FNV is only bounded accidental-corruption evidence. Hosts must authenticate checkpoints before decoding.
fn checksum(bytes: &[u8]) -> u64 {
    let mut value = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        value ^= u64::from(*byte);
        value = value.wrapping_mul(0x0000_0100_0000_01b3);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ActorSpawn, BasicAttackSpec, CombatStats, DamageSchool, DeathRule, DynamicActorProvenance,
        DynamicActorSpawn, LaneId, LanePosition, MatchConfig, RectMm, TeamId, Vec2Mm, WaveId,
        WaveUnitSpec,
    };

    const CONTENT: ContentId = ContentId::new([0x31; 32]);
    const OTHER_CONTENT: ContentId = ContentId::new([0x13; 32]);

    struct Fixture {
        game: OneLaneMatch,
        lane: LaneSpec,
        initial_bots: Vec<LaneBotSpec>,
        waves: Vec<WaveSpec>,
    }

    const fn unit_stats() -> CombatStats {
        CombatStats {
            max_health: 240,
            max_resource: 0,
            move_speed_mm_per_tick: 500,
            physical_reduction_bps: 0,
            magic_reduction_bps: 0,
        }
    }

    const fn attack() -> BasicAttackSpec {
        BasicAttackSpec {
            range_mm: 700,
            damage: 60,
            school: DamageSchool::True,
            windup_ticks: 1,
            cooldown_ticks: 2,
        }
    }

    fn wave(id: u32, team: u8, first_tick: Tick, spawn: u64, goal: u64) -> WaveSpec {
        WaveSpec {
            id: WaveId(id),
            team: TeamId(team),
            spawn: LanePosition {
                lane: LaneId(0),
                progress_mm: spawn,
            },
            goal: LanePosition {
                lane: LaneId(0),
                progress_mm: goal,
            },
            aggro_range_mm: 2_000,
            target_priority: vec![ActorKind::Minion],
            cast_ability: None,
            first_tick,
            interval_ticks: 4,
            max_alive: 8,
            units: vec![
                WaveUnitSpec {
                    progress_offset_mm: 0,
                    stats: unit_stats(),
                    abilities: Vec::new(),
                    basic_attack: Some(attack()),
                },
                WaveUnitSpec {
                    progress_offset_mm: if team == 0 { 100 } else { -100 },
                    stats: unit_stats(),
                    abilities: Vec::new(),
                    basic_attack: Some(attack()),
                },
            ],
        }
    }

    fn fixture() -> Fixture {
        let config = MatchConfig {
            map_bounds: RectMm::new(Vec2Mm::new(-1_000, -1_000), Vec2Mm::new(11_000, 1_000)),
            max_match_ticks: 40,
            ..MatchConfig::default()
        };
        let mut runtime = MatchRuntime::new(config, 0x5eed).unwrap();
        runtime
            .spawn_actor(&ActorSpawn {
                id: ActorId(1),
                owner: None,
                team: TeamId(0),
                kind: ActorKind::Hero,
                position: Vec2Mm::new(5_000, 0),
                stats: unit_stats(),
                abilities: Vec::new(),
                basic_attack: None,
                death_rule: DeathRule::StayDead,
            })
            .unwrap();
        runtime.start().unwrap();
        let lane = LaneSpec {
            id: LaneId(0),
            centerline: vec![Vec2Mm::new(0, 0), Vec2Mm::new(10_000, 0)],
            half_width_mm: 500,
        };
        let initial_bots = Vec::new();
        let waves = vec![wave(10, 0, 2, 500, 9_500), wave(20, 1, 3, 9_500, 500)];
        let game =
            OneLaneMatch::attach(runtime, &lane, initial_bots.clone(), waves.clone()).unwrap();
        Fixture {
            game,
            lane,
            initial_bots,
            waves,
        }
    }

    fn advance_to_checkpoint(fixture: &mut Fixture) {
        for _ in 0..8 {
            fixture.game.step().unwrap();
        }
        assert!(fixture.game.waves.iter().all(|wave| wave.live.len() >= 4));
    }

    #[test]
    fn multi_wave_suffix_is_identical_after_atomic_restore() {
        let mut fixture = fixture();
        advance_to_checkpoint(&mut fixture);
        let checkpoint = fixture
            .game
            .capture_checkpoint(
                CONTENT,
                &fixture.lane,
                &fixture.initial_bots,
                &fixture.waves,
            )
            .unwrap();
        let mut restored = OneLaneMatch::restore_checkpoint(
            &checkpoint,
            CONTENT,
            &fixture.lane,
            &fixture.initial_bots,
            &fixture.waves,
        )
        .unwrap();
        assert_eq!(restored.lane_digest(), fixture.game.lane_digest());

        for _ in 0..12 {
            let uninterrupted = fixture.game.step().unwrap();
            let resumed = restored.step().unwrap();
            assert_eq!(resumed, uninterrupted);
        }
    }

    #[test]
    fn corruption_content_and_definition_mismatch_fail_closed() {
        let mut fixture = fixture();
        advance_to_checkpoint(&mut fixture);
        let checkpoint = fixture
            .game
            .capture_checkpoint(
                CONTENT,
                &fixture.lane,
                &fixture.initial_bots,
                &fixture.waves,
            )
            .unwrap();

        assert!(matches!(
            OneLaneMatch::restore_checkpoint(
                &checkpoint,
                OTHER_CONTENT,
                &fixture.lane,
                &fixture.initial_bots,
                &fixture.waves,
            ),
            Err(OneLaneCheckpointError::ContentMismatch { .. })
        ));

        let mut mismatched_waves = fixture.waves.clone();
        mismatched_waves[0].interval_ticks += 1;
        assert!(matches!(
            OneLaneMatch::restore_checkpoint(
                &checkpoint,
                CONTENT,
                &fixture.lane,
                &fixture.initial_bots,
                &mismatched_waves,
            ),
            Err(OneLaneCheckpointError::DefinitionMismatch)
        ));

        let mut corrupted = checkpoint.as_bytes().to_vec();
        let last = corrupted.len() - 1;
        corrupted[last] ^= 0x80;
        let corrupted = OneLaneCheckpoint::from_bytes(&corrupted).unwrap();
        assert!(matches!(
            OneLaneMatch::restore_checkpoint(
                &corrupted,
                CONTENT,
                &fixture.lane,
                &fixture.initial_bots,
                &fixture.waves,
            ),
            Err(OneLaneCheckpointError::CorruptPayload)
        ));
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one envelope mutation matrix shares parsed section offsets to prevent test drift"
    )]
    fn unknown_wave_live_actor_stale_tick_and_digest_tamper_are_rejected() {
        let mut fixture = fixture();
        advance_to_checkpoint(&mut fixture);
        let checkpoint = fixture
            .game
            .capture_checkpoint(
                CONTENT,
                &fixture.lane,
                &fixture.initial_bots,
                &fixture.waves,
            )
            .unwrap();
        let bytes = checkpoint.as_bytes();
        let core_length = u32::from_le_bytes(
            bytes[HEADER_BYTES + 1..HEADER_BYTES + 5]
                .try_into()
                .unwrap(),
        ) as usize;
        let waves_tag_offset = HEADER_BYTES + 1 + 4 + core_length;
        let first_wave_offset = waves_tag_offset + 1 + 4;

        let mut unknown_wave = bytes.to_vec();
        unknown_wave[first_wave_offset..first_wave_offset + 4]
            .copy_from_slice(&999_u32.to_le_bytes());
        patch_checksum(&mut unknown_wave);
        let unknown_wave = OneLaneCheckpoint::from_bytes(&unknown_wave).unwrap();
        assert!(matches!(
            OneLaneMatch::restore_checkpoint(
                &unknown_wave,
                CONTENT,
                &fixture.lane,
                &fixture.initial_bots,
                &fixture.waves,
            ),
            Err(OneLaneCheckpointError::InvalidState(_))
        ));

        let mut stale_tick = bytes.to_vec();
        stale_tick[first_wave_offset + 4..first_wave_offset + 12]
            .copy_from_slice(&fixture.game.runtime().tick().to_le_bytes());
        patch_checksum(&mut stale_tick);
        let stale_tick = OneLaneCheckpoint::from_bytes(&stale_tick).unwrap();
        assert!(matches!(
            OneLaneMatch::restore_checkpoint(
                &stale_tick,
                CONTENT,
                &fixture.lane,
                &fixture.initial_bots,
                &fixture.waves,
            ),
            Err(OneLaneCheckpointError::InvalidState(
                "wave scheduler tick is stale"
            ))
        ));

        let first_live_offset = first_wave_offset + 4 + 8 + 4;
        let mut unknown_actor = bytes.to_vec();
        unknown_actor[first_live_offset..first_live_offset + 8]
            .copy_from_slice(&u64::MAX.to_le_bytes());
        patch_checksum(&mut unknown_actor);
        let unknown_actor = OneLaneCheckpoint::from_bytes(&unknown_actor).unwrap();
        assert!(matches!(
            OneLaneMatch::restore_checkpoint(
                &unknown_actor,
                CONTENT,
                &fixture.lane,
                &fixture.initial_bots,
                &fixture.waves,
            ),
            Err(OneLaneCheckpointError::InvalidState(
                "wave live set references an unknown actor"
            ))
        ));

        let mut wrong_kind = bytes.to_vec();
        wrong_kind[first_live_offset..first_live_offset + 8].copy_from_slice(&1_u64.to_le_bytes());
        patch_checksum(&mut wrong_kind);
        let wrong_kind = OneLaneCheckpoint::from_bytes(&wrong_kind).unwrap();
        assert!(matches!(
            OneLaneMatch::restore_checkpoint(
                &wrong_kind,
                CONTENT,
                &fixture.lane,
                &fixture.initial_bots,
                &fixture.waves,
            ),
            Err(OneLaneCheckpointError::InvalidState(
                "wave live actor has invalid life, ownership, kind, or team"
            ))
        ));

        let first_live_count = u32::from_le_bytes(
            bytes[first_wave_offset + 12..first_wave_offset + 16]
                .try_into()
                .unwrap(),
        ) as usize;
        let second_wave_offset = first_wave_offset + 16 + first_live_count * 8;
        let second_live_offset = second_wave_offset + 16;
        let first_actor: [u8; 8] = bytes[first_live_offset..first_live_offset + 8]
            .try_into()
            .unwrap();
        let second_actor: [u8; 8] = bytes[second_live_offset..second_live_offset + 8]
            .try_into()
            .unwrap();
        let mut wrong_teams = bytes.to_vec();
        wrong_teams[first_live_offset..first_live_offset + 8].copy_from_slice(&second_actor);
        wrong_teams[second_live_offset..second_live_offset + 8].copy_from_slice(&first_actor);
        patch_checksum(&mut wrong_teams);
        let wrong_teams = OneLaneCheckpoint::from_bytes(&wrong_teams).unwrap();
        assert!(matches!(
            OneLaneMatch::restore_checkpoint(
                &wrong_teams,
                CONTENT,
                &fixture.lane,
                &fixture.initial_bots,
                &fixture.waves,
            ),
            Err(OneLaneCheckpointError::InvalidState(
                "wave live actor has invalid life, ownership, kind, or team"
            ))
        ));

        let mut bad_digest = bytes.to_vec();
        bad_digest[LANE_DIGEST_OFFSET] ^= 1;
        let bad_digest = OneLaneCheckpoint::from_bytes(&bad_digest).unwrap();
        assert!(matches!(
            OneLaneMatch::restore_checkpoint(
                &bad_digest,
                CONTENT,
                &fixture.lane,
                &fixture.initial_bots,
                &fixture.waves,
            ),
            Err(OneLaneCheckpointError::LaneDigestMismatch { .. })
        ));
    }

    #[test]
    fn truncation_oversize_and_invalid_in_memory_state_fail_closed() {
        let mut fixture = fixture();
        advance_to_checkpoint(&mut fixture);
        let checkpoint = fixture
            .game
            .capture_checkpoint(
                CONTENT,
                &fixture.lane,
                &fixture.initial_bots,
                &fixture.waves,
            )
            .unwrap();
        let truncated = OneLaneCheckpoint::from_bytes(&checkpoint.as_bytes()[..20]).unwrap();
        assert!(matches!(
            OneLaneMatch::restore_checkpoint(
                &truncated,
                CONTENT,
                &fixture.lane,
                &fixture.initial_bots,
                &fixture.waves,
            ),
            Err(OneLaneCheckpointError::Truncated)
        ));
        let oversized = vec![0; usize::try_from(MAX_RUNTIME_CHECKPOINT_BYTES).unwrap() + 1];
        assert!(matches!(
            OneLaneCheckpoint::from_bytes(&oversized),
            Err(OneLaneCheckpointError::SizeLimitExceeded { .. })
        ));

        let tight_config = MatchConfig {
            map_bounds: RectMm::new(Vec2Mm::new(-1_000, -1_000), Vec2Mm::new(11_000, 1_000)),
            max_checkpoint_bytes: 220,
            ..MatchConfig::default()
        };
        let mut tight_runtime = MatchRuntime::new(tight_config, 1).unwrap();
        tight_runtime.start().unwrap();
        let tight_lane = LaneSpec {
            id: LaneId(0),
            centerline: vec![Vec2Mm::new(0, 0), Vec2Mm::new(10_000, 0)],
            half_width_mm: 500,
        };
        let tight_match = OneLaneMatch::attach(tight_runtime, &tight_lane, vec![], vec![]).unwrap();
        assert!(matches!(
            tight_match.capture_checkpoint(CONTENT, &tight_lane, &[], &[]),
            Err(OneLaneCheckpointError::SizeLimitExceeded { .. })
        ));

        fixture.game.waves[0].next_tick = fixture.game.runtime().tick();
        assert!(matches!(
            fixture.game.capture_checkpoint(
                CONTENT,
                &fixture.lane,
                &fixture.initial_bots,
                &fixture.waves,
            ),
            Err(OneLaneCheckpointError::InvalidState(
                "wave scheduler tick is stale"
            ))
        ));
    }

    /// A captured checkpoint plus the parsed offsets of its first encoded wave, and one server actor that is
    /// indistinguishable from a wave minion in every field except its creation record.
    struct TamperFixture {
        fixture: Fixture,
        checkpoint: OneLaneCheckpoint,
        smuggled: ActorId,
        first_wave_offset: usize,
        live_count_offset: usize,
        first_live_offset: usize,
        live_count: usize,
    }

    impl TamperFixture {
        fn new() -> Self {
            let mut fixture = fixture();
            advance_to_checkpoint(&mut fixture);

            // Alive, unowned, Minion, team 0 — everything the pre-provenance audit could inspect about a
            // wave 10 minion. Only its creation record says it was not spawned by that schedule.
            let smuggled = fixture
                .game
                .runtime
                .spawn_dynamic_actor(&DynamicActorSpawn {
                    provenance: DynamicActorProvenance::Generic,
                    team: TeamId(0),
                    kind: ActorKind::Minion,
                    position: Vec2Mm::new(5_000, 0),
                    stats: unit_stats(),
                    abilities: Vec::new(),
                    basic_attack: Some(attack()),
                    death_rule: DeathRule::Despawn,
                })
                .unwrap();
            // Spawning leaves the runtime mid-frame; one step returns it to a capturable boundary.
            fixture.game.step().unwrap();
            let view = fixture.game.runtime().actor(smuggled).unwrap();
            assert!(view.alive);
            assert_eq!(view.team, TeamId(0));
            assert_eq!(view.kind, ActorKind::Minion);
            assert_eq!(
                view.provenance,
                ActorProvenance::Dynamic {
                    spawn_ordinal: smuggled.get()
                }
            );

            let checkpoint = fixture
                .game
                .capture_checkpoint(
                    CONTENT,
                    &fixture.lane,
                    &fixture.initial_bots,
                    &fixture.waves,
                )
                .unwrap();
            let bytes = checkpoint.as_bytes();
            let core_length = u32::from_le_bytes(
                bytes[HEADER_BYTES + 1..HEADER_BYTES + 5]
                    .try_into()
                    .unwrap(),
            ) as usize;
            let first_wave_offset = HEADER_BYTES + 1 + 4 + core_length + 1 + 4;
            let live_count_offset = first_wave_offset + 12;
            let first_live_offset = first_wave_offset + 16;
            let live_count = u32::from_le_bytes(
                bytes[live_count_offset..live_count_offset + 4]
                    .try_into()
                    .unwrap(),
            ) as usize;
            assert!(live_count > 1);
            Self {
                fixture,
                checkpoint,
                smuggled,
                first_wave_offset,
                live_count_offset,
                first_live_offset,
                live_count,
            }
        }

        fn bytes(&self) -> &[u8] {
            self.checkpoint.as_bytes()
        }

        /// Restore the honest checkpoint so a test can mutate that state and ask it for the lane digest the
        /// tampered bytes genuinely hash to.
        fn restore_honest(&self) -> OneLaneMatch {
            self.restore(&self.checkpoint).unwrap()
        }

        fn restore(
            &self,
            checkpoint: &OneLaneCheckpoint,
        ) -> Result<OneLaneMatch, OneLaneCheckpointError> {
            OneLaneMatch::restore_checkpoint(
                checkpoint,
                CONTENT,
                &self.fixture.lane,
                &self.fixture.initial_bots,
                &self.fixture.waves,
            )
        }
    }

    /// A schedule cursor that is interval-aligned but is not the occurrence that would actually have
    /// happened. The rewrite is internally consistent — correct length, recomputed checksum, and the exact
    /// lane digest the tampered state hashes to — so only relational validation can reject it.
    #[test]
    fn an_aligned_schedule_jump_is_rejected_by_a_fully_consistent_envelope() {
        let harness = TamperFixture::new();
        let bytes = harness.bytes();
        let tick_offset = harness.first_wave_offset + 4;
        let encoded_tick =
            u64::from_le_bytes(bytes[tick_offset..tick_offset + 8].try_into().unwrap());
        let interval = u64::from(harness.fixture.waves[0].interval_ticks);
        assert_eq!(
            (encoded_tick - harness.fixture.waves[0].first_tick) % interval,
            0,
            "the honest cursor is itself interval-aligned, so alignment cannot be the gate"
        );

        let mut jumped_state = harness.restore_honest();
        jumped_state.waves[0].next_tick = encoded_tick + interval;
        let mut jumped = bytes.to_vec();
        jumped[tick_offset..tick_offset + 8]
            .copy_from_slice(&(encoded_tick + interval).to_le_bytes());
        patch_lane_digest(&mut jumped, jumped_state.lane_digest());
        patch_checksum(&mut jumped);
        let jumped = OneLaneCheckpoint::from_bytes(&jumped).unwrap();
        assert!(matches!(
            harness.restore(&jumped),
            Err(OneLaneCheckpointError::InvalidState(
                "wave scheduler tick is not the exact next authored occurrence"
            ))
        ));
    }

    /// Wave membership that disagrees with the creation evidence stamped on the actors themselves, in both
    /// directions: an actor smuggled into a wave that did not spawn it, and a real wave minion hidden from
    /// its own wave to free live-cap headroom.
    #[test]
    fn smuggled_and_hidden_wave_membership_is_rejected_by_a_fully_consistent_envelope() {
        let harness = TamperFixture::new();
        let bytes = harness.bytes();
        let first_live = harness.first_live_offset;
        let last_live = first_live + (harness.live_count - 1) * 8;

        let evicted = ActorId(u64::from_le_bytes(
            bytes[first_live..first_live + 8].try_into().unwrap(),
        ));
        let mut substituted_state = harness.restore_honest();
        assert!(substituted_state.waves[0].live.remove(&evicted));
        assert!(substituted_state.waves[0].live.insert(harness.smuggled));
        let mut substituted = bytes.to_vec();
        substituted[first_live..first_live + 8]
            .copy_from_slice(&harness.smuggled.get().to_le_bytes());
        patch_lane_digest(&mut substituted, substituted_state.lane_digest());
        patch_checksum(&mut substituted);
        let substituted = OneLaneCheckpoint::from_bytes(&substituted).unwrap();
        assert!(matches!(
            harness.restore(&substituted),
            Err(OneLaneCheckpointError::InvalidState(
                "wave live actor was not spawned by this wave's authored schedule"
            ))
        ));

        // Every retained entry is legitimate, so only derived-versus-encoded equality catches the omission.
        let hidden = ActorId(u64::from_le_bytes(
            bytes[last_live..last_live + 8].try_into().unwrap(),
        ));
        let mut omitted_state = harness.restore_honest();
        assert!(omitted_state.waves[0].live.remove(&hidden));
        let mut omitted = bytes[..last_live].to_vec();
        omitted.extend_from_slice(&bytes[last_live + 8..]);
        omitted[harness.live_count_offset..harness.live_count_offset + 4]
            .copy_from_slice(&u32::try_from(harness.live_count - 1).unwrap().to_le_bytes());
        patch_lane_digest(&mut omitted, omitted_state.lane_digest());
        patch_length_and_checksum(&mut omitted);
        let omitted = OneLaneCheckpoint::from_bytes(&omitted).unwrap();
        assert!(matches!(
            harness.restore(&omitted),
            Err(OneLaneCheckpointError::InvalidState(
                "wave live set does not match the spawn provenance recorded on authoritative actors"
            ))
        ));
    }

    fn patch_checksum(bytes: &mut [u8]) {
        let payload_checksum = checksum(&bytes[HEADER_BYTES..]);
        bytes[PAYLOAD_CHECKSUM_OFFSET..PAYLOAD_CHECKSUM_OFFSET + 8]
            .copy_from_slice(&payload_checksum.to_le_bytes());
    }

    fn patch_lane_digest(bytes: &mut [u8], digest: LaneMatchDigest) {
        bytes[LANE_DIGEST_OFFSET..LANE_DIGEST_OFFSET + 8].copy_from_slice(&digest.0.to_le_bytes());
    }

    fn patch_length_and_checksum(bytes: &mut [u8]) {
        let payload_length = u32::try_from(bytes.len() - HEADER_BYTES).unwrap();
        bytes[PAYLOAD_LENGTH_OFFSET..PAYLOAD_LENGTH_OFFSET + 4]
            .copy_from_slice(&payload_length.to_le_bytes());
        patch_checksum(bytes);
    }
}
