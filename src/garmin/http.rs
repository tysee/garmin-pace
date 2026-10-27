//! src/garmin/http.rs owns bounded HTTP requests to Garmin and sanitized failures.
//! It exposes one-shot writes and limited retries for idempotent reads only. Every request is
//! paced per host; reads wait out HTTP 429 as Garmin asks, within configured bounds.

use super::pacing::{self, RateWait};
use crate::config::GarminSettings;
use anyhow::{Context, Result, bail, ensure};
use reqwest::{
    Method, StatusCode,
    blocking::{Client, Response},
    header::{HeaderMap, HeaderValue, RETRY_AFTER},
};
use serde::de::DeserializeOwned;
use std::{
    io::Read,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Garmin answered with a non-success status: for 4xx the request was definitely not applied.
#[derive(Debug, thiserror::Error)]
#[error("Garmin returned HTTP {status}; request stopped")]
pub(super) struct Rejected {
    pub status: u16,
}
impl Rejected {
    /// Client errors are definite refusals; server errors leave the outcome uncertain.
    pub(super) fn definite(error: &anyhow::Error) -> Option<u16> {
        error
            .downcast_ref::<Self>()
            .filter(|rejected| (400..500).contains(&rejected.status))
            .map(|rejected| rejected.status)
    }
}

/// Garmin kept answering HTTP 429 to a read after every permitted wait.
#[derive(Debug, thiserror::Error)]
#[error(
    "Garmin rate limit (HTTP 429) persisted after {waits} waits; later requests are now spaced further apart. Wait a few minutes and continue"
)]
pub(super) struct RateLimited {
    pub waits: u32,
}

/// Without Retry-After, the first wait for a rate-limited read; it doubles per retry.
const RATE_LIMIT_FALLBACK_SECONDS: u64 = 15;

pub(super) struct GarminHttp {
    client: Client,
    base: String,
    headers: HeaderMap,
    settings: GarminSettings,
}

impl GarminHttp {
    pub(super) fn new(base: &str, headers: HeaderMap, settings: &GarminSettings) -> Result<Self> {
        settings.validate()?;
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(settings.connect_timeout_seconds))
            .timeout(Duration::from_secs(settings.request_timeout_seconds))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .context("creating Garmin HTTP client")?;
        Ok(Self {
            client,
            base: base.trim_end_matches('/').into(),
            headers,
            settings: settings.clone(),
        })
    }

    pub(super) fn json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&serde_json::Value>,
    ) -> Result<T> {
        let data = self.request(method, path, query, body)?;
        serde_json::from_slice(&data)
            .map_err(|_| anyhow::anyhow!("Garmin returned an unexpected JSON response"))
    }

    pub(super) fn request(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&serde_json::Value>,
    ) -> Result<Vec<u8>> {
        let read = method == Method::GET;
        let retries = if read { self.settings.read_retries } else { 0 };
        let rate_retries = if read {
            self.settings.rate_limit_retries
        } else {
            0
        };
        let interval = Duration::from_millis(self.settings.request_interval_ms);
        let (mut attempt, mut waits) = (0, 0);
        loop {
            pacing::before_request(&self.base, interval)?;
            let mut request = self
                .client
                .request(method.clone(), format!("{}{path}", self.base))
                .headers(self.headers.clone())
                .query(query);
            if let Some(body) = body {
                request = request.json(body);
            }
            match request.send() {
                Ok(response) => {
                    let status = response.status();
                    if status == StatusCode::TOO_MANY_REQUESTS {
                        pacing::slow_down(&self.base, interval)?;
                        if waits < rate_retries {
                            let seconds = self.rate_limit_wait(&response, waits)?;
                            waits += 1;
                            pacing::wait(&RateWait {
                                seconds,
                                attempt: waits,
                                attempts: rate_retries,
                            })?;
                            continue;
                        }
                        if read {
                            return Err(RateLimited { waits }.into());
                        }
                    }
                    if status.is_server_error() && attempt < retries {
                        self.backoff(attempt)?;
                        attempt += 1;
                        continue;
                    }
                    if !status.is_success() {
                        return Err(Rejected {
                            status: status.as_u16(),
                        }
                        .into());
                    }
                    let mut data = Vec::new();
                    response.take(self.settings.max_response_bytes as u64 + 1).read_to_end(&mut data)
                        .map_err(|_| anyhow::anyhow!("reading Garmin response failed; writes may have an uncertain outcome"))?;
                    ensure!(
                        data.len() <= self.settings.max_response_bytes,
                        "Garmin response exceeds configured size limit"
                    );
                    return Ok(data);
                }
                Err(error) if (error.is_timeout() || error.is_connect()) && attempt < retries => {
                    self.backoff(attempt)?;
                    attempt += 1;
                }
                Err(_) => bail!(
                    "Garmin network request failed; writes may have an uncertain outcome and must not be repeated blindly"
                ),
            }
        }
    }

    /// Follow Garmin's Retry-After seconds, or double a fallback; never beyond the configured cap.
    fn rate_limit_wait(&self, response: &Response, waits: u32) -> Result<u64> {
        let cap = self.settings.rate_limit_max_wait_seconds;
        let asked = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok());
        match asked {
            Some(seconds) => {
                ensure!(
                    seconds <= cap,
                    "Garmin rate limit asks to wait {seconds} s, longer than garmin.rate_limit_max_wait_seconds ({cap}); continue later"
                );
                Ok(seconds.max(1))
            }
            None => Ok((RATE_LIMIT_FALLBACK_SECONDS << waits.min(8)).min(cap)),
        }
    }

    fn backoff(&self, attempt: u32) -> Result<()> {
        let base = self.settings.retry_base_ms * (1u64 << attempt);
        let jitter = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("reading retry clock")?
            .subsec_nanos() as u64
            % base;
        eprintln!("warning: transient Garmin failure; retrying read");
        std::thread::sleep(Duration::from_millis(base + jitter));
        Ok(())
    }
}

pub(super) fn secret_header(value: &str) -> Result<HeaderValue> {
    ensure!(
        !value.is_empty() && value.bytes().all(|b| b.is_ascii_graphic()),
        "credential must be nonempty ASCII without whitespace"
    );
    let mut header =
        HeaderValue::from_str(value).map_err(|_| anyhow::anyhow!("invalid credential header"))?;
    header.set_sensitive(true);
    Ok(header)
}
