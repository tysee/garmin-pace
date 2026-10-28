//! src/sync/service.rs runs one full sync: every step in order over aligned windows, journaling
//! each window before moving on. A failed window ends only its own step; later steps still run,
//! so a partial result is reported per step instead of hiding behind one mark. Resume continues
//! the latest unfinished run and skips its done windows. It exposes the ports and `SyncService`.

use super::domain::{
    Outcome, Period, RunOverview, RunRecord, RunState, Step, StepReport, StepState, SyncError,
    SyncReport, Window, WindowRecord,
};
use std::ops::ControlFlow;

/// One importable step. Implementations wrap an existing feature use case and its adapter.
pub trait StepSource {
    fn step(&self) -> Step;
    /// The step's own per-request bound, e.g. seven days for Coach.
    fn window_days(&self) -> usize;
    /// Why the step cannot run now (no key, no Garmin login), decided from local state only.
    fn unavailable(&self) -> Option<String>;
    /// Import one window, committing it as the owning feature does; returns saved records.
    fn import(&self, window: Window) -> anyhow::Result<usize>;
}

/// Durable run journal in the profile database.
pub trait SyncJournal {
    fn begin(&self, period: Period, at: &str) -> anyhow::Result<i64>;
    fn latest(&self) -> anyhow::Result<Option<RunRecord>>;
    fn windows(&self, run: i64) -> anyhow::Result<Vec<WindowRecord>>;
    fn steps(&self, run: i64) -> anyhow::Result<Vec<StepReport>>;
    fn reopen(&self, run: i64) -> anyhow::Result<()>;
    fn record_window(&self, run: i64, record: &WindowRecord, at: &str) -> anyhow::Result<()>;
    fn record_step(&self, run: i64, report: &StepReport, at: &str) -> anyhow::Result<()>;
    fn finish(&self, run: i64, state: RunState, at: &str) -> anyhow::Result<()>;
}

/// What the caller sees while a run advances; Break on Starting stops before that window.
pub enum Progress<'a> {
    Starting {
        step: Step,
        window: Window,
        index: usize,
        total: usize,
    },
    Finished {
        step: Step,
        window: Window,
        index: usize,
        total: usize,
        outcome: &'a Outcome,
    },
    Skipped {
        step: Step,
        reason: &'a str,
    },
}

pub struct SyncService<'a> {
    journal: &'a dyn SyncJournal,
}

impl<'a> SyncService<'a> {
    pub fn new(journal: &'a dyn SyncJournal) -> Self {
        Self { journal }
    }

    /// Start a new run over `period` with the sources in their fixed step order.
    pub fn start(
        &self,
        period: Period,
        sources: &[&dyn StepSource],
        now: &dyn Fn() -> String,
        on_progress: &mut dyn FnMut(Progress<'_>) -> ControlFlow<()>,
    ) -> Result<SyncReport, SyncError> {
        let run = self
            .journal
            .begin(period, &now())
            .map_err(SyncError::Storage)?;
        self.execute(run, period, false, &[], sources, now, on_progress)
    }

    /// Continue the latest run that did not complete, skipping its done windows.
    pub fn resume(
        &self,
        sources: &[&dyn StepSource],
        now: &dyn Fn() -> String,
        on_progress: &mut dyn FnMut(Progress<'_>) -> ControlFlow<()>,
    ) -> Result<SyncReport, SyncError> {
        let run = self.unfinished()?.ok_or(SyncError::NothingToResume)?;
        let done = self.journal.windows(run.id).map_err(SyncError::Storage)?;
        self.journal.reopen(run.id).map_err(SyncError::Storage)?;
        self.execute(run.id, run.period, true, &done, sources, now, on_progress)
    }

    /// The latest run when it did not complete, so it can be continued.
    pub fn unfinished(&self) -> Result<Option<RunRecord>, SyncError> {
        Ok(self
            .journal
            .latest()
            .map_err(SyncError::Storage)?
            .filter(|run| run.state != RunState::Complete))
    }

    /// The latest run and its per-step results, for local status screens.
    pub fn overview(&self) -> Result<Option<RunOverview>, SyncError> {
        let Some(run) = self.journal.latest().map_err(SyncError::Storage)? else {
            return Ok(None);
        };
        let steps = self.journal.steps(run.id).map_err(SyncError::Storage)?;
        Ok(Some(RunOverview { run, steps }))
    }

    #[allow(clippy::too_many_arguments)]
    fn execute(
        &self,
        run: i64,
        period: Period,
        resumed: bool,
        previous: &[WindowRecord],
        sources: &[&dyn StepSource],
        now: &dyn Fn() -> String,
        on_progress: &mut dyn FnMut(Progress<'_>) -> ControlFlow<()>,
    ) -> Result<SyncReport, SyncError> {
        if sources.iter().map(|source| source.step()).ne(Step::ORDER) {
            return Err(SyncError::Invalid(
                "full sync needs every step in its fixed order",
            ));
        }
        let mut steps = Vec::new();
        let mut stopped = false;
        for source in sources {
            let report = if stopped {
                let windows = period.windows(source.window_days())?.len();
                StepReport {
                    step: source.step(),
                    windows,
                    done: 0,
                    reused: 0,
                    saved: 0,
                    state: StepState::Skipped {
                        reason: "stopped before this step".into(),
                    },
                }
            } else {
                let (report, stop) =
                    self.run_step(run, period, previous, *source, now, on_progress)?;
                stopped = stop;
                report
            };
            self.journal
                .record_step(run, &report, &now())
                .map_err(SyncError::Storage)?;
            steps.push(report);
        }
        let state = if stopped {
            RunState::Stopped
        } else if steps.iter().all(|s| s.state == StepState::Complete) {
            RunState::Complete
        } else {
            RunState::Partial
        };
        self.journal
            .finish(run, state, &now())
            .map_err(SyncError::Storage)?;
        Ok(SyncReport {
            run_id: run,
            period,
            resumed,
            state,
            steps,
        })
    }

    /// Run one step; the flag reports whether the caller asked to stop the whole run.
    fn run_step(
        &self,
        run: i64,
        period: Period,
        previous: &[WindowRecord],
        source: &dyn StepSource,
        now: &dyn Fn() -> String,
        on_progress: &mut dyn FnMut(Progress<'_>) -> ControlFlow<()>,
    ) -> Result<(StepReport, bool), SyncError> {
        let step = source.step();
        let windows = period.windows(source.window_days())?;
        let earlier = |window: &Window| {
            previous.iter().find_map(|record| match record.outcome {
                Outcome::Done { saved } if record.step == step && record.window == *window => {
                    Some(saved)
                }
                _ => None,
            })
        };
        let mut report = StepReport {
            step,
            windows: windows.len(),
            done: 0,
            reused: 0,
            saved: 0,
            state: StepState::Complete,
        };
        let pending: Vec<_> = windows
            .iter()
            .enumerate()
            .filter(|(_, window)| match earlier(window) {
                Some(saved) => {
                    report.done += 1;
                    report.reused += 1;
                    report.saved += saved;
                    false
                }
                None => true,
            })
            .collect();
        if pending.is_empty() {
            return Ok((report, false));
        }
        if let Some(reason) = source.unavailable() {
            let _ = on_progress(Progress::Skipped {
                step,
                reason: &reason,
            });
            report.state = StepState::Skipped { reason };
            return Ok((report, false));
        }
        let total = windows.len();
        for (index, window) in pending {
            let window = *window;
            if on_progress(Progress::Starting {
                step,
                window,
                index,
                total,
            })
            .is_break()
            {
                report.state = interrupted(&report, "stopped");
                return Ok((report, true));
            }
            let outcome = match source.import(window) {
                Ok(saved) => Outcome::Done { saved },
                Err(error) => Outcome::Failed {
                    reason: format!("{error:#}"),
                },
            };
            self.journal
                .record_window(
                    run,
                    &WindowRecord {
                        step,
                        window,
                        outcome: outcome.clone(),
                    },
                    &now(),
                )
                .map_err(SyncError::Storage)?;
            let flow = on_progress(Progress::Finished {
                step,
                window,
                index,
                total,
                outcome: &outcome,
            });
            match outcome {
                Outcome::Done { saved } => {
                    report.done += 1;
                    report.saved += saved;
                }
                // The first failure ends this step: its cause usually repeats in later windows.
                Outcome::Failed { reason } => {
                    let reason = format!("{} to {}: {reason}", window.start, window.end);
                    report.state = interrupted(&report, &reason);
                    return Ok((report, flow.is_break()));
                }
            }
            if flow.is_break() {
                report.state = if report.done == total {
                    StepState::Complete
                } else {
                    interrupted(&report, "stopped")
                };
                return Ok((report, true));
            }
        }
        Ok((report, false))
    }
}

/// Partial when some windows are done, otherwise failed (or skipped for a stop).
fn interrupted(report: &StepReport, reason: &str) -> StepState {
    let reason = reason.to_owned();
    match (report.done, reason.as_str()) {
        (0, "stopped") => StepState::Skipped {
            reason: "stopped before this step".into(),
        },
        (0, _) => StepState::Failed { reason },
        _ => StepState::Partial { reason },
    }
}
