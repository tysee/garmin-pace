//! src/installation/https.rs validates public hostnames and renders the bounded TLS edge.
//! PublicDomain and proxy_config are pure inputs for the installer and its checks.

use anyhow::{Result, ensure};
use std::net::{IpAddr, SocketAddr};

/// A normalized public DNS hostname, without a scheme, port, wildcard or path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicDomain(String);

impl PublicDomain {
    /// Validate before interpolating the domain into Caddy configuration or HTTPS URLs.
    pub fn parse(value: &str) -> Result<Self> {
        ensure!(value.len() <= 253, "DNS hostname exceeds size limit");
        let name = value.to_ascii_lowercase();
        let labels: Vec<_> = name.split('.').collect();
        ensure!(
            labels.len() >= 2
                && name.parse::<IpAddr>().is_err()
                && labels.iter().all(|label| !label.is_empty()
                    && label.len() <= 63
                    && label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                    && !label.starts_with('-')
                    && !label.ends_with('-'))
                && labels
                    .last()
                    .is_some_and(|label| label.bytes().any(|byte| byte.is_ascii_alphabetic())),
            "enter a public DNS hostname only, such as mcp.your-domain.com (no scheme, port or path)"
        );
        for suffix in [
            "localhost",
            "local",
            "internal",
            "test",
            "invalid",
            "example",
            "onion",
            "home.arpa",
        ] {
            ensure!(
                name != suffix && !name.ends_with(&format!(".{suffix}")),
                "a publicly trusted certificate requires a public DNS hostname"
            );
        }
        Ok(Self(name))
    }

    /// Borrow the validated hostname.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The URL to configure in a Bearer-capable MCP client.
    pub fn mcp_url(&self) -> String {
        format!("https://{}/api/v1/mcp", self.0)
    }
}

/// Render a proxy that never exposes a non-loopback backend or unbounded HTTP body.
pub fn proxy_config(
    domain: &PublicDomain,
    upstream: SocketAddr,
    body_limit: usize,
    timeout: u64,
) -> Result<String> {
    ensure!(
        upstream.ip().is_loopback() && upstream.port() != 0,
        "HTTPS proxy requires a loopback MCP listener"
    );
    ensure!(
        (1024..=1_048_576).contains(&body_limit) && (1..=120).contains(&timeout),
        "invalid HTTPS resource bounds"
    );
    Ok(include_str!("../../deploy/Caddyfile")
        .replace("{$GARMIN_PACE_DOMAIN}", domain.as_str())
        .replace("{$GARMIN_PACE_UPSTREAM}", &upstream.to_string())
        .replace("{$GARMIN_PACE_BODY_LIMIT}", &body_limit.to_string())
        .replace("{$GARMIN_PACE_TIMEOUT}", &timeout.to_string()))
}
