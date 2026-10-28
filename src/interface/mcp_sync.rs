//! src/interface/mcp_sync.rs composes explicit bounded Garmin imports for an authorized
//! profile. Credentials come only from its private file; sync shares the write concurrency gate.

use super::{
    mcp::{GarminAccess, HistoryMcp},
    responses::ActivityResponse,
};
use crate::{
    garmin::{ConnectApi, GarminBenchmarks, GarminHealth, PersonalApi},
    health::{Date, HealthError, document::DayDocument},
    history::{ActivityPeriod, HistoryError},
    secrets::Secrets,
    workouts::{
        WorkoutError, WorkoutService,
        benchmarks::document::{ActivitySetsDocument, window},
        document::LibraryDocument,
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
struct ActivitySync {
    /// One page, within the configured history limit.
    limit: usize,
    offset: Option<usize>,
    /// Optional inclusive Garmin LOCAL dates; provide both start and end, at most 31 days.
    start: Option<String>,
    end: Option<String>,
    /// Quote the user's actual instruction requesting this import.
    user_request: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct HealthSync {
    date: String,
    user_request: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct StrengthSync {
    start: String,
    end: String,
    user_request: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ActivitySetsSync {
    /// Garmin ID of a strength activity already stored locally.
    garmin_id: String,
    user_request: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct LibrarySync {
    limit: usize,
    offset: Option<usize>,
    user_request: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RefreshInput {
    id: u64,
    user_request: String,
}

/// One MCP activity import stays one bounded page over a short period.
const MAX_PERIOD_DAYS: usize = 31;

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn personal_key(access: &GarminAccess) -> Result<Secrets, WorkoutError> {
    Secrets::from_file(&access.personal_key_path).map_err(|_|WorkoutError::InvalidInput("sync requires private garmin.env beside this profile's database; server environment credentials are never used"))
}
fn key(secrets: &Secrets) -> Result<&str, WorkoutError> {
    secrets.require("GARMIN_PERSONAL_API_KEY").map_err(|_| {
        WorkoutError::InvalidInput("profile garmin.env must contain GARMIN_PERSONAL_API_KEY")
    })
}
fn upstream(error: anyhow::Error) -> WorkoutError {
    WorkoutError::Upstream(error.to_string())
}
fn history(error: HistoryError) -> WorkoutError {
    match error {
        HistoryError::Storage(e) => WorkoutError::Storage(e),
        HistoryError::Upstream(e) => WorkoutError::Upstream(e),
        _ => WorkoutError::InvalidInput("activity page or period exceeds configured bounds"),
    }
}
fn health(error: HealthError) -> WorkoutError {
    match error {
        HealthError::Storage(e) => WorkoutError::Storage(e),
        HealthError::Upstream(e) => WorkoutError::Upstream(e),
        HealthError::Invalid(_) => {
            WorkoutError::InvalidInput("invalid health import date or bounds")
        }
    }
}
fn request(text: &str) -> Result<(), ErrorData> {
    validate_request(text).map_err(super::mcp_workouts::workout_error)
}

#[tool_router(router = sync_tools, vis = "pub(super)")]
impl HistoryMcp {
    #[tool(
        description = "Explicitly import one Garmin activity page into this profile. Requires the Garmin grant and this profile's private Personal API key. Optional start/end are Garmin local dates, at most 31 days. Advance next_offset until null; this is not a complete archive. On timeout wait for the server to become idle and inspect saved data before continuing. Quote the user's sync request.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn sync_activities_v1(
        &self,
        Parameters(input): Parameters<ActivitySync>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        request(&input.user_request)?;
        let offset = input.offset.unwrap_or(0);
        self.profile(&context)?
            .history
            .validate_page(input.limit, offset)
            .map_err(super::mcp::history_error)?;
        let period = match (&input.start, &input.end) {
            (None, None) => None,
            (Some(start), Some(end)) => {
                let date = |v: &str| v.parse::<Date>().map_err(super::mcp::health_error);
                let (start, end) = (date(start)?, date(end)?);
                start.range(end, MAX_PERIOD_DAYS).map_err(|_| {
                    ErrorData::invalid_params(
                        format!("activity period must contain 1..={MAX_PERIOD_DAYS} days"),
                        None,
                    )
                })?;
                Some(
                    ActivityPeriod::new(start.into(), end.into())
                        .map_err(super::mcp::history_error)?,
                )
            }
            _ => {
                return Err(ErrorData::invalid_params(
                    "provide both start and end, or neither",
                    None,
                ));
            }
        };
        self.garmin_call(&context,move |profile,access| {
            let secrets=personal_key(access)?;
            let api=PersonalApi::connect(&access.settings,key(&secrets)?).map_err(upstream)?;
            let saved=profile.history.sync_page(&api,period,input.limit,offset,now).map_err(history)?;
            Ok(json!({"scope":"imported_page","date_basis":"Garmin_local","received":saved.received_count,"synced_at":saved.completed_at,
                "next_offset":(saved.received_count as usize==input.limit).then_some(offset+input.limit)}))
        }).await
    }
    #[tool(
        description = "Explicitly import all eight health datasets for ONE Garmin calendar date. Requires the Garmin grant, profile Personal API key and Connect session. The day commits atomically; a failure retains its previous snapshot. For multiple days call sequentially. After timeout inspect get_daily_health and server busy state. Quote the user's sync request.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn sync_health_v1(
        &self,
        Parameters(input): Parameters<HealthSync>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        request(&input.user_request)?;
        let date = input
            .date
            .parse::<Date>()
            .map_err(super::mcp::health_error)?;
        self.garmin_call(&context, move |profile, access| {
            let secrets = personal_key(access)?;
            let api = GarminHealth::connect_profile(
                &access.settings,
                &access.auth,
                &access.health,
                key(&secrets)?,
                access.health.limits(),
            )
            .map_err(upstream)?;
            Ok(json!(DayDocument::from(
                profile.health.sync_day(&api, date, now).map_err(health)?
            )))
        })
        .await
    }
    #[tool(
        description = "Explicitly import the benchmark catalogue and recorded strength sets for an inclusive UTC window of at most 31 days. Requires the Garmin grant and this profile's Personal API key and Connect session. Never changes Garmin benchmark values. On timeout inspect benchmark context/sets after the server becomes idle. Quote the user's sync request.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn sync_strength_v1(
        &self,
        Parameters(input): Parameters<StrengthSync>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        request(&input.user_request)?;
        let dates = window(&input.start, &input.end).map_err(super::mcp_workouts::workout_error)?;
        self.garmin_call(&context,move |profile,access| {
            let secrets=personal_key(access)?;
            let api=GarminBenchmarks::connect(&access.settings,&access.auth,Some(key(&secrets)?)).map_err(upstream)?;
            let count=profile.benchmarks.sync(&api,dates)?;
            Ok(json!({"sessions":count,"start":input.start,"end":input.end,"scope":"local_snapshot"}))
        }).await
    }
    #[tool(
        description = "Explicitly import the recorded sets of one strength activity already stored locally, by its Garmin ID. Requires the Garmin grant and Connect session only: no Personal API key, activity search or benchmark catalogue. Replaces only that activity's sets; it does not mark its date as covered for other activities. Returns the same document as get_activity_sets_v1. Quote the user's sync request.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn sync_activity_sets_v1(
        &self,
        Parameters(input): Parameters<ActivitySetsSync>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        request(&input.user_request)?;
        self.garmin_call(&context, move |profile, access| {
            let activity = profile
                .history
                .find(&input.garmin_id)
                .map_err(|error| match error {
                    HistoryError::InvalidQuery(message) => WorkoutError::InvalidInput(message),
                    other => history(other),
                })?
                .ok_or(WorkoutError::InvalidInput(
                    "activity is not stored locally; sync activities first",
                ))?;
            let api = GarminBenchmarks::connect(&access.settings, &access.auth, None)
                .map_err(upstream)?;
            let sets =
                super::benchmarks::import_activity_sets(&profile.benchmarks, &api, &activity)?;
            Ok(json!({
                "activity": ActivityResponse::from(activity),
                "sets": ActivitySetsDocument::from(sets),
            }))
        })
        .await
    }
    #[tool(
        description = "Explicitly import one page of the Garmin workout library, including newly created remote IDs after a lost save response. Requires the Garmin grant and Connect session. Advance next_offset until null, then inspect candidate definitions before reconciliation. Each completed workout snapshot remains saved on a later failure. Quote the user's sync request.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn sync_workouts_v1(
        &self,
        Parameters(input): Parameters<LibrarySync>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        request(&input.user_request)?;
        let offset = input.offset.unwrap_or(0);
        WorkoutService::page(input.limit, offset).map_err(super::mcp_workouts::workout_error)?;
        self.garmin_call(&context,move |profile,access| {
            let api=ConnectApi::connect(&access.settings,&access.auth).map_err(upstream)?;
            let count=profile.workouts.sync(&api,offset,input.limit)?;
            Ok(json!({"received":count,"next_offset":(count==input.limit).then_some(offset+count),"scope":"local_snapshot"}))
        }).await
    }
    #[tool(
        description = "Explicitly read one Garmin workout ID and replace its local library snapshot. Requires the Garmin grant and Connect session. Use to resolve a stale source before making a new draft or inspect an uncertain create; this does not reconcile or resend a draft. Quote the user's refresh request.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn refresh_workout_v1(
        &self,
        Parameters(input): Parameters<RefreshInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        request(&input.user_request)?;
        if input.id == 0 {
            return Err(ErrorData::invalid_params(
                "workout ID must be positive",
                None,
            ));
        }
        self.garmin_call(&context, move |profile, access| {
            let api = ConnectApi::connect(&access.settings, &access.auth).map_err(upstream)?;
            Ok(json!(LibraryDocument::from(
                profile.workouts.refresh(&api, input.id)?
            )))
        })
        .await
    }
}
