//! src/garmin/coach.rs adapts bounded read-only Garmin plan and calendar endpoints.
//! Canonical workout reads supply revisions used by existing conflict-checked writes;
//! calendar payloads never substitute for the workout endpoint's source identity.

use super::{ConnectApi, connect::upstream};
use crate::{
    health::Date,
    workouts::{
        WorkoutError, WorkoutGateway,
        coach::{Gateway, Plan, Session, Snapshot, Window},
    },
};
use anyhow::{Context, Result, ensure};
use reqwest::Method;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Plans {
    training_plan_list: Vec<RemotePlan>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemotePlan {
    training_plan_id: u64,
    name: String,
    training_plan_category: String,
    training_status: Status,
    start_date: String,
    end_date: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    status_key: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Scheduled {
    workout_schedule_id: u64,
    calendar_date: String,
    workout: WorkoutRef,
    associated_activity_id: Option<u64>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkoutRef {
    workout_id: u64,
    training_plan_id: Option<u64>,
    estimated_duration_in_secs: Option<u32>,
}
fn id(value: u64) -> Result<u64> {
    ensure!(
        value > 0 && value <= i64::MAX as u64,
        "invalid Garmin Coach identity"
    );
    Ok(value)
}
fn text(value: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty() && value.len() <= 500 && !value.chars().any(char::is_control),
        "invalid Garmin Coach text"
    );
    Ok(())
}
fn date(value: &str) -> Result<Date> {
    if value.len() == 10 {
        return value.parse().context("invalid Coach calendar date");
    }
    let parsed = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
        .context("invalid Coach plan date")?;
    parsed
        .date()
        .to_string()
        .parse()
        .context("invalid Coach plan date")
}
impl RemotePlan {
    fn decode(self) -> Result<Plan> {
        text(&self.name)?;
        text(&self.training_plan_category)?;
        text(&self.training_status.status_key)?;
        let start = date(&self.start_date)?;
        let end = date(&self.end_date)?;
        ensure!(start <= end, "invalid Coach plan date order");
        Ok(Plan {
            id: id(self.training_plan_id)?,
            name: self.name,
            category: self.training_plan_category,
            status: self.training_status.status_key,
            start,
            end,
        })
    }
}
impl ConnectApi {
    fn coach_plans(&self) -> Result<Vec<Plan>> {
        let plans: Plans = self.http.json(
            Method::GET,
            "/trainingplan-service/trainingplan/plans",
            &[("limit", "100".into()), ("start", "1".into())],
            None,
        )?;
        // A full page cannot establish completeness; fail instead of serving partial context.
        ensure!(
            plans.training_plan_list.len() < 100,
            "Coach plan list reached its bound; narrow or archive plans in Garmin"
        );
        let mut ids = HashSet::new();
        plans
            .training_plan_list
            .into_iter()
            .map(|p| {
                let plan = p.decode()?;
                ensure!(ids.insert(plan.id), "duplicate Coach plan ID");
                Ok(plan)
            })
            .collect()
    }
    fn coach_sessions(&self, window: Window, plans: &[Plan]) -> Result<Vec<Session>> {
        let rows: Vec<Scheduled> = self.http.json(
            Method::GET,
            "/workout-service/schedule/workoutScheduleByPeriod",
            &[
                ("startDate", window.start.to_string()),
                ("endDate", window.end.to_string()),
            ],
            None,
        )?;
        ensure!(rows.len() <= 100, "Coach calendar exceeds 100 sessions");
        let mut sessions = Vec::new();
        let mut schedules = HashSet::new();
        let mut library = HashMap::new();
        for row in rows {
            let schedule_id = id(row.workout_schedule_id)?;
            ensure!(schedules.insert(schedule_id), "duplicate Coach schedule ID");
            let day = date(&row.calendar_date)?;
            ensure!(
                day >= window.start && day <= window.end,
                "Coach session outside requested window"
            );
            let workout_id = id(row.workout.workout_id)?;
            if let std::collections::hash_map::Entry::Vacant(entry) = library.entry(workout_id) {
                entry.insert(WorkoutGateway::read(self, workout_id).map_err(anyhow::Error::new)?);
            }
            let workout = library
                .get(&workout_id)
                .context("missing canonical Coach workout")?
                .clone();
            ensure!(
                workout.coach_plan_id == row.workout.training_plan_id,
                "Coach workout plan changed during import"
            );
            if let Some(plan) = workout.coach_plan_id {
                ensure!(
                    plans.iter().any(|p| p.id == plan),
                    "Coach session references an unavailable plan"
                );
            }
            if let Some(activity) = row.associated_activity_id {
                id(activity)?;
            }
            sessions.push(Session {
                schedule_id,
                date: day,
                workout,
                associated_activity_id: row.associated_activity_id.map(|v| v.to_string()),
                estimated_seconds: row.workout.estimated_duration_in_secs,
            });
        }
        Ok(sessions)
    }
    fn read_coach(&self, window: Window) -> Result<Snapshot> {
        let plans = self.coach_plans()?;
        let sessions = self.coach_sessions(window, &plans)?;
        Ok(Snapshot {
            window,
            synced_at: chrono::Utc::now().to_rfc3339(),
            plans,
            sessions,
        })
    }
}

impl Gateway for ConnectApi {
    fn fetch(&self, window: Window) -> Result<Snapshot, WorkoutError> {
        self.read_coach(window).map_err(upstream)
    }
}
