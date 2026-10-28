//! src/garmin/doctor.rs checks the network path to every configured Garmin host without keys.
//! It reuses the shared Garmin client (timeouts, proxy, trust roots) for one paced, unauthenticated
//! GET per endpoint; any HTTP status proves DNS, proxy and TLS work. It exposes `diagnose`.

use super::{http, pacing};
use crate::config::{GarminAuthSettings, GarminSettings};
use anyhow::{Context, Result};
use std::{path::PathBuf, time::Duration};

/// Result of one keyless HTTPS request to a Garmin host.
pub enum ProbeOutcome {
    /// Garmin answered; a 401/404 here still proves the network path, not authorization.
    Reachable {
        status: u16,
    },
    Failed {
        reason: String,
    },
}

pub struct Probe {
    pub endpoint: &'static str,
    pub host: String,
    /// Direct connection or the proxy this endpoint's requests use, after NO_PROXY.
    pub route: String,
    pub outcome: ProbeOutcome,
}

/// A trust-store variable from the environment and whether its path exists (never its content).
pub struct CertificateVariable {
    pub name: &'static str,
    pub path: String,
    pub exists: bool,
}

/// Trust settings in effect for this process and the probe of each Garmin endpoint.
pub struct NetworkReport {
    pub extra_ca_file: Option<PathBuf>,
    pub certificate_variables: Vec<CertificateVariable>,
    pub probes: Vec<Probe>,
}

/// Probe the Personal API, Connect API, SSO and token hosts with the client Pace uses.
pub fn diagnose(network: &GarminSettings, auth: &GarminAuthSettings) -> Result<NetworkReport> {
    let client = http::client_builder(network)?
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .context("creating Garmin HTTP client")?;
    let interval = Duration::from_millis(network.request_interval_ms);
    let mut probes = Vec::new();
    for (endpoint, url) in [
        ("personal_api", network.personal_base_url.as_str()),
        ("connect_api", network.connect_base_url.as_str()),
        ("sso", auth.sso_base_url.as_str()),
        ("oauth_token", auth.token_url.as_str()),
    ] {
        let mut origin = reqwest::Url::parse(url).context("invalid Garmin URL")?;
        origin.set_path("/");
        let base = origin.as_str().trim_end_matches('/').to_owned();
        pacing::before_request(&base, interval)?;
        let route = http::route(network, origin.as_str());
        let outcome = match client.get(origin).send() {
            Ok(response) => ProbeOutcome::Reachable {
                status: response.status().as_u16(),
            },
            Err(error) => ProbeOutcome::Failed {
                reason: http::describe(&error, network),
            },
        };
        probes.push(Probe {
            endpoint,
            host: http::host(url),
            route,
            outcome,
        });
    }
    Ok(NetworkReport {
        extra_ca_file: network.extra_ca_file.clone(),
        certificate_variables: ["SSL_CERT_FILE", "SSL_CERT_DIR"]
            .into_iter()
            .filter_map(|name| {
                let path = std::env::var_os(name).filter(|value| !value.is_empty())?;
                let path = PathBuf::from(path);
                Some(CertificateVariable {
                    name,
                    exists: path.exists(),
                    path: path.display().to_string(),
                })
            })
            .collect(),
        probes,
    })
}
