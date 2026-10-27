//! src/interface/mcp_coach.rs exposes local Coach snapshots and explicit granted imports.
//! The existing journaled workout draft/save tools own all edits; this router only reads Garmin.

use super::mcp::HistoryMcp;
use crate::{
    garmin::ConnectApi,
    workouts::{
        WorkoutError,
        coach::document::{SnapshotDocument, window},
        validate_request,
    },
};
use rmcp::{
    ErrorData, RoleServer, handler::server::wrapper::Parameters, model::CallToolResult, schemars,
    service::RequestContext, tool, tool_router,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct WindowInput {
    /// Inclusive Garmin calendar dates, at most seven days; not UTC activity dates.
    start: String,
    end: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct SyncInput {
    start: String,
    end: String,
    /// Quote the user's actual instruction requesting this Coach import.
    user_request: String,
}
#[tool_router(router=coach_tools,vis="pub(super)")]
impl HistoryMcp {
    #[tool(
        description = "Read the exact locally imported Coach calendar window, at most seven Garmin calendar days. Returns plans, schedule IDs, workout IDs/revisions, complete definitions and activity links. not_synced means import was never requested. Snapshot content is historical; use get_workout_v1 for the latest local source after an edit. Read recorded sets, benchmark context and health before proposing user-requested changes. Coach drafts require an explanation of evidence, uncertainty and BEFORE/AFTER changes. A workout ID may affect several dates. Never infer kilogram loads from light/moderate/heavy labels or claim that a missing activity link proves no training.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn get_coach_v1(
        &self,
        Parameters(input): Parameters<WindowInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let dates = window(&input.start, &input.end).map_err(super::mcp_workouts::workout_error)?;
        self.workout_call(&context,move|service| {
            let snapshot=service.coach(dates)?;
            Ok(json!({"status":if snapshot.is_some(){"available"}else{"not_synced"},"snapshot":snapshot.map(SnapshotDocument::from)}))
        }).await
    }
    #[tool(
        description = "Explicitly import Coach plans and scheduled workouts for at most seven Garmin calendar days. Requires this profile's Garmin grant and Connect session. Never changes Garmin. Canonical workout reads and the calendar snapshot commit atomically; errors retain previous snapshots. Quote the user's request. After a save import again to check schedule/plan linkage; Garmin may independently adapt later. This is not a rescheduling or Coach-preferences tool.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn sync_coach_v1(
        &self,
        Parameters(input): Parameters<SyncInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        validate_request(&input.user_request).map_err(super::mcp_workouts::workout_error)?;
        let dates = window(&input.start, &input.end).map_err(super::mcp_workouts::workout_error)?;
        self.garmin_call(&context,move|profile,access| {
            let api=ConnectApi::connect(&access.settings,&access.auth).map_err(|e|WorkoutError::Upstream(e.to_string()))?;
            Ok(json!({"status":"available","snapshot":SnapshotDocument::from(profile.workouts.sync_coach(&api,dates)?)}))
        }).await
    }
}
