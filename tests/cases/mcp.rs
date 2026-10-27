//! tests/cases/mcp.rs checks the real stdio and HTTP MCP processes against a seeded
//! SQLite database, including authentication, limits, protocol calls, and logs.

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::{Value, json};
use std::{
    fs,
    net::TcpListener,
    process::Stdio,
    time::{Duration, Instant},
};

use crate::common::{Fixture, Process, register_user};

#[tokio::test]
async fn stdio_discovers_tools_and_reads_persisted_history() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    fixture.seed()?;
    crate::health::seed_health(&fixture, 1234.0)?;
    fs::write(
        &fixture.config,
        fs::read_to_string(&fixture.config)?
            .replace("log_level = \"info\"", "log_level = \"debug\""),
    )?;
    let log_path = fixture.directory.path().join("stdio.log");
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_pace"));
        command
            .arg("--config")
            .arg(&fixture.config)
            .args(["serve", "--transport", "stdio"])
            .kill_on_drop(true);
        let (transport, stderr) = TokioChildProcess::builder(command)
            .stderr(fs::File::create(&log_path)?)
            .spawn()?;
        assert!(stderr.is_none());
        let client = ().serve(transport).await?;
        let tools = client.list_all_tools().await?;
        assert_eq!(tools.len(), 36);
        for tool in &tools {
            // Only the granted Garmin tools reach the network; every other tool stays local.
            let metadata=serde_json::to_value(tool)?;
            let garmin=crate::mcp_garmin::GARMIN_TOOLS.contains(&tool.name.as_ref());
            assert_eq!(metadata["annotations"]["openWorldHint"],garmin,"{}",tool.name);
            if garmin {
                let destructive = !tool.name.starts_with("sync_") && tool.name != "refresh_workout_v1" && tool.name != "list_garmin_devices_v1";
                assert_eq!(metadata["annotations"]["destructiveHint"],destructive,"{}",tool.name);
            } else if tool.name.contains("benchmark") {
                assert_eq!(metadata["annotations"]["destructiveHint"],false,"{}",tool.name);
            }
        }
        let (_,proposal)=crate::benchmarks::seed(&fixture,false)?;
        crate::benchmarks::seed_activity(&fixture)?;
        let sets=client.call_tool(CallToolRequestParams::new("get_activity_sets_v1")
            .with_arguments(json!({"garmin_id":"501"}).as_object().context("object")?.clone())).await?;
        let sets=sets.structured_content.context("activity sets")?;
        assert_eq!(sets["activity"]["name"],"Back day");
        assert_eq!(sets["sets"]["status"],"available");
        assert_eq!(sets["sets"]["session"]["sets"][1]["weight_kg"],60.0);
        assert_eq!(sets["sets"]["session"]["sets"][5]["category"],"ROW");
        // A running activity has no sets; an unknown ID is not invented.
        for id in ["fixture-1","999"] {
            assert!(client.call_tool(CallToolRequestParams::new("get_activity_sets_v1")
                .with_arguments(json!({"garmin_id":id}).as_object().context("object")?.clone())).await.is_err());
        }
        let catalogue=client.call_tool(CallToolRequestParams::new("list_benchmarks_v1")).await?;
        assert_eq!(catalogue.structured_content.context("benchmarks")?["benchmarks"][0]["exercise"]["name"],"BARBELL_ROW");
        let context=client.call_tool(CallToolRequestParams::new("get_benchmark_context_v1")
            .with_arguments(json!({"key":"BARBELL_ROW","start":"2026-10-20","end":"2026-10-24"}).as_object().context("object")?.clone())).await?;
        assert_eq!(context.structured_content.context("evidence")?["mean_weight_kg"],50.0);
        let proposal=garmin_pace::workouts::benchmarks::document::ProposalDocument::from(proposal);
        let estimate=client.call_tool(CallToolRequestParams::new("propose_benchmark_v1")
            .with_arguments(json!({"proposal":proposal}).as_object().context("object")?.clone())).await?;
        assert_eq!(estimate.structured_content.context("proposal")?["state"],"draft");
        let changed=client.call_tool(CallToolRequestParams::new("revise_benchmark_draft_v1")
            .with_arguments(json!({"id":1,"revision":1,"proposal":proposal}).as_object().context("object")?.clone())).await?;
        assert_eq!(changed.structured_content.context("revised proposal")?["revision"],2);
        let listing=client.call_tool(CallToolRequestParams::new("list_benchmark_drafts_v1")).await?;
        assert_eq!(listing.structured_content.context("proposals")?["drafts"][0]["id"],1);
        let read=client.call_tool(CallToolRequestParams::new("get_benchmark_draft_v1")
            .with_arguments(json!({"id":1}).as_object().context("object")?.clone())).await?;
        assert_eq!(read.structured_content.context("proposal")?["context"]["sample_count"],2);
        let status = client
            .call_tool(CallToolRequestParams::new("get_status"))
            .await?;
        assert_eq!(
            status.structured_content.context("status JSON")?["activity_count"],
            3
        );
        let activities = client
            .call_tool(
                CallToolRequestParams::new("list_activities")
                    .with_arguments(json!({"limit": 1}).as_object().context("object")?.clone()),
            )
            .await?;
        assert_eq!(
            activities.structured_content.context("activity JSON")?["activities"][0]["garmin_id"],
            "501"
        );
        let health = client.call_tool(CallToolRequestParams::new("get_daily_health")
            .with_arguments(json!({"date":"2026-10-24"}).as_object().context("object")?.clone())).await?;
        assert_eq!(health.structured_content.context("health JSON")?["datasets"]["daily"]["records"][0]["values"]["steps"]["value"], 1234.0);
        let samples = client.call_tool(CallToolRequestParams::new("get_health_samples")
            .with_arguments(json!({"date":"2026-10-24","metric":"heart_rate","limit":1}).as_object().context("object")?.clone())).await?;
        assert_eq!(samples.structured_content.context("sample JSON")?["samples"][0]["value"], 60.0);
        let days = client.call_tool(CallToolRequestParams::new("list_health_metrics")
            .with_arguments(json!({"start":"2026-10-23","end":"2026-10-24"}).as_object().context("object")?.clone())).await?;
        assert_eq!(days.structured_content.context("days JSON")?["missing_dates"],json!(["2026-10-23"]));
        let create = client.call_tool(CallToolRequestParams::new("create_workout_draft_v1")
            .with_arguments(json!({"workout":crate::workouts::candidate("Model tempo",1200.0)}).as_object().context("object")?.clone())).await?;
        let draft=create.structured_content.context("draft JSON")?;
        assert_eq!(draft["state"],"draft"); assert_eq!(draft["revision"],1); assert!(draft["remote_id"].is_null());
        let list=client.call_tool(CallToolRequestParams::new("list_workout_drafts_v1")).await?;
        assert_eq!(list.structured_content.context("draft list")?["drafts"][0]["name"],"Model tempo");
        let changed=client.call_tool(CallToolRequestParams::new("revise_workout_draft_v1")
            .with_arguments(json!({"id":1,"revision":1,"workout":crate::workouts::candidate("Model adjusted",900.0)}).as_object().context("object")?.clone())).await?;
        assert_eq!(changed.structured_content.context("revised draft")?["revision"],2);
        let current=client.call_tool(CallToolRequestParams::new("get_workout_draft_v1")
            .with_arguments(json!({"id":1}).as_object().context("object")?.clone())).await?;
        assert_eq!(current.structured_content.context("read draft")?["workout"]["name"],"Model adjusted");
        let library=client.call_tool(CallToolRequestParams::new("list_workouts_v1")).await?;
        assert_eq!(library.structured_content.context("library")?["workouts"],json!([]));
        client.cancel().await?;
        Ok::<(), anyhow::Error>(())
    })
    .await
    .context("stdio flow timed out")??;
    let logs = fs::read_to_string(log_path)?;
    assert!(!logs.contains("\"level\":\"ERROR\""), "{logs}");
    assert!(!logs.contains("Test run") && !logs.contains("fixture-2"));
    Ok(())
}

#[test]
fn http_authentication_and_mcp_history_flow() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    fixture.seed()?;
    let (token, _) = register_user(&fixture, &fixture, "owner")?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let text = fs::read_to_string(&fixture.config)?.replace("127.0.0.1:8080", &address.to_string());
    fs::write(&fixture.config, text)?;
    drop(listener);
    let child = fixture
        .command()
        .args(["serve", "--transport", "http"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let process = Process(Some(child));
    let client = Client::builder().timeout(Duration::from_secs(5)).build()?;
    let base = format!("http://{address}/api/v1");
    wait_for_http(&client, &base, &token)?;
    verify_http(&client, &base, &token)?;
    let output = process.stop()?;
    let logs = String::from_utf8(output.stderr)?;
    assert!(!logs.contains("\"level\":\"ERROR\""), "{logs}");
    assert!(!logs.contains(&token));
    assert!(output.stdout.is_empty());
    Ok(())
}

pub(super) fn wait_for_http(client: &Client, base: &str, token: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match client
            .get(format!("{base}/health"))
            .bearer_auth(token)
            .send()
        {
            Ok(response) if response.status().is_success() => return Ok(()),
            Ok(response) => bail!("health returned {}", response.status()),
            Err(error) if error.is_connect() => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => return Err(error.into()),
        }
    }
    bail!("HTTP server did not start")
}

fn verify_http(client: &Client, base: &str, token: &str) -> Result<()> {
    assert_eq!(client.get(format!("{base}/health")).send()?.status(), 401);
    assert_eq!(client.post(format!("{base}/mcp")).send()?.status(), 401);
    assert_eq!(
        client
            .post(format!("{base}/mcp"))
            .bearer_auth(token)
            .header("Host", "untrusted.invalid")
            .header("Accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":0,"method":"tools/list"}))
            .send()?
            .status(),
        403
    );
    assert_eq!(
        client
            .get(format!("{base}/health"))
            .bearer_auth(token)
            .header("Origin", "https://untrusted.invalid")
            .send()?
            .status(),
        403
    );
    let initialized = rpc(
        client,
        base,
        token,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),
    )?;
    assert!(
        initialized["result"]["capabilities"]["tools"].is_object(),
        "{initialized}"
    );
    assert_eq!(initialized["result"]["serverInfo"]["name"], "garmin-pace");
    let tools = rpc(
        client,
        base,
        token,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    )?;
    assert_eq!(tools["result"]["tools"].as_array().map(Vec::len), Some(36));
    let status = rpc(
        client,
        base,
        token,
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_status","arguments":{}}}),
    )?;
    assert_eq!(
        status["result"]["structuredContent"]["activity_count"], 2,
        "{status}"
    );
    let page = rpc(
        client,
        base,
        token,
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"list_activities","arguments":{"limit":1}}}),
    )?;
    assert_eq!(
        page["result"]["structuredContent"]["activities"][0]["garmin_id"], "fixture-2",
        "{page}"
    );
    let invalid = rpc(
        client,
        base,
        token,
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"list_activities","arguments":{"limit":0}}}),
    )?;
    assert!(
        invalid["error"].is_object() || invalid["result"]["isError"] == true,
        "{invalid}"
    );
    Ok(())
}

pub(super) fn rpc(client: &Client, base: &str, token: &str, payload: Value) -> Result<Value> {
    let response = client
        .post(format!("{base}/mcp"))
        .bearer_auth(token)
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-11-25")
        .json(&payload)
        .send()?
        .error_for_status()?;
    let body = response.text()?;
    if let Some(line) = body.lines().find_map(|line| line.strip_prefix("data: ")) {
        serde_json::from_str(line).context("decoding SSE response")
    } else {
        serde_json::from_str(&body).context("decoding JSON response")
    }
}

#[test]
fn key_lifetime_accepts_the_hundred_year_bound_and_rejects_longer() -> Result<()> {
    let server = Fixture::new()?;
    let alice = Fixture::new()?;
    alice.init()?;
    let (_, _) = register_user(&server, &alice, "alice")?;
    let issue = |days: &str| -> Result<std::process::Output> {
        let path = server.directory.path().join(format!("alice-{days}.key"));
        Ok(server
            .command()
            .args(["users", "issue-key", "alice", "--days", days, "--output"])
            .arg(&path)
            .output()?)
    };
    let permanent = issue("36500")?;
    assert!(permanent.status.success());
    let metadata: Value = serde_json::from_slice(&permanent.stdout)?;
    let now = chrono::Utc::now().timestamp();
    let expires = metadata["expires_at_unix"].as_i64().context("expiry")?;
    assert!(expires > now + 99 * 365 * 86400);
    let too_long = issue("36501")?;
    assert!(!too_long.status.success());
    assert!(String::from_utf8_lossy(&too_long.stderr).contains("36500"));
    assert!(!server.directory.path().join("alice-36501.key").exists());
    Ok(())
}

#[test]
fn users_are_isolated_and_revocation_applies_without_restart() -> Result<()> {
    let server = Fixture::new()?;
    let alice = Fixture::new()?;
    let bob = Fixture::new()?;
    alice.init()?;
    alice.seed()?;
    bob.init()?;
    crate::health::seed_health(&alice, 1234.0)?;
    crate::health::seed_health(&bob, 5678.0)?;
    let (alice_key, alice_id) = register_user(&server, &alice, "alice")?;
    let (bob_key, _) = register_user(&server, &bob, "bob")?;
    let duplicate = server
        .command()
        .args(["users", "add", "other", "--profile"])
        .arg(&alice.config)
        .output()?;
    assert!(!duplicate.status.success());
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let text = fs::read_to_string(&server.config)?
        .replace("127.0.0.1:8080", &address.to_string())
        .replace("log_level = \"info\"", "log_level = \"trace\"");
    fs::write(&server.config, text)?;
    drop(listener);
    let process = Process(Some(
        server
            .command()
            .args(["serve", "--transport", "http"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?,
    ));
    let client = Client::builder().timeout(Duration::from_secs(5)).build()?;
    let base = format!("http://{address}/api/v1");
    wait_for_http(&client, &base, &alice_key)?;
    for (key, steps) in [(&alice_key, 1234.0), (&bob_key, 5678.0)] {
        let health = rpc(
            &client,
            &base,
            key,
            json!({"jsonrpc":"2.0","id":77,"method":"tools/call",
            "params":{"name":"get_daily_health","arguments":{"date":"2026-10-24"}}}),
        )?;
        assert_eq!(
            health["result"]["structuredContent"]["datasets"]["daily"]["records"][0]["values"]["steps"]
                ["value"],
            steps
        );
    }
    let created = rpc(
        &client,
        &base,
        &alice_key,
        json!({"jsonrpc":"2.0","id":78,"method":"tools/call","params":{"name":"create_workout_draft_v1","arguments":{"workout":crate::workouts::candidate("Alice private workout",1200.0)}}}),
    )?;
    assert_eq!(created["result"]["structuredContent"]["id"], 1);
    let invisible = rpc(
        &client,
        &base,
        &bob_key,
        json!({"jsonrpc":"2.0","id":79,"method":"tools/call","params":{"name":"list_workout_drafts_v1","arguments":{}}}),
    )?;
    assert_eq!(
        invisible["result"]["structuredContent"]["drafts"],
        json!([])
    );
    let denied = rpc(
        &client,
        &base,
        &bob_key,
        json!({"jsonrpc":"2.0","id":80,"method":"tools/call","params":{"name":"get_workout_draft_v1","arguments":{"id":1}}}),
    )?;
    assert!(
        denied["error"].is_object() || denied["result"]["isError"] == true,
        "{denied}"
    );
    let (_, proposal) = crate::benchmarks::seed(&alice, false)?;
    let proposal = garmin_pace::workouts::benchmarks::document::ProposalDocument::from(proposal);
    let created = rpc(
        &client,
        &base,
        &alice_key,
        json!({"jsonrpc":"2.0","id":81,"method":"tools/call","params":{"name":"propose_benchmark_v1","arguments":{"proposal":proposal}}}),
    )?;
    assert_eq!(created["result"]["structuredContent"]["id"], 1);
    let other = rpc(
        &client,
        &base,
        &bob_key,
        json!({"jsonrpc":"2.0","id":82,"method":"tools/call","params":{"name":"list_benchmark_drafts_v1","arguments":{}}}),
    )?;
    assert_eq!(other["result"]["structuredContent"]["drafts"], json!([]));
    let denied = rpc(
        &client,
        &base,
        &bob_key,
        json!({"jsonrpc":"2.0","id":83,"method":"tools/call","params":{"name":"get_benchmark_draft_v1","arguments":{"id":1}}}),
    )?;
    assert!(denied["error"].is_object() || denied["result"]["isError"] == true);
    let payload = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_status","arguments":{}}});
    for (key, count) in [(&alice_key, 2), (&bob_key, 0)] {
        let result = rpc(&client, &base, key, payload.clone())?;
        assert_eq!(
            result["result"]["structuredContent"]["activity_count"], count,
            "{result}"
        );
    }
    let response = client
        .post(format!("{base}/mcp"))
        .bearer_auth(&bob_key)
        .header("X-User", "alice")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-11-25")
        .json(&payload)
        .send()?
        .json::<Value>()?;
    assert_eq!(response["result"]["structuredContent"]["activity_count"], 0);
    let page = rpc(
        &client,
        &base,
        &alice_key,
        json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"list_activities","arguments":{"limit":1}}}),
    )?;
    assert_eq!(
        page["result"]["structuredContent"]["activities"][0]["name"],
        "Test run"
    );
    let initialized = rpc(
        &client,
        &base,
        &alice_key,
        json!({"jsonrpc":"2.0","id":9,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"PRIVATE-CLIENT-MARKER","version":"1"}}}),
    )?;
    assert!(initialized["result"].is_object());
    let malformed = client
        .get(format!("{base}/health"))
        .bearer_auth("not-a-key")
        .send()?;
    assert_eq!(malformed.status(), 401);
    assert!(malformed.headers().contains_key("www-authenticate"));
    let revoked = server
        .command()
        .args(["users", "revoke-key", &alice_id])
        .output()?;
    assert!(revoked.status.success());
    assert_eq!(
        client
            .get(format!("{base}/health"))
            .bearer_auth(&alice_key)
            .send()?
            .status(),
        401
    );
    assert_eq!(
        client
            .get(format!("{base}/health"))
            .bearer_auth(&bob_key)
            .send()?
            .status(),
        200
    );
    assert!(
        server
            .command()
            .args(["users", "disable", "bob"])
            .output()?
            .status
            .success()
    );
    assert_eq!(
        client
            .get(format!("{base}/health"))
            .bearer_auth(&bob_key)
            .send()?
            .status(),
        401
    );
    assert!(
        server
            .command()
            .args(["users", "enable", "bob"])
            .output()?
            .status
            .success()
    );
    let registry = rusqlite::Connection::open(&server.settings.access.registry_path)?;
    registry.execute(
        "UPDATE access_keys SET expires_at=0 WHERE user_name='bob'",
        [],
    )?;
    assert_eq!(
        client
            .get(format!("{base}/health"))
            .bearer_auth(&bob_key)
            .send()?
            .status(),
        401
    );
    let stored = fs::read(&server.settings.access.registry_path)?;
    for key in [&alice_key, &bob_key] {
        assert!(!stored.windows(key.len()).any(|w| w == key.as_bytes()));
    }
    let output = process.stop()?;
    let logs = String::from_utf8(output.stderr)?;
    for marker in [
        &alice_key,
        &bob_key,
        "fixture-2",
        "Test run",
        "PRIVATE-CLIENT-MARKER",
    ] {
        assert!(!logs.contains(marker));
    }
    assert!(!logs.contains("\"level\":\"ERROR\""), "{logs}");
    Ok(())
}

#[test]
fn incomplete_http_connections_expire_before_authentication() -> Result<()> {
    use std::{
        io::{Read, Write},
        net::TcpStream,
    };
    let fixture = Fixture::new()?;
    fixture.init()?;
    let (token, _) = register_user(&fixture, &fixture, "owner")?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    fs::write(
        &fixture.config,
        fs::read_to_string(&fixture.config)?
            .replace("127.0.0.1:8080", &address.to_string())
            .replace("max_connections = 32", "max_connections = 2")
            .replace(
                "connection_lifetime_seconds = 90",
                "connection_lifetime_seconds = 1",
            ),
    )?;
    drop(listener);
    let process = Process(Some(
        fixture
            .command()
            .args(["serve", "--transport", "http"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?,
    ));
    let client = Client::builder()
        .pool_max_idle_per_host(0)
        .timeout(Duration::from_secs(5))
        .build()?;
    let base = format!("http://{address}/api/v1");
    wait_for_http(&client, &base, &token)?;
    let mut slow = TcpStream::connect(address)?;
    slow.set_read_timeout(Some(Duration::from_secs(3)))?;
    slow.write_all(b"GET /api/v1/mcp HTTP/1.1\r\nHost: localhost\r\n")?;
    let idle = TcpStream::connect(address)?;
    std::thread::sleep(Duration::from_millis(150));
    let started = Instant::now();
    assert_eq!(
        client
            .get(format!("{base}/health"))
            .bearer_auth(&token)
            .send()?
            .status(),
        200
    );
    assert!(
        started.elapsed() >= Duration::from_millis(500),
        "connection cap was bypassed"
    );
    let mut byte = [0u8; 1];
    match slow.read(&mut byte) {
        Ok(0) => (),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
            ) => {}
        result => panic!("incomplete request survived its lifetime: {result:?}"),
    }
    drop(idle);
    let output = process.stop()?;
    assert!(!String::from_utf8_lossy(&output.stderr).contains("\"level\":\"ERROR\""));
    Ok(())
}
