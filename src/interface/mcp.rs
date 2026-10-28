//! src/interface/mcp.rs exposes bounded local history tools through the official
//! Rust MCP SDK. Blocking SQLite work runs off the async transport thread; Garmin
//! write tools exist only for profiles that carry an administrator's grant.

use super::responses::{ActivityPage, StatusResponse};
use crate::config::{GarminAuthSettings, GarminSettings, HealthSettings};
use crate::health::{HealthError, HealthService};
use crate::history::{HistoryError, HistoryService};
use anyhow::{Context, Result};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Implementation, ServerCapabilities, ServerConfig},
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListInput {
    /// Maximum activities to return; omitted uses the configured default.
    limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct DayInput {
    /// Garmin calendar date in YYYY-MM-DD; sleep belongs to wake-up day.
    date: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RangeInput {
    /// Inclusive Garmin calendar start date, YYYY-MM-DD.
    start: String,
    /// Inclusive end date, YYYY-MM-DD; at most configured max_days.
    end: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct SamplesInput {
    date: String,
    /// One of heart_rate, body_battery, hrv, spo2.
    metric: String,
    /// Page size, 1..=500 (subject to configured bounds).
    limit: usize,
    /// Zero-based offset; omitted starts at the first sample.
    offset: Option<usize>,
}

#[derive(Clone)]
pub(super) struct HistoryMcp {
    /// Fixed local profile for stdio; HTTP resolves the profile per authenticated request.
    local: Option<AuthenticatedHistory>,
    max_health_days: usize,
    default_limit: usize,
    blocking_gate: Arc<Semaphore>,
    /// One Garmin operation per process; a second caller is told to read the draft state.
    network_gate: Arc<Semaphore>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl HistoryMcp {
    pub(super) fn network_busy(&self) -> bool {
        self.network_gate.available_permits() == 0
    }
    pub(super) fn new(
        profile: AuthenticatedHistory,
        max_health_days: usize,
        default_limit: usize,
        blocking_threads: usize,
    ) -> Self {
        Self {
            local: Some(profile),
            max_health_days,
            default_limit,
            blocking_gate: Arc::new(Semaphore::new(blocking_threads)),
            network_gate: Arc::new(Semaphore::new(1)),
            tool_router: Self::tool_router()
                + Self::workout_tools()
                + Self::benchmark_tools()
                + Self::garmin_tools()
                + Self::sync_tools()
                + Self::history_tools()
                + Self::delivery_tools()
                + Self::coach_tools(),
        }
    }

    #[tool(
        description = "Read local history size and integration status. Status reflects the last persisted sync, not a live connection.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn get_status(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let history = self.profile(&context)?.history;
        let permit = self.blocking_permit().await?;
        let summary = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            history.summary()
        })
        .await
        .map_err(join_error)?
        .map_err(history_error)?;
        result(StatusResponse::from(summary))
    }

    #[tool(
        description = "Read a bounded page of locally stored activities, newest first. An empty list means no stored activities.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn list_activities(
        &self,
        Parameters(input): Parameters<ListInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let history = self.profile(&context)?.history;
        let limit = input.limit.unwrap_or(self.default_limit);
        let permit = self.blocking_permit().await?;
        let activities = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            history.list(limit, 0)
        })
        .await
        .map_err(join_error)?
        .map_err(history_error)?;
        result(ActivityPage::from(activities))
    }

    #[tool(
        description = "Read saved daily health: steps, heart rate, stress, sleep, HRV, Body Battery, SpO2, weight and all training readiness snapshots. Includes synchronization time and no-data markers; never a live watch reading.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn get_daily_health(
        &self,
        Parameters(input): Parameters<DayInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let date = input.date.parse().map_err(health_error)?;
        let health = self.profile(&context)?.health;
        let permit = self.blocking_permit().await?;
        let value = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            super::health_responses::day(&health, date)
        })
        .await
        .map_err(join_error)?
        .map_err(health_error)?;
        result(value)
    }
    #[tool(
        description = "Read saved daily health summaries for an inclusive YYYY-MM-DD date range, at most 31 days. Reports missing dates explicitly and excludes detailed samples. Readiness records retain their original timestamp/context; do not invent a morning score.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn list_health_metrics(
        &self,
        Parameters(input): Parameters<RangeInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let start = input.start.parse().map_err(health_error)?;
        let end = input.end.parse().map_err(health_error)?;
        let health = self.profile(&context)?.health;
        let max = self.max_health_days;
        let permit = self.blocking_permit().await?;
        let value = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            super::health_responses::days(&health, start, end, max)
        })
        .await
        .map_err(join_error)?
        .map_err(health_error)?;
        result(value)
    }
    #[tool(
        description = "Read a bounded page of stored heart_rate, body_battery, hrv or spo2 samples for one date. UTC timestamps, units, null gaps and next_offset are explicit. No network requests.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn get_health_samples(
        &self,
        Parameters(input): Parameters<SamplesInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let date = input.date.parse().map_err(health_error)?;
        let metric = input.metric.parse().map_err(health_error)?;
        let health = self.profile(&context)?.health;
        let permit = self.blocking_permit().await?;
        let value = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            super::health_responses::samples(
                &health,
                date,
                metric,
                input.limit,
                input.offset.unwrap_or(0),
            )
        })
        .await
        .map_err(join_error)?
        .map_err(health_error)?;
        result(value)
    }
}

/// Garmin session settings for a profile whose administrator granted model-initiated writes.
#[derive(Clone)]
pub(super) struct GarminAccess {
    pub settings: GarminSettings,
    pub auth: GarminAuthSettings,
    pub health: HealthSettings,
    pub personal_key_path: std::path::PathBuf,
}

/// One profile's local services; HTTP middleware attaches it, request bodies cannot construct it.
#[derive(Clone)]
pub(super) struct AuthenticatedHistory {
    pub history: HistoryService,
    pub health: HealthService,
    pub workouts: crate::workouts::WorkoutService,
    pub benchmarks: crate::workouts::benchmarks::BenchmarkService,
    /// None keeps the profile read-only towards Garmin.
    pub garmin: Option<GarminAccess>,
}

impl HistoryMcp {
    pub(super) fn authenticated(
        default_limit: usize,
        max_health_days: usize,
        blocking_gate: Arc<Semaphore>,
        network_gate: Arc<Semaphore>,
    ) -> Self {
        Self {
            local: None,
            max_health_days,
            default_limit,
            blocking_gate,
            network_gate,
            tool_router: Self::tool_router()
                + Self::workout_tools()
                + Self::benchmark_tools()
                + Self::garmin_tools()
                + Self::sync_tools()
                + Self::history_tools()
                + Self::delivery_tools()
                + Self::coach_tools(),
        }
    }
    /// Resolve the profile serving this call: the fixed stdio profile or the authenticated user.
    pub(super) fn profile(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Result<AuthenticatedHistory, ErrorData> {
        if let Some(local) = &self.local {
            return Ok(local.clone());
        }
        context
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<AuthenticatedHistory>())
            .cloned()
            .ok_or_else(|| ErrorData::internal_error("authenticated profile is missing", None))
    }
    /// Wait for a blocking slot; the transport's request timeout bounds the wait.
    pub(super) async fn blocking_permit(&self) -> Result<OwnedSemaphorePermit, ErrorData> {
        self.blocking_gate
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ErrorData::internal_error("history service is shutting down", None))
    }
    /// Claim the single Garmin slot without waiting; a busy answer tells the model to re-read state.
    pub(super) fn network_permit(&self) -> Result<OwnedSemaphorePermit, ErrorData> {
        self.network_gate.clone().try_acquire_owned().map_err(|_| {
            ErrorData::internal_error(
                "another Garmin operation is running on this server; read the draft state before retrying",
                Some(serde_json::json!({"outcome":"busy"})),
            )
        })
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for HistoryMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions("Read local SQLite history, health, Coach plans and reusable workout snapshots. For Coach requests explicitly import the current seven-day window with sync_coach_v1, read get_coach_v1 and observed results/benchmark context, then propose a source-linked draft with explanation. Show BEFORE/AFTER including recommended weights and affected dates; only save on a separate explicit instruction. Preserve relative loads and minimum repetitions; never infer kilograms from load labels. Re-import after save to check plan and schedule linkage. No background adaptation or changes to Coach preferences. Create or revise local workout drafts when asked; source ID/revision edits an existing workout, omitted source creates new. The user reviews drafts in Pace. Garmin tools (sync, refresh, save, reconcile, send, device list) work only for profiles an administrator granted; call them only on the user's explicit instruction in this conversation and quote it verbatim in user_request. One Garmin operation runs at a time; after a timeout or a busy answer read the draft state with get_workout_draft_v1/get_benchmark_draft_v1 and never save twice. Watch receipt is never confirmed. Never invent unsupported exercise IDs or replace unreadable features. Benchmark estimates are model-authored from the user's recent exact-exercise averages, with evidence, method and uncertainty. Propose only on an actual user request, or proactively for an unset benchmark required by a specific unsaved workout draft. Never proactively refresh filled benchmarks or invent permission. Review existing proposals before creating duplicates. Explicit granted sync tools refresh this profile in bounded pages or days. Read get_capabilities_v1 after a timeout; inspect local snapshots when idle. get_workout_deliveries_v1 returns status_label per revision: present accepted requests as Sent to watch, not as a live queue. The legacy queued code means Garmin accepted the send request, never verified watch receipt. Use search_activities_v1 for UTC date/sport/name pagination and search_exercises_v1 for known exact exercise identities. Activity history contains imported pages, not a complete archive. Health dates use Garmin's calendar; sleep belongs to wake-up day. Report synced_at, not_synced and no_data accurately. Body Battery is physiological energy, not device charge. Readiness snapshots retain original context; do not invent a morning reading or equate a saved score with live readiness. Never claim the watch is connected or that missing data proves no activity occurred.")
    }
}

pub(super) async fn serve_stdio(server: HistoryMcp) -> Result<()> {
    tracing::info!(transport = "stdio", "starting MCP server");
    let service = server
        .serve(rmcp::transport::stdio())
        .await
        .context("starting stdio MCP")?;
    service.waiting().await.context("serving stdio MCP")?;
    Ok(())
}

pub(super) fn result(value: impl Serialize) -> Result<CallToolResult, ErrorData> {
    let value = serde_json::to_value(value)
        .map_err(|_| ErrorData::internal_error("could not encode history response", None))?;
    Ok(CallToolResult::structured(value))
}

pub(super) fn history_error(error: HistoryError) -> ErrorData {
    match error {
        HistoryError::InvalidLimit { .. }
        | HistoryError::InvalidPeriod
        | HistoryError::InvalidQuery(_) => ErrorData::invalid_params(error.to_string(), None),
        HistoryError::Upstream(_) => ErrorData::internal_error("Garmin is unavailable", None),
        HistoryError::Storage(_) => {
            tracing::error!(error = %error, "history query failed");
            ErrorData::internal_error("local history storage is unavailable", None)
        }
    }
}

pub(super) fn join_error(error: tokio::task::JoinError) -> ErrorData {
    tracing::error!(error = %error, "history task failed");
    ErrorData::internal_error("local history task failed", None)
}

pub(super) fn health_error(error: HealthError) -> ErrorData {
    match error {
        HealthError::Invalid(_) => ErrorData::invalid_params(error.to_string(), None),
        HealthError::Upstream(_) => ErrorData::internal_error("Garmin health is unavailable", None),
        HealthError::Storage(_) => {
            tracing::error!("local health storage is unavailable");
            ErrorData::internal_error("local health storage is unavailable", None)
        }
    }
}
