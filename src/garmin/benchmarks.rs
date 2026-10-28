//! src/garmin/benchmarks.rs adapts native benchmark reads/writes and strength evidence imports.
//! The Personal API lists activities; exercise sets exist only on the native Connect session.
//! All reads are bounded; source comparison precedes a single non-retried POST or PUT.

use super::{ConnectApi, PersonalApi, benchmark_decode};
use crate::health::Date;
use crate::{
    config::{GarminAuthSettings, GarminSettings},
    history::Activity,
    workouts::{WorkoutError, benchmarks::*},
};
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Days, NaiveDate};
use reqwest::Method;
use serde_json::{Value, json};

const PAGE: usize = 50;
const MAX_ACTIVITIES: usize = 100;
const MAX_CANDIDATES: usize = 1_000;
/// Garmin's activity search filters by parent type only; strength is one of its children.
const SEARCH_TYPE: &str = "fitness_equipment";
const STRENGTH: &str = "strength_training";

/// Composition used by CLI operations and, for granted profiles, the MCP Garmin tools.
pub struct GarminBenchmarks {
    native: ConnectApi,
    personal: Option<PersonalApi>,
}
impl GarminBenchmarks {
    /// Native auth is required; Personal API credentials are required only for window imports.
    pub fn connect(
        settings: &GarminSettings,
        auth: &GarminAuthSettings,
        personal_key: Option<&str>,
    ) -> Result<Self> {
        Ok(Self {
            native: ConnectApi::connect(settings, auth)?,
            personal: personal_key
                .map(|key| PersonalApi::connect(settings, key))
                .transpose()?,
        })
    }
    fn read_catalog(&self) -> Result<Vec<Value>> {
        let rows: Vec<Value> =
            self.native
                .http
                .json(Method::GET, "/workout-service/benchmarks", &[], None)?;
        ensure!(rows.len() <= 128, "benchmark catalogue exceeds 128 entries");
        Ok(rows)
    }
    fn read_sets(&self, id: u64) -> Result<Vec<ObservedSet>> {
        let value: Value = self.native.http.json(
            Method::GET,
            &format!("/activity-service/activity/{id}/exerciseSets"),
            &[],
            None,
        )?;
        benchmark_decode::sets(&value, id)
    }
    fn import_strength(&self, window: Window) -> Result<StrengthImport> {
        let api = self
            .personal
            .as_ref()
            .context("strength import requires the Personal API credential")?;
        // Garmin's date filter is local; pad the query by a day on each side of the UTC window.
        // The parent type filter also returns sibling sports, which are skipped locally.
        let start = NaiveDate::from(window.start)
            .checked_sub_days(Days::new(1))
            .context("date range underflow")?;
        let end = NaiveDate::from(window.end)
            .checked_add_days(Days::new(1))
            .context("date range overflow")?;
        let now = chrono::Utc::now().to_rfc3339();
        let mut activities = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut offset = 0;
        loop {
            ensure!(
                offset < MAX_CANDIDATES,
                "too many candidate activities; choose a shorter window"
            );
            let rows: Vec<super::personal::RemoteActivity> = api.http.json(
                Method::GET,
                "/activitylist-service/activities/search/activities",
                &[
                    ("start", offset.to_string()),
                    ("limit", PAGE.to_string()),
                    ("startDate", start.to_string()),
                    ("endDate", end.to_string()),
                    ("activityType", SEARCH_TYPE.into()),
                ],
                None,
            )?;
            ensure!(
                rows.len() <= PAGE,
                "Garmin exceeded the requested activity page"
            );
            let count = rows.len();
            offset += count;
            for row in rows {
                let activity: Activity = row.try_into()?;
                if activity.sport != STRENGTH {
                    continue;
                }
                ensure!(
                    seen.insert(activity.garmin_id.clone()),
                    "duplicate strength activity"
                );
                activities.push(activity);
            }
            ensure!(
                activities.len() <= MAX_ACTIVITIES,
                "more than 100 strength activities; choose a shorter window"
            );
            if count < PAGE {
                break;
            }
        }
        let mut sessions = Vec::new();
        for activity in activities {
            let date = Date::from(DateTime::parse_from_rfc3339(&activity.started_at)?.date_naive());
            // Keep only activities whose UTC date falls inside the requested evidence window.
            if date < window.start || date > window.end {
                continue;
            }
            let id = activity.garmin_id.parse::<u64>()?;
            sessions.push(StrengthSession {
                activity_id: id,
                date,
                started_at: activity.started_at,
                synced_at: now.clone(),
                sets: self.read_sets(id)?,
            });
        }
        Ok(StrengthImport {
            window,
            synced_at: now,
            sessions,
        })
    }
}
use super::connect::upstream;
impl BenchmarkGateway for GarminBenchmarks {
    fn catalog(&self) -> Result<Vec<Benchmark>, WorkoutError> {
        let rows = self.read_catalog().map_err(upstream)?;
        let now = chrono::Utc::now().to_rfc3339();
        let rows = rows
            .iter()
            .map(|row| benchmark_decode::benchmark(row, &now).map_err(upstream))
            .collect::<Result<Vec<_>, _>>()?;
        let unique: std::collections::HashSet<_> =
            rows.iter().map(|row| &row.exercise.name).collect();
        if unique.len() != rows.len() {
            return Err(WorkoutError::InvalidInput(
                "duplicate Garmin benchmark keys",
            ));
        }
        Ok(rows)
    }
    fn strength(&self, window: Window) -> Result<StrengthImport, WorkoutError> {
        self.import_strength(window).map_err(upstream)
    }
    fn activity_session(
        &self,
        activity_id: u64,
        date: Date,
        started_at: &str,
    ) -> Result<StrengthSession, WorkoutError> {
        Ok(StrengthSession {
            activity_id,
            date,
            started_at: started_at.to_owned(),
            synced_at: chrono::Utc::now().to_rfc3339(),
            sets: self.read_sets(activity_id).map_err(upstream)?,
        })
    }
    fn save(&self, source: &Benchmark, proposal: &Proposal) -> Result<(), WorkoutError> {
        let rows = self.read_catalog().map_err(upstream)?;
        let mut raw = rows
            .into_iter()
            .find(|row| row["benchmarkKey"].as_str() == Some(&proposal.key))
            .ok_or(WorkoutError::Conflict)?;
        let current = benchmark_decode::benchmark(&raw, &chrono::Utc::now().to_rfc3339())
            .map_err(upstream)?;
        if current.revision != source.revision {
            return Err(WorkoutError::Conflict);
        }
        let linked = if source.id.is_none() {
            json!(
                source
                    .linked
                    .iter()
                    .map(|e| json!({"exerciseKey":e.name,"categoryKey":e.category}))
                    .collect::<Vec<_>>()
            )
        } else {
            raw.get_mut("linkedBenchmarkExercises")
                .ok_or(WorkoutError::InvalidInput(
                    "missing benchmark exercise links",
                ))?
                .take()
        };
        // The write contract excludes read-only ID/date metadata from the catalogue.
        let payload = json!({
            "benchmarkKey":source.exercise.name,
            "benchmarkCategoryKey":source.exercise.category,
            "maxWeight":proposal.weight_kg,"reps":proposal.repetitions,
            "oneRepMax":proposal.one_rep_max_kg,"weightUnit":{"unitKey":"kilogram"},
            "linkedBenchmarkExercises":linked
        });
        let (method, path) = match source.id {
            Some(id) => (Method::PUT, format!("/workout-service/benchmarks/{id}")),
            None => (Method::POST, "/workout-service/benchmarks".into()),
        };
        self.native
            .http
            .request(method, &path, &[], Some(&payload))
            .map_err(upstream)?;
        Ok(())
    }
}
