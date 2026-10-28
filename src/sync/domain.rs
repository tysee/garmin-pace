//! src/sync/domain.rs defines the full-sync vocabulary: the four ordered import steps, the
//! bounded period they cover, per-window outcomes and the honest per-step report. It exposes
//! pure types only; windows are aligned to the period start so a repeated run hits the same keys.

use crate::health::Date;
use std::fmt;

/// Longest period one full sync covers; a year of health days is about 50 minutes of paced requests.
pub const MAX_PERIOD_DAYS: usize = 366;

/// One import step, run in this fixed order: activities, strength sets, health, Coach.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Step {
    Activities,
    Strength,
    Health,
    Coach,
}

impl Step {
    pub const ORDER: [Step; 4] = [Step::Activities, Step::Strength, Step::Health, Step::Coach];

    /// Stable storage and JSON identifier.
    pub fn key(self) -> &'static str {
        match self {
            Step::Activities => "activities",
            Step::Strength => "strength",
            Step::Health => "health",
            Step::Coach => "coach",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        Self::ORDER.into_iter().find(|step| step.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Step::Activities => "Activities",
            Step::Strength => "Strength sets",
            Step::Health => "Health",
            Step::Coach => "Coach",
        }
    }
}

/// An inclusive period of Garmin calendar dates, validated before any I/O.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Period {
    pub start: Date,
    pub end: Date,
}

impl Period {
    pub fn new(start: Date, end: Date) -> Result<Self, SyncError> {
        start
            .range(end, MAX_PERIOD_DAYS)
            .map_err(|_| SyncError::Invalid("period must contain 1..=366 days"))?;
        Ok(Self { start, end })
    }

    /// Consecutive windows of at most `days`, the first starting at the period start.
    pub fn windows(self, days: usize) -> Result<Vec<Window>, SyncError> {
        Ok(self
            .start
            .windows(self.end, days)
            .map_err(|_| SyncError::Invalid("window must contain at least one day"))?
            .into_iter()
            .map(|(start, end)| Window { start, end })
            .collect())
    }
}

impl fmt::Display for Period {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} to {}", self.start, self.end)
    }
}

/// One bounded slice of a step's period.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Window {
    pub start: Date,
    pub end: Date,
}

/// The recorded result of one imported window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    Done { saved: usize },
    Failed { reason: String },
}

/// How completely one step covered the period.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StepState {
    Complete,
    /// Some windows are done; the reason names the window that failed or the stop.
    Partial {
        reason: String,
    },
    /// No window completed.
    Failed {
        reason: String,
    },
    /// The step could not run at all, e.g. no key or no Garmin login.
    Skipped {
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StepReport {
    pub step: Step,
    pub windows: usize,
    pub done: usize,
    /// Windows done by an earlier attempt of a resumed run.
    pub reused: usize,
    pub saved: usize,
    pub state: StepState,
}

/// Overall result of a run; `Complete` only when every step covered every window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunState {
    Running,
    Complete,
    Partial,
    Stopped,
}

impl RunState {
    pub fn key(self) -> &'static str {
        match self {
            RunState::Running => "running",
            RunState::Complete => "complete",
            RunState::Partial => "partial",
            RunState::Stopped => "stopped",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        [
            RunState::Running,
            RunState::Complete,
            RunState::Partial,
            RunState::Stopped,
        ]
        .into_iter()
        .find(|state| state.key() == key)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncReport {
    pub run_id: i64,
    pub period: Period,
    pub resumed: bool,
    pub state: RunState,
    pub steps: Vec<StepReport>,
}

/// A journaled run as stored, for resume and the Status screen.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunRecord {
    pub id: i64,
    pub period: Period,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub state: RunState,
}

/// One journaled window outcome of a run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowRecord {
    pub step: Step,
    pub window: Window,
    pub outcome: Outcome,
}

/// A journaled run with its per-step reports, as the Status screen reads it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunOverview {
    pub run: RunRecord,
    pub steps: Vec<StepReport>,
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("no unfinished sync to continue; start one with a period")]
    NothingToResume,
    #[error("sync journal storage failed: {0:#}")]
    Storage(anyhow::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(value: &str) -> Result<Date, SyncError> {
        value
            .parse()
            .map_err(|_| SyncError::Invalid("invalid test date"))
    }

    #[test]
    fn windows_align_to_the_period_start_and_cover_it_exactly() -> Result<(), SyncError> {
        let period = Period::new(date("2026-01-30")?, date("2026-02-14")?)?;
        let windows = period.windows(7)?;
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0].start, date("2026-01-30")?);
        assert_eq!(windows[0].end, date("2026-02-05")?);
        assert_eq!(windows[2].start, date("2026-02-13")?);
        assert_eq!(windows[2].end, date("2026-02-14")?);
        Ok(())
    }

    #[test]
    fn periods_are_bounded_and_ordered() -> Result<(), SyncError> {
        assert!(Period::new(date("2026-02-02")?, date("2026-02-01")?).is_err());
        assert!(Period::new(date("2025-01-01")?, date("2026-01-01")?).is_ok());
        assert!(Period::new(date("2025-01-01")?, date("2026-01-02")?).is_err());
        Ok(())
    }

    #[test]
    fn identifiers_round_trip() {
        for step in Step::ORDER {
            assert_eq!(Step::parse(step.key()), Some(step));
        }
        assert_eq!(RunState::parse("partial"), Some(RunState::Partial));
        assert_eq!(Step::parse("sleep"), None);
    }
}
