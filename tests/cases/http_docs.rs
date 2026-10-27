//! tests/cases/http_docs.rs exercises public documentation and authenticated browser
//! requests through a real HTTP process, checking origin boundaries and key revocation.

use crate::common::{Fixture, Process, register_user};
use anyhow::{Context, Result};
use reqwest::blocking::Client;
use serde_json::{Value, json};
use std::{fs, net::TcpListener, process::Stdio, time::Duration};

#[test]
fn swagger_documents_and_executes_the_authenticated_http_contract() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    fixture.seed()?;
    let (token, key_id) = register_user(&fixture, &fixture, "owner")?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    fs::write(
        &fixture.config,
        fs::read_to_string(&fixture.config)?.replace("127.0.0.1:8080", &address.to_string()),
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
    let client = Client::builder().timeout(Duration::from_secs(5)).build()?;
    let origin = format!("http://{address}");
    let base = format!("{origin}/api/v1");
    crate::mcp::wait_for_http(&client, &base, &token)?;
    let spec = verify_documentation(&client, &base)?;
    verify_origins(&client, &base, &origin, &token)?;
    verify_examples(&client, &base, &origin, &token, &spec)?;
    verify_revocation(&fixture, &client, &base, &origin, &token, &key_id)?;
    let logs = String::from_utf8(process.stop()?.stderr)?;
    assert!(
        !logs.contains("\"level\":\"ERROR\"") && !logs.contains("FATAL"),
        "{logs}"
    );
    assert!(!logs.contains(&token) && !logs.contains("fixture-2"));
    Ok(())
}

fn verify_examples(
    client: &Client,
    base: &str,
    origin: &str,
    token: &str,
    spec: &Value,
) -> Result<()> {
    let examples = spec["paths"]["/api/v1/mcp"]["post"]["requestBody"]["content"]
        ["application/json"]["examples"].as_object().context("MCP examples")?;
    for (name, example) in examples {
        let response = client
            .post(format!("{base}/mcp"))
            .bearer_auth(token)
            .header("Origin", origin)
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2025-11-25")
            .json(&example["value"])
            .send()?;
        assert_eq!(response.status(), 200, "{name}");
        let reply: Value = response.json()?;
        assert!(reply["result"].is_object(), "{name}: {reply}");
        if name == "status" {
            assert_eq!(reply["result"]["structuredContent"]["activity_count"], 2);
        }
    }
    Ok(())
}

fn verify_revocation(
    fixture: &Fixture,
    client: &Client,
    base: &str,
    origin: &str,
    token: &str,
    key_id: &str,
) -> Result<()> {
    assert!(
        fixture
            .command()
            .args(["users", "revoke-key", key_id])
            .output()?
            .status
            .success()
    );
    assert_eq!(
        client
            .get(format!("{base}/health"))
            .bearer_auth(token)
            .header("Origin", origin)
            .send()?
            .status(),
        401
    );
    assert_eq!(client.get(format!("{base}/docs")).send()?.status(), 200);
    Ok(())
}

fn verify_documentation(client: &Client, base: &str) -> Result<Value> {
    let page = client.get(format!("{base}/docs")).send()?;
    assert_eq!(page.status(), 200);
    assert_eq!(page.headers()["cache-control"], "no-store");
    assert_eq!(page.headers()["x-content-type-options"], "nosniff");
    assert!(
        page.headers()["content-security-policy"]
            .to_str()?
            .contains("connect-src 'self'")
    );
    let html = page.text()?;
    assert!(html.contains("Swagger UI") && html.contains("integrity=\"sha384-"));
    let script = client
        .get(format!("{base}/docs/init.js"))
        .send()?
        .error_for_status()?
        .text()?;
    assert!(
        script.contains("persistAuthorization: false") && script.contains("validatorUrl: null")
    );
    let spec: Value = client
        .get(format!("{base}/openapi.json"))
        .send()?
        .error_for_status()?
        .json()?;
    assert_eq!(spec["openapi"], "3.0.3");
    assert_eq!(spec["servers"][0]["url"], "/");
    assert_eq!(
        spec["components"]["securitySchemes"]["bearerAuth"]["scheme"],
        "bearer"
    );
    assert_eq!(spec["security"], json!([{"bearerAuth": []}]));
    assert!(spec["paths"]["/api/v1/health"]["get"].is_object());
    assert!(spec["paths"]["/api/v1/mcp"]["post"].is_object());
    Ok(spec)
}

fn verify_origins(client: &Client, base: &str, origin: &str, token: &str) -> Result<()> {
    for path in ["docs", "docs/init.js", "openapi.json", "health", "mcp"] {
        let url = format!("{base}/{path}");
        assert_eq!(
            client
                .get(&url)
                .header("Host", "untrusted.invalid")
                .send()?
                .status(),
            403
        );
        verify_rejected_origins(client, &url, origin, token)?;
        assert_eq!(
            client
                .get(&url)
                .header("Origin", origin)
                .header("Origin", origin)
                .send()?
                .status(),
            403
        );
        assert_eq!(
            client
                .get(format!("{url}?token=not-accepted"))
                .send()?
                .status(),
            400
        );
    }
    verify_same_origin_auth(client, base, origin, token)
}

fn verify_same_origin_auth(client: &Client, base: &str, origin: &str, token: &str) -> Result<()> {
    for path in ["health", "mcp"] {
        assert_eq!(
            client
                .get(format!("{base}/{path}"))
                .header("Origin", origin)
                .send()?
                .status(),
            401
        );
    }
    let health = client
        .get(format!("{base}/health"))
        .bearer_auth(token)
        .header("Origin", origin)
        .send()?;
    assert_eq!(health.status(), 200);
    assert!(!health.headers().contains_key("access-control-allow-origin"));
    assert_eq!(health.json::<Value>()?, json!({"status":"ok"}));
    Ok(())
}

fn verify_rejected_origins(client: &Client, url: &str, origin: &str, token: &str) -> Result<()> {
    for rejected in [
        "https://untrusted.invalid".to_owned(),
        "null".to_owned(),
        format!("{origin}/"),
        format!("{origin}.evil.invalid"),
        origin.replace("http:", "https:"),
        "http://127.0.0.1:1".to_owned(),
    ] {
        assert_eq!(
            client
                .get(url)
                .bearer_auth(token)
                .header("Origin", &rejected)
                .header("X-Forwarded-Host", "untrusted.invalid")
                .header("X-Forwarded-Proto", "https")
                .send()?
                .status(),
            403,
            "{url}: {rejected}"
        );
    }
    Ok(())
}
