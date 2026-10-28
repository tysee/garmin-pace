//! src/interface/full_sync.rs composes the full-sync feature with the existing import use cases:
//! activities, strength sets, health and Coach, each wrapped as a step adapter. It exposes the
//! `pace sync` command, `execute` shared with the terminal menu, and the report renderers.

use super::{benchmarks, garmin, health, output::Output, workouts};
use crate::{
    config::Settings,
    garmin::{ConnectApi, GarminBenchmarks},
    health::Date,
    history::ActivityPeriod,
    secrets::Secrets,
    sync::{
        Period, Progress, SqliteSyncJournal, StepReport, StepSource, StepState, SyncReport,
        SyncService, Window,
    },
    workouts::{benchmarks::Window as StrengthWindow, coach::Window as CoachWindow},
};
use anyhow::{Context, Result, ensure};
use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use std::{fmt::Write as _, ops::ControlFlow};

const KEY: &str = "GARMIN_PERSONAL_API_KEY";

/// What one full sync covers: a new period, or the latest unfinished run.
#[derive(Clone, Copy)]
pub(super) enum Request {
    Period(Period),
    Resume,
}

fn missing_key(secrets: &Secrets) -> Option<String> {
    secrets.require(KEY).err().map(|_| {
        "no Garmin Personal API key; save one in Onboarding or garmin.env beside the database"
            .into()
    })
}

/// The native session is checked by file presence only; Garmin decides if it still works.
fn missing_login(settings: &Settings) -> Option<String> {
    match settings.garmin_auth.session_path.try_exists() {
        Ok(true) => None,
        Ok(false) => Some("not signed in to Garmin Connect; run pace garmin login".into()),
        Err(error) => Some(format!("cannot read the Garmin session path: {error}")),
    }
}

struct Activities<'a> {
    settings: &'a Settings,
    secrets: &'a Secrets,
}
impl StepSource for Activities<'_> {
    fn step(&self) -> crate::sync::Step {
        crate::sync::Step::Activities
    }
    fn window_days(&self) -> usize {
        31
    }
    fn unavailable(&self) -> Option<String> {
        missing_key(self.secrets)
    }
    fn import(&self, window: Window) -> Result<usize> {
        let period = ActivityPeriod::new(window.start.into(), window.end.into())?;
        let result = garmin::sync_period(self.settings, self.secrets, period, |_| {
            ControlFlow::Continue(())
        })?;
        result
            .completed
            .context("activity import stopped before its last page")?;
        Ok(usize::try_from(result.progress.saved)?)
    }
}

struct Strength<'a> {
    settings: &'a Settings,
    secrets: &'a Secrets,
}
impl StepSource for Strength<'_> {
    fn step(&self) -> crate::sync::Step {
        crate::sync::Step::Strength
    }
    fn window_days(&self) -> usize {
        31
    }
    fn unavailable(&self) -> Option<String> {
        missing_key(self.secrets).or_else(|| missing_login(self.settings))
    }
    fn import(&self, window: Window) -> Result<usize> {
        let gateway = GarminBenchmarks::connect(
            &self.settings.garmin,
            &self.settings.garmin_auth,
            Some(self.secrets.require(KEY)?),
        )?;
        Ok(benchmarks::service(self.settings)?.sync(
            &gateway,
            StrengthWindow {
                start: window.start,
                end: window.end,
            },
        )?)
    }
}

/// One-day windows: each day already commits atomically, so the journal can resume per day.
/// Readiness is read through Connect, so health needs the key and the Garmin login.
struct Health<'a> {
    settings: &'a Settings,
    secrets: &'a Secrets,
}
impl StepSource for Health<'_> {
    fn step(&self) -> crate::sync::Step {
        crate::sync::Step::Health
    }
    fn window_days(&self) -> usize {
        1
    }
    fn unavailable(&self) -> Option<String> {
        missing_key(self.secrets).or_else(|| missing_login(self.settings))
    }
    fn import(&self, window: Window) -> Result<usize> {
        let service = health::service(self.settings)?;
        let mut saved = 0;
        let _: ControlFlow<()> = health::sync_window(
            &service,
            self.settings,
            self.secrets,
            window.start,
            window.end,
            |_| {
                saved += 1;
                Ok(ControlFlow::Continue(()))
            },
        )?;
        Ok(saved)
    }
}

struct Coach<'a> {
    settings: &'a Settings,
}
impl StepSource for Coach<'_> {
    fn step(&self) -> crate::sync::Step {
        crate::sync::Step::Coach
    }
    fn window_days(&self) -> usize {
        7
    }
    fn unavailable(&self) -> Option<String> {
        missing_login(self.settings)
    }
    fn import(&self, window: Window) -> Result<usize> {
        let api = ConnectApi::connect(&self.settings.garmin, &self.settings.garmin_auth)?;
        let snapshot = workouts::service(self.settings)?
            .sync_coach(&api, CoachWindow::new(window.start, window.end)?)?;
        Ok(snapshot.sessions.len())
    }
}

/// Run one full sync with the profile's credentials, reporting every window to `on_progress`.
pub(super) fn execute(
    settings: &Settings,
    secrets: &Secrets,
    request: Request,
    on_progress: &mut dyn FnMut(Progress<'_>) -> ControlFlow<()>,
) -> Result<SyncReport> {
    let journal = SqliteSyncJournal::open(settings.database.clone())?;
    let service = SyncService::new(&journal);
    let activities = Activities { settings, secrets };
    let strength = Strength { settings, secrets };
    let health = Health { settings, secrets };
    let coach = Coach { settings };
    let sources: [&dyn StepSource; 4] = [&activities, &strength, &health, &coach];
    let now = || Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    Ok(match request {
        Request::Period(period) => service.start(period, &sources, &now, on_progress)?,
        Request::Resume => service.resume(&sources, &now, on_progress)?,
    })
}

/// The latest run when it did not complete, for menus that offer to continue it.
pub(super) fn unfinished(settings: &Settings) -> Result<Option<crate::sync::RunRecord>> {
    let journal = SqliteSyncJournal::open(settings.database.clone())?;
    Ok(SyncService::new(&journal).unfinished()?)
}

/// `pace sync`: one journaled run; progress goes to stderr, the report to stdout.
pub(super) fn run(
    start: Option<Date>,
    end: Option<Date>,
    resume: bool,
    settings: &Settings,
    secrets: &Secrets,
    output: Output<'_>,
) -> Result<()> {
    let request = match (start, resume) {
        (_, true) => Request::Resume,
        (Some(start), false) => Request::Period(Period::new(
            start,
            end.unwrap_or_else(|| Utc::now().date_naive().into()),
        )?),
        (None, false) => anyhow::bail!("--start or --resume is required"),
    };
    let report = execute(settings, secrets, request, &mut |progress| {
        if let Some(line) = progress_line(&progress) {
            eprintln!("{line}");
        }
        ControlFlow::Continue(())
    })?;
    output.message(&to_json(&report), &human(&report))?;
    ensure!(
        report.state == crate::sync::RunState::Complete,
        "full sync is {}; committed data is kept. Continue with: pace sync --resume",
        report.state.key()
    );
    Ok(())
}

/// One readable line per finished window or skipped step; None for window starts.
pub(super) fn progress_line(progress: &Progress<'_>) -> Option<String> {
    match progress {
        Progress::Starting { .. } => None,
        Progress::Finished {
            step,
            window,
            index,
            total,
            outcome,
        } => Some(format!(
            "{} {}/{} {}: {}",
            step.label(),
            index + 1,
            total,
            span(window),
            match outcome {
                crate::sync::Outcome::Done { saved } => format!("saved {saved}"),
                crate::sync::Outcome::Failed { reason } => format!("failed: {reason}"),
            }
        )),
        Progress::Skipped { step, reason } => Some(format!("{} skipped: {reason}", step.label())),
    }
}

fn span(window: &Window) -> String {
    if window.start == window.end {
        window.start.to_string()
    } else {
        format!("{} to {}", window.start, window.end)
    }
}

fn unit(report: &StepReport) -> &'static str {
    match report.step {
        crate::sync::Step::Activities => "activities",
        crate::sync::Step::Strength => "strength sessions",
        crate::sync::Step::Health => "days",
        crate::sync::Step::Coach => "scheduled workouts",
    }
}

fn state(report: &StepReport) -> (&'static str, Option<&str>) {
    match &report.state {
        StepState::Complete => ("complete", None),
        StepState::Partial { reason } => ("partial", Some(reason)),
        StepState::Failed { reason } => ("failed", Some(reason)),
        StepState::Skipped { reason } => ("skipped", Some(reason)),
    }
}

pub(super) fn to_json(report: &SyncReport) -> Value {
    json!({
        "run_id": report.run_id,
        "start": report.period.start.to_string(),
        "end": report.period.end.to_string(),
        "resumed": report.resumed,
        "state": report.state.key(),
        "steps": report.steps.iter().map(|step| {
            let (name, reason) = state(step);
            json!({
                "step": step.step.key(),
                "state": name,
                "windows": step.windows,
                "done": step.done,
                "reused": step.reused,
                "saved": step.saved,
                "reason": reason,
            })
        }).collect::<Vec<_>>(),
    })
}

/// One line per step, then what to do next when the run did not complete.
pub(super) fn human(report: &SyncReport) -> String {
    let mut text = format!(
        "Full sync {}: {}{}",
        report.period,
        report.state.key(),
        if report.resumed { " (continued)" } else { "" }
    );
    for step in &report.steps {
        let (name, reason) = state(step);
        let _ = write!(
            text,
            "\n{:<14} {:<9} {}/{} windows, {} {}",
            step.step.label(),
            name,
            step.done,
            step.windows,
            step.saved,
            unit(step)
        );
        if step.reused > 0 {
            let _ = write!(text, " ({} windows from the earlier attempt)", step.reused);
        }
        if let Some(reason) = reason {
            let _ = write!(text, "\n               {reason}");
        }
    }
    if report.state != crate::sync::RunState::Complete {
        text.push_str("\nCommitted data is kept. Continue with: pace sync --resume");
    }
    text
}
