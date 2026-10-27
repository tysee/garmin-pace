//! src/workouts/benchmarks/domain.rs defines strength evidence and model-proposed benchmark values.
//! Arithmetic summaries describe observed sets; only the connected model estimates lifting ability.

use crate::{
    health::Date,
    workouts::{DraftState, WorkoutError},
};

/// Exact Garmin exercise identity; related variants are never silently pooled.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExerciseKey {
    pub category: String,
    pub name: String,
}
/// A supported Garmin benchmark, with no values when the user has not set it.
#[derive(Clone, Debug, PartialEq)]
pub struct Benchmark {
    pub id: Option<u64>,
    pub exercise: ExerciseKey,
    pub weight_kg: Option<f64>,
    pub repetitions: Option<u32>,
    pub one_rep_max_kg: Option<f64>,
    pub linked: Vec<ExerciseKey>,
    pub revision: String,
    pub synced_at: String,
}
/// One recorded set, retaining incomplete observations for explicit exclusion reporting.
/// `category` is Garmin's agreed exercise category when no single exact exercise was
/// recorded; it describes the set but never counts as benchmark evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct ObservedSet {
    pub position: usize,
    pub active: bool,
    pub exercise: Option<ExerciseKey>,
    pub category: Option<String>,
    pub weight_kg: Option<f64>,
    pub repetitions: Option<u32>,
}
/// A bounded strength activity snapshot, with date taken from its UTC activity timestamp.
#[derive(Clone, Debug, PartialEq)]
pub struct StrengthSession {
    pub activity_id: u64,
    pub date: Date,
    pub started_at: String,
    pub synced_at: String,
    pub sets: Vec<ObservedSet>,
}
/// The stored sets of one strength activity. `import_synced_at` names the latest import
/// covering the activity's UTC date, so an absent session is explained, never a zero.
#[derive(Clone, Debug, PartialEq)]
pub struct ActivitySets {
    pub activity_id: u64,
    pub date: Date,
    pub import_synced_at: Option<String>,
    pub session: Option<StrengthSession>,
}
/// A validated inclusive evidence window; dates are explicit even when no data exists.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub start: Date,
    pub end: Date,
}
impl Window {
    /// Reject reversed and unbounded queries before storage or network calls.
    pub fn validate(self) -> Result<(), WorkoutError> {
        self.start
            .range(self.end, 31)
            .map_err(|_| WorkoutError::InvalidInput("benchmark window must contain 1..=31 days"))?;
        Ok(())
    }
}
/// One completed import window, including an explicit empty result.
#[derive(Clone, Debug, PartialEq)]
pub struct StrengthImport {
    pub window: Window,
    pub synced_at: String,
    pub sessions: Vec<StrengthSession>,
}
/// Exact evidence snapshot returned by storage with its content revision.
#[derive(Clone, Debug)]
pub struct Evidence {
    pub source: Benchmark,
    pub window: Window,
    pub revision: String,
    pub synced_at: Option<String>,
    pub sessions: Vec<StrengthSession>,
}
/// Arithmetic summaries for the benchmark's exact exercise, never an estimated 1RM.
#[derive(Clone, Debug)]
pub struct Context {
    pub evidence: Evidence,
    pub sample_count: usize,
    pub session_count: usize,
    pub mean_weight_kg: Option<f64>,
    pub mean_repetitions: Option<f64>,
    pub excluded_sets: usize,
    pub unrelated_sets: usize,
}
/// The only authorized triggers for a benchmark proposal; no automatic refresh mode exists.
#[derive(Clone, Debug, PartialEq)]
pub enum ProposalOrigin {
    UserRequest {
        request: String,
    },
    MissingForWorkout {
        draft_id: u64,
        draft_revision: u64,
        reason: String,
    },
}
/// A local model estimate with a reproducible source window and explanation.
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    pub origin: ProposalOrigin,
    pub key: String,
    pub source_revision: String,
    pub context_revision: String,
    pub window: Window,
    pub weight_kg: f64,
    pub repetitions: u32,
    pub one_rep_max_kg: f64,
    pub method: String,
    pub rationale: String,
    pub confidence: String,
}
impl Proposal {
    /// Validate model output without replacing the model's estimate with a built-in formula.
    pub fn validate(&self) -> Result<(), WorkoutError> {
        self.window.validate()?;
        let valid_origin = match &self.origin {
            ProposalOrigin::UserRequest { request } => text(request, 1000),
            ProposalOrigin::MissingForWorkout {
                draft_id,
                draft_revision,
                reason,
            } => *draft_id > 0 && *draft_revision > 0 && text(reason, 1000),
        };
        if !valid_origin {
            return Err(WorkoutError::InvalidInput(
                "record the explicit user request or the workout draft that requires a missing benchmark",
            ));
        }

        if !valid_key(&self.key)
            || self.source_revision.len() != 64
            || self.context_revision.len() != 64
        {
            return Err(WorkoutError::InvalidInput(
                "benchmark key and exact source/context revisions are required",
            ));
        }
        if !self.weight_kg.is_finite()
            || !(0.1..=453.5).contains(&self.weight_kg)
            || !(1..=20).contains(&self.repetitions)
            || !self.one_rep_max_kg.is_finite()
            || !(self.weight_kg..=1000.0).contains(&self.one_rep_max_kg)
        {
            return Err(WorkoutError::InvalidInput(
                "benchmark requires 0.1..453.5 kg, 1..20 reps and a finite estimated 1RM between the load and 1000 kg",
            ));
        }
        if !text(&self.method, 500)
            || !text(&self.rationale, 2000)
            || !matches!(self.confidence.as_str(), "low" | "medium" | "high")
        {
            return Err(WorkoutError::InvalidInput(
                "provide a method, rationale and low/medium/high confidence",
            ));
        }
        Ok(())
    }
}
/// Reviewed estimate plus its immutable source/evidence summaries and durable write state.
#[derive(Clone, Debug)]
pub struct BenchmarkDraft {
    pub id: u64,
    pub revision: u64,
    pub proposal: Proposal,
    pub source: Benchmark,
    pub context: Context,
    pub state: DraftState,
    /// User instructions a model quoted for MCP-initiated Garmin writes; None for CLI/menu saves.
    pub mcp_request: Option<String>,
}
/// Bounded draft-list row; full evidence is read only after selecting a draft.
#[derive(Clone, Debug)]
pub struct BenchmarkDraftSummary {
    pub id: u64,
    pub revision: u64,
    pub key: String,
    pub state: DraftState,
}

/// Public validation shared with wire and persistence boundaries.
pub fn valid_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
}
fn text(value: &str, bound: usize) -> bool {
    !value.trim().is_empty() && value.len() <= bound && !value.chars().any(char::is_control)
}
/// Distinguish missing/unusable sets from zero load; averages use equal weight per usable set.
pub fn summarize(mut evidence: Evidence) -> Result<Context, WorkoutError> {
    let mut count = 0;
    let mut session_count = 0;
    let mut weight = 0.0;
    let mut reps = 0u64;
    let mut excluded = 0;
    let mut unrelated = 0;
    for session in &evidence.sessions {
        let before = count;
        for set in &session.sets {
            if set
                .exercise
                .as_ref()
                .is_some_and(|e| e != &evidence.source.exercise)
            {
                unrelated += 1;
                continue;
            }
            match (
                set.active,
                set.exercise.as_ref(),
                set.weight_kg,
                set.repetitions,
            ) {
                (true, Some(_), Some(w), Some(r))
                    if w.is_finite() && (0.1..=453.5).contains(&w) && (1..=20).contains(&r) =>
                {
                    count += 1;
                    weight += w;
                    reps += u64::from(r);
                }
                _ => excluded += 1,
            }
        }
        if count > before {
            session_count += 1;
        }
    }
    if count > 2000 {
        return Err(WorkoutError::InvalidInput(
            "more than 2000 matched sets; select a shorter evidence window",
        ));
    }
    // Keep only usable observed sets in the immutable review snapshot; counts explain exclusions.
    for session in &mut evidence.sessions {
        session.sets.retain(|set| {
            set.active
                && set.exercise.as_ref() == Some(&evidence.source.exercise)
                && set
                    .weight_kg
                    .is_some_and(|w| w.is_finite() && (0.1..=453.5).contains(&w))
                && set.repetitions.is_some_and(|r| (1..=20).contains(&r))
        });
    }
    evidence.sessions.retain(|session| !session.sets.is_empty());
    Ok(Context {
        evidence,
        sample_count: count,
        session_count,
        mean_weight_kg: if count > 0 {
            Some(weight / count as f64)
        } else {
            None
        },
        mean_repetitions: if count > 0 {
            Some(reps as f64 / count as f64)
        } else {
            None
        },
        excluded_sets: excluded,
        unrelated_sets: unrelated,
    })
}
