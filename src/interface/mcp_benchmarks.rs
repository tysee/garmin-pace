//! src/interface/mcp_benchmarks.rs exposes local strength evidence and conditional model proposals.
//! These tools never call Garmin; granted saves live in mcp_garmin. Filled benchmarks require an actual user request.

use super::{
    mcp::{HistoryMcp, history_error, join_error, result},
    responses::ActivityResponse,
};
use crate::workouts::{
    WorkoutError,
    benchmarks::{
        BenchmarkService,
        document::{
            self, ActivitySetsDocument, BenchmarkDocument, ContextDocument, DraftDocument,
            ProposalDocument,
        },
    },
};
use rmcp::{
    ErrorData, RoleServer, handler::server::wrapper::Parameters, model::CallToolResult, schemars,
    service::RequestContext, tool, tool_router,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ContextInput {
    /// Exact key returned by list_benchmarks_v1. Related variants are not pooled.
    key: String,
    /// Inclusive UTC activity-date window, at most 31 days. Choose recent observed data.
    start: String,
    end: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProposalInput {
    proposal: ProposalDocument,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReviseInput {
    id: u64,
    revision: u64,
    proposal: ProposalDocument,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct PageInput {
    limit: Option<usize>,
    offset: Option<usize>,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct IdInput {
    id: u64,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ActivityInput {
    /// Garmin activity ID exactly as list_activities returns it.
    garmin_id: String,
}

#[tool_router(router = benchmark_tools, vis = "pub(super)")]
impl HistoryMcp {
    #[tool(
        description = "List locally imported Garmin benchmark exercises and their current 1RM values; null means not set. Empty catalogue means sync strength data in Pace first. Do not proactively change filled entries. Only propose on a user's actual request or when an unset benchmark is required for a specific workout draft.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn list_benchmarks_v1(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.benchmark_call(&context, |s| Ok(json!({"benchmarks":s.catalog()?.into_iter().map(BenchmarkDocument::from).collect::<Vec<_>>()}))).await
    }
    #[tool(
        description = "Read recent observed strength sets and arithmetic mean load/repetitions for one exact exercise over an explicit UTC date window. Includes count, exclusions, timestamps and source/context revisions. Missing coverage or zero usable sets cannot support a proposal; ask for a sync or observed exercise data. Also use saved health/readiness context when relevant; do not infer lifting ability from generic population averages or unrelated exercises.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn get_benchmark_context_v1(
        &self,
        Parameters(input): Parameters<ContextInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.benchmark_call(&context, move |s| {
            Ok(json!(ContextDocument::from(s.context(
                &input.key,
                document::window(&input.start, &input.end)?
            )?)))
        })
        .await
    }
    #[tool(
        description = "Read the exercise sets recorded in one locally stored strength activity: per set the exact exercise (or only Garmin's category when no single exercise was recorded), load in kilograms and repetitions; rest sets are inactive. Sets exist locally only after a strength sync covering the activity's UTC date or a per-activity sync_activity_sets_v1 import: status is available, no_data (the covering import holds no session for this activity) or not_synced (ask for either import). Non-strength activities have no sets. No network access.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn get_activity_sets_v1(
        &self,
        Parameters(input): Parameters<ActivityInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let profile = self.profile(&context)?;
        let permit = self.blocking_permit().await?;
        let value = tokio::task::spawn_blocking(move || -> Result<Value, ErrorData> {
            let _permit = permit;
            let activity = profile
                .history
                .find(&input.garmin_id)
                .map_err(history_error)?
                .ok_or_else(|| {
                    ErrorData::invalid_params(
                        "activity is not stored locally; sync activities first",
                        None,
                    )
                })?;
            let sets = super::benchmarks::activity_sets(&profile.benchmarks, &activity)
                .map_err(super::mcp_workouts::workout_error)?;
            Ok(json!({
                "activity": ActivityResponse::from(activity),
                "sets": ActivitySetsDocument::from(sets),
            }))
        })
        .await
        .map_err(join_error)??;
        result(value)
    }
    #[tool(
        description = "Store a LOCAL model estimate based on get_benchmark_context_v1. Derive representative load/reps and estimated 1RM from this user's recent observed averages; explain the method, reasoning, data limits and confidence. Origin must record an actual explicit user request (required to change a filled benchmark), or missing_for_workout with the current ID/revision of an unsaved draft using this unset exercise and why it needs the benchmark. Never invent user authorization. No automatic refresh or Garmin write occurs; the user reviews and saves in Pace or explicitly asks you to call save_benchmark_draft_to_garmin_v1. After an uncertain response, inspect proposals before calling again.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn propose_benchmark_v1(
        &self,
        Parameters(input): Parameters<ProposalInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.benchmark_call(&context, move |s| {
            Ok(json!(DraftDocument::from(
                s.propose(input.proposal.try_into()?)?
            )))
        })
        .await
    }
    #[tool(
        description = "List a page of local benchmark proposals and pending saves. Omitted limit/offset uses 20/0; limit 1..100, offset at most 100000. No network access.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn list_benchmark_drafts_v1(
        &self,
        Parameters(input): Parameters<PageInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.benchmark_call(&context, move |s| {
            let (limit, offset)=(input.limit.unwrap_or(20),input.offset.unwrap_or(0));
            let rows=s.drafts(limit,offset)?;
            Ok(json!({"drafts":rows.iter().map(super::benchmark_view::summary).collect::<Vec<_>>(),"next_offset":if rows.len()==limit {Some(offset+limit)} else {None}}))
        }).await
    }
    #[tool(
        description = "Read a benchmark proposal with its current revision, immutable observed evidence, estimated values and save state. A model estimate is not a measured personal record.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn get_benchmark_draft_v1(
        &self,
        Parameters(input): Parameters<IdInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.benchmark_call(&context, move |s| {
            Ok(json!(DraftDocument::from(s.draft(input.id)?)))
        })
        .await
    }
    #[tool(
        description = "Revise an unsaved local benchmark proposal using its current revision and freshly reviewed context revisions. The same trigger restrictions apply: filled benchmarks only on explicit user request; proactive proposals only for a missing benchmark needed by the referenced workout draft. Does not write Garmin; saving is a separate explicit step.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn revise_benchmark_draft_v1(
        &self,
        Parameters(input): Parameters<ReviseInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.benchmark_call(&context, move |s| {
            Ok(json!(DraftDocument::from(s.revise(
                input.id,
                input.revision,
                input.proposal.try_into()?
            )?)))
        })
        .await
    }
}
impl HistoryMcp {
    async fn benchmark_call(
        &self,
        context: &RequestContext<RoleServer>,
        action: impl FnOnce(BenchmarkService) -> Result<Value, WorkoutError> + Send + 'static,
    ) -> Result<CallToolResult, ErrorData> {
        let service = self.profile(context)?.benchmarks;
        let permit = self.blocking_permit().await?;
        let value = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            action(service)
        })
        .await
        .map_err(join_error)?
        .map_err(super::mcp_workouts::workout_error)?;
        result(value)
    }
}
