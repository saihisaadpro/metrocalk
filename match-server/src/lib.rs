//! Implement this feature in accordance with the Engine UI/UX Architecture Constitution.
//!
//! This MOB-2 candidate is the smallest honest headless-host boundary around the portable gameplay kernel.
//! It owns lifecycle, a deterministic core-objective fixture, and bounded operational evidence. The fixture
//! does not exercise the lane controller or wave scheduler. The host deliberately has no socket, protocol
//! codec, database, control plane, real-time scheduler, or container integration.

use std::collections::VecDeque;
use std::error::Error;
use std::fmt::{self, Display, Formatter};

use metrocalk_gameplay::{
    ActorId, ActorIntent, ActorKind, ActorSpawn, BasicAttackSpec, Bounty, CombatStats, CommandKind,
    CompiledLane, DamageSchool, DeathRule, FrameDigest, InvariantViolation, LaneError, LaneId,
    LanePosition, LaneSpec, MatchConfig, MatchEndReason, MatchEvent, MatchOutcome, MatchPhase,
    MatchRuntime, RuntimeError, StatGrowth, TeamId, Tick, Vec2Mm, WorldDigest,
};

/// Immutable build metadata exposed by the `--version-json` command.
pub const VERSION_JSON: &str = concat!(
    "{\"schemaVersion\":1,\"artifact\":\"metrocalk-match-server\",\"version\":\"",
    env!("CARGO_PKG_VERSION"),
    "\",\"status\":\"mob2-candidate-not-production\",\"transport\":\"none\"}"
);

pub const REFERENCE_PROFILE_ID: &str = "mob2-core-objective-host-smoke-v1";
pub const REFERENCE_SEED: u64 = 0x4D4F_4241_0000_0002;
pub const REFERENCE_MAX_TICKS: Tick = 64;
pub const DEFAULT_DIAGNOSTIC_CAPACITY: usize = 32;
const MAX_DIAGNOSTIC_CAPACITY: usize = 256;

const TEAM_BLUE: TeamId = TeamId(0);
const TEAM_RED: TeamId = TeamId(1);
const HERO_BLUE: ActorId = ActorId(100);
const CORE_BLUE: ActorId = ActorId(101);
const HERO_RED: ActorId = ActorId(200);
const CORE_RED: ActorId = ActorId(201);

/// Testable host lifecycle. Readiness is true only in `Ready`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostState {
    Ready,
    Running,
    Draining,
    Stopped,
}

/// Fixed operational event taxonomy. Diagnostics carry no unbounded strings or payloads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticCode {
    MatchStarted,
    MatchCompleted,
    DrainRequested,
    MatchCancelledForDrain,
    HostStopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub at_tick: Tick,
}

/// Fixed-cardinality counters suitable for a later metrics adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostMetrics {
    pub matches_started: u64,
    pub matches_completed: u64,
    pub matches_cancelled_for_drain: u64,
    pub ticks_executed: u64,
    pub internal_intents_issued: u64,
    pub internal_intents_rejected: u64,
    pub diagnostics_dropped: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostConfig {
    pub diagnostic_capacity: usize,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            diagnostic_capacity: DEFAULT_DIAGNOSTIC_CAPACITY,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchReport {
    pub profile_id: &'static str,
    pub seed: u64,
    pub final_tick: Tick,
    pub outcome: MatchOutcome,
    pub reason: MatchEndReason,
    pub world_digest: WorldDigest,
    pub terminal_frame_digest: FrameDigest,
}

impl MatchReport {
    /// Stable single-line output for CI smoke evidence. Values are drawn only from fixed identifiers and
    /// integers, so this does not need a general-purpose JSON serializer.
    #[must_use]
    pub fn to_json(self) -> String {
        let outcome = match self.outcome {
            MatchOutcome::Winner(team) => format!("{{\"winnerTeam\":{}}}", team.get()),
            MatchOutcome::Draw => "\"draw\"".to_owned(),
        };
        let reason = match self.reason {
            MatchEndReason::ObjectiveDestroyed => "objective-destroyed",
            MatchEndReason::TimeLimit => "time-limit",
            MatchEndReason::Host => "host",
        };
        format!(
            concat!(
                "{{\"schemaVersion\":1,\"profileId\":\"{}\",\"seed\":{},",
                "\"finalTick\":{},\"outcome\":{},\"reason\":\"{}\",",
                "\"worldDigest\":{},\"terminalFrameDigest\":{}}}"
            ),
            self.profile_id,
            self.seed,
            self.final_tick,
            outcome,
            reason,
            self.world_digest.0,
            self.terminal_frame_digest.0
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum HostError {
    InvalidDiagnosticCapacity,
    InvalidState {
        expected: HostState,
        actual: HostState,
    },
    Runtime(RuntimeError),
    Invariant(InvariantViolation),
    ReferenceLaneInvalid(LaneError),
    ReferenceMatchDidNotTerminate,
    TerminalFrameMissingResult,
}

impl Display for HostError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDiagnosticCapacity => write!(
                formatter,
                "diagnostic capacity must be between 1 and {MAX_DIAGNOSTIC_CAPACITY}"
            ),
            Self::InvalidState { expected, actual } => {
                write!(
                    formatter,
                    "host state must be {expected:?}, but is {actual:?}"
                )
            }
            Self::Runtime(error) => Display::fmt(error, formatter),
            Self::Invariant(error) => Display::fmt(error, formatter),
            Self::ReferenceLaneInvalid(error) => {
                write!(formatter, "reference lane is invalid: {error}")
            }
            Self::ReferenceMatchDidNotTerminate => {
                formatter.write_str("reference match exceeded its bounded smoke window")
            }
            Self::TerminalFrameMissingResult => {
                formatter.write_str("gameplay emitted a terminal frame without a terminal result")
            }
        }
    }
}

impl Error for HostError {}

impl From<RuntimeError> for HostError {
    fn from(value: RuntimeError) -> Self {
        Self::Runtime(value)
    }
}

impl From<InvariantViolation> for HostError {
    fn from(value: InvariantViolation) -> Self {
        Self::Invariant(value)
    }
}

struct ActiveMatch {
    runtime: MatchRuntime,
}

/// In-process candidate host. One instance intentionally owns at most one active match.
pub struct MatchHost {
    state: HostState,
    config: HostConfig,
    active: Option<ActiveMatch>,
    diagnostics: VecDeque<Diagnostic>,
    metrics: HostMetrics,
    last_report: Option<MatchReport>,
}

impl MatchHost {
    pub fn new(config: HostConfig) -> Result<Self, HostError> {
        if config.diagnostic_capacity == 0 || config.diagnostic_capacity > MAX_DIAGNOSTIC_CAPACITY {
            return Err(HostError::InvalidDiagnosticCapacity);
        }
        let _validated_fixture = build_ready_reference_runtime()?;
        Ok(Self {
            state: HostState::Ready,
            config,
            active: None,
            diagnostics: VecDeque::with_capacity(config.diagnostic_capacity),
            metrics: HostMetrics::default(),
            last_report: None,
        })
    }

    #[must_use]
    pub const fn state(&self) -> HostState {
        self.state
    }

    #[must_use]
    pub const fn is_ready(&self) -> bool {
        matches!(self.state, HostState::Ready)
    }

    #[must_use]
    pub const fn metrics(&self) -> HostMetrics {
        self.metrics
    }

    #[must_use]
    pub fn diagnostics(&self) -> &VecDeque<Diagnostic> {
        &self.diagnostics
    }

    #[must_use]
    pub const fn last_report(&self) -> Option<MatchReport> {
        self.last_report
    }

    /// Starts the fixed reference fixture and returns only after the tick-zero baseline has been produced.
    pub fn start_reference_match(&mut self) -> Result<(), HostError> {
        self.require_state(HostState::Ready)?;
        let runtime = build_ready_reference_runtime()?;
        self.active = Some(ActiveMatch { runtime });
        self.state = HostState::Running;
        self.metrics.matches_started = self.metrics.matches_started.saturating_add(1);
        self.push_diagnostic(DiagnosticCode::MatchStarted, 0);
        Ok(())
    }

    /// Advances one fixed tick without wall-clock sleeping. `Some` is returned exactly once at completion.
    pub fn step_reference_match(&mut self) -> Result<Option<MatchReport>, HostError> {
        self.require_state(HostState::Running)?;
        let intents = self.reference_bot_intents();
        self.metrics.internal_intents_issued = self
            .metrics
            .internal_intents_issued
            .saturating_add(u64::try_from(intents.len()).unwrap_or(u64::MAX));
        let active = self.active.as_mut().expect("running host owns a match");
        let frame = active.runtime.step_with_intents(&intents)?;
        active.runtime.check_invariants()?;
        self.metrics.internal_intents_rejected =
            self.metrics.internal_intents_rejected.saturating_add(
                u64::try_from(
                    frame
                        .events
                        .iter()
                        .filter(|event| matches!(event, MatchEvent::InternalIntentRejected { .. }))
                        .count(),
                )
                .unwrap_or(u64::MAX),
            );
        self.metrics.ticks_executed = self.metrics.ticks_executed.saturating_add(1);
        if !matches!(frame.phase, MatchPhase::Complete { .. }) {
            return Ok(None);
        }
        let report = report_from_terminal_frame(&frame, REFERENCE_SEED)?;
        self.active = None;
        self.state = HostState::Ready;
        self.last_report = Some(report);
        self.metrics.matches_completed = self.metrics.matches_completed.saturating_add(1);
        self.push_diagnostic(DiagnosticCode::MatchCompleted, report.final_tick);
        Ok(Some(report))
    }

    /// Runs the deterministic smoke fixture as fast as the host permits; this is not a real-time scheduler.
    pub fn run_reference_smoke(&mut self) -> Result<MatchReport, HostError> {
        self.start_reference_match()?;
        for _ in 0..REFERENCE_MAX_TICKS {
            if let Some(report) = self.step_reference_match()? {
                return Ok(report);
            }
        }
        Err(HostError::ReferenceMatchDidNotTerminate)
    }

    /// Enters a non-ready drain state. An active match is ended as a draw with host provenance; the host
    /// never fabricates a winning team during shutdown.
    pub fn request_drain(&mut self) -> Result<Option<MatchReport>, HostError> {
        if matches!(self.state, HostState::Stopped) {
            return Err(HostError::InvalidState {
                expected: HostState::Ready,
                actual: self.state,
            });
        }
        if matches!(self.state, HostState::Draining) {
            return Ok(None);
        }

        let at_tick = self
            .active
            .as_ref()
            .map_or(0, |active| active.runtime.tick());
        self.state = HostState::Draining;
        self.push_diagnostic(DiagnosticCode::DrainRequested, at_tick);

        let Some(mut active) = self.active.take() else {
            return Ok(None);
        };
        let terminal = active
            .runtime
            .finish(MatchOutcome::Draw, MatchEndReason::Host)?;
        active.runtime.check_invariants()?;
        let report = report_from_terminal_frame(&terminal, REFERENCE_SEED)?;
        self.last_report = Some(report);
        self.metrics.matches_completed = self.metrics.matches_completed.saturating_add(1);
        self.metrics.matches_cancelled_for_drain =
            self.metrics.matches_cancelled_for_drain.saturating_add(1);
        self.push_diagnostic(DiagnosticCode::MatchCancelledForDrain, report.final_tick);
        Ok(Some(report))
    }

    /// Completes a previously requested drain after the active match has reached a terminal state.
    pub fn complete_drain(&mut self) -> Result<(), HostError> {
        self.require_state(HostState::Draining)?;
        debug_assert!(self.active.is_none());
        let at_tick = self.last_report.map_or(0, |report| report.final_tick);
        self.state = HostState::Stopped;
        self.push_diagnostic(DiagnosticCode::HostStopped, at_tick);
        Ok(())
    }

    fn reference_bot_intents(&self) -> Vec<ActorIntent> {
        let active = self.active.as_ref().expect("running host owns a match");
        let next_tick = active.runtime.tick().saturating_add(1);
        let mut intents = Vec::with_capacity(2);
        for (source, target) in [(HERO_BLUE, CORE_RED), (HERO_RED, CORE_BLUE)] {
            let source_view = active
                .runtime
                .actor(source)
                .expect("reference hero remains addressable");
            let target_view = active
                .runtime
                .actor(target)
                .expect("reference objective remains addressable");
            let can_attack = source_view.alive
                && target_view.alive
                && source_view.basic_attack.is_none()
                && source_view
                    .basic_attack_ready_at
                    .is_some_and(|ready_at| ready_at <= next_tick);
            if !can_attack {
                continue;
            }
            intents.push(ActorIntent {
                actor: source,
                kind: CommandKind::BasicAttack { target },
            });
        }
        intents
    }

    fn require_state(&self, expected: HostState) -> Result<(), HostError> {
        if self.state == expected {
            Ok(())
        } else {
            Err(HostError::InvalidState {
                expected,
                actual: self.state,
            })
        }
    }

    fn push_diagnostic(&mut self, code: DiagnosticCode, at_tick: Tick) {
        if self.diagnostics.len() == self.config.diagnostic_capacity {
            self.diagnostics.pop_front();
            self.metrics.diagnostics_dropped = self.metrics.diagnostics_dropped.saturating_add(1);
        }
        self.diagnostics.push_back(Diagnostic { code, at_tick });
    }
}

fn build_ready_reference_runtime() -> Result<MatchRuntime, HostError> {
    let mut runtime = build_reference_runtime()?;
    let baseline = runtime.start()?;
    debug_assert_eq!(baseline.tick, 0);
    runtime.check_invariants()?;
    Ok(runtime)
}

fn build_reference_runtime() -> Result<MatchRuntime, HostError> {
    let config = MatchConfig {
        max_match_ticks: REFERENCE_MAX_TICKS,
        ..MatchConfig::competitive_mobile()
    };
    let mut runtime = MatchRuntime::new(config, REFERENCE_SEED)?;
    let lane = CompiledLane::compile(
        &LaneSpec {
            id: LaneId(0),
            centerline: vec![Vec2Mm::new(-1_000, 0), Vec2Mm::new(1_000, 0)],
            half_width_mm: 250,
        },
        config,
    )
    .map_err(HostError::ReferenceLaneInvalid)?;
    let lane_point = |progress_mm| {
        lane.point_at(LanePosition {
            lane: LaneId(0),
            progress_mm,
        })
        .map_err(HostError::ReferenceLaneInvalid)
    };

    for actor in [
        reference_hero(HERO_BLUE, TEAM_BLUE, lane_point(100)?),
        reference_core(CORE_BLUE, TEAM_BLUE, TEAM_RED, lane_point(0)?),
        reference_hero(HERO_RED, TEAM_RED, lane_point(1_900)?),
        reference_core(CORE_RED, TEAM_RED, TEAM_BLUE, lane_point(2_000)?),
    ] {
        runtime.spawn_actor(&actor)?;
    }
    Ok(runtime)
}

fn reference_hero(id: ActorId, team: TeamId, position: Vec2Mm) -> ActorSpawn {
    ActorSpawn {
        id,
        owner: None,
        team,
        kind: ActorKind::Hero,
        position,
        growth: StatGrowth::NONE,
        bounty: Bounty::NONE,
        stats: CombatStats {
            max_health: 500,
            max_resource: 0,
            move_speed_mm_per_tick: 100,
            physical_reduction_bps: 0,
            magic_reduction_bps: 0,
        },
        abilities: Vec::new(),
        basic_attack: Some(BasicAttackSpec {
            range_mm: 3_000,
            damage: 100,
            school: DamageSchool::Physical,
            windup_ticks: 0,
            cooldown_ticks: 1,
        }),
        death_rule: DeathRule::StayDead,
    }
}

fn reference_core(id: ActorId, team: TeamId, winner: TeamId, position: Vec2Mm) -> ActorSpawn {
    ActorSpawn {
        id,
        owner: None,
        team,
        kind: ActorKind::Structure,
        position,
        growth: StatGrowth::NONE,
        bounty: Bounty::NONE,
        stats: CombatStats {
            max_health: 300,
            max_resource: 0,
            move_speed_mm_per_tick: 0,
            physical_reduction_bps: 0,
            magic_reduction_bps: 0,
        },
        abilities: Vec::new(),
        basic_attack: None,
        death_rule: DeathRule::ObjectiveVictory { winner },
    }
}

fn report_from_terminal_frame(
    frame: &metrocalk_gameplay::ServerFrame,
    seed: u64,
) -> Result<MatchReport, HostError> {
    let MatchPhase::Complete { outcome, reason } = frame.phase else {
        return Err(HostError::TerminalFrameMissingResult);
    };
    debug_assert!(frame.events.iter().any(|event| matches!(
        event,
        MatchEvent::MatchFinished {
            outcome: event_outcome,
            reason: event_reason,
        } if *event_outcome == outcome && *event_reason == reason
    )));
    Ok(MatchReport {
        profile_id: REFERENCE_PROFILE_ID,
        seed,
        final_tick: frame.tick,
        outcome,
        reason,
        world_digest: frame.world_digest,
        terminal_frame_digest: frame.frame_digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> MatchHost {
        MatchHost::new(HostConfig::default()).unwrap()
    }

    #[test]
    fn repeated_max_speed_reference_matches_are_identical() {
        let mut reusable_host = host();
        let first = reusable_host.run_reference_smoke().unwrap();
        let second = reusable_host.run_reference_smoke().unwrap();
        let third = host().run_reference_smoke().unwrap();
        assert_eq!(first, second);
        assert_eq!(second, third);
        assert_eq!(first.outcome, MatchOutcome::Draw);
        assert_eq!(first.reason, MatchEndReason::ObjectiveDestroyed);
        assert!(first.final_tick < REFERENCE_MAX_TICKS);
        assert_eq!(reusable_host.state(), HostState::Ready);
        assert_eq!(reusable_host.metrics().matches_started, 2);
        assert_eq!(reusable_host.metrics().matches_completed, 2);
        assert_eq!(reusable_host.metrics().internal_intents_issued, 12);
        assert_eq!(reusable_host.metrics().internal_intents_rejected, 0);
    }

    #[test]
    fn active_host_drain_is_explicit_draw_and_never_invents_a_winner() {
        let mut host = host();
        assert!(host.is_ready());
        host.start_reference_match().unwrap();
        assert_eq!(host.state(), HostState::Running);

        let report = host.request_drain().unwrap().unwrap();
        assert_eq!(host.state(), HostState::Draining);
        assert!(!host.is_ready());
        assert_eq!(report.outcome, MatchOutcome::Draw);
        assert_eq!(report.reason, MatchEndReason::Host);

        host.complete_drain().unwrap();
        assert_eq!(host.state(), HostState::Stopped);
        assert!(!host.is_ready());
        assert_eq!(host.metrics().matches_cancelled_for_drain, 1);
    }

    #[test]
    fn diagnostics_are_strictly_bounded() {
        let mut host = MatchHost::new(HostConfig {
            diagnostic_capacity: 2,
        })
        .unwrap();
        host.start_reference_match().unwrap();
        host.request_drain().unwrap();
        host.complete_drain().unwrap();
        assert_eq!(host.diagnostics().len(), 2);
        assert_eq!(host.metrics().diagnostics_dropped, 2);
        assert_eq!(
            host.diagnostics().back().unwrap().code,
            DiagnosticCode::HostStopped
        );
    }

    #[test]
    fn version_metadata_declares_candidate_and_absent_transport() {
        assert!(VERSION_JSON.contains("\"status\":\"mob2-candidate-not-production\""));
        assert!(VERSION_JSON.contains("\"transport\":\"none\""));
        assert!(!VERSION_JSON.contains(['\n', '\r']));
    }
}
