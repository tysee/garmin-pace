//! src/interface/mcp_history.rs exposes versioned, bounded local activity searches and
//! connection diagnostics so a model can identify its capabilities and inspect older history.

use super::{
    mcp::{HistoryMcp, history_error, join_error, result},
    responses::{ActivityResponse, StatusResponse},
};
use crate::{health::Date, history::ActivityQuery};
use rmcp::{
    ErrorData, RoleServer, handler::server::wrapper::Parameters, model::CallToolResult, schemars,
    service::RequestContext, tool, tool_router,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchInput {
    /// Inclusive UTC date, YYYY-MM-DD. This differs from Garmin's local-date sync filter.
    start: String,
    end: String,
    /// Exact sport key from stored activities; omitted includes all sports.
    sport: Option<String>,
    limit: usize,
    offset: Option<usize>,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ActivityInput {
    garmin_id: String,
}

#[tool_router(router = history_tools, vis = "pub(super)")]
impl HistoryMcp {
    #[tool(
        description = "Read this connection's toolset version, Garmin grant, server busy state and local activity/health coverage. No network access, credentials or filesystem paths. Use after reconnecting and after a timeout; busy is server-wide, and an idle server does not resolve a pending write.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn get_capabilities_v1(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let profile = self.profile(&context)?;
        let allowed = profile.garmin.is_some();
        let busy = self.network_busy();
        let permit = self.blocking_permit().await?;
        let value = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let history = profile.history.summary().map_err(history_error)?;
            let health = profile.health.summary().map_err(super::mcp::health_error)?;
            Ok(json!({"contract_version":1,"toolset_version":"2026-10-27-coach-targets.1","coach_support":{"window_days":7,"relative_load":true,"minimum_repetitions":true,"adaptation":"reviewed_drafts","rescheduling":false},"garmin_access":allowed,
                "garmin_operation_running":busy,"history":StatusResponse::from(history),
                "health":{"day_count":health.day_count,"latest":health.latest.map(|d|d.to_string())},
                "watch_receipt_supported":false,"sync_credentials":"profile_private_file"}))
        }).await.map_err(join_error)??;
        result(value)
    }
    #[tool(
        description = "Search stored activities by inclusive UTC dates and optional exact sport, newest first, with limit and offset. Returns next_offset; a full page may be followed by an empty page. No Garmin refresh occurs; use sync_activities_v1 explicitly when needed.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn search_activities_v1(
        &self,
        Parameters(input): Parameters<SearchInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let query = ActivityQuery {
            start: Some(
                input
                    .start
                    .parse::<Date>()
                    .map_err(super::mcp::health_error)?
                    .into(),
            ),
            end: Some(
                input
                    .end
                    .parse::<Date>()
                    .map_err(super::mcp::health_error)?
                    .into(),
            ),
            sport: input.sport,
            name: None,
            limit: input.limit,
            offset: input.offset.unwrap_or(0),
        };
        let history = self.profile(&context)?.history;
        let permit = self.blocking_permit().await?;
        let value = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let rows = history.search(&query).map_err(history_error)?.activities;
            let next = (rows.len()==query.limit).then_some(query.offset+rows.len());
            Ok(json!({"activities":rows.into_iter().map(ActivityResponse::from).collect::<Vec<_>>(),"next_offset":next,"scope":"local_snapshot","date_basis":"UTC"}))
        }).await.map_err(join_error)??;
        result(value)
    }
    #[tool(
        description = "Read one saved activity by its exact Garmin ID even when it is outside the recent history page. This is a summary; get_activity_sets_v1 reads strength sets. Missing means not stored locally.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn get_activity_v1(
        &self,
        Parameters(input): Parameters<ActivityInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let history = self.profile(&context)?.history;
        let permit = self.blocking_permit().await?;
        let row = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            history.find(&input.garmin_id)
        })
        .await
        .map_err(join_error)?
        .map_err(history_error)?;
        result(json!({"activity":row.map(ActivityResponse::from),"scope":"local_snapshot"}))
    }
}
