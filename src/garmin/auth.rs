//! src/garmin/auth.rs implements a single native Garmin login flow with MFA and refresh.
//! Authentication never retries credentials or prints response bodies; passwords are not persisted.

use super::{
    ConnectApi,
    session::{Session, SessionStore},
};
use crate::config::{GarminAuthSettings, GarminSettings};
use crate::workouts::WorkoutGateway;
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use reqwest::blocking::{Client, Response};
use serde::Deserialize;
use serde_json::json;
use std::{io::Read, time::Duration};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginResponse {
    response_status: ResponseStatus,
    service_ticket_id: Option<String>,
    customer_mfa_info: Option<MfaInfo>,
}
#[derive(Deserialize)]
struct ResponseStatus {
    #[serde(rename = "type")]
    kind: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MfaInfo {
    mfa_last_method_used: String,
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
}
#[derive(Deserialize)]
struct Expiry {
    exp: i64,
}

struct AuthHttp<'a> {
    client: Client,
    auth: &'a GarminAuthSettings,
    network: &'a GarminSettings,
}
impl<'a> AuthHttp<'a> {
    fn new(auth: &'a GarminAuthSettings, network: &'a GarminSettings) -> Result<Self> {
        auth.validate()?;
        network.validate()?;
        let client = Client::builder()
            .cookie_store(true)
            .timeout(Duration::from_secs(network.request_timeout_seconds))
            .connect_timeout(Duration::from_secs(network.connect_timeout_seconds))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()?;
        Ok(Self {
            client,
            auth,
            network,
        })
    }
    fn body(&self, response: Result<Response, reqwest::Error>, stage: &str) -> Result<Vec<u8>> {
        let response = response.map_err(|_| {
            anyhow::anyhow!(
                "Garmin authentication network failure during {stage}; retry login explicitly"
            )
        })?;
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let delay = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| {
                    value.parse::<u64>().ok().or_else(|| {
                        chrono::DateTime::parse_from_rfc2822(value)
                            .ok()
                            .map(|date| {
                                date.timestamp()
                                    .saturating_sub(Utc::now().timestamp())
                                    .max(0) as u64
                            })
                    })
                });
            match delay {
                Some(seconds) => anyhow::bail!(
                    "Garmin authentication returned HTTP 429 during {stage}; retry after {seconds} seconds; no automatic retry"
                ),
                None => anyhow::bail!(
                    "Garmin authentication returned HTTP 429 during {stage}; server gave no retry delay; no automatic retry"
                ),
            }
        }
        ensure!(
            response.status().is_success(),
            "Garmin authentication returned HTTP {} during {stage}; login/refresh stopped",
            response.status().as_u16()
        );
        let mut data = Vec::new();
        response
            .take(self.network.max_response_bytes as u64 + 1)
            .read_to_end(&mut data)
            .map_err(|_| anyhow::anyhow!("reading Garmin authentication response failed"))?;
        ensure!(
            data.len() <= self.network.max_response_bytes,
            "Garmin authentication response exceeds size limit"
        );
        Ok(data)
    }
    fn sign_in_url(&self) -> Result<reqwest::Url> {
        let mut url = reqwest::Url::parse(&format!(
            "{}/mobile/sso/en_US/sign-in",
            self.auth.sso_base_url.trim_end_matches('/')
        ))
        .context("constructing Garmin sign-in URL")?;
        url.query_pairs_mut()
            .append_pair("clientId", &self.auth.sso_client_id)
            .append_pair("service", &self.auth.service_url);
        Ok(url)
    }
    fn post_sso(&self, path: &str, payload: serde_json::Value) -> Result<LoginResponse> {
        let response = self
            .client
            .post(format!(
                "{}{path}",
                self.auth.sso_base_url.trim_end_matches('/')
            ))
            .query(&[
                ("clientId", self.auth.sso_client_id.as_str()),
                ("locale", "en-US"),
                ("service", self.auth.service_url.as_str()),
            ])
            .header("User-Agent", &self.auth.sso_user_agent)
            .header("Origin", &self.auth.sso_base_url)
            .header("Accept", "application/json, text/plain, */*")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Referer", self.sign_in_url()?.as_str())
            .json(&payload)
            .send();
        let stage = if path == "/mobile/api/login" {
            "SSO credentials"
        } else {
            "MFA verification"
        };
        serde_json::from_slice(&self.body(response, stage)?).map_err(|_| {
            anyhow::anyhow!(
                "Garmin login returned an unexpected response; browser challenge may be required"
            )
        })
    }
    fn token(&self, form: &[(&str, &str)]) -> Result<TokenResponse> {
        let response = self
            .client
            .post(&self.auth.token_url)
            .basic_auth(&self.auth.oauth_client_id, Some(""))
            .header("User-Agent", &self.auth.api_user_agent)
            .header("Accept", "application/json")
            .form(form)
            .send();
        serde_json::from_slice(&self.body(response, "OAuth token exchange")?)
            .map_err(|_| anyhow::anyhow!("Garmin returned an invalid token response"))
    }
}

/// Complete interactive credentials/MFA login, verify access and save only OAuth tokens.
pub fn login(
    auth: &GarminAuthSettings,
    network: &GarminSettings,
    email: &str,
    password: &str,
    mfa: impl FnOnce() -> Result<String>,
) -> Result<()> {
    ensure!(
        !email.trim().is_empty() && !password.is_empty(),
        "email and password are required"
    );
    let store = SessionStore::lock(&auth.session_path)?;
    let http = AuthHttp::new(auth, network)?;
    let response = http
        .client
        .get(http.sign_in_url()?)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("User-Agent", &auth.sso_user_agent)
        .send();
    http.body(response, "SSO sign-in page")?;
    std::thread::sleep(Duration::from_millis(auth.login_delay_ms));
    let mut login = http.post_sso(
        "/mobile/api/login",
        json!({"username":email,"password":password,"rememberMe":true,"captchaToken":""}),
    )?;
    if login.response_status.kind == "MFA_REQUIRED" {
        let method = login
            .customer_mfa_info
            .context("Garmin did not specify an MFA method")?
            .mfa_last_method_used;
        let code = mfa()?;
        ensure!(!code.trim().is_empty(), "MFA code is required");
        login=http.post_sso("/mobile/api/mfa/verifyCode",json!({"mfaMethod":method,"mfaVerificationCode":code,"rememberMyBrowser":true,"reconsentList":[],"mfaSetup":false}))?;
    }
    ensure!(
        login.response_status.kind == "SUCCESSFUL",
        "Garmin rejected login or requires a browser challenge; session not saved"
    );
    let ticket = login
        .service_ticket_id
        .context("Garmin did not return a service ticket")?;
    let tokens = http.token(&[
        ("client_id", &auth.oauth_client_id),
        ("service_ticket", &ticket),
        (
            "grant_type",
            "https://connectapi.garmin.com/di-oauth2-service/oauth/grant/service_ticket",
        ),
        ("service_url", &auth.service_url),
    ])?;
    let session = build_session(tokens, None, auth, network)?;
    ConnectApi::with_token(network, &auth.api_user_agent, &session.access_token)?.devices()?;
    store.save(&session)
}

/// Read or renew a saved native access token under an exclusive file lock.
pub(super) fn access_token(auth: &GarminAuthSettings, network: &GarminSettings) -> Result<String> {
    let store = SessionStore::lock(&auth.session_path)?;
    let mut session = store.load()?;
    ensure!(
        session.client_id == auth.oauth_client_id
            && session.token_url == auth.token_url
            && session.api_base_url == network.connect_base_url,
        "saved Garmin session belongs to a different configuration; run garmin login"
    );
    if session.expires_at <= Utc::now().timestamp() + auth.refresh_margin_seconds {
        let http = AuthHttp::new(auth, network)?;
        let tokens = http
            .token(&[
                ("grant_type", "refresh_token"),
                ("client_id", &auth.oauth_client_id),
                ("refresh_token", &session.refresh_token),
            ])
            .context("renewing Garmin session failed; run garmin login")?;
        session = build_session(tokens, Some(session.refresh_token), auth, network)?;
        store.save(&session)?;
    }
    Ok(session.access_token)
}

fn build_session(
    tokens: TokenResponse,
    previous_refresh: Option<String>,
    auth: &GarminAuthSettings,
    network: &GarminSettings,
) -> Result<Session> {
    super::http::secret_header(&tokens.access_token)?;
    let encoded = tokens
        .access_token
        .split('.')
        .nth(1)
        .context("Garmin access token has no expiration payload")?;
    let decoded = URL_SAFE_NO_PAD
        .decode(encoded.trim_end_matches('='))
        .map_err(|_| anyhow::anyhow!("invalid Garmin token encoding"))?;
    let expiry: Expiry = serde_json::from_slice(&decoded)
        .map_err(|_| anyhow::anyhow!("Garmin access token has no valid expiry"))?;
    ensure!(
        expiry.exp > Utc::now().timestamp() + auth.refresh_margin_seconds,
        "Garmin returned an expired access token"
    );
    // OAuth refresh responses may omit a replacement refresh token; retain the existing grant.
    let refresh_token = tokens
        .refresh_token
        .or(previous_refresh)
        .context("Garmin did not return a refresh token")?;
    ensure!(!refresh_token.is_empty(), "Garmin refresh token is empty");
    Ok(Session {
        version: 1,
        access_token: tokens.access_token,
        refresh_token,
        expires_at: expiry.exp,
        client_id: auth.oauth_client_id.clone(),
        token_url: auth.token_url.clone(),
        api_base_url: network.connect_base_url.clone(),
    })
}

/// Return local token expiry only, without claiming that Garmin still accepts the session.
pub fn session_expiry(auth: &GarminAuthSettings) -> Result<i64> {
    Ok(SessionStore::lock(&auth.session_path)?.load()?.expires_at)
}
/// Report saved-session expiry, or None without touching the filesystem when no session exists.
pub fn saved_session_expiry(auth: &GarminAuthSettings) -> Result<Option<i64>> {
    if !auth.session_path.try_exists()? {
        return Ok(None);
    }
    session_expiry(auth).map(Some)
}
/// Remove the local session; this does not revoke credentials on Garmin's server.
pub fn logout(auth: &GarminAuthSettings) -> Result<()> {
    SessionStore::lock(&auth.session_path)?.remove()
}
