//! src/interface/terminal/full_sync.rs is the menu's Sync everything: the same journaled
//! full sync as `pace sync` (activities, strength sets, health, Coach) for Today, Yesterday,
//! Last 7 days or a custom period, plus continuing the latest unfinished run. Each choice shows
//! exact inclusive dates before confirmation; Esc stops before the next window.

use super::{
    App, Choice, Selection, Style,
    setup_health::parse_range,
    sync_shortcuts::{PRESETS, period},
    tty::Key,
};
use crate::{
    config::Settings,
    health::Date,
    interface::full_sync::{self, Request},
    secrets::Secrets,
    sync::{Outcome, Period, Progress, Step},
};
use anyhow::{Context, Result};
use std::ops::ControlFlow;

const TITLE: &str = "Sync everything";

/// The last seven days ending today (UTC), as onboarding's first full download uses it.
pub(super) fn last_week() -> Result<Period> {
    let (start, end) = period(chrono::Utc::now().date_naive(), 2)?;
    Ok(Period::new(start.into(), end.into())?)
}

/// One line per step, redrawn after every finished window.
struct Board {
    period: Period,
    steps: Vec<(Step, String)>,
    current: String,
}

impl Board {
    fn new(period: Period) -> Self {
        Self {
            period,
            steps: Step::ORDER
                .iter()
                .map(|step| (*step, "waiting".to_owned()))
                .collect(),
            current: "Starting".into(),
        }
    }

    fn update(&mut self, progress: &Progress<'_>) {
        let (step, text) = match progress {
            Progress::Starting {
                step,
                window,
                index,
                total,
            } => {
                self.current = format!(
                    "Now: {} {}/{} ({} to {})",
                    step.label(),
                    index + 1,
                    total,
                    window.start,
                    window.end
                );
                return;
            }
            Progress::Finished {
                step,
                index,
                total,
                outcome,
                ..
            } => (
                *step,
                match outcome {
                    Outcome::Done { .. } => format!("{}/{total} windows done", index + 1),
                    Outcome::Failed { reason } => format!("failed: {reason}"),
                },
            ),
            Progress::Skipped { step, reason } => (*step, format!("skipped: {reason}")),
        };
        if let Some(row) = self.steps.iter_mut().find(|(s, _)| *s == step) {
            row.1 = text;
        }
    }

    fn lines(&self) -> Vec<(String, Style)> {
        let mut lines = vec![(format!("{TITLE}: {}", self.period), Style::Plain)];
        lines.extend(
            self.steps
                .iter()
                .map(|(step, text)| (format!("{:<14} {text}", step.label()), Style::Plain)),
        );
        lines.push((self.current.clone(), Style::Plain));
        lines
    }
}

impl App {
    /// Choose a period (or the unfinished run), confirm it and run the full sync with progress.
    pub(super) fn full_sync(&mut self) -> Result<()> {
        let settings = Settings::load(&self.config)?;
        let today = chrono::Utc::now().date_naive();
        let mut choices = PRESETS
            .iter()
            .enumerate()
            .map(|(index, name)| {
                let (start, end) = period(today, index)?;
                Ok(Choice::new(*name, format!("{start} to {end}, inclusive")))
            })
            .collect::<Result<Vec<_>>>()?;
        choices.push(Choice::new("Custom period", "Enter START or START END"));
        let unfinished = full_sync::unfinished(&settings)?;
        if let Some(run) = &unfinished {
            choices.push(Choice::new(
                "Continue unfinished sync",
                format!(
                    "{}, {} since {}; done windows are skipped",
                    run.period,
                    run.state.key(),
                    run.started_at
                ),
            ));
        }
        let Some(index) = self
            .view
            .select(TITLE, &choices, &mut Selection::default(), false)?
        else {
            return Ok(());
        };
        let (request, period) = match index {
            0..=2 => {
                let (start, end) = period(today, index)?;
                let period = Period::new(start.into(), end.into())?;
                (Request::Period(period), period)
            }
            3 => {
                let label = format!(
                    "Garmin dates: START, or START END (YYYY-MM-DD). END defaults to today, {today} (UTC)."
                );
                let Some(input) = self.view.prompt(TITLE, &label, false)? else {
                    return Ok(());
                };
                let (start, end) = parse_range::<Date>(&input, today.into())?;
                let period = Period::new(start, end)?;
                (Request::Period(period), period)
            }
            _ => {
                let run = unfinished.context("no unfinished sync")?;
                (Request::Resume, run.period)
            }
        };
        let details = format!(
            "{period}, inclusive. Today is based on UTC; Garmin filters calendar dates.\nSteps in order: activities, strength sets (Garmin login), all 8 health datasets per day, Coach schedule (Garmin login).\nA step without its key or login is skipped and named. Requests are paced; Esc stops before the next window and committed data is kept."
        );
        if !self.confirm(TITLE, "Import this period from Garmin?", &details)? {
            return Ok(());
        }
        self.run_full_sync(&settings, request, period)
    }

    /// Run one full sync with live progress; shared by the menu and onboarding.
    pub(super) fn run_full_sync(
        &mut self,
        settings: &Settings,
        request: Request,
        period: Period,
    ) -> Result<()> {
        let secrets = Secrets::load(self.env_file.as_deref())?;
        let _reporting = self.report_rate_waits()?;
        let mut board = Board::new(period);
        let mut failure = None;
        let result = full_sync::execute(settings, &secrets, request, &mut |progress| {
            board.update(&progress);
            let step = self
                .view
                .frame(
                    TITLE,
                    &board.lines(),
                    "Esc stops before the next window; continue later",
                )
                .and_then(|()| Ok(self.view.terminal.pending_key()? == Key::Escape));
            // Drawing errors and Ctrl-C stop the run and are reported after it.
            match step {
                Ok(false) => ControlFlow::Continue(()),
                Ok(true) => ControlFlow::Break(()),
                Err(error) => {
                    failure = Some(error);
                    ControlFlow::Break(())
                }
            }
        });
        // Discard typing during the run so it cannot confirm a later action.
        rustix::termios::tcflush(std::io::stdin(), rustix::termios::QueueSelector::IFlush)?;
        if let Some(error) = failure {
            return Err(error);
        }
        self.view.result(TITLE, &full_sync::human(&result?))?;
        Ok(())
    }
}
