//! src/workouts/domain.rs defines reusable workouts, editable drafts and delivery outcomes.
//! These values and invariants are independent of Garmin JSON, storage and model providers.

mod equivalence;
mod validation;

/// A reusable workout definition, separate from a completed activity.
#[derive(Clone, Debug, PartialEq)]
pub struct Workout {
    pub name: String,
    pub description: String,
    pub sport: String,
    pub pool_length_meters: Option<f64>,
    pub segments: Vec<Segment>,
}

/// One sport-specific segment; multisport definitions contain several segments.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub sport: String,
    pub steps: Vec<Step>,
}

/// A single exercise or a bounded group of repeated steps.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Exercise {
        kind: String,
        description: String,
        duration: Duration,
        target: Target,
        secondary_target: Option<Target>,
        exercise: Option<Box<Exercise>>,
    },
    Repeat {
        count: u32,
        skip_last_rest: bool,
        steps: Vec<Step>,
    },
}

/// Completion condition with explicit physical units.
#[derive(Clone, Debug, PartialEq)]
pub enum Duration {
    Seconds(f64),
    Meters(f64),
    Repetitions(u32),
    MinimumRepetitions(u32),
    LapButton,
}

/// An explicit intensity range, athlete zone, or secondary swim CSS offset.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    None,
    Range {
        unit: String,
        minimum: f64,
        maximum: f64,
    },
    Zone {
        kind: String,
        number: u8,
    },
    SwimCssOffset(f64),
}

/// Strength exercise identity with one load mode: kilograms, benchmark share or Coach label.
#[derive(Clone, Debug, PartialEq)]
pub struct Exercise {
    pub category: String,
    pub name: String,
    pub weight_kg: Option<f64>,
    pub benchmark: Option<BenchmarkLoad>,
    /// Coach intensity label: light, moderate or heavy; excludes other load modes.
    pub relative_load: Option<String>,
}

/// A load expressed as a percentage of the athlete's benchmark for a Garmin exercise key.
#[derive(Clone, Debug, PartialEq)]
pub struct BenchmarkLoad {
    pub percentage: u8,
    pub key: String,
}

/// Read-back snapshot; unsupported definitions remain visible without lossy edits.
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryWorkout {
    /// Garmin plan identity, preserved on update and checked during verification.
    pub coach_plan_id: Option<u64>,
    pub id: u64,
    pub name: String,
    pub sport: String,
    pub revision: String,
    pub synced_at: String,
    pub definition: Option<Workout>,
    pub read_only_reason: Option<String>,
    /// Display-only reading of a read-only workout; unsupported semantics appear as step
    /// notes, and it is never written back to Garmin.
    pub outline: Option<Workout>,
}

/// Bounded list row without loading every workout definition.
#[derive(Clone, Debug)]
pub struct WorkoutSummary {
    pub id: u64,
    pub name: String,
    pub sport: String,
    pub revision: String,
    pub synced_at: String,
    pub editable: bool,
}

/// A model-authored candidate tied to the exact reviewed source snapshot.
#[derive(Clone, Debug)]
pub struct Draft {
    /// Model reasoning and evidence for the reviewed change; never sent as workout copy.
    pub explanation: Option<String>,
    pub id: u64,
    pub revision: u64,
    pub workout: Workout,
    pub source: Option<LibraryWorkout>,
    pub state: DraftState,
    pub remote_id: Option<u64>,
    /// User instructions a model quoted for MCP-initiated Garmin writes; None for CLI/menu saves.
    pub mcp_request: Option<String>,
}

/// A pending write is never silently retried, even after a process restart.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DraftState {
    Draft,
    Saving,
    Saved,
    Verified,
    Discarded,
}
impl DraftState {
    const ALL: [Self; 5] = [
        Self::Draft,
        Self::Saving,
        Self::Saved,
        Self::Verified,
        Self::Discarded,
    ];
    /// Stable persistence and transport label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Saving => "saving",
            Self::Saved => "saved",
            Self::Verified => "verified",
            Self::Discarded => "discarded",
        }
    }
}
impl std::str::FromStr for DraftState {
    type Err = WorkoutError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|state| state.as_str() == value)
            .ok_or(WorkoutError::InvalidInput("invalid stored draft state"))
    }
}

/// Registered device returned by the selected Garmin account.
#[derive(Clone, Debug)]
pub struct Device {
    pub id: u64,
    pub model: String,
}

/// An accepted delivery request does not establish receipt on the physical watch.
#[derive(Clone, Debug)]
pub struct Delivery {
    pub workout_id: u64,
    pub device_id: u64,
    pub revision: String,
    pub queued: bool,
    /// The user instruction a model quoted when MCP requested this delivery.
    pub request: Option<String>,
}

/// Accept a quoted user instruction for a Garmin write: 1..=1000 bytes of visible text.
pub fn validate_request(text: &str) -> Result<(), WorkoutError> {
    if text.trim().is_empty() || text.len() > 1000 || text.chars().any(char::is_control) {
        return Err(WorkoutError::InvalidInput(
            "user_request must be 1..=1000 bytes of non-blank text without control characters",
        ));
    }
    Ok(())
}

/// Workout errors distinguish invalid proposals, stale reviews and uncertain writes.
#[derive(Debug, thiserror::Error)]
pub enum WorkoutError {
    #[error("{0}")]
    InvalidInput(&'static str),
    #[error("training source or draft changed; refresh and review the current revision")]
    Conflict,
    #[error("training source or draft was not found in this profile")]
    NotFound,
    #[error("Garmin write outcome is unresolved; reconcile before continuing")]
    Uncertain,
    #[error("Garmin rejected the request with HTTP {0}; nothing was changed remotely")]
    Rejected(u16),
    #[error("Garmin training request failed: {0}")]
    Upstream(String),
    #[error("training storage is unavailable")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Counts across all actionable drafts and the oldest pending save (otherwise oldest draft).
#[derive(Debug, PartialEq)]
pub struct DraftAttention {
    pub ready: u64,
    pub pending: u64,
    pub oldest_ready: Option<u64>,
    pub oldest_pending: Option<u64>,
}

impl DraftAttention {
    /// Reconcile uncertain saves before considering any new remote write.
    pub fn next_id(&self) -> Option<u64> {
        self.oldest_pending.or(self.oldest_ready)
    }
}
