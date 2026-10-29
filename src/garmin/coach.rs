//! src/garmin/coach.rs adapts bounded Garmin plan and calendar endpoints: read-only plan and
//! week imports, plus the compare-and-set move of one calendar entry. Canonical workout reads
//! supply revisions used by existing conflict-checked writes; calendar payloads never
//! substitute for the workout endpoint's source identity. A move re-reads the entry, sends
//! Garmin's own object back with only the date and workout identity set, and never retries.

use super::{ConnectApi, connect::upstream};
use crate::{
    health::Date,
    workouts::{
        WorkoutError, WorkoutGateway,
        coach::{Entry, Gateway, Plan, Scheduler, Session, Snapshot, Window},
    },
};
use anyhow::{Context, Result, ensure};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{Value, json};
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

// --- calendar entry moves ---

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduleEntry {
    workout_schedule_id: u64,
    calendar_date: String,
    workout: WorkoutRef,
}
/// Decode only the identity Pace compares; the rest of Garmin's object is passed through.
fn entry_of(value: &Value, schedule_id: u64) -> Result<Entry> {
    let decoded: ScheduleEntry =
        serde_json::from_value(value.clone()).context("unexpected Garmin calendar entry")?;
    ensure!(
        id(decoded.workout_schedule_id)? == schedule_id,
        "Garmin returned another calendar entry"
    );
    Ok(Entry {
        schedule_id,
        date: date(&decoded.calendar_date)?,
        workout_id: id(decoded.workout.workout_id)?,
    })
}
/// Garmin's entry with the new date and the workout reduced to its identity, nothing else changed.
fn retarget(mut value: Value, to: Date, workout_id: u64) -> Result<Value> {
    let fields = value
        .as_object_mut()
        .context("Garmin calendar entry is not an object")?;
    fields.insert("calendarDate".into(), json!(to.to_string()));
    fields.insert("workout".into(), json!({"workoutId": workout_id}));
    Ok(value)
}
impl ConnectApi {
    fn schedule_path(schedule_id: u64) -> String {
        format!("/workout-service/schedule/{schedule_id}")
    }
    fn read_entry(&self, schedule_id: u64) -> Result<(Value, Entry), WorkoutError> {
        let value: Value = self
            .http
            .json(Method::GET, &Self::schedule_path(schedule_id), &[], None)
            .map_err(|error| match upstream(error) {
                WorkoutError::Rejected(404) => WorkoutError::NotFound,
                other => other,
            })?;
        let entry = entry_of(&value, schedule_id).map_err(upstream)?;
        Ok((value, entry))
    }
}
impl Scheduler for ConnectApi {
    fn entry(&self, schedule_id: u64) -> Result<Entry, WorkoutError> {
        self.read_entry(schedule_id).map(|(_, entry)| entry)
    }
    fn move_entry(&self, expected: Entry, to: Date) -> Result<(), WorkoutError> {
        let (value, current) = self.read_entry(expected.schedule_id)?;
        if current != expected {
            return Err(WorkoutError::Conflict);
        }
        let body = retarget(value, to, expected.workout_id).map_err(upstream)?;
        self.http
            .request(
                Method::PUT,
                &Self::schedule_path(expected.schedule_id),
                &[],
                Some(&body),
            )
            .map_err(upstream)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> Value {
        json!({"workoutScheduleId":99,"calendarDate":"2026-10-31","createdDate":"2026-10-01T10:00:00.0",
            "workout":{"workoutId":777,"workoutName":"Leg Day","trainingPlanId":42,"estimatedDurationInSecs":3600},
            "associatedActivityId":null,"ownerId":5})
    }
    #[test]
    fn decodes_only_the_compared_identity() -> Result<()> {
        let decoded = entry_of(&entry(), 99)?;
        assert_eq!(decoded.workout_id, 777);
        assert_eq!(decoded.date.to_string(), "2026-10-31");
        assert!(entry_of(&entry(), 98).is_err());
        let mut undated = entry();
        undated["calendarDate"] = json!("31/10/2026");
        assert!(entry_of(&undated, 99).is_err());
        Ok(())
    }
    #[test]
    fn retarget_sets_date_and_workout_identity_and_keeps_the_rest() -> Result<()> {
        let moved = retarget(entry(), "2026-11-02".parse()?, 777)?;
        assert_eq!(moved["calendarDate"], "2026-11-02");
        assert_eq!(moved["workout"], json!({"workoutId":777}));
        assert_eq!(moved["workoutScheduleId"], 99);
        assert_eq!(moved["createdDate"], "2026-10-01T10:00:00.0");
        assert_eq!(moved["ownerId"], 5);
        assert!(retarget(json!([]), "2026-11-02".parse()?, 777).is_err());
        Ok(())
    }
}
