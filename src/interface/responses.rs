//! src/interface/responses.rs maps history domain records into the v1 JSON
//! contract shared by the CLI and MCP tools, including explicit integration status.

use crate::history::{Activity, HistorySummary};
use serde::Serialize;

#[derive(Serialize)]
pub(super) struct StatusResponse {
    pub version: &'static str,
    pub activity_count: u64,
    pub garmin_integration: &'static str,
    pub history_scope: &'static str,
    pub last_sync_requested_limit: Option<u32>,
    pub last_successful_sync_at: Option<String>,
}

impl From<HistorySummary> for StatusResponse {
    fn from(summary: HistorySummary) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            activity_count: summary.activity_count,
            garmin_integration: if summary.last_sync.is_some() {
                "previously_synced"
            } else {
                "not_synced"
            },
            history_scope: "local_recent_pages",
            last_sync_requested_limit: summary.last_sync.as_ref().map(|s| s.requested_limit),
            last_successful_sync_at: summary.last_sync.map(|s| s.completed_at),
        }
    }
}

#[derive(Serialize)]
pub(super) struct ActivityResponse {
    garmin_id: String,
    name: String,
    sport: String,
    started_at: String,
    duration_seconds: f64,
    distance_meters: Option<f64>,
}

impl From<Activity> for ActivityResponse {
    fn from(activity: Activity) -> Self {
        Self {
            garmin_id: activity.garmin_id,
            name: activity.name,
            sport: activity.sport,
            started_at: activity.started_at,
            duration_seconds: activity.duration_seconds,
            distance_meters: activity.distance_meters,
        }
    }
}

#[derive(Serialize)]
pub(super) struct ActivityPage {
    activities: Vec<ActivityResponse>,
}

impl From<Vec<Activity>> for ActivityPage {
    fn from(activities: Vec<Activity>) -> Self {
        Self {
            activities: activities.into_iter().map(Into::into).collect(),
        }
    }
}
