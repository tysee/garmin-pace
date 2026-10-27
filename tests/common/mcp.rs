//! tests/common/mcp.rs owns an isolated authenticated HTTP MCP process for workflow tests.
//! It exposes JSON-RPC calls and checks application logs and credential non-disclosure on stop.

use super::{Fixture, Process, register_user};
use anyhow::{Context, Result, ensure};
use reqwest::blocking::{Client, Response};
use serde_json::{Value, json};
use std::{fs, net::TcpListener, process::Stdio, time::Duration};

pub struct HttpMcp {
    pub client: Client,
    pub base: String,
    pub token: String,
    process: Process,
}
impl HttpMcp {
    pub fn start(fixture: &Fixture, timeout: u64) -> Result<Self> {
        let (token, _) = register_user(fixture, fixture, "owner")?;
        let result = fixture
            .command()
            .args(["users", "allow-garmin-writes", "owner"])
            .output()?;
        ensure!(result.status.success(), "grant failed");
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let mut config = garmin_pace::config::Settings::load(&fixture.config)?;
        config.http.bind = address;
        config.http.request_timeout_seconds = timeout;
        config.garmin.request_interval_ms = 1;
        fs::write(&fixture.config, toml::to_string(&config)?)?;
        drop(listener);
        let process = Process(Some(
            fixture
                .command()
                .args(["serve", "--transport", "http"])
                .env(
                    "GARMIN_PERSONAL_API_KEY",
                    "must-never-cross-profile-boundary",
                )
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?,
        ));
        let client = Client::builder().timeout(Duration::from_secs(15)).build()?;
        let base = format!("http://{address}/api/v1");
        crate::mcp::wait_for_http(&client, &base, &token)?;
        Ok(Self {
            client,
            base,
            token,
            process,
        })
    }
    pub fn call(&self, name: &str, args: Value) -> Result<Value> {
        crate::mcp::rpc(&self.client, &self.base, &self.token, payload(name, args))
    }
    pub fn content(&self, name: &str, args: Value) -> Result<Value> {
        let result = self.call(name, args)?;
        ensure!(
            result["error"].is_null() && result["result"]["isError"] != true,
            "{result}"
        );
        Ok(result["result"]["structuredContent"].clone())
    }
    pub fn stop(self) -> Result<()> {
        let output = self.process.stop()?;
        let logs = String::from_utf8(output.stderr)?;
        ensure!(
            !logs.contains("\"level\":\"ERROR\"") && !logs.contains("\"level\":\"FATAL\""),
            "{logs}"
        );
        ensure!(
            !logs.contains(&self.token) && !logs.contains(crate::garmin::KEY),
            "credentials in logs"
        );
        ensure!(output.stdout.is_empty(), "HTTP process wrote to stdout");
        Ok(())
    }
}
pub fn payload(name: &str, args: Value) -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}})
}
pub fn request(
    client: &Client,
    base: &str,
    token: &str,
    name: &str,
    args: Value,
) -> Result<Response> {
    client
        .post(format!("{base}/mcp"))
        .bearer_auth(token)
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-11-25")
        .json(&payload(name, args))
        .send()
        .context("sending MCP request")
}
