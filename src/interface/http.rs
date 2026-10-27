//! src/interface/http.rs authenticates each HTTP request against the local access registry.
//! Only the verified user's history enters MCP; socket and request bounds also cover unauthenticated traffic.

use super::{
    listener::BoundedListener,
    mcp::{AuthenticatedHistory, GarminAccess, HistoryMcp},
};
use crate::{
    access::{AccessError, AccessService, key_digest},
    config::{DatabaseSettings, GarminAuthSettings, GarminSettings, HealthSettings, Settings},
    health::{HealthService, Limits, SqliteHealth},
    history::{HistoryService, SqliteHistory},
};
use anyhow::{Context, Result};
use axum::{
    Router,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use std::{future::IntoFuture, sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tower::limit::ConcurrencyLimitLayer;
use tower_http::{limit::RequestBodyLimitLayer, timeout::TimeoutLayer};

#[derive(Clone)]
struct AuthState {
    access: AccessService,
    database: DatabaseSettings,
    garmin: GarminSettings,
    garmin_auth: GarminAuthSettings,
    health_settings: HealthSettings,
    max_page_size: usize,
    health_limits: Limits,
    allowed_hosts: Vec<String>,
    blocking_gate: Arc<Semaphore>,
}

pub(super) async fn serve(access: AccessService, settings: &Settings) -> Result<()> {
    let http = &settings.http;
    let cancellation = CancellationToken::new();
    let config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(http.allowed_hosts.clone())
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_max_request_body_bytes(http.max_body_bytes)
        .with_cancellation_token(cancellation.child_token());
    let default_limit = settings.history.default_page_size;
    let max_health_days = settings.health.max_days;
    let blocking_gate = Arc::new(Semaphore::new(settings.runtime.blocking_threads));
    let server_gate = blocking_gate.clone();
    let network_gate = Arc::new(Semaphore::new(1));
    let service = StreamableHttpService::new(
        move || {
            Ok(HistoryMcp::authenticated(
                default_limit,
                max_health_days,
                server_gate.clone(),
                network_gate.clone(),
            ))
        },
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let state = AuthState {
        access,
        database: settings.database.clone(),
        garmin: settings.garmin.clone(),
        garmin_auth: settings.garmin_auth.clone(),
        health_settings: settings.health.clone(),
        max_page_size: settings.history.max_page_size,
        health_limits: settings.health.limits(),
        allowed_hosts: http.allowed_hosts.clone(),
        blocking_gate,
    };
    let router = Router::new()
        .nest_service("/api/v1/mcp", service)
        .route(
            "/api/v1/health",
            get(|| async { axum::Json(serde_json::json!({"status":"ok"})) }),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), authorize))
        // Documentation is explicitly public; API routes above always require a key.
        .merge(super::http_docs::router())
        .layer(RequestBodyLimitLayer::new(http.max_body_bytes))
        .layer(middleware::from_fn_with_state(state, request_boundary))
        .layer(ConcurrencyLimitLayer::new(http.max_in_flight))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(http.request_timeout_seconds),
        ));
    let listener = tokio::net::TcpListener::bind(http.bind)
        .await
        .context("binding HTTP listener")?;
    let listener = BoundedListener::new(
        listener,
        http.max_connections,
        Duration::from_secs(http.connection_lifetime_seconds),
    );
    tracing::info!(address=%http.bind,"HTTP MCP server listening");
    if !http.bind.ip().is_loopback() {
        tracing::warn!("non-loopback HTTP listener requires a protected network and HTTPS ingress");
    }
    let server = axum::serve(listener, router)
        .with_graceful_shutdown(cancellation.clone().cancelled_owned())
        .into_future();
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => result.context("serving HTTP"),
        signal = shutdown_signal() => {
            cancellation.cancel();
            server.await.context("stopping HTTP server")?;
            signal
        }
    }
}

async fn request_boundary(
    State(state): State<AuthState>,
    request: Request,
    next: Next,
) -> Response {
    if !allowed_host(&request, &state.allowed_hosts) || !same_origin(&request) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if request.uri().query().is_some() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn same_origin(request: &Request) -> bool {
    let mut origins = request.headers().get_all(header::ORIGIN).iter();
    let Some(origin) = origins.next() else {
        return true;
    };
    if origins.next().is_some() {
        return false;
    }
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    let expected = host.and_then(|host| reqwest::Url::parse(&format!("http://{host}")).ok());
    expected.is_some_and(|url| {
        origin.to_str().ok() == Some(url.origin().ascii_serialization().as_str())
    })
}

async fn authorize(State(state): State<AuthState>, mut request: Request, next: Next) -> Response {
    let digest = match bearer(&request).and_then(|token| key_digest(token).ok()) {
        Some(digest) => digest,
        None => return unauthorized(),
    };
    // The request timeout bounds this wait; the gate only serializes blocking SQLite work.
    let permit = match state.blocking_gate.clone().acquire_owned().await {
        Ok(permit) => permit,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let user = state
            .access
            .authorize(&digest, chrono::Utc::now().timestamp())?;
        // The server config's Garmin endpoints apply to every profile; only the session is per user.
        let personal_key_path = crate::secrets::profile_key_path(&user.database_path)
            .map_err(|e| AccessError::Storage(e.into()))?;
        let garmin = user.garmin_writes.then(|| GarminAccess {
            settings: state.garmin.clone(),
            health: state.health_settings.clone(),
            personal_key_path,
            auth: GarminAuthSettings {
                session_path: user.session_path,
                ..state.garmin_auth.clone()
            },
        });
        let settings = DatabaseSettings {
            path: user.database_path,
            ..state.database
        };
        // One validating open covers the profile; the other adapters attach without I/O.
        let history =
            SqliteHistory::open(settings.clone()).map_err(|e| AccessError::Storage(e.into()))?;
        Ok::<_, AccessError>(AuthenticatedHistory {
            history: HistoryService::new(Arc::new(history), state.max_page_size),
            health: HealthService::new(
                Arc::new(SqliteHealth::attach(settings.clone())),
                state.health_limits,
            ),
            workouts: crate::workouts::WorkoutService::new(Arc::new(
                crate::workouts::SqliteWorkouts::attach(settings.clone()),
            )),
            benchmarks: crate::workouts::benchmarks::BenchmarkService::new(Arc::new(
                crate::workouts::benchmarks::SqliteBenchmarks::attach(settings),
            )),
            garmin,
        })
    })
    .await;
    match result {
        Ok(Ok(history)) => {
            request.headers_mut().remove(header::AUTHORIZATION);
            request.extensions_mut().insert(history);
            next.run(request).await
        }
        Ok(Err(AccessError::Denied)) => unauthorized(),
        _ => {
            tracing::error!("access registry or user history unavailable");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

fn bearer(request: &Request) -> Option<&str> {
    if request
        .headers()
        .get_all(header::AUTHORIZATION)
        .iter()
        .count()
        != 1
    {
        return None;
    }
    let value = request
        .headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("Bearer").then_some(token)
}
fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [
            (header::WWW_AUTHENTICATE, "Bearer realm=\"garmin-pace\""),
            (header::CACHE_CONTROL, "no-store"),
        ],
    )
        .into_response()
}
fn allowed_host(request: &Request, allowed: &[String]) -> bool {
    if request.headers().get_all(header::HOST).iter().count() != 1 {
        return false;
    }
    let Some(authority) = request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.parse::<axum::http::uri::Authority>().ok())
    else {
        return false;
    };
    allowed.iter().any(|entry| {
        entry.eq_ignore_ascii_case(authority.as_str())
            || entry.eq_ignore_ascii_case(authority.host().trim_matches(['[', ']']))
    })
}
async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .context("registering SIGTERM")?;
        tokio::select! { result=tokio::signal::ctrl_c()=>result.context("waiting for SIGINT"), _=terminate.recv()=>Ok(()) }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c()
        .await
        .context("waiting for interrupt")
}
