//! tests/cases/terminal_flows.rs covers populated non-onboarding menus end to end.
//! Real PTYs, CLI commands and SQLite exercise pagination, review and local changes;
//! only Garmin's external API is replaced with a scripted local HTTP server.

use super::{Session, confirm, scroll_until};
use crate::common::Fixture;
use anyhow::{Result, ensure};
use garmin_pace::workouts::{
    DraftState, SqliteWorkouts, WorkoutService, document::WorkoutDocument,
};
use std::sync::Arc;

#[test]
fn activity_pages_search_details_and_back_preserve_the_selected_record() -> Result<()> {
    for (width, height) in [(100, 24), (32, 10)] {
        let mut fixture = Fixture::new()?;
        fixture.settings.history.default_page_size = 1;
        std::fs::write(&fixture.config, toml::to_string(&fixture.settings)?)?;
        fixture.init()?;
        fixture.seed()?;
        let mut session = Session::start(&fixture, width, height, width < 76)?;
        session.until("Search actions")?;
        session.search("Activities")?;
        session.until("Activities 1-1")?;
        session.send(b"\t")?;
        session.until("Enter/Esc back")?;
        scroll_until(&mut session, "2026-10-02 08:00")?;
        session.send(b"\x1b")?;
        session.until("Activities 1-1")?;
        session.search("Next page")?;
        session.until("Activities 2-2")?;
        session.send(b"\r")?;
        session.until("Enter/Esc back")?;
        scroll_until(&mut session, "Garmin ID: fixture-1")?;
        session.send(b"\x1b")?;
        session.until("Activities 2-2")?;
        session.search("Next page")?;
        session.until("Activities: no more entries")?;
        session.send(b"\r")?;
        session.until("Activities 2-2")?;
        session.search("Previous page")?;
        session.until("Activities 1-1")?;
        session.exit()?;
    }
    Ok(())
}

#[test]
fn draft_pagination_and_discard_change_only_the_reviewed_candidate() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let service = WorkoutService::new(Arc::new(SqliteWorkouts::open(
        fixture.settings.database.clone(),
    )?));
    for index in 1..=21 {
        let doc: WorkoutDocument = serde_json::from_value(crate::workouts::candidate(
            &format!("Candidate {index:02}"),
            1200.0,
        ))?;
        service.propose(doc.try_into()?, None)?;
    }
    let mut session = Session::start(&fixture, 100, 24, false)?;
    session.until("Search actions")?;
    session.search("Workout drafts")?;
    session.until("Candidate 21")?;
    session.search("Next page")?;
    session.until("Candidate 01")?;
    ensure!(!session.saw("Next page"));
    session.search("Candidate 01")?;
    session.until("> Review before/after")?;
    session.search("Discard draft")?;
    confirm(&mut session)?;
    session.until("Discarded locally")?;
    ensure!(service.draft(1)?.state == DraftState::Discarded);
    ensure!(service.draft(2)?.state == DraftState::Draft);
    session.exit()
}

#[test]
fn benchmark_pages_and_discard_return_to_the_current_page() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let (service, proposal) = crate::benchmarks::seed(&fixture, false)?;
    for _ in 0..21 {
        service.propose(proposal.clone())?;
    }
    let mut session = Session::start(&fixture, 100, 24, false)?;
    session.until("Search actions")?;
    session.search("Benchmark exercises")?;
    session.until("Sync strength data")?;
    session.search("Model proposals")?;
    session.until("Refresh list")?;
    session.search("Next page")?;
    session.until("barbell row")?;
    session.until("Enter open Tab info Esc back")?;
    ensure!(!session.saw("Next page"));
    session.search("barbell row")?;
    session.until("Benchmark proposal #1")?;
    session.send(b"\r")?;
    session.until("> Back")?;
    session.send(b"\x1b[B\x1b[B\r")?;
    confirm(&mut session)?;
    session.until("Discarded locally")?;
    ensure!(service.draft(1)?.state == DraftState::Discarded);
    session.send(b"\r")?;
    session.until("/ barbell row_")?;
    session.send(b"\x1b")?;
    session.until("Search actions")?;
    session.search("Previous page")?;
    session.until("Refresh list")?;
    session.exit()
}

#[test]
fn readonly_workout_keeps_its_outline_without_offering_model_edits() -> Result<()> {
    use crate::garmin::{Server, configure_connect};
    use serde_json::json;
    let fixture = Fixture::new()?;
    fixture.init()?;
    let mut workout = crate::workouts::remote_workout("Read only tempo", 1200.0);
    workout["workoutSegments"][0]["workoutSteps"][1]["newSemanticField"] = json!("preserve me");
    let server = Server::start(vec![
        (
            "GET",
            "/workout-service/workouts?start=0&limit=20",
            200,
            json!([{"workoutId":777}]).to_string(),
        ),
        (
            "GET",
            "/workout-service/workout/777",
            200,
            workout.to_string(),
        ),
    ])?;
    configure_connect(&fixture, &server.base)?;
    let mut session = Session::start(&fixture, 100, 24, false)?;
    session.until("Search actions")?;
    session.search("Standalone workout library")?;
    session.until("> Sync from Garmin")?;
    session.send(b"\r")?;
    session.until("Imported 1 workouts")?;
    session.send(b"\r")?;
    session.until("Read only tempo")?;
    session.search("Read only tempo")?;
    session.until("> Full text")?;
    session.until("Enter open Tab info Esc back")?;
    ensure!(!session.saw("Edit with model"));
    session.send(b"\r")?;
    session.until("Read-only:")?;
    scroll_until(&mut session, "STEPS (read-only outline)")?;
    session.send(b"\x1b")?;
    session.until("> Full text")?;
    session.search("interval")?;
    session.until("Duration: 1200 sec")?;
    session.exit()?;
    server.finish()
}

#[test]
fn users_status_and_keys_show_access_without_revealing_tokens() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    fixture.seed()?;
    let (token, key_id) = crate::common::register_user(&fixture, &fixture, "tester")?;
    for args in [
        vec!["users", "allow-garmin-writes", "tester"],
        vec!["users", "disable", "tester"],
    ] {
        ensure!(fixture.command().args(args).output()?.status.success());
    }
    let mut session = Session::start(&fixture, 100, 24, false)?;
    session.until("2 activities | last sync never")?;
    session.search("Status")?;
    session.until("Config and database")?;
    session.search("Activities")?;
    session.until("Next: Sync activities")?;
    session.exit()?;
    let mut session = Session::start(&fixture, 100, 24, false)?;
    session.until("Search actions")?;
    session.search("MCP users")?;
    session.until("disabled; its keys are blocked")?;
    session.search("tester")?;
    session.until("> Profile")?;
    session.send(b"\r")?;
    session.until("Garmin writes by its model: allowed")?;
    ensure!(!session.received.contains(&token));
    session.send(b"\x1b")?;
    session.until("> Profile")?;
    session.send(b"\x1b[B\r")?;
    session.until(&format!("Key ID: {key_id}"))?;
    ensure!(!session.received.contains(&token));
    session.exit()
}

#[test]
fn pasted_unicode_search_and_prompt_help_preserve_input() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    fixture.seed()?;
    rusqlite::Connection::open(&fixture.settings.database.path)?.execute(
        "UPDATE activities SET name=?1 WHERE garmin_id=?2",
        rusqlite::params!["Бег утром", "fixture-2"],
    )?;
    let mut session = Session::start(&fixture, 40, 15, true)?;
    session.until("Search actions")?;
    session.search("Activities")?;
    session.until("Activities 1-2")?;
    session.send(b"/")?;
    session.until("/ _")?;
    session.send("\x1b[200~Бег\r\x1b[201~".as_bytes())?;
    session.until("/ Бег_")?;
    ensure!(!session.saw("Garmin ID:"));
    session.send(b"\r")?;
    session.until("Garmin ID: fixture-2")?;
    session.exit()?;
    let mut session = Session::start(&fixture, 32, 10, true)?;
    session.until("Search actions")?;
    session.search("Health")?;
    session.until("> Import health")?;
    session.send(b"\r")?;
    session.until("> Custom period")?;
    session.send(b"\r")?;
    session.until("Tab: full instructions")?;
    session.send(b"2026-10-24\t")?;
    session.until("Enter/Esc back")?;
    scroll_until(&mut session, "rate limit.")?;
    session.send(b"\x1b")?;
    session.until("> 2026-10-24_")?;
    session.exit()
}
