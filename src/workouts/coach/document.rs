//! src/workouts/coach/document.rs defines the v1 Coach snapshot transport and storage contract.
//! Conversion keeps Garmin DTOs outside the domain and returns local workout revisions for edits.

use super::{Plan, Reschedule, Session, Snapshot, Window};
use crate::workouts::{WorkoutError, document::LibraryDocument};
use serde::{Deserialize, Serialize};

/// Versioned local Coach snapshot; all dates use Garmin calendar dates.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotDocument {
    pub version: u32,
    pub start: String,
    pub end: String,
    pub synced_at: String,
    pub plans: Vec<PlanDocument>,
    pub sessions: Vec<SessionDocument>,
}
/// Plan identity and lifecycle metadata, independent of a workout definition.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanDocument {
    pub id: u64,
    pub name: String,
    pub category: String,
    pub status: String,
    pub start: String,
    pub end: String,
}
/// One assignment, carrying its complete source definition for before/after review.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionDocument {
    pub schedule_id: u64,
    pub date: String,
    pub workout: LibraryDocument,
    pub associated_activity_id: Option<String>,
    pub estimated_seconds: Option<u32>,
}
/// One journaled move as CLI and MCP report it; dates are Garmin calendar dates.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RescheduleDocument {
    pub id: u64,
    pub schedule_id: u64,
    pub workout_id: u64,
    pub from: String,
    pub to: String,
    pub state: String,
    pub requested_at: String,
    pub mcp_request: Option<String>,
}
impl From<Reschedule> for RescheduleDocument {
    fn from(r: Reschedule) -> Self {
        Self {
            id: r.id,
            schedule_id: r.schedule_id,
            workout_id: r.workout_id,
            from: r.from.to_string(),
            to: r.to.to_string(),
            state: r.state.as_str().into(),
            requested_at: r.requested_at,
            mcp_request: r.request,
        }
    }
}
/// Validate the same public window contract for CLI and MCP.
pub fn window(start: &str, end: &str) -> Result<Window, WorkoutError> {
    let date = |v: &str| {
        v.parse()
            .map_err(|_| WorkoutError::InvalidInput("Coach dates must be YYYY-MM-DD"))
    };
    Window::new(date(start)?, date(end)?)
}
impl From<Snapshot> for SnapshotDocument {
    fn from(s: Snapshot) -> Self {
        Self {
            version: 1,
            start: s.window.start.to_string(),
            end: s.window.end.to_string(),
            synced_at: s.synced_at,
            plans: s
                .plans
                .into_iter()
                .map(|p| PlanDocument {
                    id: p.id,
                    name: p.name,
                    category: p.category,
                    status: p.status,
                    start: p.start.to_string(),
                    end: p.end.to_string(),
                })
                .collect(),
            sessions: s
                .sessions
                .into_iter()
                .map(|s| SessionDocument {
                    schedule_id: s.schedule_id,
                    date: s.date.to_string(),
                    workout: s.workout.into(),
                    associated_activity_id: s.associated_activity_id,
                    estimated_seconds: s.estimated_seconds,
                })
                .collect(),
        }
    }
}
impl TryFrom<SnapshotDocument> for Snapshot {
    type Error = WorkoutError;
    fn try_from(s: SnapshotDocument) -> Result<Self, Self::Error> {
        if s.version != 1 {
            return Err(WorkoutError::InvalidInput(
                "unsupported Coach document version",
            ));
        }
        Ok(Self {
            window: window(&s.start, &s.end)?,
            synced_at: s.synced_at,
            plans: s
                .plans
                .into_iter()
                .map(|p| {
                    Ok(Plan {
                        id: p.id,
                        name: p.name,
                        category: p.category,
                        status: p.status,
                        start: window(&p.start, &p.start)?.start,
                        end: window(&p.end, &p.end)?.end,
                    })
                })
                .collect::<Result<_, WorkoutError>>()?,
            sessions: s
                .sessions
                .into_iter()
                .map(Session::try_from)
                .collect::<Result<_, WorkoutError>>()?,
        })
    }
}
impl TryFrom<SessionDocument> for Session {
    type Error = WorkoutError;
    fn try_from(s: SessionDocument) -> Result<Self, Self::Error> {
        Ok(Self {
            schedule_id: s.schedule_id,
            date: window(&s.date, &s.date)?.start,
            workout: s.workout.try_into()?,
            associated_activity_id: s.associated_activity_id,
            estimated_seconds: s.estimated_seconds,
        })
    }
}
