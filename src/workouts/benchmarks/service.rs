//! src/workouts/benchmarks/service.rs coordinates evidence imports and explicit benchmark publication.
//! Models author proposals; revision checks bind review to observed history and the Garmin source.

use super::domain::*;
use crate::{
    health::Date,
    workouts::{DraftState, WorkoutError, validate_request},
};
use std::sync::Arc;

/// Garmin adapter owns bounded reads and never retries writes.
pub trait BenchmarkGateway {
    fn catalog(&self) -> Result<Vec<Benchmark>, WorkoutError>;
    fn strength(&self, window: Window) -> Result<StrengthImport, WorkoutError>;
    /// Read one known activity's sets by ID; no activity search or catalogue read.
    fn activity_session(
        &self,
        activity_id: u64,
        date: Date,
        started_at: &str,
    ) -> Result<StrengthSession, WorkoutError>;
    /// Conflict and Rejected mean no write was applied; preserve linked exercise mappings.
    fn save(&self, source: &Benchmark, proposal: &Proposal) -> Result<(), WorkoutError>;
}
/// Profile-local storage with explicit import transactions and draft compare-and-set updates.
pub trait BenchmarkRepository: Send + Sync {
    fn catalog(&self) -> Result<Vec<Benchmark>, WorkoutError>;
    fn replace_catalog(&self, rows: &[Benchmark]) -> Result<(), WorkoutError>;
    fn import(&self, catalog: &[Benchmark], import: &StrengthImport) -> Result<(), WorkoutError>;
    /// Replace one activity's session without recording window coverage for its date.
    fn import_session(&self, session: &StrengthSession) -> Result<(), WorkoutError>;
    fn evidence(&self, key: &str, window: Window) -> Result<Evidence, WorkoutError>;
    /// Read one activity's imported sets with the import that covered its UTC date.
    fn activity_sets(&self, activity_id: u64, date: Date) -> Result<ActivitySets, WorkoutError>;
    fn workout_draft(&self, id: u64) -> Result<crate::workouts::Draft, WorkoutError>;
    fn drafts(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<BenchmarkDraftSummary>, WorkoutError>;
    fn draft(&self, id: u64) -> Result<BenchmarkDraft, WorkoutError>;
    fn insert(
        &self,
        proposal: &Proposal,
        context: &Context,
    ) -> Result<BenchmarkDraft, WorkoutError>;
    fn revise(
        &self,
        id: u64,
        revision: u64,
        proposal: &Proposal,
        context: &Context,
    ) -> Result<BenchmarkDraft, WorkoutError>;
    fn transition(
        &self,
        id: u64,
        revision: u64,
        from: DraftState,
        to: DraftState,
    ) -> Result<(), WorkoutError>;
    /// Append a quoted user instruction to the proposal's journal before a Garmin call.
    fn record_request(&self, id: u64, revision: u64, request: &str) -> Result<(), WorkoutError>;
}
/// Shared local boundary used by CLI and authenticated MCP.
#[derive(Clone)]
pub struct BenchmarkService {
    repository: Arc<dyn BenchmarkRepository>,
}
impl BenchmarkService {
    /// Bind one profile without authenticating to Garmin.
    pub fn new(repository: Arc<dyn BenchmarkRepository>) -> Self {
        Self { repository }
    }
    /// Read the known catalogue, including unset benchmarks.
    pub fn catalog(&self) -> Result<Vec<Benchmark>, WorkoutError> {
        self.repository.catalog()
    }
    /// Compute observed averages for one exact exercise and explicit evidence window.
    pub fn context(&self, key: &str, window: Window) -> Result<Context, WorkoutError> {
        window.validate()?;
        if !valid_key(key) {
            return Err(WorkoutError::InvalidInput(
                "select a known Garmin benchmark key",
            ));
        }
        summarize(self.repository.evidence(key, window)?)
    }
    /// Read the sets recorded in one strength activity, as imported for its UTC date.
    pub fn activity_sets(
        &self,
        activity_id: u64,
        date: Date,
    ) -> Result<ActivitySets, WorkoutError> {
        if activity_id == 0 {
            return Err(WorkoutError::InvalidInput("activity ID must be positive"));
        }
        self.repository.activity_sets(activity_id, date)
    }
    /// Import the sets of one strength activity already stored locally, by its Garmin ID.
    pub fn import_activity(
        &self,
        gateway: &dyn BenchmarkGateway,
        activity_id: u64,
        date: Date,
        started_at: &str,
    ) -> Result<ActivitySets, WorkoutError> {
        if activity_id == 0 {
            return Err(WorkoutError::InvalidInput("activity ID must be positive"));
        }
        let session = gateway.activity_session(activity_id, date, started_at)?;
        if session.activity_id != activity_id || session.date != date || session.sets.len() > 500 {
            return Err(WorkoutError::InvalidInput(
                "activity sets exceeded their requested bounds",
            ));
        }
        self.repository.import_session(&session)?;
        self.repository.activity_sets(activity_id, date)
    }
    /// Import a complete bounded strength window before publishing any of its sets.
    pub fn sync(
        &self,
        gateway: &dyn BenchmarkGateway,
        window: Window,
    ) -> Result<usize, WorkoutError> {
        window.validate()?;
        let rows = gateway.catalog()?;
        if rows.len() > 128 {
            return Err(WorkoutError::InvalidInput(
                "benchmark catalogue exceeds 128 entries",
            ));
        }
        let import = gateway.strength(window)?;
        if import.window != window
            || import.sessions.len() > 100
            || import.sessions.iter().any(|s| s.sets.len() > 500)
        {
            return Err(WorkoutError::InvalidInput(
                "strength import exceeded its requested bounds",
            ));
        }
        self.repository.import(&rows, &import)?;
        Ok(import.sessions.len())
    }
    fn validate_proposal(&self, proposal: &Proposal) -> Result<Context, WorkoutError> {
        proposal.validate()?;
        let context = self.context(&proposal.key, proposal.window)?;
        if let ProposalOrigin::MissingForWorkout {
            draft_id,
            draft_revision,
            ..
        } = proposal.origin
        {
            if context.evidence.source.id.is_some() {
                return Err(WorkoutError::InvalidInput(
                    "filled benchmarks require an explicit user request",
                ));
            }
            let draft = self.repository.workout_draft(draft_id)?;
            let exercise = &context.evidence.source.exercise;
            if draft.state != DraftState::Draft || draft.revision != draft_revision {
                return Err(WorkoutError::Conflict);
            }
            if !draft
                .workout
                .segments
                .iter()
                .any(|s| contains_exercise(&s.steps, exercise))
            {
                return Err(WorkoutError::InvalidInput(
                    "the referenced workout draft does not use this benchmark exercise",
                ));
            }
        }

        if context.evidence.source.revision != proposal.source_revision
            || context.evidence.revision != proposal.context_revision
        {
            return Err(WorkoutError::Conflict);
        }
        if context.evidence.synced_at.is_none() || context.sample_count == 0 {
            return Err(WorkoutError::InvalidInput(
                "insufficient observed loaded sets for this benchmark; sync recent strength history first",
            ));
        }
        Ok(context)
    }
    /// Persist a model estimate only when its referenced evidence still matches.
    pub fn propose(&self, proposal: Proposal) -> Result<BenchmarkDraft, WorkoutError> {
        let context = self.validate_proposal(&proposal)?;
        self.repository.insert(&proposal, &context)
    }
    /// Correct an unsaved estimate and bind it to the model's newly reviewed evidence.
    pub fn revise(
        &self,
        id: u64,
        revision: u64,
        proposal: Proposal,
    ) -> Result<BenchmarkDraft, WorkoutError> {
        let old = self.repository.draft(id)?;
        if old.proposal.key != proposal.key {
            return Err(WorkoutError::InvalidInput(
                "a draft cannot change its benchmark key",
            ));
        }
        let context = self.validate_proposal(&proposal)?;
        self.repository.revise(id, revision, &proposal, &context)
    }
    /// Read bounded local proposals and unresolved saves.
    pub fn drafts(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<BenchmarkDraftSummary>, WorkoutError> {
        if !(1..=100).contains(&limit) || offset > 100000 {
            return Err(WorkoutError::InvalidInput("invalid draft page"));
        }
        self.repository.drafts(limit, offset)
    }
    /// Read the complete reviewable proposal.
    pub fn draft(&self, id: u64) -> Result<BenchmarkDraft, WorkoutError> {
        self.repository.draft(id)
    }
    /// Discard only the reviewed unsaved version; do not clear Garmin benchmarks.
    pub fn discard(&self, id: u64, revision: u64) -> Result<(), WorkoutError> {
        self.repository
            .transition(id, revision, DraftState::Draft, DraftState::Discarded)
    }
    /// Give up on an unresolved save after the user inspected Garmin; nothing is retried.
    pub fn abandon(&self, id: u64, revision: u64) -> Result<(), WorkoutError> {
        let draft = self.repository.draft(id)?;
        if !matches!(draft.state, DraftState::Saving | DraftState::Saved) {
            return Err(WorkoutError::Conflict);
        }
        self.repository
            .transition(id, revision, draft.state, DraftState::Discarded)
    }
    /// Save one reviewed estimate once, with source and evidence conflict protection.
    pub fn save(
        &self,
        gateway: &dyn BenchmarkGateway,
        id: u64,
        revision: u64,
        request: Option<&str>,
    ) -> Result<BenchmarkDraft, WorkoutError> {
        let draft = self.repository.draft(id)?;
        if draft.state != DraftState::Draft || draft.revision != revision {
            return Err(WorkoutError::Conflict);
        }
        self.validate_proposal(&draft.proposal)?;
        if let Some(text) = request {
            validate_request(text)?;
            self.repository.record_request(id, revision, text)?;
        }
        self.repository
            .transition(id, revision, DraftState::Draft, DraftState::Saving)?;
        if let Err(error) = gateway.save(&draft.source, &draft.proposal) {
            if matches!(error, WorkoutError::Conflict | WorkoutError::Rejected(_)) {
                // Garmin answered without applying the write; the proposal stays editable.
                self.repository
                    .transition(id, revision, DraftState::Saving, DraftState::Draft)?;
            }
            return Err(error);
        }
        self.repository
            .transition(id, revision, DraftState::Saving, DraftState::Saved)?;
        self.reconcile(gateway, id, None)
    }
    /// Resolve a pending write only after Garmin reports the proposed values and links.
    pub fn reconcile(
        &self,
        gateway: &dyn BenchmarkGateway,
        id: u64,
        request: Option<&str>,
    ) -> Result<BenchmarkDraft, WorkoutError> {
        let draft = self.repository.draft(id)?;
        if !matches!(draft.state, DraftState::Saving | DraftState::Saved) {
            return Err(WorkoutError::Conflict);
        }
        if let Some(text) = request {
            validate_request(text)?;
            self.repository.record_request(id, draft.revision, text)?;
        }
        let rows = gateway.catalog()?;
        let saved = rows
            .iter()
            .find(|b| b.exercise == draft.source.exercise)
            .ok_or(WorkoutError::NotFound)?;
        let mut saved_links: Vec<_> = saved.linked.iter().collect();
        let mut source_links: Vec<_> = draft.source.linked.iter().collect();
        saved_links.sort_unstable();
        source_links.sort_unstable();
        let matches = saved.id.is_some()
            && draft.source.id.is_none_or(|id| saved.id == Some(id))
            && saved_links == source_links
            && saved.repetitions == Some(draft.proposal.repetitions)
            && saved
                .weight_kg
                .is_some_and(|w| (w - draft.proposal.weight_kg).abs() < 0.001)
            && saved
                .one_rep_max_kg
                .is_some_and(|w| (w - draft.proposal.one_rep_max_kg).abs() < 0.051);
        if !matches {
            return Err(WorkoutError::Uncertain);
        }
        self.repository.replace_catalog(&rows)?;
        self.repository
            .transition(id, draft.revision, draft.state, DraftState::Verified)?;
        self.repository.draft(id)
    }
}

fn contains_exercise(steps: &[crate::workouts::Step], key: &ExerciseKey) -> bool {
    steps.iter().any(|step| match step {
        crate::workouts::Step::Exercise {
            exercise: Some(e), ..
        } => e.category == key.category && e.name == key.name,
        crate::workouts::Step::Repeat { steps, .. } => contains_exercise(steps, key),
        _ => false,
    })
}
