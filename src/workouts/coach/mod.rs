//! src/workouts/coach/mod.rs models bounded Coach calendar snapshots, their import ports and
//! the explicit, journaled move of one scheduled session. The service commits plans, sessions
//! and editable library sources together; adaptation remains a model-authored, explicitly
//! reviewed workout draft, and a move is a user-requested one-shot write, never background work.

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
/// Where one explicit move stands; only `moving` is unresolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RescheduleState {
    /// Journaled; Garmin's answer is unknown or the read-back did not confirm the new date.
    Moving,
    /// Garmin returned the entry on the requested date.
    Moved,
    /// Garmin refused outright, or the read-back still showed the original date.
    NotApplied,
    /// Garmin no longer returns the calendar entry at all.
    Missing,
}
impl RescheduleState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Moving => "moving",
            Self::Moved => "moved",
            Self::NotApplied => "not_applied",
            Self::Missing => "missing",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "moving" => Some(Self::Moving),
            "moved" => Some(Self::Moved),
            "not_applied" => Some(Self::NotApplied),
            "missing" => Some(Self::Missing),
            _ => None,
        }
    }
}
/// One journaled move of a scheduled session, recorded before Garmin is contacted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reschedule {
    pub id: u64,
    pub schedule_id: u64,
    pub workout_id: u64,
    pub from: Date,
    pub to: Date,
    pub state: RescheduleState,
    pub requested_at: String,
    /// The user instruction a model quoted when MCP requested the move.
    pub request: Option<String>,
}
/// The identity of one Garmin calendar entry as the scheduling adapter reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub schedule_id: u64,
    pub date: Date,
    pub workout_id: u64,
}
/// Read-only Garmin source. The adapter owns pacing, DTO validation and failure policy.
pub trait Gateway {
    /// Fetch one bounded source snapshot; never modify Garmin.
    fn fetch(&self, window: Window) -> Result<Snapshot, WorkoutError>;
}
/// Garmin calendar writes for one entry. `Rejected` and `Conflict` mean nothing changed
/// remotely; any other failure of `move_entry` leaves the outcome uncertain.
pub trait Scheduler {
    /// Read one calendar entry; `NotFound` when Garmin no longer returns it.
    fn entry(&self, schedule_id: u64) -> Result<Entry, WorkoutError>;
    /// Move the entry only while its remote date and workout still equal `expected`.
    fn move_entry(&self, expected: Entry, to: Date) -> Result<(), WorkoutError>;
}
/// Local snapshot storage; the adapter implements the use case's atomic commit boundary.
pub trait Repository: Send + Sync {
    /// Commit calendar context and its canonical library sources atomically.
    fn put_coach(&self, snapshot: &Snapshot) -> Result<(), WorkoutError>;
    /// Read one exact imported window; None means it has not been imported.
    fn get_coach(&self, window: Window) -> Result<Option<Snapshot>, WorkoutError>;
    /// The most recently imported copy of one session, from any stored window.
    fn find_session(&self, schedule_id: u64) -> Result<Option<Session>, WorkoutError>;
    /// Journal a move as `moving`; `Conflict` when one is already unresolved for that entry.
    fn begin_reschedule(&self, reschedule: &Reschedule) -> Result<u64, WorkoutError>;
    /// Close the unresolved `moving` row of one entry with its terminal state.
    fn finish_reschedule(
        &self,
        schedule_id: u64,
        state: RescheduleState,
    ) -> Result<(), WorkoutError>;
    /// The unresolved move of one entry, if any.
    fn pending_reschedule(&self, schedule_id: u64) -> Result<Option<Reschedule>, WorkoutError>;
    /// Every journaled move of one entry, newest first.
    fn reschedules_of(&self, schedule_id: u64) -> Result<Vec<Reschedule>, WorkoutError>;
    /// Journaled moves leaving or joining a window, newest first; bounded by the window.
    fn reschedules(&self, window: Window) -> Result<Vec<Reschedule>, WorkoutError>;
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
    /// Journaled moves touching a window, read locally beside its snapshot.
    pub fn coach_reschedules(&self, window: Window) -> Result<Vec<Reschedule>, WorkoutError> {
        self.repository.reschedules(window)
    }
    /// Journaled moves of one scheduled session, newest first.
    pub fn session_reschedules(&self, schedule_id: u64) -> Result<Vec<Reschedule>, WorkoutError> {
        positive(schedule_id)?;
        self.repository.reschedules_of(schedule_id)
    }
    /// Move one imported session to another Garmin calendar date, journaling first.
    /// A definite refusal closes the row; an unknown outcome leaves it `moving`.
    pub fn reschedule(
        &self,
        scheduler: &dyn Scheduler,
        schedule_id: u64,
        to: Date,
        request: Option<&str>,
    ) -> Result<Reschedule, WorkoutError> {
        if let Some(text) = request {
            super::validate_request(text)?;
        }
        positive(schedule_id)?;
        let session =
            self.repository
                .find_session(schedule_id)?
                .ok_or(WorkoutError::InvalidInput(
                    "schedule ID is not in any imported Coach window; sync that week first",
                ))?;
        if session.date == to {
            return Err(WorkoutError::InvalidInput(
                "the session is already scheduled on that date",
            ));
        }
        let mut reschedule = Reschedule {
            id: 0,
            schedule_id,
            workout_id: session.workout.id,
            from: session.date,
            to,
            state: RescheduleState::Moving,
            requested_at: chrono::Utc::now().to_rfc3339(),
            request: request.map(str::to_owned),
        };
        reschedule.id =
            self.repository
                .begin_reschedule(&reschedule)
                .map_err(|error| match error {
                    WorkoutError::Conflict => WorkoutError::InvalidInput(
                        "this session has an unresolved move; reconcile or clear it first",
                    ),
                    other => other,
                })?;
        let expected = Entry {
            schedule_id,
            date: session.date,
            workout_id: session.workout.id,
        };
        if let Err(error) = scheduler.move_entry(expected, to) {
            return Err(match error {
                WorkoutError::Rejected(_) => {
                    self.repository
                        .finish_reschedule(schedule_id, RescheduleState::NotApplied)?;
                    error
                }
                WorkoutError::Conflict => {
                    self.repository
                        .finish_reschedule(schedule_id, RescheduleState::NotApplied)?;
                    WorkoutError::InvalidInput(
                        "the Garmin calendar entry's date or workout no longer matches the imported week; sync that week and ask again",
                    )
                }
                WorkoutError::NotFound => {
                    self.repository
                        .finish_reschedule(schedule_id, RescheduleState::Missing)?;
                    error
                }
                // The write may have reached Garmin; only a read-back can tell.
                _ => WorkoutError::Uncertain,
            });
        }
        self.settle(scheduler, reschedule, true)
    }
    /// Resolve an unresolved move by reading the entry back; never writes.
    pub fn reconcile_reschedule(
        &self,
        scheduler: &dyn Scheduler,
        schedule_id: u64,
        request: Option<&str>,
    ) -> Result<Reschedule, WorkoutError> {
        if let Some(text) = request {
            super::validate_request(text)?;
        }
        positive(schedule_id)?;
        let pending = self
            .repository
            .pending_reschedule(schedule_id)?
            .ok_or(NO_PENDING_MOVE)?;
        self.settle(scheduler, pending, false)
    }
    /// Close an unresolved move after the user checked the Garmin calendar themselves.
    pub fn clear_reschedule(&self, schedule_id: u64) -> Result<(), WorkoutError> {
        positive(schedule_id)?;
        self.repository
            .pending_reschedule(schedule_id)?
            .ok_or(NO_PENDING_MOVE)?;
        self.repository
            .finish_reschedule(schedule_id, RescheduleState::NotApplied)
    }
    /// Compare the remote entry with the journaled move and record what Garmin shows. Right
    /// after an accepted write the original date is not proof of refusal, so it stays open.
    fn settle(
        &self,
        scheduler: &dyn Scheduler,
        mut reschedule: Reschedule,
        write_accepted: bool,
    ) -> Result<Reschedule, WorkoutError> {
        let state = match scheduler.entry(reschedule.schedule_id) {
            Ok(entry) if entry.date == reschedule.to => RescheduleState::Moved,
            Ok(entry) if entry.date == reschedule.from && !write_accepted => {
                RescheduleState::NotApplied
            }
            Ok(_) => return Err(WorkoutError::Uncertain),
            Err(WorkoutError::NotFound) => RescheduleState::Missing,
            Err(_) => return Err(WorkoutError::Uncertain),
        };
        self.repository
            .finish_reschedule(reschedule.schedule_id, state)?;
        reschedule.state = state;
        Ok(reschedule)
    }
}
const NO_PENDING_MOVE: WorkoutError =
    WorkoutError::InvalidInput("no unresolved move is journaled for this schedule ID");
fn positive(id: u64) -> Result<(), WorkoutError> {
    if id == 0 || id > i64::MAX as u64 {
        return Err(WorkoutError::InvalidInput(
            "schedule ID must be a positive Garmin identifier",
        ));
    }
    Ok(())
}

mod sqlite;
