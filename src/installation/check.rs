//! src/installation/check.rs verifies local HTTP and trusted HTTPS without leaking keys to DNS targets.
//! Its bounded readiness adapters distinguish local authorization from unauthenticated DNS-path checks;
//! probe_service tells the menu, without a key, whether Pace already serves its listener.

use super::{PublicDomain, receipt_key, validate_listener};
use crate::config::Settings;
use anyhow::{Context, Result, ensure};
use reqwest::{
    StatusCode,
    blocking::{Client, ClientBuilder, Response},
    header::WWW_AUTHENTICATE,
};
use std::{
    io::Read,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::Path,
    time::{Duration, Instant},
};

fn client(timeout: Duration) -> ClientBuilder {
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(timeout)
        .timeout(timeout)
}

fn wait_for_response(client: &Client, url: &str, key: &str, wait: Duration) -> Result<Response> {
    let deadline = Instant::now() + wait;
    loop {
        match client.get(url).bearer_auth(key).send() {
            Ok(response) => return Ok(response),
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(250)),
            Err(_) => anyhow::bail!(
                "MCP is not reachable with a trusted connection; inspect the service journal and, for HTTPS, check DNS and TCP ports 80/443"
            ),
        }
    }
}

fn authorized_health(response: Response) -> Result<()> {
    ensure!(
        response.status() == StatusCode::OK,
        "MCP readiness rejected the issued key"
    );
    let mut body = Vec::new();
    response.take(513).read_to_end(&mut body)?;
    ensure!(body.len() <= 512, "unexpected MCP readiness response size");
    let value: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|_| anyhow::anyhow!("invalid MCP readiness response"))?;
    ensure!(
        value.get("status").and_then(serde_json::Value::as_str) == Some("ok"),
        "MCP readiness did not confirm service health"
    );
    Ok(())
}

/// Pace answers a keyless request with its own Bearer challenge.
fn pace_challenge(response: &Response) -> bool {
    response.status() == StatusCode::UNAUTHORIZED
        && response
            .headers()
            .get(WWW_AUTHENTICATE)
            .is_some_and(|value| value == "Bearer realm=\"garmin-pace\"")
}

fn anonymous_rejected(client: &Client, url: &str) -> Result<()> {
    let response = client
        .get(url)
        .send()
        .context("checking access without credentials")?;
    ensure!(
        pace_challenge(&response),
        "the endpoint must reject unauthenticated requests with the MCP Bearer challenge"
    );
    Ok(())
}

/// What answers on a config's HTTP listener, checked once without a key.
#[derive(Debug, PartialEq)]
pub enum ServiceProbe {
    Running,
    NotRunning,
    OtherProgram,
}

/// Ask the listener once, without credentials; wildcard binds are probed on loopback.
pub fn probe_service(settings: &Settings) -> Result<ServiceProbe> {
    let bind = settings.http.bind;
    let ip = match bind.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    let timeout = Duration::from_secs(settings.http.request_timeout_seconds.min(3));
    let client = client(timeout).build()?;
    let url = format!("http://{}/api/v1/health", SocketAddr::new(ip, bind.port()));
    Ok(match client.get(&url).send() {
        Ok(response) if pace_challenge(&response) => ServiceProbe::Running,
        Ok(_) => ServiceProbe::OtherProgram,
        Err(error) if error.is_connect() || error.is_timeout() => ServiceProbe::NotRunning,
        Err(error) => return Err(error).context("checking the local MCP listener"),
    })
}

/// Verify the started local service with a key issued by setup.
pub fn check_service(settings: &Settings, profile: &Path) -> Result<()> {
    validate_listener(settings)?;
    let key = receipt_key(profile)?;
    let timeout = Duration::from_secs(settings.http.request_timeout_seconds);
    let client = client(timeout).build()?;
    let url = format!("http://{}/api/v1/health", settings.http.bind);
    authorized_health(wait_for_response(&client, &url, &key, timeout)?)?;
    anonymous_rejected(&client, &url)
}

/// Check trusted TLS against loopback before using a key; DNS-path requests never carry credentials.
pub fn check_https(settings: &Settings, profile: &Path, domain: &PublicDomain) -> Result<()> {
    validate_listener(settings)?;
    let wait = Duration::from_secs(settings.http.request_timeout_seconds);
    let timeout = wait.min(Duration::from_secs(5));
    let url = format!("https://{}/api/v1/health", domain.as_str());
    let local = client(timeout)
        .resolve(
            domain.as_str(),
            SocketAddr::new(settings.http.bind.ip(), 443),
        )
        .build()?;
    let key = receipt_key(profile)?;
    authorized_health(wait_for_response(&local, &url, &key, wait)?)?;
    anonymous_rejected(&local, &url)?;
    let public = client(timeout).build()?;
    anonymous_rejected(&public, &url).context("local HTTPS works, but the DNS path is not verified; check DNS, firewall and router forwarding")
}
