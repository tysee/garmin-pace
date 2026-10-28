//! tests/cases/terminal_overview.rs drives Actions, summaries, change review and explicit sync presets.
//! Built CLI/PTY flows use real SQLite; only Garmin HTTP is replaced with a local fixture.

use super::{Session, scroll_until};
use crate::{
    common::Fixture,
    garmin::{Server, configure},
    workouts::{candidate, seed_draft},
};
use anyhow::{Result, ensure};
use garmin_pace::workouts::{
    DraftState, LibraryWorkout, SqliteWorkouts, Workout, WorkoutRepository, WorkoutService,
    document::WorkoutDocument,
};
use std::{process::Command, sync::Arc};

fn definition(name: &str, seconds: f64) -> Result<Workout> {
    let doc: WorkoutDocument = serde_json::from_value(candidate(name, seconds))?;
    Ok(doc.try_into()?)
}
#[test]
fn draft_attention_prioritizes_old_pending_save_beyond_first_page_and_refreshes() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let repo = Arc::new(SqliteWorkouts::open(f.settings.database.clone())?);
    let service = WorkoutService::new(repo.clone());
    for i in 1..=25 {
        service.propose(definition(&format!("Candidate {i}"), 600.0)?, None)?;
    }
    repo.transition(1, 1, DraftState::Draft, DraftState::Saving, None)?;
    let attention = service.attention()?;
    assert_eq!(
        (attention.ready, attention.pending, attention.next_id()),
        (24, 1, Some(1))
    );
    service.abandon(1, 1)?;
    assert_eq!(service.attention()?.next_id(), Some(2));
    service.discard(2, 1)?;
    assert_eq!(service.attention()?.next_id(), Some(3));
    Ok(())
}
#[test]
fn summary_and_change_review_work_in_cli_and_small_terminal() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let repo = Arc::new(SqliteWorkouts::open(f.settings.database.clone())?);
    repo.put(&LibraryWorkout {
        coach_plan_id: None,
        id: 777,
        name: "Original".into(),
        sport: "running".into(),
        revision: "source-revision".into(),
        synced_at: "2026-10-24T12:00:00Z".into(),
        definition: Some(definition("Original", 1200.0)?),
        read_only_reason: None,
        outline: None,
    })?;
    let service = WorkoutService::new(repo);
    service.propose(
        definition("Changed", 600.0)?,
        Some((777, "source-revision".into())),
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_pace"))
        .arg("--config")
        .arg(&f.config)
        .args(["workout", "review", "--id", "1"])
        .output()?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout)?;
    assert!(text.contains("Name: Original -> Changed"));
    assert!(text.contains("Step 1.2 duration: 1200 sec -> 600 sec"));
    assert!(text.contains("Timed steps: 1500 sec (25.0 min)"));
    assert!(text.contains("BEFORE") && text.contains("AFTER"));
    let mut ui = Session::start_args(&f, 32, 10, &["ui", "--compact"])?;
    ui.until("Pace / Actions")?;
    ui.search("Workout drafts")?;
    ui.until("Refresh drafts")?;
    ui.search("Changed")?;
    ui.until("Draft #1:")?;
    ui.search("Summary")?;
    ui.until("Workout summary")?;
    scroll_until(&mut ui, "25.0 min")?;
    ui.send(b"\x1b")?;
    ui.until("Draft #1:")?;
    ui.send(b"\x1b")?;
    ui.until("Search actions")?;
    ui.search("Change overview")?;
    scroll_until(&mut ui, "1200 sec -> 600 sec")?;
    ui.exit()
}
#[test]
fn freshness_does_not_present_empty_health_as_measured() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    f.seed()?;
    crate::health::seed_health(&f, 4321.0)?;
    let mut ui = Session::start_args(&f, 100, 28, &[])?;
    ui.until("Pace / Actions")?;
    ui.search("Health")?;
    ui.until("Import health")?;
    ui.search("Data freshness")?;
    ui.until("Latest activity: 2026-10-02")?;
    scroll_until(&mut ui, "Latest saved day: 2026-10-24")?;
    scroll_until(&mut ui, "Training readiness: no observed data")?;
    ui.exit()
}
#[test]
fn date_preset_cancel_never_imports_and_confirm_imports_exact_dates() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let today = chrono::Utc::now().date_naive();
    let path: &'static str = Box::leak(format!("/activitylist-service/activities/search/activities?start=0&limit=100&startDate={today}&endDate={today}").into_boxed_str());
    let server = Server::start(vec![
        (
            "POST",
            crate::garmin::TOKEN_PATH,
            200,
            crate::garmin::TOKEN.into(),
        ),
        ("GET", path, 200, "[]".into()),
    ])?;
    configure(&f, &server.base)?;
    let mut ui = Session::start_with_key(&f, 100, 28, false, Some("fixture-api-key-never-real"))?;
    ui.search("Sync activities")?;
    ui.until("> Recent activities")?;
    ui.send(b"\x1b[B\x1b[B\r")?;
    ui.until("Review action")?;
    ui.until(&format!("{today} to {today}"))?;
    ui.send(b"\x1b")?;
    ui.until("Pace / Actions")?;
    let history = garmin_pace::history::HistoryService::new(
        Arc::new(garmin_pace::history::SqliteHistory::open(
            f.settings.database.clone(),
        )?),
        100,
    );
    assert!(history.summary()?.last_sync.is_none());
    ui.send(b"\r")?;
    ui.until("> Recent activities")?;
    ui.send(b"\x1b[B\x1b[B\r")?;
    ui.until("Review action")?;
    ui.send(b"\r")?;
    ui.until("> Cancel")?;
    ui.send(b"\x1b[B\r")?;
    ui.until(&format!("No activities between {today} and {today}."))?;
    assert!(history.summary()?.last_sync.is_some());
    server.finish()?;
    ui.exit()
}
#[test]
fn health_presets_show_exact_dates_and_leave_drafts_unchanged_on_cancel() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    seed_draft(&f)?;
    let mut ui = Session::start_args(&f, 40, 15, &["ui", "--compact"])?;
    ui.until("Pace / Actions")?;
    ui.search("Health")?;
    ui.until("> Import health")?;
    ui.send(b"\r")?;
    ui.until("> Custom period")?;
    ui.send(b"\x1b[B\x1b[B\x1b[B\r")?;
    ui.until("Review action")?;
    // Wait for the confirmation frame, whose footer differs from a read-only result.
    ui.until("Enter next  Esc cancel")?;
    ui.send(b"\x1b[6~")?;
    ui.until("snapshots for these dates")?;
    ui.send(b"\x1b")?;
    ui.until("> Import health")?;
    assert_eq!(
        WorkoutService::new(Arc::new(SqliteWorkouts::open(f.settings.database.clone())?))
            .attention()?
            .ready,
        1
    );
    ui.exit()
}

#[test]
fn seven_day_health_preset_respects_small_windows_and_persists_real_snapshots() -> Result<()> {
    let mut f = Fixture::new()?;
    f.settings.health.max_days = 3;
    std::fs::write(&f.config, toml::to_string(&f.settings)?)?;
    f.init()?;
    let today = chrono::Utc::now().date_naive();
    let mut script = Vec::new();
    for offset in 0..7 {
        if offset % 3 == 0 {
            script.push((
                "POST",
                crate::garmin::TOKEN_PATH,
                200,
                crate::garmin::TOKEN.into(),
            ));
        }
        let date = today - chrono::Days::new(6 - offset);
        script.extend(crate::health::day_script(&date.to_string()));
    }
    let server = Server::start(script)?;
    configure(&f, &server.base)?;
    crate::garmin::configure_connect(&f, &server.base)?;
    let mut ui = Session::start_with_key(&f, 100, 28, false, Some(crate::garmin::KEY))?;
    ui.search("Health")?;
    ui.until("> Import health")?;
    ui.send(b"\r")?;
    ui.until("> Custom period")?;
    ui.send(b"\x1b[B\x1b[B\x1b[B\r")?;
    ui.until("Review action")?;
    ui.send(b"\r")?;
    ui.until("> Cancel")?;
    ui.send(b"\x1b[B\r")?;
    ui.until("Health import complete: 7 days saved")?;
    server.finish()?;
    let db = rusqlite::Connection::open(&f.settings.database.path)?;
    let count: i64 = db.query_row("SELECT count(*) FROM health_days", [], |r| r.get(0))?;
    assert_eq!(count, 7);
    ui.send(b"\r")?;
    ui.until("Saved days: 7")?;
    ui.search("Data freshness")?;
    scroll_until(&mut ui, "Training readiness: no observed data")?;
    ui.exit()
}

#[test]
fn actions_retry_keeps_the_failed_non_secret_input() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let mut ui = Session::start_args(&f, 40, 15, &["ui", "--compact"])?;
    ui.until("Pace / Actions")?;
    ui.search("Sync activities")?;
    ui.until("> Recent activities")?;
    ui.send(b"\r")?;
    ui.until("Enter = 20")?;
    ui.send(b"999\r")?;
    ui.until("Action failed")?;
    ui.send(b"\r")?;
    ui.until("Pace / Actions")?;
    ui.send(b"\r")?;
    ui.until("> Recent activities")?;
    ui.send(b"\r")?;
    ui.until("> 999_")?;
    ui.exit()
}

#[test]
fn startup_opens_actions_directly_and_escape_exits() -> Result<()> {
    for (width, height, args) in [(100, 24, vec![]), (32, 10, vec!["ui", "--compact"])] {
        let f = Fixture::new()?;
        f.init()?;
        let mut ui = Session::start_args(&f, width, height, &args)?;
        ui.until("Pace / Actions")?;
        assert!(!ui.saw("Pace / Home"));
        ui.send(b"\x1b")?;
        ui.finish()?;
    }
    Ok(())
}

#[test]
fn every_action_explains_its_state_and_next_step_without_running_it() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let cases = [
        (
            "Onboarding",
            "setup checks need attention",
            "Open the setup checklist",
        ),
        (
            "Status",
            "checks need attention",
            "Inspect findings and remedies",
        ),
        (
            "Activities",
            "No activities saved yet",
            "Use Sync activities",
        ),
        (
            "Sync everything",
            "Last full sync: Never run",
            "Set the Personal API key",
        ),
        (
            "Sync activities",
            "Never synced",
            "Set the Personal API key",
        ),
        ("Health", "No health days saved yet", "Open metrics"),
        ("Garmin login", "Not signed in", "Sign in for workouts"),
        (
            "Garmin devices",
            "not fetched by this menu",
            "Use Garmin login first",
        ),
        (
            "Session status",
            "Not signed in",
            "Inspect local session expiry",
        ),
        ("Garmin logout", "Not signed in", "Confirm removal"),
        ("Workouts", "0 standalone workouts saved", "Browse steps"),
        ("Coach", "not synced", "Open the week"),
        (
            "Workout drafts",
            "0 unresolved saves | 0 unsaved drafts",
            "Ask the model",
        ),
        (
            "Benchmark exercises",
            "0 exercises saved",
            "Sync strength data",
        ),
        (
            "Check configuration",
            "Valid configuration",
            "Run configuration validation",
        ),
        ("Show configuration", "Valid configuration", "Inspect paths"),
        (
            "Garmin network",
            "Proxy: environment; extra CA: none",
            "Change the proxy or CA file",
        ),
        (
            "Internet access (HTTPS)",
            "HTTPS not configured on this host",
            "Choose a domain",
        ),
        (
            "Create configuration",
            "overwrite refused",
            "Use Check configuration",
        ),
        (
            "Initialize database",
            "Config and database ready",
            "Create storage",
        ),
        ("MCP users", "No MCP users registered yet", "Inspect users"),
        ("Exit", "Close this menu", "Return to the terminal"),
    ];
    let mut ui = Session::start_args(&f, 100, 40, &[])?;
    ui.until("Pace / Actions")?;
    for (label, state, next) in cases {
        ui.send(b"\t")?;
        ui.until("Pace / Details")?;
        ui.until(label)?;
        ui.until(state)?;
        ui.until(&format!("Next: {next}"))?;
        ui.send(b"\x1b")?;
        ui.until("Pace / Actions")?;
        ui.send(b"\x1b[B")?;
    }
    ui.exit()
}

#[test]
fn actions_refresh_draft_counts_after_return_and_never_create_missing_storage() -> Result<()> {
    let f = Fixture::new()?;
    let mut ui = Session::start_args(&f, 100, 40, &[])?;
    ui.until("Pace / Actions")?;
    ui.until("Database not initialized yet")?;
    assert!(!f.settings.database.path.exists());
    ui.exit()?;
    f.init()?;
    let mut ui = Session::start_args(&f, 100, 40, &[])?;
    ui.until("0 unresolved saves | 0 unsaved drafts")?;
    ui.search("Status")?;
    ui.until("Pace / Status")?;
    seed_draft(&f)?;
    ui.send(b"\x1b")?;
    ui.until("Pace / Actions")?;
    ui.send(b"\x1b")?;
    ui.until("0 unresolved saves | 1 unsaved drafts")?;
    ui.search("Workout drafts")?;
    ui.until("Refresh drafts")?;
    ui.exit()
}
