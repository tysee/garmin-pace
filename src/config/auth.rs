//! src/config/auth.rs defines and validates the explicit native Garmin login endpoints.
//! Test servers are limited to literal loopback; production credentials stay on Garmin HTTPS.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{net::IpAddr, path::PathBuf};

/// Native Connect login settings. Session tokens live outside committed configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GarminAuthSettings {
    pub session_path: PathBuf,
    pub sso_base_url: String,
    pub token_url: String,
    pub service_url: String,
    pub sso_client_id: String,
    pub oauth_client_id: String,
    pub sso_user_agent: String,
    pub api_user_agent: String,
    pub login_delay_ms: u64,
    pub refresh_margin_seconds: i64,
}
impl GarminAuthSettings {
    /// Validate exact production hosts and paths before any credentials are accepted.
    pub fn validate(&self) -> Result<()> {
        for (value, host, path) in [
            (&self.sso_base_url, "sso.garmin.com", "/"),
            (
                &self.token_url,
                "diauth.garmin.com",
                "/di-oauth2-service/oauth/token",
            ),
            (
                &self.service_url,
                "mobile.integration.garmin.com",
                "/gcm/ios",
            ),
        ] {
            let url = reqwest::Url::parse(value)
                .map_err(|_| anyhow::anyhow!("invalid Garmin auth URL"))?;
            let loopback = url
                .host_str()
                .and_then(|s| s.trim_matches(['[', ']']).parse::<IpAddr>().ok())
                .is_some_and(|ip| ip.is_loopback());
            ensure!(
                (url.scheme() == "https" && url.host_str() == Some(host) && url.port().is_none())
                    || (url.scheme() == "http" && loopback),
                "invalid Garmin auth host"
            );
            ensure!(
                url.path() == path
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "invalid Garmin auth URL path"
            );
        }
        ensure!(
            !self.session_path.as_os_str().is_empty(),
            "garmin_auth.session_path is required"
        );
        ensure!(
            self.sso_client_id == "GCM_IOS_DARK"
                && self.oauth_client_id == "GARMIN_CONNECT_MOBILE_ANDROID_DI_2025Q2",
            "unsupported Garmin login client configuration"
        );
        ensure!(
            !self.sso_user_agent.is_empty() && !self.api_user_agent.is_empty(),
            "Garmin login user agents are required"
        );
        ensure!(
            self.login_delay_ms <= 5000 && (30..=900).contains(&self.refresh_margin_seconds),
            "invalid Garmin auth timing bounds"
        );
        Ok(())
    }
}
