//! tests/cases/mcp_garmin.rs runs the granted Garmin write tools against the real HTTP and
//! stdio MCP processes with a scripted Garmin mock, and checks the registry grant migration.

use crate::{
    benchmarks::{ACTIVITIES, CATALOG, SETS, activity, invoke, proposal, remote, sets, window},
    common::{Fixture, Process, register_user},
    garmin::{Server, TOKEN, TOKEN_PATH, configure, configure_connect},
    mcp::{rpc, wait_for_http},
    workouts::{remote_workout, seed_draft},
};
use anyhow::{Context, Result, ensure};
use garmin_pace::workouts::benchmarks::{BenchmarkService, SqliteBenchmarks};
use reqwest::blocking::Client;
use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::{Value, json};
use std::{fs, net::TcpListener, process::Stdio, sync::Arc, time::Duration};

/// Tools that may contact Garmin; everything else stays local.
pub(super) const GARMIN_TOOLS: [&str; 13] = [
    "sync_coach_v1",
    "sync_activities_v1",
    "sync_health_v1",
    "sync_strength_v1",
    "sync_activity_sets_v1",
    "sync_workouts_v1",
    "refresh_workout_v1",
    "list_garmin_devices_v1",
    "save_workout_draft_to_garmin_v1",
    "reconcile_workout_draft_v1",
    "send_workout_to_device_v1",
    "save_benchmark_draft_to_garmin_v1",
    "reconcile_benchmark_draft_v1",
];
const REQUEST: &str = "Save my tempo session to Garmin and send it to the watch";

fn speed_up(fixture: &Fixture) -> Result<()> {
    fs::write(
        &fixture.config,
        fs::read_to_string(&fixture.config)?
            .replace("request_interval_ms = 1000", "request_interval_ms = 1"),
    )?;
    Ok(())
}
fn call(
    client: &Client,
    base: &str,
    token: &str,
    id: u64,
    name: &str,
    args: Value,
) -> Result<Value> {
    rpc(
        client,
        base,
        token,
        json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}}),
    )
}
fn users(fixture: &Fixture, args: &[&str]) -> Result<Value> {
    let output = fixture.command().arg("users").args(args).output()?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

#[test]
fn http_garmin_writes_require_grant_journal_the_request_and_report_refusals() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let devices = json!([{"unitId":99,"productDisplayName":"fenix 8"}]).to_string();
    let saved = remote_workout("Tempo session", 1200.0).to_string();
    let server = Server::start(vec![
        ("POST", "/workout-service/workout", 400, "{}".into()),
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
    // The session directory must exist before registration canonicalizes it.
    configure_connect(&fixture, &server.base)?;
    speed_up(&fixture)?;
    seed_draft(&fixture)?;
    let (token, _) = register_user(&fixture, &fixture, "owner")?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    fs::write(
        &fixture.config,
        fs::read_to_string(&fixture.config)?.replace("127.0.0.1:8080", &address.to_string()),
    )?;
    drop(listener);
    let child = fixture
        .command()
        .args(["serve", "--transport", "http"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let process = Process(Some(child));
    let client = Client::builder().timeout(Duration::from_secs(30)).build()?;
    let base = format!("http://{address}/api/v1");
    wait_for_http(&client, &base, &token)?;
    let save = json!({"id":1,"revision":1,"user_request":REQUEST});

    // Without the grant nothing reaches Garmin; the mock script is untouched.
    let denied = call(
        &client,
        &base,
        &token,
        1,
        "save_workout_draft_to_garmin_v1",
        save.clone(),
    )?;
    assert!(
        denied["error"]["message"]
            .as_str()
            .context("denied")?
            .contains("not enabled"),
        "{denied}"
    );
    assert_eq!(denied["error"]["data"]["outcome"], "denied");
    let listed = call(
        &client,
        &base,
        &token,
        2,
        "list_garmin_devices_v1",
        json!({}),
    )?;
    assert_eq!(listed["error"]["data"]["outcome"], "denied", "{listed}");

    // The grant applies on the next request without a restart.
    assert_eq!(
        users(&fixture, &["allow-garmin-writes", "owner"])?["garmin_writes"],
        true
    );
    for bad in ["   ", &"x".repeat(1001)] {
        let refused = call(
            &client,
            &base,
            &token,
            3,
            "save_workout_draft_to_garmin_v1",
            json!({"id":1,"revision":1,"user_request":bad}),
        )?;
        assert!(
            refused["error"]["message"]
                .as_str()
                .context("refused")?
                .contains("user_request"),
            "{refused}"
        );
    }

    // A definite refusal reopens the draft and names the status.
    let rejected = call(
        &client,
        &base,
        &token,
        4,
        "save_workout_draft_to_garmin_v1",
        save.clone(),
    )?;
    assert!(
        rejected["error"]["message"]
            .as_str()
            .context("rejected")?
            .contains("HTTP 400"),
        "{rejected}"
    );
    assert_eq!(rejected["error"]["data"]["outcome"], "rejected");
    assert_eq!(rejected["error"]["data"]["garmin_status"], 400);
    let draft = call(
        &client,
        &base,
        &token,
        5,
        "get_workout_draft_v1",
        json!({"id":1}),
    )?;
    assert_eq!(
        draft["result"]["structuredContent"]["state"], "draft",
        "{draft}"
    );

    // The accepted save verifies the read-back and echoes the journaled instruction.
    let verified = call(
        &client,
        &base,
        &token,
        6,
        "save_workout_draft_to_garmin_v1",
        save,
    )?;
    let verified = &verified["result"]["structuredContent"];
    assert_eq!(verified["state"], "verified", "{verified}");
    assert_eq!(verified["remote_id"], 777);
    assert_eq!(verified["mcp_request"], format!("{REQUEST}\n{REQUEST}"));

    let devices = call(
        &client,
        &base,
        &token,
        7,
        "list_garmin_devices_v1",
        json!({}),
    )?;
    assert_eq!(
        devices["result"]["structuredContent"]["devices"],
        json!([{"id":99,"model":"fenix 8"}]),
        "{devices}"
    );
    let sent = call(
        &client,
        &base,
        &token,
        8,
        "send_workout_to_device_v1",
        json!({"id":777,"device_id":99,"user_request":REQUEST}),
    )?;
    let sent = &sent["result"]["structuredContent"];
    assert_eq!(sent["queued"], true, "{sent}");
    assert_eq!(sent["watch_receipt"], "unconfirmed");
    assert_eq!(sent["mcp_request"], REQUEST);

    // The user sees the quoted instruction in the CLI review and the delivery journal.
    let review = fixture
        .command()
        .args(["workout", "review", "--id", "1"])
        .output()?;
    let review: Value = serde_json::from_slice(&review.stdout)?;
    assert_eq!(review["mcp_request"], format!("{REQUEST}\n{REQUEST}"));
    let text = std::process::Command::new(env!("CARGO_BIN_EXE_pace"))
        .arg("--config")
        .arg(&fixture.config)
        .args(["workout", "review", "--id", "1"])
        .output()?;
    assert!(
        String::from_utf8_lossy(&text.stdout)
            .contains(&format!("Saved via MCP on user request: {REQUEST}"))
    );
    let db = rusqlite::Connection::open(&fixture.settings.database.path)?;
    assert_eq!(
        db.query_row(
            "SELECT mcp_request FROM workout_deliveries WHERE workout_id=777 AND device_id=99",
            [],
            |r| r.get::<_, String>(0)
        )?,
        REQUEST
    );

    assert_eq!(
        users(&fixture, &["deny-garmin-writes", "owner"])?["garmin_writes"],
        false
    );
    let denied = call(
        &client,
        &base,
        &token,
        9,
        "list_garmin_devices_v1",
        json!({}),
    )?;
    assert_eq!(denied["error"]["data"]["outcome"], "denied", "{denied}");

    let output = process.stop()?;
    server.finish()?;
    let logs = String::from_utf8(output.stderr)?;
    assert!(!logs.contains("\"level\":\"ERROR\""), "{logs}");
    assert!(!logs.contains(&token) && !logs.contains(REQUEST));
    Ok(())
}

#[tokio::test]
async fn stdio_flag_enables_benchmark_save_and_reconcile() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    // The catalogue and evidence come from the mock through the CLI sync, exactly as in use.
    let server = Server::start(vec![
        ("POST", TOKEN_PATH, 200, TOKEN.into()),
        ("GET", CATALOG, 200, json!([remote(false, 0.0)]).to_string()),
        ("GET", ACTIVITIES, 200, activity().to_string()),
        ("GET", SETS, 200, sets().to_string()),
        ("GET", CATALOG, 200, json!([remote(false, 0.0)]).to_string()),
        ("POST", CATALOG, 503, "{}".into()),
        ("GET", CATALOG, 200, json!([remote(true, 50.0)]).to_string()),
    ])?;
    configure(&fixture, &server.base)?;
    configure_connect(&fixture, &server.base)?;
    speed_up(&fixture)?;
    invoke(
        &fixture,
        &["sync", "--start", "2026-10-20", "--end", "2026-10-24"],
    )?;
    let service = BenchmarkService::new(Arc::new(SqliteBenchmarks::open(
        fixture.settings.database.clone(),
    )?));
    service.propose(proposal(&service.context("BARBELL_ROW", window()?)?))?;
    fs::write(
        &fixture.config,
        fs::read_to_string(&fixture.config)?
            .replace("stdio_garmin_writes = false", "stdio_garmin_writes = true"),
    )?;
    let log_path = fixture.directory.path().join("stdio.log");
    let writes = server.writes.clone();
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_pace"));
        command
            .arg("--config")
            .arg(&fixture.config)
            .args(["serve", "--transport", "stdio"])
            .kill_on_drop(true);
        let (transport, _) = TokioChildProcess::builder(command)
            .stderr(fs::File::create(&log_path)?)
            .spawn()?;
        let client = ().serve(transport).await?;
        let request = "Save my barbell row estimate to Garmin";
        let save = CallToolRequestParams::new("save_benchmark_draft_to_garmin_v1").with_arguments(
            json!({"id":1,"revision":1,"user_request":request})
                .as_object()
                .context("args")?
                .clone(),
        );
        // A 5xx on the write is an uncertain outcome: the proposal stays saving, nothing is retried.
        let error = client
            .call_tool(save)
            .await
            .err()
            .context("503 must fail")?
            .to_string();
        assert!(error.contains("Garmin training request failed"), "{error}");
        let draft = client
            .call_tool(
                CallToolRequestParams::new("get_benchmark_draft_v1")
                    .with_arguments(json!({"id":1}).as_object().context("args")?.clone()),
            )
            .await?
            .structured_content
            .context("draft")?;
        assert_eq!(draft["state"], "saving", "{draft}");
        assert_eq!(draft["mcp_request"], request);
        let reconciled = client
            .call_tool(
                CallToolRequestParams::new("reconcile_benchmark_draft_v1").with_arguments(
                    json!({"id":1,"user_request":"Check whether it was saved"})
                        .as_object()
                        .context("args")?
                        .clone(),
                ),
            )
            .await?
            .structured_content
            .context("reconciled")?;
        assert_eq!(reconciled["state"], "verified", "{reconciled}");
        assert_eq!(
            reconciled["mcp_request"],
            format!("{request}\nCheck whether it was saved")
        );
        client.cancel().await?;
        Ok::<(), anyhow::Error>(())
    })
    .await
    .context("stdio flow timed out")??;
    assert_eq!(
        writes.lock().map_err(|_| anyhow::anyhow!("writes"))?.len(),
        1
    );
    server.finish()?;
    let logs = fs::read_to_string(log_path)?;
    assert!(!logs.contains("\"level\":\"ERROR\""), "{logs}");

    // Without the flag the same profile is read-only towards Garmin.
    let denied = Fixture::new()?;
    denied.init()?;
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_pace"));
        command
            .arg("--config")
            .arg(&denied.config)
            .args(["serve", "--transport", "stdio"])
            .kill_on_drop(true);
        let (transport, _) = TokioChildProcess::builder(command)
            .stderr(Stdio::null())
            .spawn()?;
        let client = ().serve(transport).await?;
        let error = client
            .call_tool(CallToolRequestParams::new("list_garmin_devices_v1"))
            .await
            .err()
            .context("must be denied")?
            .to_string();
        assert!(error.contains("not enabled"), "{error}");
        client.cancel().await?;
        Ok::<(), anyhow::Error>(())
    })
    .await
    .context("denied stdio flow timed out")??;
    Ok(())
}

#[cfg(unix)]
#[test]
fn registry_migration_adds_the_grant_and_the_cli_toggles_it() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new()?;
    let path = fixture.settings.registry_database().path;
    let directory = path.parent().context("registry directory")?;
    fs::create_dir_all(directory)?;
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    let db = rusqlite::Connection::open(&path)?;
    db.execute_batch(include_str!("../../migrations/access/0001_registry.sql"))?;
    db.execute(
        "INSERT INTO users VALUES('legacy',?1,?2,1)",
        [
            fixture
                .directory
                .path()
                .join("legacy.sqlite3")
                .to_string_lossy()
                .into_owned(),
            fixture
                .directory
                .path()
                .join("legacy-session.json")
                .to_string_lossy()
                .into_owned(),
        ],
    )?;
    db.pragma_update(None, "user_version", 1)?;
    drop(db);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;

    users(&fixture, &["init"])?;
    let db = rusqlite::Connection::open(&path)?;
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
        2
    );
    assert_eq!(
        users(&fixture, &["list"])?["users"][0]["garmin_writes"],
        false
    );
    assert_eq!(
        users(&fixture, &["allow-garmin-writes", "legacy"])?["garmin_writes"],
        true
    );
    assert_eq!(
        users(&fixture, &["list"])?["users"][0]["garmin_writes"],
        true
    );
    assert_eq!(
        users(&fixture, &["deny-garmin-writes", "legacy"])?["garmin_writes"],
        false
    );
    assert!(
        !fixture
            .command()
            .args(["users", "allow-garmin-writes", "nobody"])
            .output()?
            .status
            .success()
    );
    Ok(())
}
