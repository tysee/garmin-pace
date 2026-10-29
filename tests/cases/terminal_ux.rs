//! tests/cases/terminal_ux.rs verifies complete local search and workout recovery flows.
//! The built CLI and PTY menu share real SQLite; Garmin alone uses a scripted HTTP fixture.

use super::{Session, confirm, scroll_until};
use crate::{
    common::Fixture,
    garmin::{Server, configure_connect},
    workouts::{remote_workout, seed_draft},
};
use anyhow::{Result, ensure};
use serde_json::json;

#[test]
fn cli_search_counts_all_matches_and_treats_name_as_literal() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    f.seed()?;
    let db = rusqlite::Connection::open(&f.settings.database.path)?;
    db.execute("INSERT INTO activities VALUES('literal','100%_Lift','strength_training','2026-10-02T10:00:00Z',60,NULL)", [])?;
    let search = |args: &[&str]| -> Result<serde_json::Value> {
        let output = f
            .command()
            .args(["activities", "search"])
            .args(args)
            .output()?;
        ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(serde_json::from_slice(&output.stdout)?)
    };
    let page = search(&["--name", "RUN", "--limit", "1"])?;
    assert_eq!(page["total"], 2);
    assert_eq!(page["next_offset"], 1);
    let page = search(&["--name", "RUN", "--limit", "1", "--offset", "1"])?;
    assert_eq!(page["activities"][0]["garmin_id"], "fixture-1");
    assert!(page["next_offset"].is_null());
    assert_eq!(search(&["--name", "%_"])?["total"], 1);
    assert_eq!(search(&["--name", "' OR 1=1 --"])?["total"], 0);
    assert_eq!(
        search(&[
            "--start",
            "2026-10-02",
            "--end",
            "2026-10-02",
            "--sport",
            "running"
        ])?["total"],
        1
    );
    assert_eq!(search(&["--end", "2026-10-01"])?["total"], 1);
    assert!(
        !f.command()
            .args([
                "activities",
                "search",
                "--start",
                "2026-10-03",
                "--end",
                "2026-10-01"
            ])
            .output()?
            .status
            .success()
    );
    Ok(())
}

#[test]
fn menu_search_finds_an_activity_outside_the_loaded_page() -> Result<()> {
    let mut f = Fixture::new()?;
    f.settings.history.default_page_size = 1;
    std::fs::write(&f.config, toml::to_string(&f.settings)?)?;
    f.init()?;
    f.seed()?;
    rusqlite::Connection::open(&f.settings.database.path)?.execute(
        "UPDATE activities SET name='Older lift' WHERE garmin_id='fixture-1'",
        [],
    )?;
    let mut ui = Session::start(&f, 40, 15, true)?;
    ui.until("Search actions")?;
    ui.search("Activities")?;
    ui.until("Activities 1-1")?;
    ui.search("Search all history")?;
    ui.until("Name contains")?;
    ui.send(b"older\r")?;
    ui.until("Sport")?;
    ui.send(b"\r")?;
    ui.until("Start UTC")?;
    ui.send(b"\r")?;
    ui.until("End UTC")?;
    ui.send(b"\r")?;
    ui.until("Matches 1-1 of 1")?;
    ui.send(b"\r")?;
    ui.until("Enter/Esc back")?;
    scroll_until(&mut ui, "Garmin ID: fixture-1")?;
    ui.send(b"\x1b")?;
    ui.until("Matches 1-1 of 1")?;
    ui.search("Clear filters")?;
    ui.until("Activities 1-1")?;
    ui.search("Search all history")?;
    ui.until("Name contains")?;
    ensure!(ui.saw("> _"));
    ui.exit()
}

#[test]
fn verified_save_offers_delivery_without_reopening_the_library() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    seed_draft(&f)?;
    let saved = remote_workout("Tempo session", 1200.0).to_string();
    let devices = json!([{"unitId":99,"productDisplayName":"Test watch"}]).to_string();
    let server = Server::start(vec![
        (
            "POST",
            "/workout-service/workout",
            200,
            json!({"workoutId":777}).to_string(),
        ),
        ("GET", "/workout-service/workout/777", 200, saved.clone()),
        (
            "GET",
            "/device-service/deviceregistration/devices",
            200,
            devices.clone(),
        ),
        ("GET", "/workout-service/workout/777", 200, saved),
        (
            "GET",
            "/device-service/deviceregistration/devices",
            200,
            devices,
        ),
        (
            "POST",
            "/device-service/devicemessage/messages",
            200,
            "[]".into(),
        ),
    ])?;
    configure_connect(&f, &server.base)?;
    let mut ui = Session::start(&f, 100, 24, false)?;
    ui.until("Search actions")?;
    ui.search("Workout drafts")?;
    ui.until("Refresh drafts")?;
    ui.search("Tempo session")?;
    ui.until("> Review before/after")?;
    ui.search("Save to Garmin")?;
    confirm(&mut ui)?;
    ui.until("Saved and verified in Garmin")?;
    ui.send(b"\r")?;
    ui.until("Back to drafts")?;
    ui.send(b"\x1b[B\r")?;
    ui.until("> Test watch")?;
    ui.send(b"\r")?;
    confirm(&mut ui)?;
    ui.until("Sent to watch")?;
    ui.send(b"\r")?;
    ui.until("Delivery status")?;
    ui.until("Sent to watch")?;
    ui.exit()?;
    server.finish()
}

#[test]
fn uncertain_save_stays_on_draft_and_reconciles_without_posting_twice() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    seed_draft(&f)?;
    let server = Server::start(vec![
        ("POST", "/workout-service/workout", 504, "{}".into()),
        (
            "GET",
            "/workout-service/workout/777",
            200,
            remote_workout("Tempo session", 1200.0).to_string(),
        ),
    ])?;
    configure_connect(&f, &server.base)?;
    let mut ui = Session::start(&f, 100, 24, false)?;
    ui.until("Search actions")?;
    ui.search("Workout drafts")?;
    ui.until("Refresh drafts")?;
    ui.search("Tempo session")?;
    ui.until("> Review before/after")?;
    ui.send(b"\x1b[B\r")?;
    confirm(&mut ui)?;
    ui.until("Action failed")?;
    ui.send(b"\r")?;
    ui.until("Reconcile save")?;
    ensure!(!ui.saw("Save to Garmin"));
    ui.send(b"\x1b[B\r")?;
    ui.until("Existing Garmin workout ID")?;
    ui.send(b"bad\r")?;
    ui.until("Action failed")?;
    ui.send(b"\r")?;
    ui.until("> bad_")?;
    ui.send(b"\x7f\x7f\x7f777\r")?;
    ui.until("Saved and verified in Garmin")?;
    ui.exit()?;
    server.finish()
}

#[test]
fn failed_health_import_retains_its_screen_and_input() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let mut ui = Session::start(&f, 40, 15, true)?;
    ui.until("Search actions")?;
    ui.search("Health")?;
    ui.until("> Import health")?;
    ui.send(b"\r")?;
    ui.until("> Custom period")?;
    ui.send(b"\r")?;
    ui.until("Enter continue  Esc cancel")?;
    ui.send(b"2026-10-24\r")?;
    ui.until("Action failed")?;
    ui.send(b"\r")?;
    ui.until("> Import health")?;
    ui.send(b"\r")?;
    ui.until("> Custom period")?;
    ui.send(b"\r")?;
    ui.until("> 2026-10-24_")?;
    ui.exit()
}

#[test]
fn verified_save_can_finish_without_requesting_devices_or_delivery() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    seed_draft(&f)?;
    let server = Server::start(vec![
        (
            "POST",
            "/workout-service/workout",
            200,
            json!({"workoutId":777}).to_string(),
        ),
        (
            "GET",
            "/workout-service/workout/777",
            200,
            remote_workout("Tempo session", 1200.0).to_string(),
        ),
    ])?;
    configure_connect(&f, &server.base)?;
    let mut ui = Session::start(&f, 32, 10, true)?;
    ui.until("Search actions")?;
    ui.search("Workout drafts")?;
    ui.until("Refresh drafts")?;
    ui.search("Tempo session")?;
    ui.until("> Review before/after")?;
    ui.send(b"\x1b[B\r")?;
    confirm(&mut ui)?;
    ui.until("Saved and verified in Garmin")?;
    ui.send(b"\r")?;
    ui.until("> Back to drafts")?;
    ui.send(b"\r")?;
    ui.until("/ Tempo session_")?;
    ui.exit()?;
    let count: i64 = rusqlite::Connection::open(&f.settings.database.path)?.query_row(
        "SELECT count(*) FROM workout_deliveries",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(count, 0);
    server.finish()
}

#[test]
fn delivery_card_distinguishes_versions_and_clears_only_unresolved_requests() -> Result<()> {
    use garmin_pace::workouts::{Delivery, SqliteWorkouts, WorkoutRepository};
    let f = Fixture::new()?;
    f.init()?;
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
            remote_workout("Tempo session", 1200.0).to_string(),
        ),
    ])?;
    configure_connect(&f, &server.base)?;
    ensure!(
        f.command()
            .args(["workout", "sync"])
            .output()?
            .status
            .success()
    );
    server.finish()?;
    let repo = SqliteWorkouts::open(f.settings.database.clone())?;
    let current = repo.get(777)?;
    let mut ui = Session::start(&f, 100, 24, false)?;
    ui.until("Search actions")?;
    ui.search("Standalone workout library")?;
    ui.until("New with model")?;
    ui.search("Tempo session")?;
    ui.until("> Full text")?;
    ui.search("Delivery status")?;
    ui.until("Delivery status: no recorded request")?;
    for (device, revision, queued) in [
        (99, "a".repeat(64), false),
        (100, current.revision.clone(), true),
        (101, "b".repeat(64), true),
    ] {
        let d = Delivery {
            workout_id: 777,
            device_id: device,
            revision,
            queued: false,
            request: None,
        };
        repo.reserve_delivery(&d)?;
        if queued {
            repo.complete_delivery(&d)?;
        }
    }
    ui.send(b"\r")?;
    ui.until("Device 99: Outcome unknown")?;
    ui.send(b"\x1b[B\r")?;
    ui.until("Version: older")?;
    ui.until("Watch receipt: unconfirmed.")?;
    ui.send(b"\r")?;
    ui.until("Clear unresolved delivery")?;
    ui.send(b"\x1b[B\r")?;
    confirm(&mut ui)?;
    ui.until("Unresolved delivery reservation cleared")?;
    ui.send(b"\r")?;
    ui.until("Device 100: Sent to watch")?;
    ui.send(b"\r")?;
    ui.until("Version: current")?;
    ui.exit()?;
    let output = f
        .command()
        .args(["workout", "deliveries", "--id", "777"])
        .output()?;
    ensure!(output.status.success());
    let data: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(data["deliveries"].as_array().map(Vec::len), Some(2));
    assert_eq!(data["deliveries"][0]["state"], "queued");
    assert_eq!(data["deliveries"][0]["status_label"], "Sent to watch");
    assert_eq!(data["deliveries"][0]["current_version"], true);
    assert_eq!(data["deliveries"][1]["current_version"], false);
    Ok(())
}

#[test]
fn coach_menu_is_separate_and_week_navigation_stays_local() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let mut ui = Session::start(&f, 100, 24, false)?;
    ui.until("Search actions")?;
    ui.search("Coach")?;
    ui.until("Coach: not synced")?;
    ui.search("Choose week")?;
    ui.until("Garmin calendar date")?;
    ui.send(b"2026-10-27\r")?;
    ui.until("2026-10-26 to 2026-11-01")?;
    ui.search("Next week")?;
    ui.until("2026-11-02 to 2026-11-08")?;
    ui.search("Plan details")?;
    ui.until("Import this week explicitly")?;
    ui.send(b"\x1b")?;
    ui.until("Plan details")?;
    ui.send(b"\x1b")?;
    ui.until("Sync this week")?;
    ui.search("Previous week")?;
    ui.until("2026-10-26 to 2026-11-01")?;
    ui.exit()
}

#[test]
fn coach_sessions_open_steps_and_library_excludes_plan_workouts() -> Result<()> {
    use crate::coach::{PLANS, WEEK, plans, schedule, workout};
    use garmin_pace::workouts::{SqliteWorkouts, WorkoutRepository};
    let f = Fixture::new()?;
    f.init()?;
    let original = workout();
    let g = Server::start(vec![
        ("GET", PLANS, 200, plans()),
        ("GET", WEEK, 200, schedule(&original)),
        (
            "GET",
            "/workout-service/workout/777",
            200,
            original.to_string(),
        ),
    ])?;
    configure_connect(&f, &g.base)?;
    let imported = f
        .command()
        .args([
            "coach",
            "sync",
            "--start",
            "2026-10-26",
            "--end",
            "2026-11-01",
        ])
        .output()?;
    ensure!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    g.finish()?;
    let repo = SqliteWorkouts::open(f.settings.database.clone())?;
    let mut independent = repo.get(777)?;
    independent.id = 888;
    independent.coach_plan_id = None;
    independent.name = "Independent session".into();
    repo.put(&independent)?;
    let listed = f
        .command()
        .args(["workout", "list", "--standalone"])
        .output()?;
    ensure!(listed.status.success());
    let listed: serde_json::Value = serde_json::from_slice(&listed.stdout)?;
    assert_eq!(listed["workouts"].as_array().map(Vec::len), Some(1));
    assert_eq!(listed["workouts"][0]["id"], 888);
    let db = rusqlite::Connection::open(&f.settings.database.path)?;
    let plan:String=db.query_row("EXPLAIN QUERY PLAN SELECT id,name FROM workout_library WHERE json_extract(document,'$.coach_plan_id') IS NULL ORDER BY name COLLATE NOCASE,id LIMIT ?1 OFFSET ?2",[20,0],|r|r.get(3))?;
    ensure!(plan.contains("workout_library_standalone"), "{plan}");
    for (width, height) in [(100, 24), (40, 15)] {
        let mut ui = Session::start(&f, width, height, width < 76)?;
        ui.until("Search actions")?;
        ui.search("Coach")?;
        ui.until("Sync this week")?;
        ui.search("Choose week")?;
        ui.until("Garmin calendar date")?;
        ui.send(b"2026-10-27\r")?;
        ui.until("Coach week")?;
        ui.search("Push Day")?;
        ui.until("Workout steps")?;
        ui.send(b"\r")?;
        ui.until("Full text")?;
        ui.send(b"\r")?;
        scroll_until(&mut ui, "Coach plan: 42")?;
        ui.send(b"\x1b")?;
        ui.until("Full text")?;
        ui.send(b"\x1b")?;
        ui.until("Workout steps")?;
        ui.search("Recorded result")?;
        ui.until("No activity is linked")?;
        ui.send(b"\x1b")?;
        ui.until("Recorded result")?;
        ui.send(b"\x1b")?;
        ui.until("Workout steps")?;
        ui.send(b"\x1b")?;
        ui.until("Coach week")?;
        ui.send(b"\x1b")?;
        ui.until("Sync this week")?;
        ui.send(b"\x1b")?;
        ui.until("Coach")?;
        ui.send(b"\x1b")?;
        ui.until("Search actions")?;
        ui.search("Standalone workout library")?;
        ui.until("Independent session")?;
        ensure!(
            !ui.saw("Push Day"),
            "Coach entry leaked into standalone library"
        );
        ui.exit()?;
    }
    Ok(())
}

#[test]
fn menu_imports_one_strength_activity_sets_by_id() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    crate::benchmarks::seed_activity(&f)?;
    // The exercise sets request is the only Garmin exchange the menu import may make.
    let server = Server::start(vec![(
        "GET",
        crate::benchmarks::SETS,
        200,
        crate::benchmarks::sets().to_string(),
    )])?;
    configure_connect(&f, &server.base)?;
    let mut ui = Session::start(&f, 80, 24, true)?;
    ui.until("Search actions")?;
    ui.search("Activities")?;
    ui.until("Activities 1-1")?;
    ui.send(b"\r")?;
    ui.until("> View details")?;
    ui.send(b"\r")?;
    ui.until("Sets: not synced")?;
    ui.send(b"\x1b")?;
    ui.until("> View details")?;
    ui.send(b"\x1b[B")?;
    ui.until("> Import sets from Garmin")?;
    ui.send(b"\r")?;
    ui.until("2. barbell row | 60.0 kg | 4 reps")?;
    ui.send(b"\x1b")?;
    ui.until("> Import sets from Garmin")?;
    ui.exit()?;
    server.finish()
}

#[test]
fn menu_changes_the_garmin_proxy_and_refuses_an_unreadable_ca_file() -> Result<()> {
    let f = Fixture::new()?;
    let mut ui = Session::start(&f, 100, 24, true)?;
    ui.until("Search actions")?;
    ui.search("Garmin network")?;
    ui.until("> Proxy URL")?;
    ensure!(ui.saw("from the environment apply"));
    ui.send(b"\r")?;
    ui.until("blank = use environment")?;
    ui.send(b"http://proxy.test:3128\r")?;
    ui.until("Saved. A running MCP server applies it after a restart.")?;
    ui.send(b"\x1b")?;
    ui.until("http://proxy.test:3128; replaces the environment's proxy")?;
    let edited = std::fs::read_to_string(&f.config)?;
    ensure!(edited.contains("\nproxy_url = \"http://proxy.test:3128\"\n"));
    ui.send(b"\x1b[B")?;
    ui.until("> Extra CA file")?;
    ui.send(b"\r")?;
    ui.until("blank = none")?;
    ui.send(b"missing.pem\r")?;
    ui.until("Action failed")?;
    ensure!(ui.saw("garmin.extra_ca_file"));
    ensure!(std::fs::read_to_string(&f.config)? == edited);
    ui.send(b"\x1b")?;
    ui.until("> Extra CA file")?;
    ui.exit()
}

#[test]
fn menu_internet_access_names_the_system_install_off_a_service_host() -> Result<()> {
    let f = Fixture::new()?;
    let mut ui = Session::start(&f, 100, 24, true)?;
    ui.until("Search actions")?;
    ui.search("Internet access")?;
    ui.until("No Pace system service on this host")?;
    ensure!(ui.saw("never expose port 8080 directly"));
    ui.send(b"\x1b")?;
    ui.until("> @ Internet access (HTTPS)")?;
    ui.exit()
}

#[test]
fn coach_session_reschedule_reviews_confirms_and_shows_the_journal() -> Result<()> {
    use crate::coach::{PLANS, WEEK, plans, schedule, workout};
    let f = Fixture::new()?;
    f.init()?;
    let original = workout();
    let entry = |date: &str| {
        json!({"workoutScheduleId":99,"calendarDate":date,"ownerId":5,
            "workout":{"workoutId":777,"workoutName":"Push Day","trainingPlanId":42},"associatedActivityId":null})
        .to_string()
    };
    let g = Server::start(vec![
        ("GET", PLANS, 200, plans()),
        ("GET", WEEK, 200, schedule(&original)),
        (
            "GET",
            "/workout-service/workout/777",
            200,
            original.to_string(),
        ),
        (
            "GET",
            "/workout-service/schedule/99",
            200,
            entry("2026-10-27"),
        ),
        ("PUT", "/workout-service/schedule/99", 200, "{}".into()),
        (
            "GET",
            "/workout-service/schedule/99",
            200,
            entry("2026-11-02"),
        ),
    ])?;
    configure_connect(&f, &g.base)?;
    std::fs::write(
        &f.config,
        std::fs::read_to_string(&f.config)?
            .replace("request_interval_ms = 1000", "request_interval_ms = 1"),
    )?;
    ensure!(
        f.command()
            .args([
                "coach",
                "sync",
                "--start",
                "2026-10-26",
                "--end",
                "2026-11-01"
            ])
            .output()?
            .status
            .success()
    );
    let mut ui = Session::start(&f, 100, 24, false)?;
    ui.until("Search actions")?;
    ui.search("Coach")?;
    ui.until("Sync this week")?;
    ui.search("Choose week")?;
    ui.until("Garmin calendar date")?;
    ui.send(b"2026-10-27\r")?;
    ui.until("Coach week")?;
    ui.search("Push Day")?;
    ui.until("Workout steps")?;
    ui.search("Reschedule")?;
    ui.until("New Garmin calendar date")?;
    ui.send(b"2026-11-02\r")?;
    ui.until("Schedule ID: 99")?;
    confirm(&mut ui)?;
    ui.until("Reschedule result")?;
    ui.until("Moved | schedule 99")?;
    ui.send(b"\x1b")?;
    // The search filter is still applied when the result closes; clear it first.
    ui.until("> Reschedule")?;
    ui.send(b"\x1b")?;
    ui.until("Workout steps")?;
    ui.search("Schedule details")?;
    ui.until("Journaled moves")?;
    ui.send(b"\x1b")?;
    ui.until("> Schedule details")?;
    ui.send(b"\x1b")?;
    ui.until("Workout steps")?;
    ui.send(b"\x1b")?;
    ui.until("moved to 2026-11-02")?;
    ui.exit()?;
    let writes = g
        .writes
        .lock()
        .map_err(|_| anyhow::anyhow!("write mutex"))?
        .clone();
    ensure!(writes.len() == 1 && writes[0]["calendarDate"] == "2026-11-02");
    g.finish()?;
    let shown = f
        .command()
        .args([
            "coach",
            "show",
            "--start",
            "2026-10-26",
            "--end",
            "2026-11-01",
        ])
        .output()?;
    ensure!(shown.status.success());
    let shown: serde_json::Value = serde_json::from_slice(&shown.stdout)?;
    ensure!(shown["reschedules"][0]["state"] == "moved", "{shown}");
    ensure!(shown["snapshot"]["sessions"][0]["date"] == "2026-10-27");
    Ok(())
}
