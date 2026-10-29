//! src/interface/mcp_garmin.rs exposes Garmin save, reconcile, delivery and device tools to
//! profiles whose administrator granted Garmin writes. Every write quotes the user's
//! instruction, which the use case journals before the network call. Garmin work runs on a
//! dedicated thread outside the async runtime and the SQLite blocking pool, one per process.

use super::{
    mcp::{AuthenticatedHistory, GarminAccess, HistoryMcp, result},
    workout_view,
};
use crate::{
    garmin::{ConnectApi, GarminBenchmarks},
    workouts::{WorkoutError, WorkoutGateway, benchmarks::document::DraftDocument},
};
use rmcp::{
    ErrorData, RoleServer, handler::server::wrapper::Parameters, model::CallToolResult, schemars,
    service::RequestContext, tool, tool_router,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct SaveInput {
    /// Local draft or proposal ID.
    id: u64,
    /// Exact reviewed revision; stale revisions are rejected.
    revision: u64,
    /// The user's instruction quoted verbatim, 1..=1000 bytes without control characters; journaled with the write and shown to the user.
    user_request: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReconcileWorkoutInput {
    /// Local draft ID in state saving or saved.
    id: u64,
    /// Garmin workout ID the save targeted (remote_id of the draft).
    workout_id: u64,
    /// The user's instruction quoted verbatim, 1..=1000 bytes without control characters.
    user_request: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReconcileBenchmarkInput {
    /// Local proposal ID in state saving or saved.
    id: u64,
    /// The user's instruction quoted verbatim, 1..=1000 bytes without control characters.
    user_request: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct SendInput {
    /// Garmin workout ID from list_workouts_v1 or a verified draft's remote_id, not the draft ID.
    id: u64,
    /// Device ID from list_garmin_devices_v1.
    device_id: u64,
    /// The user's instruction quoted verbatim, 1..=1000 bytes without control characters; journaled with the delivery.
    user_request: String,
}

macro_rules! rules {
    () => {
        " Requires the administrator's Garmin-writes grant for this profile. Call only after the user explicitly asked for this exact action in this conversation and quote that instruction in user_request. One Garmin operation runs at a time; after a timeout or a busy answer read the draft state (draft = not applied, saving = in flight or unresolved, saved = accepted but unverified, verified = done) and never save twice."
    };
}

#[tool_router(router = garmin_tools, vis = "pub(super)")]
impl HistoryMcp {
    #[tool(
        description = "List the user's registered Garmin devices by ID and model through the Connect session, for send_workout_to_device_v1. Requires the administrator's Garmin-writes grant for this profile.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn list_garmin_devices_v1(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.garmin_call(&context, |_, access| {
            let devices = connect(access)?.devices()?;
            Ok(json!({"devices": devices
                .iter()
                .map(|d| json!({"id": d.id, "model": d.model}))
                .collect::<Vec<_>>()}))
        })
        .await
    }
    #[tool(
        description = concat!("Save a reviewed LOCAL workout draft to Garmin Connect at exactly this revision and verify the read-back. A 4xx refusal leaves the draft editable; an unresolved outcome leaves it saving/saved for reconcile_workout_draft_v1. Does not deliver to a watch.", rules!()),
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn save_workout_draft_to_garmin_v1(
        &self,
        Parameters(input): Parameters<SaveInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.garmin_call(&context, move |profile, access| {
            let draft = profile.workouts.save(
                &connect(access)?,
                input.id,
                input.revision,
                Some(&input.user_request),
            )?;
            Ok(workout_view::draft_json(&draft))
        })
        .await
    }
    #[tool(
        description = concat!("Resolve a workout draft left in saving or saved by reading the Garmin workout back; verified only when the full definition matches. Never writes.", rules!()),
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn reconcile_workout_draft_v1(
        &self,
        Parameters(input): Parameters<ReconcileWorkoutInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.garmin_call(&context, move |profile, access| {
            let draft = profile.workouts.reconcile(
                &connect(access)?,
                input.id,
                input.workout_id,
                Some(&input.user_request),
            )?;
            Ok(workout_view::draft_json(&draft))
        })
        .await
    }
    #[tool(
        description = concat!("Ask Garmin to send a saved, unchanged library workout to one registered device. Present status_label (Sent to watch) as the send result: Garmin accepted the request; receipt on the watch is not verified. The legacy queued boolean records acceptance, not live queue state. One delivery per device and remote revision.", rules!()),
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn send_workout_to_device_v1(
        &self,
        Parameters(input): Parameters<SendInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.garmin_call(&context, move |profile, access| {
            let sent = profile.workouts.send(
                &connect(access)?,
                input.id,
                input.device_id,
                Some(&input.user_request),
            )?;
            Ok(
                json!({"workout_id":sent.workout_id,"device_id":sent.device_id,"queued":sent.queued,
                "status_label":workout_view::delivery_label(sent.queued),"status_detail":workout_view::DELIVERY_RECEIPT_NOTE,
                "watch_receipt":"unconfirmed","mcp_request":sent.request}),
            )
        })
        .await
    }
    #[tool(
        description = concat!("Save a reviewed LOCAL benchmark proposal to Garmin at exactly this revision and verify the catalogue read-back. A 4xx refusal leaves the proposal editable; an unresolved outcome leaves it saving/saved for reconcile_benchmark_draft_v1.", rules!()),
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn save_benchmark_draft_to_garmin_v1(
        &self,
        Parameters(input): Parameters<SaveInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.garmin_call(&context, move |profile, access| {
            let draft = profile.benchmarks.save(
                &connect_benchmarks(access)?,
                input.id,
                input.revision,
                Some(&input.user_request),
            )?;
            Ok(json!(DraftDocument::from(draft)))
        })
        .await
    }
    #[tool(
        description = concat!("Resolve a benchmark proposal left in saving or saved by reading the Garmin catalogue back; verified only when the saved values and links match. Never writes.", rules!()),
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn reconcile_benchmark_draft_v1(
        &self,
        Parameters(input): Parameters<ReconcileBenchmarkInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.garmin_call(&context, move |profile, access| {
            let draft = profile.benchmarks.reconcile(
                &connect_benchmarks(access)?,
                input.id,
                Some(&input.user_request),
            )?;
            Ok(json!(DraftDocument::from(draft)))
        })
        .await
    }
}

impl HistoryMcp {
    /// Run one Garmin operation for a granted profile on its own thread; the handler only waits.
    pub(super) async fn garmin_call(
        &self,
        context: &RequestContext<RoleServer>,
        action: impl FnOnce(AuthenticatedHistory, &GarminAccess) -> Result<Value, WorkoutError>
        + Send
        + 'static,
    ) -> Result<CallToolResult, ErrorData> {
        let profile = self.profile(context)?;
        let Some(access) = profile.garmin.clone() else {
            return Err(ErrorData::invalid_params(
                "Garmin writes are not enabled for this profile; an administrator runs `pace users allow-garmin-writes USER` (HTTP) or sets runtime.stdio_garmin_writes = true (stdio)",
                Some(json!({"outcome":"denied"})),
            ));
        };
        let permit = self.network_permit()?;
        // A fresh OS thread has no runtime context, so the blocking Garmin client is safe there,
        // and the SQLite blocking pool stays free for authentication. The permit travels with the
        // work: a dropped response future (request timeout) never cancels or unlocks the write.
        let (sender, receiver) = tokio::sync::oneshot::channel();
        std::thread::Builder::new()
            .name("garmin-mcp".into())
            .spawn(move || {
                let _permit = permit;
                let _ = sender.send(action(profile, &access));
            })
            .map_err(|_| ErrorData::internal_error("could not start the Garmin operation", None))?;
        let value = receiver
            .await
            .map_err(|_| {
                ErrorData::internal_error("Garmin operation ended without a result", None)
            })?
            .map_err(garmin_error)?;
        result(value)
    }
}

fn connect(access: &GarminAccess) -> Result<ConnectApi, WorkoutError> {
    ConnectApi::connect(&access.settings, &access.auth)
        .map_err(|error| WorkoutError::Upstream(error.to_string()))
}
fn connect_benchmarks(access: &GarminAccess) -> Result<GarminBenchmarks, WorkoutError> {
    GarminBenchmarks::connect(&access.settings, &access.auth, None)
        .map_err(|error| WorkoutError::Upstream(error.to_string()))
}

/// Garmin outcomes stay distinguishable for the model; storage failures stay generic.
fn garmin_error(error: WorkoutError) -> ErrorData {
    match error {
        WorkoutError::InvalidInput(_) | WorkoutError::Conflict | WorkoutError::NotFound => {
            ErrorData::invalid_params(error.to_string(), None)
        }
        WorkoutError::Rejected(status) => ErrorData::internal_error(
            format!("{error}; no save, delivery or move stays pending for it"),
            Some(json!({"outcome":"rejected","garmin_status":status})),
        ),
        WorkoutError::Uncertain => {
            ErrorData::internal_error(error.to_string(), Some(json!({"outcome":"uncertain"})))
        }
        WorkoutError::Upstream(_) => {
            ErrorData::internal_error(error.to_string(), Some(json!({"outcome":"upstream"})))
        }
        WorkoutError::Storage(_) => {
            tracing::error!("local workout storage failed during a Garmin operation");
            ErrorData::internal_error("local workout storage is unavailable", None)
        }
    }
}
