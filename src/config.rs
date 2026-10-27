//! src/config.rs loads the explicit TOML configuration and validates resource
//! bounds before startup. It exposes Settings and creation of a starter config.

use std::{
    fs,
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
};

mod auth;
pub use auth::GarminAuthSettings;
mod health;
pub use health::HealthSettings;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

/// Non-secret configuration, loaded once at the composition boundary.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub database: DatabaseSettings,
    pub history: HistorySettings,
    #[serde(default)]
    pub health: HealthSettings,
    pub http: HttpSettings,
    pub runtime: RuntimeSettings,
    pub garmin: GarminSettings,
    pub garmin_auth: GarminAuthSettings,
    pub access: AccessSettings,
}

/// Non-secret registry location and default key lifetime for local administrators.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccessSettings {
    pub registry_path: PathBuf,
    pub key_lifetime_days: u32,
}

/// SQLite location and bounded cache/lock policy.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseSettings {
    pub path: PathBuf,
    pub busy_timeout_ms: u64,
    pub cache_kib: u32,
}

/// Pagination limits shared by the CLI and MCP use cases.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HistorySettings {
    pub default_page_size: usize,
    pub max_page_size: usize,
}

/// HTTP listener and request resource limits; credentials are kept separate.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HttpSettings {
    pub bind: SocketAddr,
    pub allowed_hosts: Vec<String>,
    pub max_in_flight: usize,
    pub max_connections: usize,
    pub connection_lifetime_seconds: u64,
    pub max_body_bytes: usize,
    pub request_timeout_seconds: u64,
}

/// Runtime bounds for constrained devices.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSettings {
    pub blocking_threads: usize,
    pub log_level: String,
    /// stdio only: the local MCP client may call the Garmin write tools; HTTP uses the registry grant.
    #[serde(default)]
    pub stdio_garmin_writes: bool,
}

/// Garmin endpoint and bounded one-shot synchronization policy; secrets are separate.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GarminSettings {
    pub personal_base_url: String,
    pub connect_base_url: String,
    pub connect_timeout_seconds: u64,
    pub request_timeout_seconds: u64,
    pub max_response_bytes: usize,
    pub read_retries: u32,
    pub retry_base_ms: u64,
    /// Minimum spacing between requests to one Garmin host; doubles after HTTP 429.
    #[serde(default = "default_request_interval_ms")]
    pub request_interval_ms: u64,
    /// How many times one read waits out HTTP 429 before stopping.
    #[serde(default = "default_rate_limit_retries")]
    pub rate_limit_retries: u32,
    /// Longest single rate-limit wait Pace accepts from Garmin.
    #[serde(default = "default_rate_limit_max_wait_seconds")]
    pub rate_limit_max_wait_seconds: u64,
}

// Optional pacing tuning; profiles written before these settings existed keep loading.
fn default_request_interval_ms() -> u64 {
    1000
}
fn default_rate_limit_retries() -> u32 {
    3
}
fn default_rate_limit_max_wait_seconds() -> u64 {
    120
}

impl GarminSettings {
    /// Restrict credentials to Garmin or literal loopback test servers.
    pub fn validate(&self) -> Result<()> {
        for (value, host) in [
            (&self.personal_base_url, "personal.connectapi.garmin.com"),
            (&self.connect_base_url, "connectapi.garmin.com"),
        ] {
            let url = reqwest::Url::parse(value).context("invalid Garmin base URL")?;
            let loopback = url
                .host_str()
                .and_then(|s| s.trim_matches(['[', ']']).parse::<std::net::IpAddr>().ok())
                .is_some_and(|ip| ip.is_loopback());
            ensure!(
                (url.scheme() == "https" && url.host_str() == Some(host) && url.port().is_none())
                    || (url.scheme() == "http" && loopback),
                "Garmin URL must use its official HTTPS host or literal loopback for tests"
            );
            ensure!(
                url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "Garmin URL must not contain credentials, query or fragment"
            );
            ensure!(url.path() == "/", "invalid Garmin base URL path");
        }
        ensure!(
            (1..=30).contains(&self.connect_timeout_seconds),
            "garmin.connect_timeout_seconds must be 1..=30"
        );
        ensure!(
            (1..=120).contains(&self.request_timeout_seconds),
            "garmin.request_timeout_seconds must be 1..=120"
        );
        ensure!(
            (1024..=8_388_608).contains(&self.max_response_bytes),
            "garmin.max_response_bytes must be 1024..=8388608"
        );
        ensure!(
            self.read_retries <= 2 && (1..=5000).contains(&self.retry_base_ms),
            "invalid Garmin retry limits"
        );
        ensure!(
            (1..=60_000).contains(&self.request_interval_ms),
            "garmin.request_interval_ms must be 1..=60000"
        );
        ensure!(
            self.rate_limit_retries <= 10,
            "garmin.rate_limit_retries must be 0..=10"
        );
        ensure!(
            (1..=900).contains(&self.rate_limit_max_wait_seconds),
            "garmin.rate_limit_max_wait_seconds must be 1..=900"
        );
        Ok(())
    }
}

impl Settings {
    /// Read and validate a TOML file, resolving paths relative to that file.
    pub fn load(path: &Path) -> Result<Self> {
        let absolute = fs::canonicalize(path).context("locating configuration file")?;
        let text = fs::read_to_string(&absolute).context("reading configuration file")?;
        // Do not include TOML parser diagnostics: they can reproduce file contents.
        let mut settings: Self = toml::from_str(&text).map_err(|_| {
            anyhow::anyhow!("invalid configuration; check required fields and types")
        })?;
        settings.validate()?;
        settings.health.validate()?;
        settings.garmin.validate()?;
        settings.garmin_auth.validate()?;
        ensure!(
            !settings.access.registry_path.as_os_str().is_empty(),
            "access.registry_path is required"
        );
        ensure!(
            (1..=crate::access::MAX_KEY_LIFETIME_DAYS).contains(&settings.access.key_lifetime_days),
            "access.key_lifetime_days must be 1..=36500"
        );
        if settings.access.registry_path.is_relative() {
            settings.access.registry_path = absolute
                .parent()
                .context("configuration parent")?
                .join(&settings.access.registry_path);
        }
        if settings.database.path.is_relative() {
            let parent = absolute
                .parent()
                .context("configuration has no parent directory")?;
            settings.database.path = parent.join(&settings.database.path);
        }
        if settings.garmin_auth.session_path.is_relative() {
            let parent = absolute
                .parent()
                .context("configuration has no parent directory")?;
            settings.garmin_auth.session_path = parent.join(&settings.garmin_auth.session_path);
        }
        Ok(settings)
    }

    /// Use the server's bounded SQLite policy for the independent access registry.
    pub fn registry_database(&self) -> DatabaseSettings {
        DatabaseSettings {
            path: self.access.registry_path.clone(),
            ..self.database.clone()
        }
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            !self.database.path.as_os_str().is_empty(),
            "database.path must not be empty"
        );
        ensure!(
            (1..=60_000).contains(&self.database.busy_timeout_ms),
            "database.busy_timeout_ms must be 1..=60000"
        );
        ensure!(
            (64..=16_384).contains(&self.database.cache_kib),
            "database.cache_kib must be 64..=16384"
        );
        ensure!(
            (1..=1000).contains(&self.history.max_page_size),
            "history.max_page_size must be 1..=1000"
        );
        ensure!(
            (1..=self.history.max_page_size).contains(&self.history.default_page_size),
            "history.default_page_size must fit max_page_size"
        );
        ensure!(
            self.http.bind.port() != 0,
            "http.bind requires a nonzero port"
        );
        ensure!(
            !self.http.allowed_hosts.is_empty()
                && self
                    .http
                    .allowed_hosts
                    .iter()
                    .all(|host| !host.is_empty() && host != "*"),
            "http.allowed_hosts must contain explicit hostnames or IP addresses"
        );
        ensure!(
            (1..=128).contains(&self.http.max_connections),
            "http.max_connections must be 1..=128"
        );
        ensure!(
            (1..=300).contains(&self.http.connection_lifetime_seconds),
            "http.connection_lifetime_seconds must be 1..=300"
        );
        ensure!(
            (1..=32).contains(&self.http.max_in_flight),
            "http.max_in_flight must be 1..=32"
        );
        ensure!(
            (1024..=1_048_576).contains(&self.http.max_body_bytes),
            "http.max_body_bytes must be 1024..=1048576"
        );
        ensure!(
            (1..=120).contains(&self.http.request_timeout_seconds),
            "http.request_timeout_seconds must be 1..=120"
        );
        ensure!(
            (1..=4).contains(&self.runtime.blocking_threads),
            "runtime.blocking_threads must be 1..=4"
        );
        self.runtime
            .log_level
            .parse::<tracing::level_filters::LevelFilter>()
            .context("invalid runtime.log_level")?;
        Ok(())
    }
}

/// Create a starter configuration without overwriting an existing file.
pub fn create(path: &Path) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .context("creating configuration; target must not already exist")?;
    file.write_all(include_bytes!("../config.example.toml"))
        .context("writing configuration")?;
    file.sync_all().context("persisting configuration")
}
