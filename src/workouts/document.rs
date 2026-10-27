//! src/workouts/document.rs defines the v1 model/file contract and persistence codec.
//! Conversion validates domain invariants; Garmin-specific JSON never enters this document.

use super::{
    BenchmarkLoad, Duration, Exercise, LibraryWorkout, Segment, Step, Target, Workout, WorkoutError,
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

/// Complete candidate supplied by a model, with explicit units and no server IDs.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkoutDocument {
    /// Must be 1.
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// running, cycling, other, swimming, strength_training, cardio_training, yoga, pilates, hiit, multi_sport or mobility.
    pub sport: String,
    pub pool_length_meters: Option<f64>,
    pub segments: Vec<SegmentDocument>,
}
/// A sport-specific section of the model proposal.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SegmentDocument {
    pub sport: String,
    pub steps: Vec<StepDocument>,
}

/// Tagged executable step or counted repeat group.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum StepDocument {
    Step {
        /// warmup, cooldown, interval, recovery, rest, other or main.
        kind: String,
        #[serde(default)]
        description: String,
        duration: DurationDocument,
        target: TargetDocument,
        secondary_target: Option<TargetDocument>,
        exercise: Option<Box<ExerciseDocument>>,
    },
    Repeat {
        count: u32,
        #[serde(default)]
        skip_last_rest: bool,
        steps: Vec<StepDocument>,
    },
}
/// Completion values expressed in seconds, meters or repetition counts.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DurationDocument {
    Time { seconds: f64 },
    Distance { meters: f64 },
    Repetitions { count: u32 },
    MinimumRepetitions { count: u32 },
    LapButton,
}
/// Intensity expressed as an explicit physical range or athlete zone.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TargetDocument {
    None,
    Range {
        /// pace_seconds_per_km, speed_mps, heart_rate_bpm, power_watts or cadence_per_minute.
        unit: String,
        minimum: f64,
        maximum: f64,
    },
    Zone {
        /// heart_rate (1-5) or power (1-7).
        kind: String,
        number: u8,
    },
    SwimCssOffset {
        seconds: f64,
    },
}
/// Garmin exercise identity with an optional kilogram, benchmark or relative Coach load.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExerciseDocument {
    /// Use an exercise category/name returned by Garmin; do not invent identifiers.
    pub category: String,
    pub name: String,
    /// Absolute load in kilograms; omit when the load is a benchmark percentage.
    pub weight_kg: Option<f64>,
    /// Load as a whole-number percentage (1-100) of the athlete's benchmark one-rep max
    /// for `benchmark_key`; requires `benchmark_key` and excludes `weight_kg`.
    pub benchmark_percentage: Option<u8>,
    /// Garmin benchmark exercise key the percentage refers to, such as BARBELL_DEADLIFT
    /// from list_benchmarks_v1; requires `benchmark_percentage`.
    pub benchmark_key: Option<String>,
    /// Coach load label: light, moderate or heavy. Excludes kilograms and benchmark load.
    pub relative_load: Option<String>,
}

/// Stored/read-only library snapshot with a revision required for editing.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryDocument {
    /// Absent on existing stored snapshots until the workout is refreshed.
    #[serde(default)]
    pub coach_plan_id: Option<u64>,
    pub id: u64,
    pub name: String,
    pub sport: String,
    pub revision: String,
    pub synced_at: String,
    pub definition: Option<WorkoutDocument>,
    pub read_only_reason: Option<String>,
    /// Display-only steps of a read-only workout; absent for rows synced before outlines existed.
    #[serde(default)]
    pub outline: Option<WorkoutDocument>,
}

impl TryFrom<WorkoutDocument> for Workout {
    type Error = WorkoutError;
    fn try_from(d: WorkoutDocument) -> Result<Self, Self::Error> {
        if d.version != 1 {
            return Err(WorkoutError::InvalidInput(
                "unsupported workout document version",
            ));
        }
        let value = Self {
            name: d.name,
            description: d.description,
            sport: d.sport,
            pool_length_meters: d.pool_length_meters,
            segments: d
                .segments
                .into_iter()
                .map(|s| Segment {
                    sport: s.sport,
                    steps: s.steps.into_iter().map(Into::into).collect(),
                })
                .collect(),
        };
        value.validate()?;
        Ok(value)
    }
}
impl From<Workout> for WorkoutDocument {
    fn from(d: Workout) -> Self {
        Self {
            version: 1,
            name: d.name,
            description: d.description,
            sport: d.sport,
            pool_length_meters: d.pool_length_meters,
            segments: d
                .segments
                .into_iter()
                .map(|s| SegmentDocument {
                    sport: s.sport,
                    steps: s.steps.into_iter().map(Into::into).collect(),
                })
                .collect(),
        }
    }
}
impl From<StepDocument> for Step {
    fn from(d: StepDocument) -> Self {
        match d {
            StepDocument::Repeat {
                count,
                skip_last_rest,
                steps,
            } => Self::Repeat {
                count,
                skip_last_rest,
                steps: steps.into_iter().map(Into::into).collect(),
            },
            StepDocument::Step {
                kind,
                description,
                duration,
                target,
                secondary_target,
                exercise,
            } => Self::Exercise {
                kind,
                description,
                duration: duration.into(),
                target: target.into(),
                secondary_target: secondary_target.map(Into::into),
                exercise: exercise.map(|e| {
                    Box::new(Exercise {
                        category: e.category,
                        name: e.name,
                        weight_kg: e.weight_kg,
                        relative_load: e.relative_load,
                        benchmark: benchmark_load(e.benchmark_percentage, e.benchmark_key),
                    })
                }),
            },
        }
    }
}
/// A half-specified benchmark load keeps a sentinel that validation rejects by message.
fn benchmark_load(percentage: Option<u8>, key: Option<String>) -> Option<BenchmarkLoad> {
    match (percentage, key) {
        (None, None) => None,
        (percentage, key) => Some(BenchmarkLoad {
            percentage: percentage.unwrap_or(0),
            key: key.unwrap_or_default(),
        }),
    }
}
impl From<Step> for StepDocument {
    fn from(d: Step) -> Self {
        match d {
            Step::Repeat {
                count,
                skip_last_rest,
                steps,
            } => Self::Repeat {
                count,
                skip_last_rest,
                steps: steps.into_iter().map(Into::into).collect(),
            },
            Step::Exercise {
                kind,
                description,
                duration,
                target,
                secondary_target,
                exercise,
            } => Self::Step {
                kind,
                description,
                duration: duration.into(),
                target: target.into(),
                secondary_target: secondary_target.map(Into::into),
                exercise: exercise.map(|e| {
                    Box::new(ExerciseDocument {
                        category: e.category,
                        name: e.name,
                        weight_kg: e.weight_kg,
                        relative_load: e.relative_load,
                        benchmark_percentage: e.benchmark.as_ref().map(|b| b.percentage),
                        benchmark_key: e.benchmark.map(|b| b.key),
                    })
                }),
            },
        }
    }
}
impl From<DurationDocument> for Duration {
    fn from(d: DurationDocument) -> Self {
        match d {
            DurationDocument::Time { seconds } => Self::Seconds(seconds),
            DurationDocument::Distance { meters } => Self::Meters(meters),
            DurationDocument::Repetitions { count } => Self::Repetitions(count),
            DurationDocument::MinimumRepetitions { count } => Self::MinimumRepetitions(count),
            DurationDocument::LapButton => Self::LapButton,
        }
    }
}
impl From<Duration> for DurationDocument {
    fn from(d: Duration) -> Self {
        match d {
            Duration::Seconds(seconds) => Self::Time { seconds },
            Duration::Meters(meters) => Self::Distance { meters },
            Duration::Repetitions(count) => Self::Repetitions { count },
            Duration::MinimumRepetitions(count) => Self::MinimumRepetitions { count },
            Duration::LapButton => Self::LapButton,
        }
    }
}
impl From<TargetDocument> for Target {
    fn from(d: TargetDocument) -> Self {
        match d {
            TargetDocument::None => Self::None,
            TargetDocument::Range {
                unit,
                minimum,
                maximum,
            } => Self::Range {
                unit,
                minimum,
                maximum,
            },
            TargetDocument::Zone { kind, number } => Self::Zone { kind, number },
            TargetDocument::SwimCssOffset { seconds } => Self::SwimCssOffset(seconds),
        }
    }
}
impl From<Target> for TargetDocument {
    fn from(d: Target) -> Self {
        match d {
            Target::None => Self::None,
            Target::Range {
                unit,
                minimum,
                maximum,
            } => Self::Range {
                unit,
                minimum,
                maximum,
            },
            Target::Zone { kind, number } => Self::Zone { kind, number },
            Target::SwimCssOffset(seconds) => Self::SwimCssOffset { seconds },
        }
    }
}
impl From<LibraryWorkout> for LibraryDocument {
    fn from(d: LibraryWorkout) -> Self {
        Self {
            id: d.id,
            coach_plan_id: d.coach_plan_id,
            name: d.name,
            sport: d.sport,
            revision: d.revision,
            synced_at: d.synced_at,
            definition: d.definition.map(Into::into),
            read_only_reason: d.read_only_reason,
            outline: d.outline.map(Into::into),
        }
    }
}
impl TryFrom<LibraryDocument> for LibraryWorkout {
    type Error = WorkoutError;
    fn try_from(d: LibraryDocument) -> Result<Self, Self::Error> {
        Ok(Self {
            id: d.id,
            coach_plan_id: d.coach_plan_id,
            name: d.name,
            sport: d.sport,
            revision: d.revision,
            synced_at: d.synced_at,
            definition: d.definition.map(TryInto::try_into).transpose()?,
            read_only_reason: d.read_only_reason,
            outline: d.outline.map(TryInto::try_into).transpose()?,
        })
    }
}
