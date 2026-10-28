//! src/interface/mcp_workouts.rs declares versioned local workout tools for connected models.
//! Models read library snapshots and write drafts; Garmin saves need the granted tools in mcp_garmin.

use super::mcp::{HistoryMcp, join_error, result};
use crate::workouts::{
    WorkoutError, WorkoutService,
    document::{LibraryDocument, WorkoutDocument},
};
use rmcp::{
    ErrorData, RoleServer, handler::server::wrapper::Parameters, model::CallToolResult, schemars,
    service::RequestContext, tool, tool_router,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct PageInput {
    /// 1..100; omitted uses 20. Library is a local snapshot; refresh through Pace.
    limit: Option<usize>,
    /// Zero-based offset, at most 100000.
    offset: Option<usize>,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct LibraryInput {
    /// 1..100; omitted uses 20. Library is a local snapshot; refresh through Pace.
    limit: Option<usize>,
    /// Zero-based offset, at most 100000.
    offset: Option<usize>,
    /// True excludes Coach plan workouts, which get_coach_v1 reads by week.
    standalone: Option<bool>,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct IdInput {
    id: u64,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct SourceInput {
    /// Existing Garmin workout ID returned by get_workout_v1.
    id: u64,
    /// Exact opaque revision returned with that snapshot.
    revision: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateInput {
    workout: WorkoutDocument,
    /// Evidence, uncertainty and specific before/after changes. Required for Coach sources.
    explanation: Option<String>,
    /// Omit to create new. Supply to update the same Garmin workout rather than duplicating it.
    source: Option<SourceInput>,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReviseInput {
    id: u64,
    /// Current draft revision from get_workout_draft_v1; stale revisions are rejected.
    revision: u64,
    workout: WorkoutDocument,
    /// Evidence, uncertainty and specific before/after changes. Required for Coach sources.
    explanation: Option<String>,
}

#[tool_router(router = workout_tools, vis = "pub(super)")]
impl HistoryMcp {
    #[tool(
        description = "List a page of the user's locally synchronized Garmin workout library. These are reusable definitions, not completed activities; standalone=true excludes Coach plan workouts. Empty means not imported or no workouts; ask the user to sync in Pace. Read a definition before editing.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn list_workouts_v1(
        &self,
        Parameters(input): Parameters<LibraryInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.workout_call(&context, move |service| {
            let (limit, offset) = (input.limit.unwrap_or(20), input.offset.unwrap_or(0));
            let rows = if input.standalone == Some(true) {
                service.standalone(limit, offset)?
            } else {
                service.list(limit, offset)?
            };
            Ok(
                json!({"workouts":rows.iter().map(super::workout_view::summary).collect::<Vec<_>>(),
                "next_offset":if rows.len()==limit { Some(offset+limit) } else { None }}),
            )
        })
        .await
    }
    #[tool(
        description = "Read a stored workout definition, source revision and synchronization time. Unsupported Garmin features are explicitly read-only. Use this ID and revision as source when editing; never invent exercise identifiers or discard unsupported steps.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn get_workout_v1(
        &self,
        Parameters(input): Parameters<IdInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.workout_call(&context, move |service| {
            Ok(json!(LibraryDocument::from(service.get(input.id)?)))
        })
        .await
    }
    #[tool(
        description = "Create a LOCAL model-authored draft. First use saved activities and health when relevant; discuss goals and constraints with the user. Submit a complete v1 definition with explicit units. If this workout needs an unset benchmark, first create the workout draft with its known exercise identity, then use get_benchmark_context_v1 and propose_benchmark_v1 with missing_for_workout and the returned draft ID/revision. Do not proactively change a filled benchmark. Supply source ID+revision to edit an existing workout; omit source for a new workout. Does not save to Garmin or send to a watch by itself; the user saves in Pace or explicitly asks you to call save_workout_draft_to_garmin_v1. Do not call twice after an uncertain tool response; inspect drafts first.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn create_workout_draft_v1(
        &self,
        Parameters(input): Parameters<CreateInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.workout_call(&context, move |service| {
            let draft = service.propose_explained(
                input.workout.try_into()?,
                input.source.map(|s| (s.id, s.revision)),
                input.explanation.as_deref(),
            )?;
            Ok(super::workout_view::draft_json(&draft))
        })
        .await
    }
    #[tool(
        description = "List local workout draft IDs, revisions and save states. A draft is not saved to Garmin; saving or saved requires explicit reconciliation if verification did not finish. Verified means Garmin content was read back, not watch receipt.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn list_workout_drafts_v1(
        &self,
        Parameters(input): Parameters<PageInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.workout_call(&context,move |service| {
            let (limit,offset)=(input.limit.unwrap_or(20),input.offset.unwrap_or(0));
            let rows=service.drafts(limit,offset)?;
            Ok(json!({"drafts":rows.iter().map(super::workout_view::draft_summary).collect::<Vec<_>>(),
                "next_offset":if rows.len()==limit { Some(offset+limit) } else { None }}))
        }).await
    }
    #[tool(
        description = "Read a complete local draft and its immutable original source for before/after review. Read its current revision before proposing further changes.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn get_workout_draft_v1(
        &self,
        Parameters(input): Parameters<IdInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.workout_call(&context, move |service| {
            Ok(super::workout_view::draft_json(&service.draft(input.id)?))
        })
        .await
    }
    #[tool(
        description = "Replace the contents of an unsaved LOCAL draft at exactly the supplied revision, retaining its original Garmin source. Use for user-requested corrections. Returns a new revision requiring another review in Pace. Cannot change a saving, saved, verified or discarded draft. Does not call Garmin; saving is a separate explicit step.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn revise_workout_draft_v1(
        &self,
        Parameters(input): Parameters<ReviseInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.workout_call(&context, move |service| {
            Ok(super::workout_view::draft_json(&service.revise_explained(
                input.id,
                input.revision,
                input.workout.try_into()?,
                input.explanation.as_deref(),
            )?))
        })
        .await
    }
}
impl HistoryMcp {
    pub(super) async fn workout_call(
        &self,
        context: &RequestContext<RoleServer>,
        action: impl FnOnce(WorkoutService) -> Result<Value, WorkoutError> + Send + 'static,
    ) -> Result<CallToolResult, ErrorData> {
        let service = self.profile(context)?.workouts;
        let permit = self.blocking_permit().await?;
        let value = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            action(service)
        })
        .await
        .map_err(join_error)?
        .map_err(workout_error)?;
        result(value)
    }
}
pub(super) fn workout_error(error: WorkoutError) -> ErrorData {
    match error {
        WorkoutError::InvalidInput(_) | WorkoutError::Conflict | WorkoutError::NotFound => {
            ErrorData::invalid_params(error.to_string(), None)
        }
        _ => {
            tracing::error!("local workout operation failed");
            ErrorData::internal_error("local workout storage is unavailable", None)
        }
    }
}
