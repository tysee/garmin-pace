//! tests/cases/onboarding.rs drives onboarding through the real terminal and SQLite.
//! It verifies self-check marks, explicit preparation, automatic and hidden Garmin credentials,
//! key delivery and HTTP handover.

use super::Session;
use crate::common::Fixture;
use anyhow::{Context, Result};
use std::{
    fs,
    time::{Duration, Instant},
};

fn back(session: &mut Session) -> Result<()> {
    session.send(b"\r")?;
    session.until("Pace / Onboarding")?;
    session.send(b"\x1b")?;
    session.until("Search actions")
}

#[test]
fn onboarding_prepares_profile_and_issues_private_access_key() -> Result<()> {
    let fixture = Fixture::new()?;
    std::fs::remove_file(&fixture.config)?;
    let mut session = Session::start(&fixture, 100, 28, false)?;
    session.until("Pace / Actions")?;
    session.search("onboarding")?;
    session.until("Pace / Onboarding")?;
    session.until("× 1. Prepare profile")?;
    session.until("0/9 ready")?;
    assert!(!fixture.config.exists());
    session.search("prepare profile")?;
    session.until("Profile ready.")?;
    assert!(fixture.settings.database.path.exists());
    back(&mut session)?;
    session.until("√ 1. Prepare profile")?;
    session.until("No MCP users registered yet")?;
    session.search("register mcp user")?;
    session.until("Username")?;
    session.send(b"alice\r")?;
    session.until("Review action")?;
    session.send(b"\r")?;
    session.until("> Cancel")?;
    session.send(b"\x1b[B\r")?;
    session.until("Registered user: alice")?;
    back(&mut session)?;
    session.until("√ 7. Register MCP user")?;
    session.until("No active access key")?;
    session.search("issue access key")?;
    session.until("× alice")?;
    session.until("no active key")?;
    session.send(b"\r")?;
    session.until("Key lifetime")?;
    session.until("> 30 days")?;
    session.send(b"\r")?;
    session.until("Lifetime: 30 days")?;
    session.until("Issue a new key for alice?")?;
    session.send(b"\r")?;
    session.until("> Cancel")?;
    session.send(b"\x1b[B\r")?;
    session.until("Copy it now: it is shown only once.")?;
    let token = session
        .received
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .find(|word| word.starts_with("pace_") && word.len() == 48)
        .context("revealed key")?
        .to_string();
    assert!(!fixture.directory.path().join("alice.key").exists());
    back(&mut session)?;
    session.until("√ 8. Issue access key")?;
    session.until("√ 10. Start MCP")?;
    assert!(!session.received.contains(token.trim()));
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    fs::write(
        &fixture.config,
        fs::read_to_string(&fixture.config)?.replace("127.0.0.1:8080", &address.to_string()),
    )?;
    session.search("start mcp")?;
    session.until("Review action")?;
    session.send(b"\r")?;
    session.until("> Cancel")?;
    session.send(b"\r")?;
    session.until("Pace / Onboarding")?;
    assert!(std::net::TcpStream::connect(address).is_err());
    session.send(b"\r")?;
    session.until("Review action")?;
    session.send(b"\r")?;
    session.until("> Cancel")?;
    session.send(b"\x1b[B\r")?;
    session.until("HTTP MCP server listening")?;
    assert!(!session.received.contains(token.trim()));
    assert!(!session.received.contains("\"level\":\"ERROR\""));
    let restored = rustix::termios::tcgetattr(&session.slave)?;
    assert_eq!(restored.local_modes, session.original.local_modes);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let base = format!("http://{address}/api/v1");
    assert_eq!(client.get(format!("{base}/health")).send()?.status(), 401);
    assert_eq!(
        client
            .get(format!("{base}/health"))
            .bearer_auth(token.trim())
            .send()?
            .status(),
        200
    );
    let reply: serde_json::Value = client.post(format!("{base}/mcp"))
        .bearer_auth(token.trim()).header("Accept", "application/json, text/event-stream")
        .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_status","arguments":{}}}))
        .send()?.error_for_status()?.json()?;
    assert_eq!(reply["result"]["structuredContent"]["activity_count"], 0);
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(session.child.id().try_into()?).context("child pid")?,
        rustix::process::Signal::TERM,
    )?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = session.child.try_wait()? {
            assert!(status.success());
            break;
        }
        anyhow::ensure!(Instant::now() < deadline, "HTTP handover did not shut down");
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

#[test]
fn onboarding_hides_pasted_credentials_and_imports_from_garmin() -> Result<()> {
    use crate::garmin::{Server, configure};
    let fixture = Fixture::new()?;
    fixture.init()?;
    let server = Server::start(vec![
        ("POST", "/personalapitoken-service/personal-api/personal-api-key/token", 200, "fixture-access-token-never-real".into()),
        ("GET", "/activitylist-service/activities/search/activities?start=0&limit=1", 200, "[]".into()),
        ("POST", "/personalapitoken-service/personal-api/personal-api-key/token", 200, "fixture-access-token-never-real".into()),
        ("GET", "/activitylist-service/activities/search/activities?start=0&limit=2", 200,
            serde_json::json!([{"activityId": 777, "activityName":"Wizard imported run", "activityType":{"typeKey":"running"}, "startTimeGMT":"2026-10-24 08:12:30", "duration":600.0,"distance":1000.0}]).to_string()),
    ])?;
    configure(&fixture, &server.base)?;
    let mut session = Session::start_args(&fixture, 100, 28, &["setup", "--compact"])?;
    session.until("Pace / Onboarding")?;
    session.until("× 2. Personal API key")?;
    session.search("personal api key")?;
    session.until("Get Garmin key")?;
    session.send(b"\r")?;
    session.until("Nothing received yet")?;
    // An empty hidden field cannot be submitted; the paste below still lands in it.
    session.send(b"\r")?;
    // Surrounding copy artifacts are trimmed; a pasted newline cannot submit the field.
    session.send(b"\x1b[200~ fixture-api-key-never-real \n\x1b[201~")?;
    session.until("28 characters received")?;
    let path = garmin_pace::secrets::profile_key_path(&fixture.settings.database.path)?;
    assert!(!path.exists());
    session.send(b"\r")?;
    session.until("Garmin accepted the key")?;
    assert!(!session.received.contains("fixture-api-key-never-real"));
    assert_eq!(
        garmin_pace::secrets::Secrets::load(Some(&path))?.require("GARMIN_PERSONAL_API_KEY")?,
        "fixture-api-key-never-real"
    );
    back(&mut session)?;
    // Saved and checked once, but no import has used it yet.
    session.until("~ 2. Personal API key")?;
    session.search("import activities")?;
    session.until("> Recent activities")?;
    session.send(b"\r")?;
    session.until("How many recent activities to import (1-100)? Enter = 20")?;
    session.send(b"2\r")?;
    session.until("Imported 1 activities")?;
    assert!(!session.received.contains("fixture-api-key-never-real"));
    back(&mut session)?;
    session.until("√ 4. Import activities")?;
    session.until("√ 2. Personal API key")?;
    server.finish()?;
    let db = rusqlite::Connection::open(&fixture.settings.database.path)?;
    let name: String = db.query_row(
        "SELECT name FROM activities WHERE garmin_id='777'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(name, "Wizard imported run");
    session.exit()
}

#[test]
fn private_credential_creation_round_trips_and_never_overwrites() -> Result<()> {
    use garmin_pace::secrets::Secrets;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("private.env");
    let key = r#"fixture-'quote"-backslash\-dollar$HOME-${HOME}"#;
    Secrets::save_personal_key(&path, key)?;
    assert_eq!(
        Secrets::load(Some(&path))?.require("GARMIN_PERSONAL_API_KEY")?,
        key
    );
    assert!(Secrets::save_personal_key(&path, "replacement").is_err());
    assert_eq!(
        Secrets::load(Some(&path))?.require("GARMIN_PERSONAL_API_KEY")?,
        key
    );
    let invalid = directory.path().join("invalid.env");
    assert!(Secrets::save_personal_key(&invalid, "value\nINJECTED=1").is_err());
    assert!(!invalid.exists());
    Ok(())
}

#[test]
fn setup_can_be_cancelled_before_any_changes_and_reopened() -> Result<()> {
    let fixture = Fixture::new()?;
    fs::remove_file(&fixture.config)?;
    let mut session = Session::start_args(&fixture, 40, 15, &["setup", "--compact"])?;
    session.until("Pace / Onboarding")?;
    session.exit()?;
    assert!(!fixture.config.exists());
    assert!(!fixture.settings.database.path.exists());
    let mut session = Session::start_args(&fixture, 100, 28, &["setup"])?;
    session.until("Pace / Onboarding")?;
    session.until("Config not created yet")?;
    session.search("prepare profile")?;
    session.until("Profile ready.")?;
    back(&mut session)?;
    session.search("personal api key")?;
    session.until("Get Garmin key")?;
    session.send(b"\r")?;
    session.until("Paste Personal API key")?;
    session.send(b"fixture-cancelled-secret\x1b")?;
    session.until("Pace / Onboarding")?;
    let path = garmin_pace::secrets::profile_key_path(&fixture.settings.database.path)?;
    assert!(!session.received.contains("fixture-cancelled-secret"));
    assert!(!path.exists());
    session.exit()
}

#[test]
fn onboarding_finds_the_saved_profile_key_without_prompting() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let path = garmin_pace::secrets::profile_key_path(&fixture.settings.database.path)?;
    garmin_pace::secrets::Secrets::save_personal_key(&path, "fixture-saved-key-never-real")?;
    let mut session = Session::start_args(&fixture, 100, 28, &["setup"])?;
    session.until("Pace / Onboarding")?;
    session.until("~ 2. Personal API key")?;
    session.search("personal api key")?;
    session.until("Personal API key found.")?;
    session.until(&format!("To replace it, delete {}", path.display()))?;
    assert!(!session.received.contains("Paste"));
    assert!(!session.received.contains("fixture-saved-key-never-real"));
    session.exit()
}

#[test]
fn onboarding_imports_a_long_health_range_in_windows_with_progress() -> Result<()> {
    use crate::{
        garmin::{Server, configure, configure_connect},
        health::day_script,
    };
    let token = (
        "POST",
        "/personalapitoken-service/personal-api/personal-api-key/token",
        200,
        "fixture-access-token-never-real".to_string(),
    );
    let mut script = vec![token.clone()];
    // Garmin rate-limits the first read once; the import waits and continues.
    script.push((
        "GET",
        "/usersummary-service/usersummary/daily?calendarDate=2026-10-22",
        429,
        "1".to_string(),
    ));
    script.extend(day_script("2026-10-22"));
    script.extend(day_script("2026-10-23"));
    script.push(token);
    script.extend(day_script("2026-10-24"));
    let server = Server::start(script)?;
    let fixture = Fixture::new()?;
    fixture.init()?;
    configure(&fixture, &server.base)?;
    configure_connect(&fixture, &server.base)?;
    // Two-day windows make a three-day range take two parts.
    fs::write(
        &fixture.config,
        fs::read_to_string(&fixture.config)?.replace("max_days = 31", "max_days = 2"),
    )?;
    let path = garmin_pace::secrets::profile_key_path(&fixture.settings.database.path)?;
    garmin_pace::secrets::Secrets::save_personal_key(&path, "fixture-api-key-never-real")?;
    let mut session = Session::start_args(&fixture, 100, 28, &["setup"])?;
    session.until("No health days saved yet")?;
    session.search("import health")?;
    session.until("END defaults to today")?;
    session.send(b"2026-10-22 2026-10-24\r")?;
    session.until("Import 3 days of health data?")?;
    session.until("Pace spaces requests 1 ms apart")?;
    session.send(b"\r")?;
    session.until("> Cancel")?;
    session.send(b"\x1b[B\r")?;
    session.until("Garmin rate limit: waiting 1 s as Garmin asked (retry 1/3)")?;
    session.until("Health import complete: 3 days saved from 2026-10-22 to 2026-10-24.")?;
    server.finish()?;
    back(&mut session)?;
    session.until("√ 5. Import health")?;
    session.until("3 days saved; latest 2026-10-24")?;
    session.exit()
}

#[test]
fn mcp_users_screen_lists_users_and_key_state() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    for args in [
        vec!["users", "init"],
        vec!["users", "add", "alice", "--profile"],
    ] {
        let mut command = fixture.command();
        command.args(&args);
        if args.len() == 4 {
            command.arg(&fixture.config);
        }
        let output = command.output()?;
        anyhow::ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let mut session = Session::start(&fixture, 100, 28, false)?;
    session.until("Pace / Actions")?;
    session.search("mcp users")?;
    session.until("> alice")?;
    session.until("× no active key")?;
    session.send(b"\r")?;
    session.until("alice: keys")?;
    session.until("Issue keys in Onboarding step 8")?;
    session.exit()
}

#[test]
fn start_mcp_recognizes_a_server_that_already_runs_without_a_terminal() -> Result<()> {
    use crate::common::{Process, register_user};
    let fixture = Fixture::new()?;
    fixture.init()?;
    let (token, _) = register_user(&fixture, &fixture, "owner")?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    fs::write(
        &fixture.config,
        fs::read_to_string(&fixture.config)?.replace("127.0.0.1:8080", &address.to_string()),
    )?;
    drop(listener);
    let server = Process(Some(
        fixture
            .command()
            .args(["serve", "--transport", "http"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?,
    ));
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    crate::mcp::wait_for_http(&client, &format!("http://{address}/api/v1"), &token)?;
    let mut session = Session::start_args(&fixture, 100, 28, &["setup"])?;
    session.until("Pace / Onboarding")?;
    session.search("start mcp")?;
    session.until("Another Pace process already serves MCP")?;
    session.exit()?;
    server.stop()?;
    Ok(())
}

#[test]
fn onboarding_imports_activities_for_a_date_period() -> Result<()> {
    use crate::garmin::{Server, configure};
    let server = Server::start(vec![
        ("POST", "/personalapitoken-service/personal-api/personal-api-key/token", 200, "fixture-access-token-never-real".into()),
        ("GET", "/activitylist-service/activities/search/activities?start=0&limit=100&startDate=2026-10-20&endDate=2026-10-24", 200,
            serde_json::json!([{"activityId": 901, "activityName":"Period run", "activityType":{"typeKey":"running"}, "startTimeGMT":"2026-10-22 07:00:00", "duration":900.0}]).to_string()),
    ])?;
    let fixture = Fixture::new()?;
    fixture.init()?;
    configure(&fixture, &server.base)?;
    let path = garmin_pace::secrets::profile_key_path(&fixture.settings.database.path)?;
    garmin_pace::secrets::Secrets::save_personal_key(&path, "fixture-api-key-never-real")?;
    let mut session = Session::start_args(&fixture, 100, 28, &["setup"])?;
    session.until("Pace / Onboarding")?;
    session.search("import activities")?;
    session.until("Activities in a period")?;
    session.send(b"\x1b[B\r")?;
    session.until("END defaults to today")?;
    session.send(b"2026-10-20 2026-10-24\r")?;
    session.until("Imported 1 activities from 2026-10-20 to 2026-10-24 (1 pages).")?;
    server.finish()?;
    back(&mut session)?;
    session.until("√ 4. Import activities")?;
    session.exit()
}

#[test]
fn status_marks_an_expired_token_and_an_unfinished_full_sync_as_partial() -> Result<()> {
    use garmin_pace::sync::{Period, SqliteSyncJournal, Step, StepSource, SyncService, Window};
    use std::ops::ControlFlow;
    struct Unavailable(Step);
    impl StepSource for Unavailable {
        fn step(&self) -> Step {
            self.0
        }
        fn window_days(&self) -> usize {
            1
        }
        fn unavailable(&self) -> Option<String> {
            Some("fixture reason".into())
        }
        fn import(&self, _: Window) -> Result<usize> {
            anyhow::bail!("unavailable steps are never imported")
        }
    }
    let fixture = Fixture::new()?;
    fixture.init()?;
    let session_path = &fixture.settings.garmin_auth.session_path;
    let parent = session_path.parent().context("session parent")?;
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let auth = &fixture.settings.garmin_auth;
    fs::write(
        session_path,
        serde_json::json!({"version":1,"access_token":"fixture-expired-token","refresh_token":"fixture-refresh",
            "expires_at":0,"client_id":auth.oauth_client_id,"token_url":auth.token_url,
            "api_base_url":"https://connectapi.garmin.com"})
        .to_string(),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(session_path, fs::Permissions::from_mode(0o600))?;
    }
    let journal = SqliteSyncJournal::open(fixture.settings.database.clone())?;
    let sources = Step::ORDER.map(Unavailable);
    let sources: Vec<&dyn StepSource> = sources.iter().map(|s| s as &dyn StepSource).collect();
    SyncService::new(&journal).start(
        Period::new("2026-10-01".parse()?, "2026-10-01".parse()?)?,
        &sources,
        &|| "2026-10-02T00:00:00.000Z".into(),
        &mut |_| ControlFlow::Continue(()),
    )?;
    let mut session = Session::start_args(&fixture, 120, 30, &[])?;
    session.until("Pace / Actions")?;
    session.search("Status")?;
    session.until("Pace / Status")?;
    session.until("~ Connect login")?;
    session.until("access token expired, renews on the next Garmin request")?;
    session.until("~ Full sync")?;
    session.until("2026-10-01 partial; Activities: fixture reason")?;
    session.exit()
}
