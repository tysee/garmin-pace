//! tests/cases/full_sync.rs verifies the journaled full sync: honest per-step results, resume
//! that skips done windows, and stopping, against a real SQLite journal; then `pace sync` as a
//! process against the local Garmin stand-in, without a Connect login.

use crate::{
    common::Fixture,
    garmin::{KEY, Server, TOKEN, configure},
};
use anyhow::{Context, Result};
use garmin_pace::sync::{
    Period, Progress, RunState, SqliteSyncJournal, Step, StepSource, StepState, SyncService, Window,
};
use serde_json::{Value, json};
use std::{cell::Cell, ops::ControlFlow};

struct Fake {
    step: Step,
    days: usize,
    unavailable: Option<&'static str>,
    /// Fail the window starting on this date while it is set.
    fail_on: Cell<Option<&'static str>>,
    calls: Cell<usize>,
}

impl Fake {
    fn new(step: Step, days: usize) -> Self {
        Self {
            step,
            days,
            unavailable: None,
            fail_on: Cell::new(None),
            calls: Cell::new(0),
        }
    }
}

impl StepSource for Fake {
    fn step(&self) -> Step {
        self.step
    }
    fn window_days(&self) -> usize {
        self.days
    }
    fn unavailable(&self) -> Option<String> {
        self.unavailable.map(str::to_owned)
    }
    fn import(&self, window: Window) -> Result<usize> {
        self.calls.set(self.calls.get() + 1);
        if self.fail_on.get() == Some(&*window.start.to_string()) {
            anyhow::bail!("fixture failure");
        }
        Ok(2)
    }
}

fn period() -> Result<Period> {
    Ok(Period::new("2026-10-01".parse()?, "2026-10-03".parse()?)?)
}

fn now() -> String {
    "2026-10-04T00:00:00.000Z".into()
}

fn state(report: &garmin_pace::sync::SyncReport, step: Step) -> Result<StepState> {
    report
        .steps
        .iter()
        .find(|s| s.step == step)
        .map(|s| s.state.clone())
        .context("every step is reported")
}

#[test]
fn failures_and_skips_are_reported_per_step_and_resume_skips_done_windows() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let journal = SqliteSyncJournal::open(fixture.settings.database.clone())?;
    let service = SyncService::new(&journal);
    let activities = Fake::new(Step::Activities, 31);
    let strength = Fake {
        unavailable: Some("not signed in"),
        ..Fake::new(Step::Strength, 31)
    };
    let health = Fake::new(Step::Health, 1);
    health.fail_on.set(Some("2026-10-02"));
    let coach = Fake::new(Step::Coach, 7);
    let sources: [&dyn StepSource; 4] = [&activities, &strength, &health, &coach];
    let mut seen = Vec::new();
    let report = service.start(period()?, &sources, &now, &mut |progress| {
        if let Progress::Skipped { step, .. } = progress {
            seen.push(step);
        }
        ControlFlow::Continue(())
    })?;
    assert_eq!(report.state, RunState::Partial);
    assert_eq!(state(&report, Step::Activities)?, StepState::Complete);
    assert!(
        matches!(state(&report, Step::Strength)?, StepState::Skipped { reason } if reason == "not signed in")
    );
    assert!(
        matches!(state(&report, Step::Health)?, StepState::Partial { reason }
        if reason.starts_with("2026-10-02 to 2026-10-02:") && reason.contains("fixture failure"))
    );
    // A failed health day does not stop Coach.
    assert_eq!(state(&report, Step::Coach)?, StepState::Complete);
    assert_eq!(seen, [Step::Strength]);
    assert_eq!(health.calls.get(), 2, "the first failure ends the step");

    health.fail_on.set(None);
    let resumed = service.resume(&sources, &now, &mut |_| ControlFlow::Continue(()))?;
    assert!(resumed.resumed);
    assert_eq!(resumed.run_id, report.run_id);
    assert_eq!(
        activities.calls.get(),
        1,
        "done windows are not fetched again"
    );
    assert_eq!(
        health.calls.get(),
        4,
        "only the failed and untried days run"
    );
    assert_eq!(state(&resumed, Step::Health)?, StepState::Complete);
    let health_report = resumed.steps.iter().find(|s| s.step == Step::Health);
    assert_eq!(
        health_report.map(|s| (s.done, s.reused, s.saved)),
        Some((3, 1, 6))
    );
    // Strength is still unavailable, so the run stays partial and resumable.
    assert_eq!(resumed.state, RunState::Partial);
    let overview = service.overview()?.context("journaled run")?;
    assert_eq!(overview.run.state, RunState::Partial);
    assert_eq!(overview.steps.len(), 4);
    assert!(service.unfinished()?.is_some());
    Ok(())
}

#[test]
fn a_stop_ends_the_run_and_complete_runs_cannot_be_resumed() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let journal = SqliteSyncJournal::open(fixture.settings.database.clone())?;
    let service = SyncService::new(&journal);
    let fakes = Step::ORDER.map(|step| Fake::new(step, 1));
    let sources: Vec<&dyn StepSource> = fakes.iter().map(|f| f as &dyn StepSource).collect();
    let stopped = service.start(period()?, &sources, &now, &mut |progress| match progress {
        Progress::Starting {
            step: Step::Health, ..
        } => ControlFlow::Break(()),
        _ => ControlFlow::Continue(()),
    })?;
    assert_eq!(stopped.state, RunState::Stopped);
    assert_eq!(state(&stopped, Step::Activities)?, StepState::Complete);
    assert!(matches!(
        state(&stopped, Step::Coach)?,
        StepState::Skipped { .. }
    ));
    let done = service.resume(&sources, &now, &mut |_| ControlFlow::Continue(()))?;
    assert_eq!(done.state, RunState::Complete);
    assert_eq!(fakes[0].calls.get(), 3, "activity days are not repeated");
    assert!(service.unfinished()?.is_none());
    assert!(
        service
            .resume(&sources, &now, &mut |_| ControlFlow::Continue(()))
            .is_err()
    );
    Ok(())
}

fn sync(fixture: &Fixture, args: &[&str]) -> Result<(bool, Value, String)> {
    let output = fixture
        .command()
        .env("GARMIN_PERSONAL_API_KEY", KEY)
        .arg("sync")
        .args(args)
        .output()?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Ok((
        output.status.success(),
        serde_json::from_slice(&output.stdout)?,
        stderr,
    ))
}

#[test]
fn cli_sync_imports_each_step_names_skips_and_resumes_without_refetching() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let date = "2026-10-24";
    let activity = json!({"activityId":901,"activityName":"Morning run","activityType":{"typeKey":"running"},
        "startTimeGMT":"2026-10-24 07:00:00","duration":1800.0});
    // Without a Connect login only activities can run; the other steps are skipped and named.
    let server = Server::start(vec![
        ("POST", crate::garmin::TOKEN_PATH, 200, TOKEN.to_string()),
        (
            "GET",
            "/activitylist-service/activities/search/activities?start=0&limit=100&startDate=2026-10-24&endDate=2026-10-24",
            200,
            json!([activity]).to_string(),
        ),
    ])?;
    configure(&fixture, &server.base)?;
    let (ok, report, stderr) = sync(&fixture, &["--start", date, "--end", date])?;
    assert!(!ok, "skipped steps make the run partial");
    assert!(stderr.contains("pace sync --resume"), "{stderr}");
    assert!(!stderr.contains(KEY) && !stderr.contains(TOKEN));
    assert_eq!(report["state"], "partial");
    let step = |name: &str| -> Value {
        report["steps"]
            .as_array()
            .and_then(|steps| steps.iter().find(|s| s["step"] == name).cloned())
            .unwrap_or(Value::Null)
    };
    assert_eq!(step("activities")["state"], "complete", "{report:#}");
    assert_eq!(step("activities")["saved"], 1);
    for name in ["strength", "health", "coach"] {
        assert_eq!(step(name)["state"], "skipped");
        assert!(
            step(name)["reason"]
                .as_str()
                .is_some_and(|r| r.contains("pace garmin login"))
        );
    }
    server.finish()?;

    // No server is listening now: resume must not contact Garmin for done windows.
    let (ok, resumed, _) = sync(&fixture, &["--resume"])?;
    assert!(!ok);
    assert_eq!(resumed["resumed"], true);
    assert_eq!(resumed["run_id"], report["run_id"]);
    assert_eq!(resumed["steps"][0]["state"], "complete");
    assert_eq!(resumed["steps"][0]["reused"], 1);
    Ok(())
}
