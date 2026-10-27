//! tests/cases/garmin.rs drives real CLI imports and SQLite persistence against
//! a local HTTP stand-in for Garmin, including retry, authentication and rollback.

use crate::common::Fixture;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    process::Output,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub(super) const KEY: &str = "fixture-api-key-never-real";
const NATIVE_TOKEN: &str = "e30.eyJleHAiOjQxMDI0NDQ4MDB9.fixture";
pub(super) const TOKEN: &str = "fixture-access-token-never-real";
pub(super) const TOKEN_PATH: &str = "/personalapitoken-service/personal-api/personal-api-key/token";
pub(super) const ACTIVITY_PATH: &str =
    "/activitylist-service/activities/search/activities?start=0&limit=2";

type Exchange = (&'static str, &'static str, u16, String);

pub(super) struct Server {
    pub(super) base: String,
    worker: Option<JoinHandle<Result<()>>>,
    pub(super) writes: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
}
impl Server {
    pub(super) fn start(script: Vec<Exchange>) -> Result<Self> {
        Self::start_with_pause(script, None)
    }
    /// Pause one accepted request until the test releases it; the worker owns its deadline.
    pub(super) fn paused(
        script: Vec<Exchange>,
        index: usize,
    ) -> Result<(
        Self,
        std::sync::mpsc::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    )> {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let server = Self::start_with_pause(script, Some((index, entered_tx, release_rx)))?;
        Ok((server, entered_rx, release_tx))
    }
    fn start_with_pause(
        script: Vec<Exchange>,
        pause: Option<(
            usize,
            std::sync::mpsc::Sender<()>,
            std::sync::mpsc::Receiver<()>,
        )>,
    ) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let base = format!("http://{}", listener.local_addr()?);
        let writes = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = writes.clone();
        let worker = thread::spawn(move || {
            for (index, (method, path, status, body)) in script.into_iter().enumerate() {
                let deadline = Instant::now() + Duration::from_secs(10);
                let stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(e) => return Err(e.into()),
                    }
                };
                if let Some((at, entered, release)) = &pause
                    && index == *at
                {
                    entered.send(())?;
                    release.recv_timeout(Duration::from_secs(10))?;
                }
                respond(stream, method, path, status, &body, &recorded)?;
            }
            Ok(())
        });
        Ok(Self {
            base,
            writes,
            worker: Some(worker),
        })
    }
    pub(super) fn finish(mut self) -> Result<()> {
        self.worker
            .take()
            .context("missing server worker")?
            .join()
            .map_err(|_| anyhow::anyhow!("fixture server panicked"))?
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            match worker.join() {
                Ok(Ok(())) => (),
                result => eprintln!("fixture server cleanup: {result:?}"),
            }
        }
    }
}

fn respond(
    mut stream: TcpStream,
    method: &str,
    path: &str,
    status: u16,
    body: &str,
    writes: &std::sync::Mutex<Vec<Value>>,
) -> Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if path.starts_with("/mobile/") {
        assert!(line.starts_with(&format!("{method} {path}?")));
    } else {
        assert_eq!(line.trim(), format!("{method} {path} HTTP/1.1"));
    }
    let mut length = 0;
    let mut authorized = path.starts_with("/mobile/");
    let mut session_cookie = false;
    let mut sso_referer = false;
    let mut language = false;
    loop {
        line.clear();
        reader.read_line(&mut line)?;
        if line == "\r\n" {
            break;
        }
        if line.is_empty() {
            bail!("incomplete HTTP headers");
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("content-length:") {
            length = line.split(':').nth(1).context("header")?.trim().parse()?;
        }
        if lower.starts_with("accept-language:") {
            language = line.trim() == "accept-language: en-US,en;q=0.9";
        }
        if lower.starts_with("referer:") {
            let referer =
                reqwest::Url::parse(line.split_once(':').context("referer header")?.1.trim())?;
            let pairs: std::collections::HashMap<_, _> = referer.query_pairs().collect();
            sso_referer = referer.path() == "/mobile/sso/en_US/sign-in"
                && pairs.get("clientId").map(|v| v.as_ref()) == Some("GCM_IOS_DARK")
                && pairs.get("service").map(|v| v.as_ref())
                    == Some("https://mobile.integration.garmin.com/gcm/ios");
        }
        if lower.starts_with("cookie:") && line.contains("SESSION=fixture") {
            session_cookie = true;
        }
        if lower.starts_with("authorization:") {
            let expected = if path == "/di-oauth2-service/oauth/token" {
                use base64::Engine;
                format!(
                    "authorization: Basic {}",
                    base64::engine::general_purpose::STANDARD
                        .encode("GARMIN_CONNECT_MOBILE_ANDROID_DI_2025Q2:")
                )
            } else if path == TOKEN_PATH {
                format!("authorization: ApiKey {KEY}")
            } else if path.starts_with("/workout-service/")
                || path.starts_with("/trainingplan-service/")
                || path.starts_with("/device-service/")
                || path.starts_with("/metrics-service/")
                || path.starts_with("/activity-service/")
            {
                format!("authorization: Bearer {NATIVE_TOKEN}")
            } else {
                format!("authorization: Bearer {TOKEN}")
            };
            authorized = line.trim() == expected;
        }
    }
    assert!(authorized, "missing expected fixture authentication");
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload)?;
    if method == "POST" && path.starts_with("/mobile/") {
        assert!(
            sso_referer,
            "SSO Referer must include the original client and service query"
        );
        assert!(language, "SSO language header must be retained");
        assert!(
            session_cookie,
            "SSO session cookie must survive the login flow"
        );
        let sent: Value = serde_json::from_slice(&payload)?;
        if path == "/mobile/api/login" {
            assert_eq!(sent["username"], "fixture@example.invalid");
            assert_eq!(sent["password"], "fixture-password");
        } else {
            assert_eq!(sent["mfaMethod"], "email");
            assert_eq!(sent["mfaVerificationCode"], "123456");
        }
    }
    if path == "/di-oauth2-service/oauth/token" {
        let form = reqwest::Url::parse(&format!(
            "http://localhost/?{}",
            String::from_utf8(payload.clone())?
        ))?;
        let pairs: std::collections::HashMap<_, _> = form.query_pairs().collect();
        assert_eq!(
            pairs.get("client_id").map(|v| v.as_ref()),
            Some("GARMIN_CONNECT_MOBILE_ANDROID_DI_2025Q2")
        );
        if pairs.get("grant_type").map(|v| v.as_ref()) == Some("refresh_token") {
            assert_eq!(
                pairs.get("refresh_token").map(|v| v.as_ref()),
                Some("refresh-one")
            );
        } else {
            assert_eq!(
                pairs.get("grant_type").map(|v| v.as_ref()),
                Some("https://connectapi.garmin.com/di-oauth2-service/oauth/grant/service_ticket")
            );
            assert_eq!(
                pairs.get("service_ticket").map(|v| v.as_ref()),
                Some("fixture-ticket")
            );
            assert_eq!(
                pairs.get("service_url").map(|v| v.as_ref()),
                Some("https://mobile.integration.garmin.com/gcm/ios")
            );
        }
    }
    if matches!(method, "POST" | "PUT") && path.starts_with("/workout-service/") {
        writes
            .lock()
            .map_err(|_| anyhow::anyhow!("request recorder poisoned"))?
            .push(serde_json::from_slice(&payload)?);
    }
    if method == "POST" && path == "/device-service/devicemessage/messages" {
        let sent: Value = serde_json::from_slice(&payload)?;
        assert_eq!(sent[0]["deviceId"], 99);
        assert_eq!(sent[0]["metaDataId"], 777);
        assert_eq!(sent[0]["messageUrl"], "workout-service/workout/FIT/777");
    }
    // A numeric 429 body sets Retry-After in seconds; any other 429 asks for 120.
    let retry_after = if status == 429 {
        format!("Retry-After: {}\r\n", body.parse::<u64>().unwrap_or(120))
    } else {
        String::new()
    };
    write!(
        stream,
        "HTTP/1.1 {status} Test\r\n{retry_after}Set-Cookie: SESSION=fixture; Path=/; HttpOnly\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    Ok(())
}

pub(super) fn activity(name: &str) -> Value {
    json!({"activityId":123, "activityName":name,"activityType":{"typeKey":"running"},
        "startTimeGMT":"2026-10-24 08:12:30", "duration":600.25,"distance":1000.5})
}
pub(super) fn configure(fixture: &Fixture, base: &str) -> Result<()> {
    let text = fs::read_to_string(&fixture.config)?
        .replace("https://personal.connectapi.garmin.com", base)
        .replace("retry_base_ms = 500", "retry_base_ms = 1")
        .replace("request_interval_ms = 1000", "request_interval_ms = 1")
        .replace(
            "rate_limit_max_wait_seconds = 120",
            "rate_limit_max_wait_seconds = 1",
        );
    fs::write(&fixture.config, text)?;
    Ok(())
}
fn sync(fixture: &Fixture) -> Result<Output> {
    Ok(fixture
        .command()
        .env("GARMIN_PERSONAL_API_KEY", KEY)
        .args(["garmin", "sync", "--limit", "2"])
        .output()?)
}

#[test]
fn reads_wait_out_rate_limits_and_stop_clearly_when_they_persist() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let token = (
        "POST",
        "/personalapitoken-service/personal-api/personal-api-key/token",
        200,
        "fixture-access-token-never-real".to_string(),
    );
    let page = "/activitylist-service/activities/search/activities?start=0&limit=2";
    let server = Server::start(vec![
        token.clone(),
        ("GET", page, 429, "1".into()),
        ("GET", page, 200, "[]".into()),
        token.clone(),
        ("GET", page, 429, "1".into()),
        ("GET", page, 429, "1".into()),
        token,
        ("GET", page, 429, "600".into()),
    ])?;
    configure(&fixture, &server.base)?;
    let recovered = sync(&fixture)?;
    assert!(
        recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    assert!(String::from_utf8_lossy(&recovered.stderr).contains("Garmin rate limit; waiting 1 s"));
    fs::write(
        &fixture.config,
        fs::read_to_string(&fixture.config)?
            .replace("rate_limit_retries = 3", "rate_limit_retries = 1"),
    )?;
    let stopped = sync(&fixture)?;
    assert!(!stopped.status.success());
    assert!(
        String::from_utf8_lossy(&stopped.stderr)
            .contains("Garmin rate limit (HTTP 429) persisted after 1 waits")
    );
    // A wait longer than the configured cap stops at once instead of sleeping.
    let too_long = sync(&fixture)?;
    assert!(!too_long.status.success());
    assert!(
        String::from_utf8_lossy(&too_long.stderr)
            .contains("asks to wait 600 s, longer than garmin.rate_limit_max_wait_seconds (1)")
    );
    server.finish()
}

#[test]
fn period_sync_pages_through_garmin_and_rejects_activities_outside_the_period() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let page = |start: u32| -> &'static str {
        Box::leak(format!("/activitylist-service/activities/search/activities?start={start}&limit=2&startDate=2026-10-01&endDate=2026-10-25").into_boxed_str())
    };
    let run = |id: u64, at: &str| json!({"activityId":id,"activityName":"Run","activityType":{"typeKey":"running"},"startTimeGMT":at,"duration":60.0});
    let token = (
        "POST",
        "/personalapitoken-service/personal-api/personal-api-key/token",
        200,
        "fixture-access-token-never-real".to_string(),
    );
    let server = Server::start(vec![
        token.clone(),
        (
            "GET",
            page(0),
            200,
            json!([
                run(31, "2026-10-24 08:00:00"),
                run(32, "2026-10-20 08:00:00")
            ])
            .to_string(),
        ),
        (
            "GET",
            page(2),
            200,
            json!([run(33, "2026-10-02 08:00:00")]).to_string(),
        ),
        token,
        (
            "GET",
            page(0),
            200,
            json!([run(34, "2026-09-01 08:00:00")]).to_string(),
        ),
    ])?;
    configure(&fixture, &server.base)?;
    // Two-activity pages make the period take two requests.
    fs::write(
        &fixture.config,
        fs::read_to_string(&fixture.config)?
            .replace("default_page_size = 20", "default_page_size = 2")
            .replace("max_page_size = 100", "max_page_size = 2"),
    )?;
    let period = |f: &Fixture| -> Result<Output> {
        Ok(f.command()
            .env("GARMIN_PERSONAL_API_KEY", KEY)
            .args([
                "garmin",
                "sync",
                "--start",
                "2026-10-01",
                "--end",
                "2026-10-25",
            ])
            .output()?)
    };
    let imported = period(&fixture)?;
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    let summary: Value = serde_json::from_slice(&imported.stdout)?;
    assert_eq!(
        (summary["saved_count"].as_u64(), summary["pages"].as_u64()),
        (Some(3), Some(2))
    );
    assert_eq!(summary["scope"], "period");
    let rejected = period(&fixture)?;
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("outside the requested period"));
    let db = rusqlite::Connection::open(&fixture.settings.database.path)?;
    let count: i64 = db.query_row("SELECT count(*) FROM activities", [], |r| r.get(0))?;
    assert_eq!(count, 3);
    server.finish()
}

#[test]
fn cli_sync_upserts_and_failed_import_preserves_timestamp_and_history() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let invalid = json!([{"activityId":124,"activityName":"Invalid","activityType":{"typeKey":"running"},"startTimeGMT":"2026-10-24 09:00:00","duration":-1}]);
    let server = Server::start(vec![
        ("POST", TOKEN_PATH, 200, TOKEN.into()),
        ("GET", ACTIVITY_PATH, 503, "temporary".into()),
        (
            "GET",
            ACTIVITY_PATH,
            200,
            json!([activity("First")]).to_string(),
        ),
        ("POST", TOKEN_PATH, 200, TOKEN.into()),
        (
            "GET",
            ACTIVITY_PATH,
            200,
            json!([activity("Updated")]).to_string(),
        ),
        ("POST", TOKEN_PATH, 200, TOKEN.into()),
        ("GET", ACTIVITY_PATH, 200, invalid.to_string()),
    ])?;
    configure(&fixture, &server.base)?;
    for _ in 0..2 {
        let output = sync(&fixture)?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let before: Value = serde_json::from_slice(&fixture.command().arg("status").output()?.stdout)?;
    assert_eq!(before["activity_count"], 1);
    assert_eq!(before["garmin_integration"], "previously_synced");
    assert!(before["last_successful_sync_at"].is_string());
    let failed = sync(&fixture)?;
    assert!(!failed.status.success());
    assert!(!String::from_utf8_lossy(&failed.stderr).contains(KEY));
    assert!(!String::from_utf8_lossy(&failed.stderr).contains(TOKEN));
    let after: Value = serde_json::from_slice(&fixture.command().arg("status").output()?.stdout)?;
    assert_eq!(before, after);
    let page: Value = serde_json::from_slice(
        &fixture
            .command()
            .args(["activities", "list"])
            .output()?
            .stdout,
    )?;
    assert_eq!(page["activities"][0]["name"], "Updated");
    assert_eq!(
        page["activities"][0]["started_at"],
        "2026-10-24T08:12:30.000Z"
    );
    server.finish()
}

#[test]
fn authentication_failure_and_rate_limit_stop_without_exposing_response() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let server = Server::start(vec![
        ("POST", TOKEN_PATH, 401, format!("private response {KEY}")),
        ("POST", TOKEN_PATH, 200, TOKEN.into()),
        (
            "GET",
            ACTIVITY_PATH,
            429,
            format!("private response {TOKEN}"),
        ),
    ])?;
    configure(&fixture, &server.base)?;
    for _ in 0..2 {
        let output = sync(&fixture)?;
        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr)?;
        assert!(
            !stderr.contains(KEY)
                && !stderr.contains(TOKEN)
                && !stderr.contains("private response")
        );
    }
    let status: Value = serde_json::from_slice(&fixture.command().arg("status").output()?.stdout)?;
    assert_eq!(status["activity_count"], 0);
    assert!(status["last_successful_sync_at"].is_null());
    server.finish()
}

#[test]
fn migration_preserves_v1_rows_and_write_failure_rolls_back_batch() -> Result<()> {
    use garmin_pace::history::{
        Activity, ActivitySource, HistoryError, HistoryService, SqliteHistory,
    };
    let fixture = Fixture::new()?;
    fs::create_dir_all(fixture.settings.database.path.parent().context("parent")?)?;
    let db = rusqlite::Connection::open(&fixture.settings.database.path)?;
    db.execute_batch(include_str!("../../migrations/0001_history.sql"))?;
    db.pragma_update(None, "user_version", 1)?;
    // Existing profiles must explicitly repair legacy filesystem permissions before migration.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            fixture.settings.database.path.parent().context("parent")?,
            fs::Permissions::from_mode(0o700),
        )?;
        fs::set_permissions(
            &fixture.settings.database.path,
            fs::Permissions::from_mode(0o600),
        )?;
    }
    fixture.seed()?;
    fixture.init()?;
    let history = HistoryService::new(
        std::sync::Arc::new(SqliteHistory::open(fixture.settings.database)?),
        100,
    );
    struct Source;
    impl ActivitySource for Source {
        fn recent(&self, _: usize) -> Result<Vec<Activity>, HistoryError> {
            Ok(vec![
                Activity {
                    garmin_id: "new".into(),
                    name: "First".into(),
                    sport: "running".into(),
                    started_at: "2026-10-24T00:00:00.000Z".into(),
                    duration_seconds: 1.0,
                    distance_meters: None,
                },
                Activity {
                    garmin_id: "invalid".into(),
                    name: "Second".into(),
                    sport: "running".into(),
                    started_at: "2026-10-24T00:00:00.000Z".into(),
                    duration_seconds: -1.0,
                    distance_meters: None,
                },
            ])
        }
    }
    assert!(
        history
            .sync(&Source, 2, || "2026-10-24T00:00:00.000Z".into())
            .is_err()
    );
    assert_eq!(history.summary()?.activity_count, 2);
    assert!(history.summary()?.last_sync.is_none());
    Ok(())
}

pub(super) fn configure_connect(fixture: &Fixture, base: &str) -> Result<()> {
    fs::write(
        &fixture.config,
        fs::read_to_string(&fixture.config)?.replace("https://connectapi.garmin.com", base),
    )?;
    let settings = garmin_pace::config::Settings::load(&fixture.config)?;
    fs::create_dir_all(
        settings
            .garmin_auth
            .session_path
            .parent()
            .context("session parent")?,
    )?;
    fs::write(&settings.garmin_auth.session_path,json!({"version":1,"access_token":NATIVE_TOKEN,"refresh_token":"fixture-refresh",
        "expires_at":chrono::Utc::now().timestamp()+3600,"client_id":settings.garmin_auth.oauth_client_id,
        "token_url":settings.garmin_auth.token_url,"api_base_url":base}).to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            settings
                .garmin_auth
                .session_path
                .parent()
                .context("session parent")?,
            fs::Permissions::from_mode(0o700),
        )?;
        fs::set_permissions(
            &settings.garmin_auth.session_path,
            fs::Permissions::from_mode(0o600),
        )?;
    }
    Ok(())
}

#[test]
fn native_login_mfa_refresh_rotation_and_logout_use_private_session() -> Result<()> {
    use garmin_pace::{
        config::Settings,
        garmin::{login, logout, session_expiry},
    };
    let fixture = Fixture::new()?;
    let devices = json!([{"unitId":99,"productDisplayName":"fenix 8"}]).to_string();
    let token =
        |refresh: &str| json!({"access_token":NATIVE_TOKEN,"refresh_token":refresh}).to_string();
    let server=Server::start(vec![
        ("GET","/mobile/sso/en_US/sign-in",200,"sign in".into()),
        ("POST","/mobile/api/login",200,json!({"responseStatus":{"type":"MFA_REQUIRED"},"customerMfaInfo":{"mfaLastMethodUsed":"email"}}).to_string()),
        ("POST","/mobile/api/mfa/verifyCode",200,json!({"responseStatus":{"type":"SUCCESSFUL"},"serviceTicketId":"fixture-ticket"}).to_string()),
        ("POST","/di-oauth2-service/oauth/token",200,token("refresh-one")),
        ("GET","/device-service/deviceregistration/devices",200,devices.clone()),
        ("POST","/di-oauth2-service/oauth/token",200,token("refresh-two")),
        ("GET","/device-service/deviceregistration/devices",200,devices.clone()),
        ("GET","/device-service/deviceregistration/devices",200,devices),
    ])?;
    let text = fs::read_to_string(&fixture.config)?
        .replace("https://sso.garmin.com", &server.base)
        .replace("https://diauth.garmin.com", &server.base)
        .replace("https://connectapi.garmin.com", &server.base)
        .replace("login_delay_ms = 2000", "login_delay_ms = 0");
    fs::write(&fixture.config, text)?;
    let settings = Settings::load(&fixture.config)?;
    login(
        &settings.garmin_auth,
        &settings.garmin,
        "fixture@example.invalid",
        "fixture-password",
        || Ok("123456".into()),
    )?;
    assert!(session_expiry(&settings.garmin_auth)? > chrono::Utc::now().timestamp());
    let path = &settings.garmin_auth.session_path;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(path)?.permissions().mode() & 0o777, 0o600);
    }
    let mut stored: Value = serde_json::from_slice(&fs::read(path)?)?;
    assert!(!stored.to_string().contains("fixture-password"));
    stored["expires_at"] = json!(0);
    fs::write(path, stored.to_string())?;
    for _ in 0..2 {
        let output = fixture.command().args(["garmin", "devices"]).output()?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains(NATIVE_TOKEN));
    }
    let stored: Value = serde_json::from_slice(&fs::read(path)?)?;
    assert_eq!(stored["refresh_token"], "refresh-two");
    logout(&settings.garmin_auth)?;
    assert!(!path.exists());
    server.finish()
}

#[test]
fn native_login_rate_limit_names_stage_and_stops_without_saving() -> Result<()> {
    let fixture = Fixture::new()?;
    let server = Server::start(vec![(
        "GET",
        "/mobile/sso/en_US/sign-in",
        429,
        "private response".into(),
    )])?;
    let text = fs::read_to_string(&fixture.config)?.replace("https://sso.garmin.com", &server.base);
    fs::write(&fixture.config, text)?;
    let settings = garmin_pace::config::Settings::load(&fixture.config)?;
    let result = garmin_pace::garmin::login(
        &settings.garmin_auth,
        &settings.garmin,
        "fixture@example.invalid",
        "fixture-password",
        || bail!("MFA must not be requested"),
    );
    let message = result.err().context("login must stop")?.to_string();
    assert!(
        message.contains("HTTP 429 during SSO sign-in page"),
        "{message}"
    );
    assert!(message.contains("retry after 120 seconds"), "{message}");
    assert!(!message.contains("private response"));
    assert!(!settings.garmin_auth.session_path.exists());
    server.finish()
}
