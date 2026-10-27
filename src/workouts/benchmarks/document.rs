//! src/workouts/benchmarks/document.rs defines versioned model proposals and stored evidence DTOs.
//! Explicit conversion keeps JSON, schema generation and date parsing outside benchmark use cases.

use super::*;
use crate::workouts::WorkoutError;
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

/// Model-supplied estimate; source revisions come from get_benchmark_context_v1.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposalDocument {
    pub version: u32,
    pub origin: OriginDocument,
    pub key: String,
    pub source_revision: String,
    pub context_revision: String,
    pub start: String,
    pub end: String,
    /// Representative load derived from the recent observed sets, in kilograms.
    pub weight_kg: f64,
    pub repetitions: u32,
    /// Model-estimated 1RM, not a measured personal record.
    pub one_rep_max_kg: f64,
    /// Explain the formula/estimation method; Pace does not substitute its own estimate.
    pub method: String,
    /// Explain how recent per-exercise averages and any available recovery data inform this estimate.
    pub rationale: String,
    pub confidence: String,
}
/// Explicit user authorization or a narrowly scoped need for an unset benchmark.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OriginDocument {
    UserRequest {
        /// Record the user's actual request; never invent authorization.
        request: String,
    },
    MissingForWorkout {
        draft_id: u64,
        draft_revision: u64,
        reason: String,
    },
}
/// Stable exercise reference from Garmin's catalogue.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExerciseDocument {
    pub category: String,
    pub name: String,
}
/// A normalized benchmark in kilograms; unset values remain null.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkDocument {
    pub id: Option<u64>,
    pub exercise: ExerciseDocument,
    pub weight_kg: Option<f64>,
    pub repetitions: Option<u32>,
    pub one_rep_max_kg: Option<f64>,
    pub linked: Vec<ExerciseDocument>,
    pub revision: String,
    pub synced_at: String,
}
/// Observed set with explicit missing values; rest is never treated as zero lifting ability.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetDocument {
    pub position: usize,
    pub active: bool,
    pub exercise: Option<ExerciseDocument>,
    /// Garmin's category when the set has no single exact exercise; absent in older imports.
    #[serde(default)]
    pub category: Option<String>,
    pub weight_kg: Option<f64>,
    pub repetitions: Option<u32>,
}
/// Recorded strength activity, with source and import timestamps.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionDocument {
    pub activity_id: u64,
    pub date: String,
    pub started_at: String,
    pub synced_at: String,
    pub sets: Vec<SetDocument>,
}
/// One activity's imported sets. `status` is `available` with a session, `no_data` when the
/// covering import holds no session for this activity, and `not_synced` without any import.
#[derive(Clone, Debug, Serialize)]
pub struct ActivitySetsDocument {
    pub activity_id: u64,
    pub date: String,
    pub status: &'static str,
    pub import_synced_at: Option<String>,
    pub session: Option<SessionDocument>,
}
/// Full bounded evidence returned to a model and retained with its proposal.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextDocument {
    pub source: BenchmarkDocument,
    pub start: String,
    pub end: String,
    pub revision: String,
    pub synced_at: Option<String>,
    pub sessions: Vec<SessionDocument>,
    pub sample_count: usize,
    pub session_count: usize,
    pub mean_weight_kg: Option<f64>,
    pub mean_repetitions: Option<f64>,
    pub excluded_sets: usize,
    pub unrelated_sets: usize,
}
/// Complete local proposal review and write state.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftDocument {
    pub id: u64,
    pub revision: u64,
    pub proposal: ProposalDocument,
    pub context: ContextDocument,
    pub state: String,
    /// Quoted user instructions behind MCP-initiated Garmin writes, newline-separated.
    #[serde(default)]
    pub mcp_request: Option<String>,
}

impl TryFrom<ProposalDocument> for Proposal {
    type Error = WorkoutError;
    fn try_from(d: ProposalDocument) -> Result<Self, Self::Error> {
        if d.version != 1 {
            return Err(WorkoutError::InvalidInput(
                "unsupported benchmark document version",
            ));
        }
        let value = Self {
            origin: match d.origin {
                OriginDocument::UserRequest { request } => ProposalOrigin::UserRequest { request },
                OriginDocument::MissingForWorkout {
                    draft_id,
                    draft_revision,
                    reason,
                } => ProposalOrigin::MissingForWorkout {
                    draft_id,
                    draft_revision,
                    reason,
                },
            },
            key: d.key,
            source_revision: d.source_revision,
            context_revision: d.context_revision,
            window: window(&d.start, &d.end)?,
            weight_kg: d.weight_kg,
            repetitions: d.repetitions,
            one_rep_max_kg: d.one_rep_max_kg,
            method: d.method,
            rationale: d.rationale,
            confidence: d.confidence,
        };
        value.validate()?;
        Ok(value)
    }
}
impl From<Proposal> for ProposalDocument {
    fn from(d: Proposal) -> Self {
        Self {
            version: 1,
            origin: match d.origin {
                ProposalOrigin::UserRequest { request } => OriginDocument::UserRequest { request },
                ProposalOrigin::MissingForWorkout {
                    draft_id,
                    draft_revision,
                    reason,
                } => OriginDocument::MissingForWorkout {
                    draft_id,
                    draft_revision,
                    reason,
                },
            },
            key: d.key,
            source_revision: d.source_revision,
            context_revision: d.context_revision,
            start: d.window.start.to_string(),
            end: d.window.end.to_string(),
            weight_kg: d.weight_kg,
            repetitions: d.repetitions,
            one_rep_max_kg: d.one_rep_max_kg,
            method: d.method,
            rationale: d.rationale,
            confidence: d.confidence,
        }
    }
}
impl From<ExerciseKey> for ExerciseDocument {
    fn from(d: ExerciseKey) -> Self {
        Self {
            category: d.category,
            name: d.name,
        }
    }
}
impl From<ExerciseDocument> for ExerciseKey {
    fn from(d: ExerciseDocument) -> Self {
        Self {
            category: d.category,
            name: d.name,
        }
    }
}
impl From<Benchmark> for BenchmarkDocument {
    fn from(d: Benchmark) -> Self {
        Self {
            id: d.id,
            exercise: d.exercise.into(),
            weight_kg: d.weight_kg,
            repetitions: d.repetitions,
            one_rep_max_kg: d.one_rep_max_kg,
            linked: d.linked.into_iter().map(Into::into).collect(),
            revision: d.revision,
            synced_at: d.synced_at,
        }
    }
}
impl From<BenchmarkDocument> for Benchmark {
    fn from(d: BenchmarkDocument) -> Self {
        Self {
            id: d.id,
            exercise: d.exercise.into(),
            weight_kg: d.weight_kg,
            repetitions: d.repetitions,
            one_rep_max_kg: d.one_rep_max_kg,
            linked: d.linked.into_iter().map(Into::into).collect(),
            revision: d.revision,
            synced_at: d.synced_at,
        }
    }
}
impl From<StrengthSession> for SessionDocument {
    fn from(d: StrengthSession) -> Self {
        Self {
            activity_id: d.activity_id,
            date: d.date.to_string(),
            started_at: d.started_at,
            synced_at: d.synced_at,
            sets: d
                .sets
                .into_iter()
                .map(|s| SetDocument {
                    position: s.position,
                    active: s.active,
                    exercise: s.exercise.map(Into::into),
                    category: s.category,
                    weight_kg: s.weight_kg,
                    repetitions: s.repetitions,
                })
                .collect(),
        }
    }
}
impl TryFrom<SessionDocument> for StrengthSession {
    type Error = WorkoutError;
    fn try_from(d: SessionDocument) -> Result<Self, Self::Error> {
        Ok(Self {
            activity_id: d.activity_id,
            date: d
                .date
                .parse()
                .map_err(|_| WorkoutError::InvalidInput("invalid strength session date"))?,
            started_at: d.started_at,
            synced_at: d.synced_at,
            sets: d
                .sets
                .into_iter()
                .map(|s| ObservedSet {
                    position: s.position,
                    active: s.active,
                    exercise: s.exercise.map(Into::into),
                    category: s.category,
                    weight_kg: s.weight_kg,
                    repetitions: s.repetitions,
                })
                .collect(),
        })
    }
}
impl From<ActivitySets> for ActivitySetsDocument {
    fn from(d: ActivitySets) -> Self {
        Self {
            activity_id: d.activity_id,
            date: d.date.to_string(),
            status: match (&d.session, &d.import_synced_at) {
                (Some(_), _) => "available",
                (None, Some(_)) => "no_data",
                (None, None) => "not_synced",
            },
            import_synced_at: d.import_synced_at,
            session: d.session.map(Into::into),
        }
    }
}
impl From<Context> for ContextDocument {
    fn from(d: Context) -> Self {
        let e = d.evidence;
        Self {
            source: e.source.into(),
            start: e.window.start.to_string(),
            end: e.window.end.to_string(),
            revision: e.revision,
            synced_at: e.synced_at,
            sessions: e.sessions.into_iter().map(Into::into).collect(),
            sample_count: d.sample_count,
            session_count: d.session_count,
            mean_weight_kg: d.mean_weight_kg,
            mean_repetitions: d.mean_repetitions,
            excluded_sets: d.excluded_sets,
            unrelated_sets: d.unrelated_sets,
        }
    }
}
impl TryFrom<ContextDocument> for Context {
    type Error = WorkoutError;
    fn try_from(d: ContextDocument) -> Result<Self, Self::Error> {
        Ok(Self {
            evidence: Evidence {
                source: d.source.into(),
                window: window(&d.start, &d.end)?,
                revision: d.revision,
                synced_at: d.synced_at,
                sessions: d
                    .sessions
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
            },
            sample_count: d.sample_count,
            session_count: d.session_count,
            mean_weight_kg: d.mean_weight_kg,
            mean_repetitions: d.mean_repetitions,
            excluded_sets: d.excluded_sets,
            unrelated_sets: d.unrelated_sets,
        })
    }
}
impl From<BenchmarkDraft> for DraftDocument {
    fn from(d: BenchmarkDraft) -> Self {
        Self {
            id: d.id,
            revision: d.revision,
            proposal: d.proposal.into(),
            context: d.context.into(),
            state: d.state.as_str().into(),
            mcp_request: d.mcp_request,
        }
    }
}
impl TryFrom<DraftDocument> for BenchmarkDraft {
    type Error = WorkoutError;
    fn try_from(d: DraftDocument) -> Result<Self, Self::Error> {
        let context: Context = d.context.try_into()?;
        Ok(Self {
            id: d.id,
            revision: d.revision,
            proposal: d.proposal.try_into()?,
            source: context.evidence.source.clone(),
            context,
            state: d.state.parse()?,
            mcp_request: d.mcp_request,
        })
    }
}
/// Parse and validate one explicit range at a transport or storage boundary.
pub fn window(start: &str, end: &str) -> Result<Window, WorkoutError> {
    let parse = |s: &str| {
        s.parse()
            .map_err(|_| WorkoutError::InvalidInput("date must be YYYY-MM-DD"))
    };
    let w = Window {
        start: parse(start)?,
        end: parse(end)?,
    };
    w.validate()?;
    Ok(w)
}
