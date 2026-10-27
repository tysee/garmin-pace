//! src/workouts/coach/mod.rs models bounded Coach calendar snapshots and their import ports.
//! The service commits plans, sessions and editable library sources together; adaptation
//! remains a model-authored, explicitly reviewed workout draft rather than background work.

pub mod document;
use super::{LibraryWorkout, WorkoutError, WorkoutService};
use crate::health::Date;

/// Inclusive Garmin calendar window, never longer than seven days.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub start: Date,
    pub end: Date,
}
impl Window {
    /// Monday through Sunday containing the explicitly supplied Garmin calendar date.
    pub fn week_containing(day: Date) -> Result<Self, WorkoutError> {
        use chrono::Datelike;
        let offset = chrono::NaiveDate::from(day)
            .weekday()
            .num_days_from_monday();
        let start = day
            .back(u64::from(offset))
            .ok_or(WorkoutError::InvalidInput("Coach week date overflow"))?;
        let end = start
            .forward(6)
            .ok_or(WorkoutError::InvalidInput("Coach week date overflow"))?;
        Self::new(start, end)
    }
    /// The neighboring calendar week, without reading a clock or reaching storage.
    pub fn adjacent(self, next: bool) -> Result<Self, WorkoutError> {
        let start = if next {
            self.start.forward(7)
        } else {
            self.start.back(7)
        };
        Self::week_containing(start.ok_or(WorkoutError::InvalidInput("Coach week date overflow"))?)
    }

    /// Validate calendar dates and resource bounds before reaching storage or Garmin.
    pub fn new(start: Date, end: Date) -> Result<Self, WorkoutError> {
        let mut day = start;
        for _ in 0..7 {
            if day == end {
                return Ok(Self { start, end });
            }
            day = day
                .next()
                .ok_or_else(|| WorkoutError::InvalidInput("invalid Coach date window"))?;
        }
        Err(WorkoutError::InvalidInput(
            "Coach window must be ordered and at most seven days",
        ))
    }
}
/// Metadata of a plan active at some point in the requested window.
#[derive(Clone, Debug)]
pub struct Plan {
    pub id: u64,
    pub name: String,
    pub category: String,
    pub status: String,
    pub start: Date,
    pub end: Date,
}
/// One calendar assignment; repeated dates can refer to the same workout definition.
#[derive(Clone, Debug)]
pub struct Session {
    pub schedule_id: u64,
    pub date: Date,
    pub workout: LibraryWorkout,
    pub associated_activity_id: Option<String>,
    pub estimated_seconds: Option<u32>,
}
/// One complete bounded import, including absence of sessions and its synchronization time.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub window: Window,
    pub synced_at: String,
    pub plans: Vec<Plan>,
    pub sessions: Vec<Session>,
}
/// Read-only Garmin source. The adapter owns pacing, DTO validation and failure policy.
pub trait Gateway {
    /// Fetch one bounded source snapshot; never modify Garmin.
    fn fetch(&self, window: Window) -> Result<Snapshot, WorkoutError>;
}
/// Local snapshot storage; the adapter implements the use case's atomic commit boundary.
pub trait Repository: Send + Sync {
    /// Commit calendar context and its canonical library sources atomically.
    fn put_coach(&self, snapshot: &Snapshot) -> Result<(), WorkoutError>;
    /// Read one exact imported window; None means it has not been imported.
    fn get_coach(&self, window: Window) -> Result<Option<Snapshot>, WorkoutError>;
}
impl WorkoutService {
    /// Explicitly import Coach context and sources; a failed fetch cannot replace local data.
    pub fn sync_coach(
        &self,
        gateway: &dyn Gateway,
        window: Window,
    ) -> Result<Snapshot, WorkoutError> {
        let snapshot = gateway.fetch(window)?;
        if snapshot.window != window || snapshot.plans.len() > 100 || snapshot.sessions.len() > 100
        {
            return Err(WorkoutError::InvalidInput(
                "Coach response exceeds requested bounds",
            ));
        }
        let plans = snapshot
            .plans
            .into_iter()
            .filter(|p| p.start <= window.end && p.end >= window.start)
            .collect();
        let snapshot = Snapshot { plans, ..snapshot };
        if snapshot.sessions.iter().any(|s| {
            s.workout
                .coach_plan_id
                .is_some_and(|id| !snapshot.plans.iter().any(|p| p.id == id))
        }) {
            return Err(WorkoutError::InvalidInput(
                "Coach session has no plan in the requested window",
            ));
        }
        self.repository.put_coach(&snapshot)?;
        Ok(snapshot)
    }
    /// Read the exact previously imported local window without contacting Garmin.
    pub fn coach(&self, window: Window) -> Result<Option<Snapshot>, WorkoutError> {
        self.repository.get_coach(window)
    }
}

mod sqlite;
