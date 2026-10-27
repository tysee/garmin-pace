//! src/interface/mcp_delivery.rs exposes local delivery evidence and exact known exercise
//! identities. It never asks Garmin to resend and never infers receipt on a physical watch.

use super::mcp::HistoryMcp;
use rmcp::{
    ErrorData, RoleServer, handler::server::wrapper::Parameters, model::CallToolResult, schemars,
    service::RequestContext, tool, tool_router,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct DeliveryInput {
    /// Remote workout ID, not a draft ID.
    workout_id: u64,
    device_id: u64,
    limit: Option<usize>,
    offset: Option<usize>,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExerciseInput {
    /// Case-insensitive literal substring; empty lists known exact identities.
    query: String,
    limit: Option<usize>,
    offset: Option<usize>,
}
#[tool_router(router = delivery_tools, vis = "pub(super)")]
impl HistoryMcp {
    #[tool(
        description = "Read durable send records for a Garmin workout/device pair. Present status_label: Sent to watch means Garmin accepted the send request, not verified receipt. The v1 queued state records acceptance, not live queue state; sending means in flight or unresolved. No rows means no recorded request. Records are per remote revision; watch_receipt stays unconfirmed. After a timeout inspect this before any retry; an unresolved request requires the user's CLI review, never automatic resend.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn get_workout_deliveries_v1(
        &self,
        Parameters(input): Parameters<DeliveryInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.workout_call(&context,move |s| {
            let (limit,offset)=(input.limit.unwrap_or(20),input.offset.unwrap_or(0));
            let rows=s.deliveries(input.workout_id,input.device_id,limit,offset)?;
            Ok(json!({"scope":"local_journal","watch_receipt":"unconfirmed","next_offset":(rows.len()==limit).then_some(offset+rows.len()),
                "deliveries":rows.iter().map(|d|json!({"workout_id":d.workout_id,"device_id":d.device_id,"revision":d.revision,
                    "state":if d.queued {"queued"} else {"sending"},"status_label":super::workout_view::delivery_label(d.queued),"mcp_request":d.request})).collect::<Vec<_>>()}))
        }).await
    }
    #[tool(
        description = "Search exact category/name identities from imported editable workouts and benchmark catalogues, including linked exercises. This is a local known-identity catalogue, not all Garmin exercises. Never invent an identifier when no match exists; sync sources first. Does not infer an exact exercise from a category-only activity set.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn search_exercises_v1(
        &self,
        Parameters(input): Parameters<ExerciseInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.workout_call(&context,move |s| {
            let (limit,offset)=(input.limit.unwrap_or(20),input.offset.unwrap_or(0));
            let rows=s.exercises(&input.query,limit,offset)?;
            Ok(json!({"scope":"imported_exact_identities","next_offset":(rows.len()==limit).then_some(offset+rows.len()),
                "exercises":rows.iter().map(|e|json!({"category":e.category,"name":e.name})).collect::<Vec<_>>()}))
        }).await
    }
}
