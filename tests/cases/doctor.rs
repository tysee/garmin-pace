//! tests/cases/doctor.rs drives `pace doctor` and a failing sync through the built executable
//! against loopback stand-ins: keyless reachability, and classified failures that name the
//! host and route without leaking keys.

use crate::common::Fixture;
use anyhow::{Context, Result};
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    process::{Command, Output},
    thread,
};

const KEY: &str = "fixture-api-key-never-real";
const PROXY_VARIABLES: [&str; 8] = [
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
];

/// Point every Garmin endpoint at one loopback base, optionally through a configured proxy.
fn configure(fixture: &Fixture, base: &str, proxy: Option<&str>) -> Result<()> {
    let mut text = fs::read_to_string(&fixture.config)?;
    for host in [
        "https://personal.connectapi.garmin.com",
        "https://connectapi.garmin.com",
        "https://sso.garmin.com",
        "https://diauth.garmin.com",
    ] {
        text = text.replace(host, base);
    }
    text = text
        .replace("retry_base_ms = 500", "retry_base_ms = 1")
        .replace("request_interval_ms = 1000", "request_interval_ms = 1");
    if let Some(proxy) = proxy {
        text = text.replace(
            "request_interval_ms = 1",
            &format!("request_interval_ms = 1\nproxy_url = \"{proxy}\""),
        );
    }
    fs::write(&fixture.config, text)?;
    Ok(())
}

/// The CLI without inherited proxies, so loopback requests stay direct on any machine.
fn pace(fixture: &Fixture) -> Command {
    let mut command = fixture.command();
    for name in PROXY_VARIABLES {
        command.env_remove(name);
    }
    command.env("GARMIN_PERSONAL_API_KEY", KEY);
    command
}

/// A loopback port with nothing listening: connections are refused.
fn closed_base() -> Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(format!("http://{}", listener.local_addr()?))
}

/// A loopback stand-in answering `count` keyless requests with HTTP 401; returns their heads.
fn answer_unauthorized(count: usize) -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = thread::spawn(move || -> Result<Vec<String>> {
        let mut requests = Vec::new();
        for _ in 0..count {
            let (stream, _) = listener.accept()?;
            let mut reader = BufReader::new(stream.try_clone()?);
            let mut head = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line)?;
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                head.push_str(&line);
            }
            requests.push(head);
            (&stream).write_all(b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\n\r\n")?;
        }
        Ok(requests)
    });
    Ok((base, server))
}

fn json(output: &Output) -> Result<Value> {
    serde_json::from_slice(&output.stdout).context("doctor JSON")
}

#[test]
fn doctor_reaches_every_garmin_endpoint_without_sending_keys() -> Result<()> {
    let fixture = Fixture::new()?;
    let (base, server) = answer_unauthorized(4)?;
    configure(&fixture, &base, None)?;
    let output = pace(&fixture).arg("doctor").output()?;
    let requests = server
        .join()
        .map_err(|_| anyhow::anyhow!("doctor fixture panicked"))??;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = json(&output)?;
    let endpoints = report["endpoints"].as_array().context("endpoints")?;
    assert_eq!(endpoints.len(), 4);
    for endpoint in endpoints {
        assert_eq!(endpoint["reachable"], true);
        assert_eq!(endpoint["http_status"], 401);
        assert_eq!(endpoint["route"], "direct connection");
    }
    for request in &requests {
        assert!(request.starts_with("GET / HTTP/1.1"), "{request}");
        assert!(!request.to_ascii_lowercase().contains("authorization"));
        assert!(!request.contains(KEY));
    }
    Ok(())
}

#[test]
fn network_failures_name_the_cause_host_and_route() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let base = closed_base()?;
    configure(&fixture, &base, None)?;
    let doctor = pace(&fixture).arg("doctor").output()?;
    assert!(!doctor.status.success());
    let report = json(&doctor)?;
    for endpoint in report["endpoints"].as_array().context("endpoints")? {
        assert_eq!(endpoint["reachable"], false);
        assert_eq!(endpoint["reason"], "connection refused (direct connection)");
    }
    assert!(String::from_utf8_lossy(&doctor.stderr).contains("no key was sent"));

    let sync = pace(&fixture)
        .args(["garmin", "sync", "--limit", "2"])
        .output()?;
    assert!(!sync.status.success());
    let stderr = String::from_utf8_lossy(&sync.stderr);
    let host = base.trim_start_matches("http://");
    assert!(
        stderr.contains(&format!(
            "{host} failed: connection refused (direct connection)"
        )),
        "{stderr}"
    );
    assert!(stderr.contains("pace doctor"));
    assert!(!stderr.contains(KEY) && !String::from_utf8_lossy(&sync.stdout).contains(KEY));
    Ok(())
}

#[test]
fn configured_proxy_is_used_and_reported_without_credentials() -> Result<()> {
    let fixture = Fixture::new()?;
    let proxy = closed_base()?;
    configure(&fixture, "http://127.0.0.1:9", Some(&proxy))?;
    let output = pace(&fixture).arg("doctor").output()?;
    assert!(!output.status.success());
    let report = json(&output)?;
    assert_eq!(
        report["endpoints"][0]["route"],
        format!("configured proxy {proxy}")
    );
    let reason = report["endpoints"][0]["reason"]
        .as_str()
        .context("reason")?;
    assert!(reason.contains("connection refused"), "{reason}");
    assert!(
        reason.ends_with(&format!("(via configured proxy {proxy})")),
        "{reason}"
    );

    // Credentials never belong in the committed configuration.
    let with_credentials = proxy.replace("http://", "http://user:secret@");
    let text = fs::read_to_string(&fixture.config)?.replace(&proxy, &with_credentials);
    fs::write(&fixture.config, text)?;
    let output = pace(&fixture).arg("doctor").output()?;
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("without credentials") && !stderr.contains("secret"));
    Ok(())
}

#[test]
fn environment_route_is_decided_per_host_with_no_proxy() -> Result<()> {
    let fixture = Fixture::new()?;
    let (base, server) = answer_unauthorized(4)?;
    configure(&fixture, &base, None)?;
    let proxy = closed_base()?;
    let host = base.trim_start_matches("http://");
    let ip = host.split(':').next().context("loopback IP")?;
    // Loopback stand-ins use http://, so HTTP_PROXY is the variable the client consults.
    let with_proxy = |exempt: Option<&str>| {
        let mut command = pace(&fixture);
        command.env(
            "HTTP_PROXY",
            proxy.replace("http://", "http://user:secret@"),
        );
        if let Some(exempt) = exempt {
            command.env("NO_PROXY", exempt);
        }
        command
    };

    let exempted = with_proxy(Some(ip)).arg("doctor").output()?;
    let requests = server
        .join()
        .map_err(|_| anyhow::anyhow!("doctor fixture panicked"))??;
    assert_eq!(requests.len(), 4, "the client must have gone direct");
    assert!(
        exempted.status.success(),
        "{}",
        String::from_utf8_lossy(&exempted.stderr)
    );
    for endpoint in json(&exempted)?["endpoints"]
        .as_array()
        .context("endpoints")?
    {
        assert_eq!(endpoint["route"], "direct connection");
    }

    let proxied = with_proxy(None).arg("doctor").output()?;
    assert!(!proxied.status.success());
    let text = String::from_utf8_lossy(&proxied.stdout);
    assert!(!text.contains("secret"));
    let report = json(&proxied)?;
    let endpoint = &report["endpoints"][0];
    assert_eq!(endpoint["route"], format!("environment proxy {proxy}"));
    let reason = endpoint["reason"].as_str().context("reason")?;
    assert!(
        reason.ends_with(&format!("(via environment proxy {proxy})")),
        "{reason}"
    );
    Ok(())
}
