//! src/interface/mcp_coach.rs exposes local Coach snapshots, explicit granted imports and the
//! journaled move of one scheduled session. The existing workout draft/save tools own every
//! definition edit; this router reads Garmin and forwards one user-requested date change.

use super::mcp::HistoryMcp;
use crate::{
    garmin::ConnectApi,
    workouts::{
        WorkoutError,
        coach::document::{RescheduleDocument, SnapshotDocument, window},
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
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RescheduleInput {
    /// schedule_id of one session from get_coach_v1, not its workout ID.
    schedule_id: u64,
    /// New Garmin calendar date (YYYY-MM-DD). Garmin decides whether the plan allows it.
    date: String,
    /// The user's instruction naming this session and date, quoted verbatim, 1..=1000 bytes without control characters; journaled before Garmin is contacted.
    user_request: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReconcileInput {
    /// schedule_id whose move is still `moving`.
    schedule_id: u64,
    /// The user's instruction quoted verbatim, 1..=1000 bytes without control characters.
    user_request: String,
}

#[tool_router(router=coach_tools,vis="pub(super)")]
impl HistoryMcp {
    #[tool(
        description = "Read the exact locally imported Coach calendar window, at most seven Garmin calendar days. Returns plans, schedule IDs, workout IDs/revisions, complete definitions and activity links, plus reschedules: journaled moves leaving or joining the window (moving, moved, not_applied, missing) that the snapshot dates do not reflect. not_synced means import was never requested. Snapshot content is historical; use get_workout_v1 for the latest local source after an edit. Read recorded sets, benchmark context and health before proposing user-requested changes. Coach drafts require an explanation of evidence, uncertainty and BEFORE/AFTER changes. A workout ID may affect several dates. Never infer kilogram loads from light/moderate/heavy labels or claim that a missing activity link proves no training.",
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
            let reschedules:Vec<RescheduleDocument>=service.coach_reschedules(dates)?.into_iter().map(Into::into).collect();
            Ok(json!({"status":if snapshot.is_some(){"available"}else{"not_synced"},"snapshot":snapshot.map(SnapshotDocument::from),"reschedules":reschedules}))
        }).await
    }
    #[tool(
        description = "Explicitly import Coach plans and scheduled workouts for at most seven Garmin calendar days. Requires this profile's Garmin grant and Connect session. Never changes Garmin. Canonical workout reads and the calendar snapshot commit atomically; errors retain previous snapshots. Quote the user's request. After a save or a move import again to check schedule/plan linkage; Garmin may independently adapt later. This is not a Coach-preferences tool; one session is moved with reschedule_coach_session_v1.",
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
            let api=connect(access)?;
            Ok(json!({"status":"available","snapshot":SnapshotDocument::from(profile.workouts.sync_coach(&api,dates)?)}))
        }).await
    }
    #[tool(
        description = concat!("Move ONE imported Coach session (by schedule_id) to another Garmin calendar date and read the entry back. Garmin allows this for Strength Coach and Expert Running plans and refuses it for adaptive Run/Cycling/Triathlon/Fitness Coach plans; Pace enforces no date policy and never shifts the rest of a plan. The move is journaled first; state moved means Garmin returned the new date, not_applied means Garmin refused (HTTP 4xx) or kept the old date, missing means Garmin no longer returns the entry. A remote date or workout that differs from the local snapshot is a conflict: sync that week and ask the user again.", " Requires the administrator's Garmin-writes grant for this profile. Call only after the user explicitly asked to move this exact session to this date in this conversation and quote that instruction in user_request. One Garmin operation runs at a time; after a timeout or a busy answer read get_coach_v1 (its reschedules list) and never move twice. Local snapshots keep the old date until both affected weeks are synced again."),
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn reschedule_coach_session_v1(
        &self,
        Parameters(input): Parameters<RescheduleInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        validate_request(&input.user_request).map_err(super::mcp_workouts::workout_error)?;
        let to = window(&input.date, &input.date)
            .map_err(super::mcp_workouts::workout_error)?
            .start;
        self.garmin_call(&context, move |profile, access| {
            let moved = profile.workouts.reschedule(
                &connect(access)?,
                input.schedule_id,
                to,
                Some(&input.user_request),
            )?;
            Ok(json!({"reschedule": RescheduleDocument::from(moved)}))
        })
        .await
    }
    #[tool(
        description = concat!("Resolve a Coach move left in state moving by reading the calendar entry back; never writes. moved when Garmin shows the new date, not_applied when it shows the original date, missing when Garmin no longer returns the entry; any other date keeps the move unresolved for the user's CLI review.", " Requires the administrator's Garmin-writes grant for this profile. Call only after the user explicitly asked to move this exact session to this date in this conversation and quote that instruction in user_request. One Garmin operation runs at a time; after a timeout or a busy answer read get_coach_v1 (its reschedules list) and never move twice. Local snapshots keep the old date until both affected weeks are synced again."),
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn reconcile_coach_reschedule_v1(
        &self,
        Parameters(input): Parameters<ReconcileInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        validate_request(&input.user_request).map_err(super::mcp_workouts::workout_error)?;
        self.garmin_call(&context, move |profile, access| {
            let settled = profile.workouts.reconcile_reschedule(
                &connect(access)?,
                input.schedule_id,
                Some(&input.user_request),
            )?;
            Ok(json!({"reschedule": RescheduleDocument::from(settled)}))
        })
        .await
    }
}
fn connect(access: &super::mcp::GarminAccess) -> Result<ConnectApi, WorkoutError> {
    ConnectApi::connect(&access.settings, &access.auth)
        .map_err(|error| WorkoutError::Upstream(error.to_string()))
}
