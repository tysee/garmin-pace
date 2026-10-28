//! src/garmin/http.rs owns bounded HTTP requests to Garmin and sanitized failures.
//! It exposes the shared client factory (timeouts, proxy, extra trust roots), one-shot writes
//! and limited retries for idempotent reads only. Every request is paced per host; reads wait
//! out HTTP 429 as Garmin asks, within configured bounds. Network failures are classified
//! (DNS, proxy, TLS, timeout, refused) and name the host and route, never URLs or credentials.

use super::pacing::{self, RateWait};
use crate::config::GarminSettings;
use anyhow::{Context, Result, bail, ensure};
use reqwest::{
    Method, StatusCode,
    blocking::{Client, ClientBuilder, Response},
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
        let client = client_builder(settings)?
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
                Err(error) => bail!(
                    "Garmin network request to {} failed: {}; writes may have an uncertain outcome and must not be repeated blindly. Run `pace doctor` to check the network path",
                    host(&self.base),
                    describe(&error, &self.settings)
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

// --- shared client and failure diagnostics ---

/// Bounded client for every Garmin host: no redirects or library retries, configured proxy
/// (else the environment's) and extra trust roots merged into the platform store.
pub(super) fn client_builder(settings: &GarminSettings) -> Result<ClientBuilder> {
    settings.validate()?;
    let mut builder = Client::builder()
        .connect_timeout(Duration::from_secs(settings.connect_timeout_seconds))
        .timeout(Duration::from_secs(settings.request_timeout_seconds))
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never());
    if let Some(proxy) = &settings.proxy_url {
        builder = builder
            .proxy(reqwest::Proxy::all(proxy.as_str()).context("configuring garmin.proxy_url")?);
    }
    if let Some(path) = &settings.extra_ca_file {
        let pem = std::fs::read(path).context("reading garmin.extra_ca_file")?;
        let roots = reqwest::Certificate::from_pem_bundle(&pem)
            .map_err(|_| anyhow::anyhow!("garmin.extra_ca_file must contain PEM certificates"))?;
        ensure!(
            !roots.is_empty(),
            "garmin.extra_ca_file contains no certificates"
        );
        builder = builder.tls_certs_merge(roots);
    }
    Ok(builder)
}

/// Build the shared client without sending a request, proving the proxy and trust roots load.
pub fn check_client(settings: &GarminSettings) -> Result<()> {
    client_builder(settings)?
        .build()
        .context("building the Garmin HTTP client")?;
    Ok(())
}

/// How a request to `url` leaves this process: the configured proxy (reqwest then ignores the
/// environment), else the environment proxy chosen by the same hyper-util matcher reqwest applies,
/// including NO_PROXY exemptions per host. Proxy labels never carry credentials.
pub(super) fn route(settings: &GarminSettings, url: &str) -> String {
    if let Some(proxy) = &settings.proxy_url {
        return format!("configured proxy {}", redact_proxy(proxy));
    }
    let Ok(uri) = url.parse() else {
        return "route unknown: invalid URL".into();
    };
    match hyper_util::client::proxy::matcher::Matcher::from_system().intercept(&uri) {
        Some(proxy) => format!(
            "environment proxy {}",
            redact_proxy(&proxy.uri().to_string())
        ),
        None => "direct connection".into(),
    }
}

/// Show only the proxy's scheme, host and port; userinfo, paths and queries may hold secrets.
pub(super) fn redact_proxy(value: &str) -> String {
    let value = if value.contains("://") {
        value.to_owned()
    } else {
        format!("http://{value}")
    };
    match reqwest::Url::parse(&value) {
        Ok(url) => match (url.host_str(), url.port_or_known_default()) {
            (Some(host), Some(port)) => format!("{}://{host}:{port}", url.scheme()),
            (Some(host), None) => format!("{}://{host}", url.scheme()),
            _ => "(unparseable value)".into(),
        },
        Err(_) => "(unparseable value)".into(),
    }
}

/// Host part of a configured Garmin URL for messages; never the path or query.
pub(super) fn host(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|url| {
            url.host_str().map(|host| match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_owned(),
            })
        })
        .unwrap_or_else(|| "(invalid host)".into())
}

/// Classify a request that got no HTTP response into a fixed, secret-free reason, followed by
/// the route that request took. rustls and tunnel errors are not reachable as types without new
/// direct dependencies, so they are recognized by their stable Display markers.
pub(super) fn describe(error: &reqwest::Error, settings: &GarminSettings) -> String {
    let mut chain = String::new();
    let mut refused = false;
    let mut source: Option<&dyn std::error::Error> = Some(error);
    while let Some(current) = source {
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            refused |= io.kind() == std::io::ErrorKind::ConnectionRefused;
        }
        chain.push_str(&current.to_string().to_ascii_lowercase());
        chain.push('\n');
        source = current.source();
    }
    let reason = if chain.contains("proxy authorization required") {
        "the proxy requires authorization"
    } else if chain.contains("failed to create underlying connection") {
        // hyper-util wraps a failed connection to the proxy itself in its tunnel error.
        if error.is_dns() {
            "cannot reach the proxy: DNS lookup of the proxy host failed"
        } else if refused {
            "cannot reach the proxy: connection refused"
        } else if error.is_timeout() {
            "cannot reach the proxy: timed out"
        } else {
            "cannot reach the proxy"
        }
    } else if chain.contains("tunnel error") {
        "the proxy refused the tunnel (CONNECT not successful)"
    } else if chain.contains("certificate") || chain.contains("unknownissuer") {
        "TLS certificate is not trusted (an intercepting proxy needs its CA in garmin.extra_ca_file or SSL_CERT_FILE)"
    } else if error.is_timeout() {
        "timed out"
    } else if error.is_dns() {
        "DNS lookup failed"
    } else if refused {
        "connection refused"
    } else if chain.contains("tls") || chain.contains("handshake") {
        "TLS handshake failed"
    } else if error.is_connect() {
        "could not connect"
    } else {
        "no HTTP response"
    };
    // Through a proxy, connect and DNS failures concern the proxy itself.
    match error.url() {
        Some(url) => match route(settings, url.as_str()) {
            direct if direct == "direct connection" => format!("{reason} ({direct})"),
            proxy => format!("{reason} (via {proxy})"),
        },
        None => format!("{reason} (route unknown: no request URL)"),
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

#[cfg(test)]
mod tests {
    use super::{host, redact_proxy};

    #[test]
    fn proxy_and_host_labels_never_carry_credentials_or_paths() {
        assert_eq!(
            redact_proxy("http://user:token@proxy.internal:3128/path?x=1"),
            "http://proxy.internal:3128"
        );
        assert_eq!(
            redact_proxy("proxy.internal:8080"),
            "http://proxy.internal:8080"
        );
        assert_eq!(
            redact_proxy("https://proxy.internal"),
            "https://proxy.internal:443"
        );
        assert_eq!(redact_proxy("::not a url::"), "(unparseable value)");
        assert_eq!(host("https://sso.garmin.com/"), "sso.garmin.com");
        assert_eq!(host("http://127.0.0.1:8080/x?y=1"), "127.0.0.1:8080");
    }
}
