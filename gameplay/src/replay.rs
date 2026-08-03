use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{self, Display, Formatter};

use crate::model::{
    AbilitySpec, ActorSpawn, MatchConfig, MatchEndReason, MatchOutcome, RuntimeError, Tick,
};
use crate::protocol::{CommandReceipt, CommandRejection, PlayerCommand, ServerFrame, WorldDigest};
use crate::runtime::MatchRuntime;

/// One command submission at the exact authoritative tick and arrival ordinal observed by the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScenarioSubmission {
    pub submitted_at: Tick,
    pub arrival_order: u32,
    pub command: PlayerCommand,
}

/// Optional terminal lifecycle action ordered among submissions at the same authoritative tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScenarioFinish {
    pub at_tick: Tick,
    pub arrival_order: u32,
    pub outcome: MatchOutcome,
    pub reason: MatchEndReason,
}

/// Exact receipt or rejection produced for a scheduled submission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubmissionOutcome {
    pub submitted_at: Tick,
    pub arrival_order: u32,
    pub outcome: Result<CommandReceipt, CommandRejection>,
}

/// Lossless in-memory MOB-1 scenario.
///
/// Submission time/order, every outcome, the tick-zero baseline, authoritative frames, and optional match
/// completion are reproduced. A versioned binary artifact and checkpoint ring remain MOB-2 work; JSON is
/// deliberately not introduced as a deterministic reproduction format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scenario {
    pub config: MatchConfig,
    pub seed: u64,
    pub abilities: Vec<AbilitySpec>,
    pub actors: Vec<ActorSpawn>,
    pub submissions: Vec<ScenarioSubmission>,
    pub final_tick: Tick,
    pub finish: Option<ScenarioFinish>,
}

impl Scenario {
    pub fn run(&self) -> Result<ReplayOutcome, ReplayError> {
        self.validate()?;

        let mut runtime = MatchRuntime::new(self.config, self.seed)?;
        for ability in &self.abilities {
            runtime.register_ability(*ability)?;
        }
        for actor in &self.actors {
            runtime.spawn_actor(actor)?;
        }
        let baseline = runtime.start()?;

        let mut submissions = self.submissions.clone();
        submissions
            .sort_unstable_by_key(|submission| (submission.submitted_at, submission.arrival_order));
        let mut cursor = 0;
        let frame_capacity = usize::try_from(self.final_tick.min(100_000))
            .unwrap_or(100_000)
            .saturating_add(2);
        let mut frames = Vec::with_capacity(frame_capacity);
        let mut outcomes = Vec::with_capacity(submissions.len());
        frames.push(baseline);
        let mut terminal_emitted = false;

        loop {
            process_tick_actions(
                &mut runtime,
                &submissions,
                &mut cursor,
                self.finish,
                &mut terminal_emitted,
                &mut outcomes,
                &mut frames,
            )?;

            if terminal_emitted || runtime.tick() >= self.final_tick {
                break;
            }
            frames.push(runtime.step()?);
        }

        Ok(ReplayOutcome {
            final_digest: runtime.world_digest(),
            submissions: outcomes,
            frames,
        })
    }

    fn validate(&self) -> Result<(), ReplayError> {
        if self.final_tick > self.config.max_match_ticks {
            return Err(ReplayError::FinalTickExceedsMatchLimit);
        }
        if self
            .submissions
            .iter()
            .any(|submission| submission.submitted_at > self.final_tick)
        {
            return Err(ReplayError::SubmissionAfterEnd);
        }
        if self
            .finish
            .is_some_and(|finish| finish.at_tick > self.final_tick)
        {
            return Err(ReplayError::FinishAfterEnd);
        }
        if let Some(finish) = self.finish {
            if self
                .submissions
                .iter()
                .any(|submission| submission.submitted_at > finish.at_tick)
            {
                return Err(ReplayError::SubmissionAfterFinish);
            }
        }

        let mut seen_arrivals = BTreeSet::new();
        for submission in &self.submissions {
            if !seen_arrivals.insert((submission.submitted_at, submission.arrival_order)) {
                return Err(ReplayError::DuplicateArrivalOrder {
                    submitted_at: submission.submitted_at,
                    arrival_order: submission.arrival_order,
                });
            }
        }
        if let Some(finish) = self.finish {
            if !seen_arrivals.insert((finish.at_tick, finish.arrival_order)) {
                return Err(ReplayError::DuplicateArrivalOrder {
                    submitted_at: finish.at_tick,
                    arrival_order: finish.arrival_order,
                });
            }
        }
        Ok(())
    }

    /// Re-run the scenario and compare submissions plus complete authoritative frames, not only final state.
    pub fn reproduces(&self, runs: usize) -> Result<bool, ReplayError> {
        if runs < 2 {
            return Ok(false);
        }
        let expected = self.run()?;
        for _ in 1..runs {
            if self.run()? != expected {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn process_tick_actions(
    runtime: &mut MatchRuntime,
    submissions: &[ScenarioSubmission],
    cursor: &mut usize,
    finish: Option<ScenarioFinish>,
    terminal_emitted: &mut bool,
    outcomes: &mut Vec<SubmissionOutcome>,
    frames: &mut Vec<ServerFrame>,
) -> Result<(), RuntimeError> {
    loop {
        let next_submission = submissions
            .get(*cursor)
            .copied()
            .filter(|submission| submission.submitted_at == runtime.tick());
        let pending_finish =
            finish.filter(|finish| !*terminal_emitted && finish.at_tick == runtime.tick());

        match (next_submission, pending_finish) {
            (Some(submission), Some(finish)) if submission.arrival_order < finish.arrival_order => {
                outcomes.push(SubmissionOutcome {
                    submitted_at: submission.submitted_at,
                    arrival_order: submission.arrival_order,
                    outcome: runtime.submit(submission.command),
                });
                *cursor += 1;
            }
            (_, Some(finish)) => {
                frames.push(runtime.finish(finish.outcome, finish.reason)?);
                *terminal_emitted = true;
            }
            (Some(submission), None) => {
                outcomes.push(SubmissionOutcome {
                    submitted_at: submission.submitted_at,
                    arrival_order: submission.arrival_order,
                    outcome: runtime.submit(submission.command),
                });
                *cursor += 1;
            }
            (None, None) => return Ok(()),
        }
    }
}

/// Authoritative replay result including submission outcomes, every frame, and final simulation digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayOutcome {
    pub final_digest: WorldDigest,
    pub submissions: Vec<SubmissionOutcome>,
    pub frames: Vec<ServerFrame>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayError {
    Runtime(RuntimeError),
    SubmissionAfterEnd,
    SubmissionAfterFinish,
    FinishAfterEnd,
    FinalTickExceedsMatchLimit,
    DuplicateArrivalOrder {
        submitted_at: Tick,
        arrival_order: u32,
    },
}

impl Display for ReplayError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime(error) => Display::fmt(error, formatter),
            Self::SubmissionAfterEnd => {
                formatter.write_str("scenario submission occurs after the final tick")
            }
            Self::SubmissionAfterFinish => {
                formatter.write_str("scenario submission occurs after match completion")
            }
            Self::FinishAfterEnd => {
                formatter.write_str("scenario completion occurs after the final tick")
            }
            Self::FinalTickExceedsMatchLimit => {
                formatter.write_str("scenario final tick exceeds the match-duration limit")
            }
            Self::DuplicateArrivalOrder {
                submitted_at,
                arrival_order,
            } => write!(
                formatter,
                "duplicate scenario arrival order {arrival_order} at tick {submitted_at}"
            ),
        }
    }
}

impl Error for ReplayError {}

impl From<RuntimeError> for ReplayError {
    fn from(value: RuntimeError) -> Self {
        Self::Runtime(value)
    }
}
