//! tests/cases/quick_installation.rs drives the installer's sequential terminal flow.
//! It checks cancellation, preserved profiles, private delivery and actual HTTP readiness.

use super::Session;
use crate::common::{Fixture, Process};
use anyhow::{Context, Result, ensure};
use garmin_pace::{
    access::{AccessService, SqliteAccess, User},
    installation,
};
use std::{
    fs,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

fn quick(profile: &Fixture, server: &Fixture, handoff: &std::path::Path) -> Result<Session> {
    fs::set_permissions(profile.directory.path(), fs::Permissions::from_mode(0o700))?;
    Session::start_args(
        profile,
        100,
        28,
        &[
            "setup",
            "--quick",
            "--user",
            "owner",
            "--server-config",
            server.config.to_str().context("server path")?,
            "--key-handoff",
            handoff.to_str().context("handoff path")?,
        ],
    )
}

fn finish(session: &mut Session, success: bool) -> Result<()> {
    session.until("\x1b[?1049l")?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = session.child.try_wait()? {
            assert_eq!(status.success(), success);
            break;
        }
        ensure!(Instant::now() < deadline, "wizard did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        rustix::termios::tcgetattr(&session.slave)?.local_modes,
        session.original.local_modes
    );
    Ok(())
}

fn skip_garmin(session: &mut Session) -> Result<()> {
    session.until("Pace / Easy setup")?;
    session.send(b"\r")?;
    session.until("Connect Garmin now")?;
    session.send(b"\x1b[B\r")?;
    session.until("Profile configured")?;
    session.send(b"\r")?;
    finish(session, true)
}

#[test]
fn quick_setup_preserves_profiles_and_checks_real_http_access() -> Result<()> {
    let server = Fixture::new()?;
    let profile = Fixture::new()?;
    profile.init()?;
    profile.seed()?;
    let original_config = fs::read(&profile.config)?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    fs::write(
        &server.config,
        fs::read_to_string(&server.config)?.replace("127.0.0.1:8080", &address.to_string()),
    )?;
    let handoff = profile.directory.path().join("handoff.key");
    let mut tokens = Vec::new();
    for _ in 0..2 {
        fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&handoff)?;
        let mut session = quick(&profile, &server, &handoff)?;
        skip_garmin(&mut session)?;
        let token = fs::read_to_string(&handoff)?;
        assert!(!session.received.contains(token.trim()));
        assert_eq!(fs::metadata(&handoff)?.permissions().mode() & 0o077, 0);
        tokens.push(token);
        fs::remove_file(&handoff)?;
    }
    assert_ne!(tokens[0], tokens[1]);
    assert_eq!(fs::read(&profile.config)?, original_config);
    let db = rusqlite::Connection::open(&profile.settings.database.path)?;
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM activities", [], |row| row
            .get::<_, i64>(0))?,
        2
    );
    let service = Process(Some(
        server
            .command()
            .args(["serve", "--transport", "http"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?,
    ));
    let check = server
        .command()
        .arg("installation-check")
        .arg("--profile")
        .arg(&profile.config)
        .output()?;
    ensure!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    for token in &tokens {
        let reply: serde_json::Value = client.post(format!("http://{address}/api/v1/mcp"))
            .bearer_auth(token.trim()).header("Accept", "application/json, text/event-stream")
            .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_status","arguments":{}}}))
            .send()?.error_for_status()?.json()?;
        assert_eq!(reply["result"]["structuredContent"]["activity_count"], 2);
    }
    let disabled = server
        .command()
        .args(["users", "disable", "owner"])
        .output()?;
    assert!(disabled.status.success());
    let check = server
        .command()
        .arg("installation-check")
        .arg("--profile")
        .arg(&profile.config)
        .output()?;
    assert!(!check.status.success());
    let output = service.stop()?;
    let logs = String::from_utf8(output.stderr)?;
    assert!(!logs.contains("\"level\":\"ERROR\""), "{logs}");
    for token in tokens {
        assert!(!logs.contains(token.trim()));
    }
    Ok(())
}

#[test]
fn quick_setup_cancel_leaves_an_uninitialized_profile_untouched() -> Result<()> {
    let server = Fixture::new()?;
    let profile = Fixture::new()?;
    fs::remove_file(&profile.config)?;
    let handoff = profile.directory.path().join("absent.key");
    let mut session = quick(&profile, &server, &handoff)?;
    session.until("Easy setup")?;
    session.send(b"\x1b")?;
    finish(&mut session, false)?;
    assert!(!profile.config.exists());
    assert!(!profile.settings.database.path.exists());
    assert!(!server.settings.access.registry_path.exists());
    assert!(!handoff.exists());
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&handoff)?;
    let mut resumed = quick(&profile, &server, &handoff)?;
    skip_garmin(&mut resumed)?;
    assert!(profile.config.exists());
    assert!(profile.settings.database.path.exists());
    assert!(handoff.exists());
    Ok(())
}

#[test]
fn resumed_registration_refuses_a_different_or_disabled_profile() -> Result<()> {
    let fixture = Fixture::new()?;
    SqliteAccess::initialize(&fixture.settings.registry_database())?;
    let registry = SqliteAccess::open(fixture.settings.registry_database())?;
    let access = AccessService::new(Arc::new(registry));
    let mut user = User {
        name: "owner".into(),
        database_path: fixture.directory.path().join("profile.sqlite3"),
        session_path: fixture.directory.path().join("session.json"),
        enabled: true,
        garmin_writes: false,
    };
    access.add_or_match(&user)?;
    access.add_or_match(&user)?;
    user.session_path = fixture.directory.path().join("another-session.json");
    assert!(access.add_or_match(&user).is_err());
    user.session_path = fixture.directory.path().join("session.json");
    access.set_enabled("owner", false)?;
    assert!(access.add_or_match(&user).is_err());
    assert!(!access.users()?[0].enabled);
    Ok(())
}

#[test]
fn private_key_handoff_refuses_overwrite_links_and_public_files() -> Result<()> {
    let fixture = Fixture::new()?;
    let key = garmin_pace::access::NewKey::generate()?;
    let source = fixture.directory.path().join("source.key");
    key.write(&source)?;
    let destination = fixture.directory.path().join("destination.key");
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&destination)?;
    installation::handoff_key(&source, &destination)?;
    assert_eq!(fs::read(&source)?, fs::read(&destination)?);
    assert!(installation::handoff_key(&source, &destination).is_err());
    let link = fixture.directory.path().join("link.key");
    std::os::unix::fs::symlink(&destination, &link)?;
    assert!(installation::handoff_key(&source, &link).is_err());
    fs::write(&destination, "")?;
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o644))?;
    assert!(installation::handoff_key(&source, &destination).is_err());
    assert_eq!(fs::read(&destination)?, b"");
    Ok(())
}

#[test]
fn quick_setup_checks_garmin_and_downloads_without_requesting_paths() -> Result<()> {
    use crate::garmin::{Server, configure};
    let server = Fixture::new()?;
    let profile = Fixture::new()?;
    let token_path = "/personalapitoken-service/personal-api/personal-api-key/token";
    let garmin = Server::start(vec![
        ("POST", token_path, 200, "fixture-access-token-never-real".into()),
        ("GET", "/activitylist-service/activities/search/activities?start=0&limit=1", 200, "[]".into()),
        ("POST", token_path, 200, "fixture-access-token-never-real".into()),
        ("GET", "/activitylist-service/activities/search/activities?start=0&limit=20", 200,
            serde_json::json!([{"activityId":888,"activityName":"Installer run","activityType":{"typeKey":"running"},"startTimeGMT":"2026-10-24 08:00:00","duration":60.0}]).to_string()),
    ])?;
    configure(&profile, &garmin.base)?;
    let handoff = profile.directory.path().join("handoff.key");
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&handoff)?;
    let mut session = quick(&profile, &server, &handoff)?;
    session.until("Easy setup")?;
    session.send(b"\r")?;
    session.until("Connect Garmin now")?;
    session.send(b"\r")?;
    session.until("Get Garmin key")?;
    session.send(b"\r")?;
    session.until("Paste Personal API key")?;
    session.send(b"fixture-api-key-never-real\r")?;
    session.until("Load recent activities")?;
    assert!(!session.received.contains("fixture-api-key-never-real"));
    session.send(b"\r")?;
    session.until("Activities saved")?;
    session.send(b"\r")?;
    session.until("Sign in to Garmin")?;
    session.send(b"\x1b[B\r")?;
    session.until("Profile configured")?;
    session.send(b"\r")?;
    finish(&mut session, true)?;
    garmin.finish()?;
    let saved = garmin_pace::secrets::profile_key_path(&profile.settings.database.path)?;
    assert_eq!(
        garmin_pace::secrets::Secrets::load(Some(&saved))?.require("GARMIN_PERSONAL_API_KEY")?,
        "fixture-api-key-never-real"
    );
    let db = rusqlite::Connection::open(&profile.settings.database.path)?;
    let name: String = db.query_row(
        "SELECT name FROM activities WHERE garmin_id='888'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(name, "Installer run");
    Ok(())
}
