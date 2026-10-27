//! src/workouts/service.rs coordinates library reads, model drafts and explicit remote writes.
//! Revision checks and durable pending states protect reviewed content and ambiguous outcomes.

use super::{
    Delivery, Device, Draft, DraftState, LibraryWorkout, Workout, WorkoutError, WorkoutSummary,
    validate_request,
};
use std::sync::Arc;

/// External Garmin capabilities; writes are never retried by this port's adapter.
pub trait WorkoutGateway {
    fn devices(&self) -> Result<Vec<Device>, WorkoutError>;
    fn list(&self, start: usize, limit: usize) -> Result<Vec<u64>, WorkoutError>;
    fn read(&self, id: u64) -> Result<LibraryWorkout, WorkoutError>;
    fn create(&self, workout: &Workout) -> Result<u64, WorkoutError>;
    /// Conflict and Rejected mean Garmin did not apply the write; other failures may be ambiguous.
    fn update(&self, id: u64, revision: &str, workout: &Workout) -> Result<(), WorkoutError>;
    fn queue(&self, id: u64, name: &str, device: u64) -> Result<(), WorkoutError>;
}

/// Profile-local storage; compare-and-set operations include the reviewed draft revision.
pub trait WorkoutRepository: Send + Sync + super::coach::Repository {
    fn list(
        &self,
        limit: usize,
        offset: usize,
        standalone: bool,
    ) -> Result<Vec<WorkoutSummary>, WorkoutError>;
    fn get(&self, id: u64) -> Result<LibraryWorkout, WorkoutError>;
    fn put(&self, workout: &LibraryWorkout) -> Result<(), WorkoutError>;
    fn drafts(&self, limit: usize, offset: usize) -> Result<Vec<Draft>, WorkoutError>;
    /// Count all actionable records and select the next one without loading definitions.
    fn attention(&self) -> Result<super::DraftAttention, WorkoutError>;
    fn draft(&self, id: u64) -> Result<Draft, WorkoutError>;
    fn insert(
        &self,
        workout: &Workout,
        source: Option<&LibraryWorkout>,
        explanation: Option<&str>,
    ) -> Result<Draft, WorkoutError>;
    fn revise(
        &self,
        id: u64,
        revision: u64,
        workout: &Workout,
        explanation: Option<&str>,
    ) -> Result<Draft, WorkoutError>;
    fn transition(
        &self,
        id: u64,
        revision: u64,
        from: DraftState,
        to: DraftState,
        remote_id: Option<u64>,
    ) -> Result<(), WorkoutError>;
    /// Append a quoted user instruction to the draft's journal before a Garmin call.
    fn record_request(&self, id: u64, revision: u64, request: &str) -> Result<(), WorkoutError>;
    fn reserve_delivery(&self, delivery: &Delivery) -> Result<(), WorkoutError>;
    fn complete_delivery(&self, delivery: &Delivery) -> Result<(), WorkoutError>;
    /// Remove an unresolved `sending` reservation; queued deliveries stay recorded.
    fn release_delivery(&self, workout_id: u64, device_id: u64) -> Result<(), WorkoutError>;
    /// Read durable delivery reservations, including accepted requests, by workout and device.
    fn deliveries(
        &self,
        workout_id: u64,
        device_id: Option<u64>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Delivery>, WorkoutError>;
    /// Search exact exercise identities from the imported editable library and benchmarks.
    fn exercises(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<super::benchmarks::ExerciseKey>, WorkoutError>;
}

/// Cloneable local service usable by authenticated MCP without a Garmin session.
#[derive(Clone)]
pub struct WorkoutService {
    pub(super) repository: Arc<dyn WorkoutRepository>,
}
impl WorkoutService {
    /// Bind the service to one profile repository without performing network work.
    pub fn new(repository: Arc<dyn WorkoutRepository>) -> Self {
        Self { repository }
    }
    /// Validate library page bounds before storage or an external adapter is reached.
    pub fn page(limit: usize, offset: usize) -> Result<(), WorkoutError> {
        if !(1..=100).contains(&limit) || offset > 100_000 {
            return Err(WorkoutError::InvalidInput(
                "page limit must be 1..=100 and offset at most 100000",
            ));
        }
        Ok(())
    }
    /// Read a bounded local library page in name order.
    pub fn list(&self, limit: usize, offset: usize) -> Result<Vec<WorkoutSummary>, WorkoutError> {
        Self::page(limit, offset)?;
        self.repository.list(limit, offset, false)
    }
    /// Read standalone library entries, excluding Coach-owned workout definitions.
    pub fn standalone(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<WorkoutSummary>, WorkoutError> {
        Self::page(limit, offset)?;
        self.repository.list(limit, offset, true)
    }
    /// Read one locally synchronized workout and its opaque source revision.
    pub fn get(&self, id: u64) -> Result<LibraryWorkout, WorkoutError> {
        self.repository.get(id)
    }
    /// Read a bounded page of active draft and save records.
    pub fn drafts(&self, limit: usize, offset: usize) -> Result<Vec<Draft>, WorkoutError> {
        Self::page(limit, offset)?;
        self.repository.drafts(limit, offset)
    }
    /// Prioritize unresolved saves over unsaved drafts, regardless of list pagination.
    pub fn attention(&self) -> Result<super::DraftAttention, WorkoutError> {
        self.repository.attention()
    }
    /// Read a draft with its exact revision and immutable source snapshot.
    pub fn draft(&self, id: u64) -> Result<Draft, WorkoutError> {
        self.repository.draft(id)
    }
    /// Validate a new model candidate and optionally attach the current library source.
    pub fn propose(
        &self,
        workout: Workout,
        source: Option<(u64, String)>,
    ) -> Result<Draft, WorkoutError> {
        self.propose_explained(workout, source, None)
    }
    /// Attach model evidence without sending its explanation to Garmin as workout text.
    pub fn propose_explained(
        &self,
        workout: Workout,
        source: Option<(u64, String)>,
        explanation: Option<&str>,
    ) -> Result<Draft, WorkoutError> {
        workout.validate()?;
        let source = source
            .map(|(id, revision)| {
                let source = self.repository.get(id)?;
                if source.revision != revision {
                    return Err(WorkoutError::Conflict);
                }
                if source.definition.is_none() {
                    return Err(WorkoutError::InvalidInput(
                        "this remote workout has unsupported features and is read-only",
                    ));
                }
                Ok(source)
            })
            .transpose()?;
        validate_explanation(source.as_ref(), explanation)?;
        self.repository
            .insert(&workout, source.as_ref(), explanation)
    }
    /// Replace an unsaved candidate only if the model read its current revision.
    pub fn revise(&self, id: u64, revision: u64, workout: Workout) -> Result<Draft, WorkoutError> {
        self.revise_explained(id, revision, workout, None)
    }
    /// Each revised Coach proposal needs a fresh explanation matching its new content.
    pub fn revise_explained(
        &self,
        id: u64,
        revision: u64,
        workout: Workout,
        explanation: Option<&str>,
    ) -> Result<Draft, WorkoutError> {
        workout.validate()?;
        let draft = self.repository.draft(id)?;
        validate_explanation(draft.source.as_ref(), explanation)?;
        self.repository.revise(id, revision, &workout, explanation)
    }
    /// Discard the reviewed local revision without altering Garmin.
    pub fn discard(&self, id: u64, revision: u64) -> Result<(), WorkoutError> {
        self.repository
            .transition(id, revision, DraftState::Draft, DraftState::Discarded, None)
    }
    /// Give up on an unresolved save after the user inspected Garmin; nothing is retried.
    pub fn abandon(&self, id: u64, revision: u64) -> Result<(), WorkoutError> {
        let draft = self.repository.draft(id)?;
        if !matches!(draft.state, DraftState::Saving | DraftState::Saved) {
            return Err(WorkoutError::Conflict);
        }
        self.repository.transition(
            id,
            revision,
            draft.state,
            DraftState::Discarded,
            draft.remote_id,
        )
    }
    /// Each fetched workout is committed explicitly; a later failure leaves earlier snapshots valid.
    pub fn sync(
        &self,
        gateway: &dyn WorkoutGateway,
        start: usize,
        limit: usize,
    ) -> Result<usize, WorkoutError> {
        Self::page(limit, start)?;
        let ids = gateway.list(start, limit)?;
        if ids.len() > limit {
            return Err(WorkoutError::InvalidInput(
                "Garmin exceeded the requested page size",
            ));
        }
        for id in &ids {
            self.repository.put(&gateway.read(*id)?)?;
        }
        Ok(ids.len())
    }
    /// Save exactly the reviewed draft revision and verify the resulting remote content.
    /// A model-initiated save journals the user's quoted instruction before any network call.
    pub fn save(
        &self,
        gateway: &dyn WorkoutGateway,
        id: u64,
        revision: u64,
        request: Option<&str>,
    ) -> Result<Draft, WorkoutError> {
        let draft = self.repository.draft(id)?;
        if draft.revision != revision || draft.state != DraftState::Draft {
            return Err(WorkoutError::Conflict);
        }
        if let Some(text) = request {
            validate_request(text)?;
            self.repository.record_request(id, revision, text)?;
        }
        let remote_id = draft.source.as_ref().map(|s| s.id);
        self.repository.transition(
            id,
            revision,
            DraftState::Draft,
            DraftState::Saving,
            remote_id,
        )?;
        // The gateway re-reads the source and reports Conflict before sending any write.
        let written = match &draft.source {
            Some(source) => gateway
                .update(source.id, &source.revision, &draft.workout)
                .map(|()| source.id),
            None => gateway.create(&draft.workout),
        };
        let remote_id = match written {
            Ok(remote_id) => remote_id,
            Err(error) => {
                if matches!(error, WorkoutError::Conflict | WorkoutError::Rejected(_)) {
                    // Garmin answered without applying the write; the draft stays editable.
                    self.repository.transition(
                        id,
                        revision,
                        DraftState::Saving,
                        DraftState::Draft,
                        None,
                    )?;
                }
                return Err(error);
            }
        };
        self.repository.transition(
            id,
            revision,
            DraftState::Saving,
            DraftState::Saved,
            Some(remote_id),
        )?;
        self.reconcile(gateway, id, remote_id, None)
    }
    /// Explicitly resolve a pending write only when its full definition matches Garmin.
    pub fn reconcile(
        &self,
        gateway: &dyn WorkoutGateway,
        id: u64,
        remote_id: u64,
        request: Option<&str>,
    ) -> Result<Draft, WorkoutError> {
        let draft = self.repository.draft(id)?;
        if !matches!(draft.state, DraftState::Saving | DraftState::Saved) {
            return Err(WorkoutError::Conflict);
        }
        if draft.remote_id.is_some_and(|id| id != remote_id) {
            return Err(WorkoutError::Conflict);
        }
        if let Some(text) = request {
            validate_request(text)?;
            self.repository.record_request(id, draft.revision, text)?;
        }
        let saved = gateway.read(remote_id)?;
        if draft
            .source
            .as_ref()
            .is_some_and(|s| s.coach_plan_id != saved.coach_plan_id)
        {
            return Err(WorkoutError::Uncertain);
        }
        if !saved
            .definition
            .as_ref()
            .is_some_and(|value| value.equivalent(&draft.workout))
        {
            return Err(WorkoutError::Uncertain);
        }
        self.repository.put(&saved)?;
        self.repository.transition(
            id,
            draft.revision,
            draft.state,
            DraftState::Verified,
            Some(remote_id),
        )?;
        self.repository.draft(id)
    }
    /// Deliver a selected, unchanged library snapshot once per device and remote revision.
    pub fn send(
        &self,
        gateway: &dyn WorkoutGateway,
        id: u64,
        device_id: u64,
        request: Option<&str>,
    ) -> Result<Delivery, WorkoutError> {
        if let Some(text) = request {
            validate_request(text)?;
        }
        let local = self.repository.get(id)?;
        let remote = gateway.read(id)?;
        if local.revision != remote.revision {
            return Err(WorkoutError::Conflict);
        }
        if !gateway.devices()?.iter().any(|d| d.id == device_id) {
            return Err(WorkoutError::InvalidInput(
                "select a registered Garmin device",
            ));
        }
        let mut delivery = Delivery {
            workout_id: id,
            device_id,
            revision: remote.revision,
            queued: false,
            request: request.map(str::to_owned),
        };
        self.repository
            .reserve_delivery(&delivery)
            .map_err(|error| match error {
                WorkoutError::Conflict => WorkoutError::InvalidInput(
                    "this revision was already sent or has an unresolved send to that device; review unresolved requests or sync a newer revision",
                ),
                other => other,
            })?;
        if let Err(error) = gateway.queue(id, &remote.name, device_id) {
            if matches!(error, WorkoutError::Rejected(_)) {
                // Garmin refused the request outright, so no delivery is pending.
                self.repository.release_delivery(id, device_id)?;
            }
            return Err(error);
        }
        self.repository.complete_delivery(&delivery)?;
        delivery.queued = true;
        Ok(delivery)
    }
    /// Drop an unresolved delivery reservation after the user checked the watch and Garmin.
    pub fn clear_delivery(&self, id: u64, device_id: u64) -> Result<(), WorkoutError> {
        self.repository.release_delivery(id, device_id)
    }

    /// Read recorded requests only; queued never establishes physical watch receipt.
    pub fn deliveries(
        &self,
        id: u64,
        device_id: u64,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Delivery>, WorkoutError> {
        Self::page(limit, offset)?;
        if id == 0 || device_id == 0 {
            return Err(WorkoutError::InvalidInput(
                "workout and device IDs must be positive",
            ));
        }
        self.repository
            .deliveries(id, Some(device_id), limit, offset)
    }

    /// Read all recorded devices and revisions for one workout, without a Garmin call.
    pub fn delivery_history(
        &self,
        id: u64,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Delivery>, WorkoutError> {
        Self::page(limit, offset)?;
        if id == 0 {
            return Err(WorkoutError::InvalidInput("workout ID must be positive"));
        }
        self.repository.deliveries(id, None, limit, offset)
    }

    /// Search known exact exercise identities without inventing unknown Garmin identifiers.
    pub fn exercises(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<super::benchmarks::ExerciseKey>, WorkoutError> {
        Self::page(limit, offset)?;
        if query.len() > 100 || query.chars().any(char::is_control) {
            return Err(WorkoutError::InvalidInput(
                "exercise query must be at most 100 bytes without control characters",
            ));
        }
        self.repository.exercises(query, limit, offset)
    }

    /// Refresh one explicit remote identity for editing or uncertain-create reconciliation.
    pub fn refresh(
        &self,
        gateway: &dyn WorkoutGateway,
        id: u64,
    ) -> Result<LibraryWorkout, WorkoutError> {
        if id == 0 {
            return Err(WorkoutError::InvalidInput("workout ID must be positive"));
        }
        let workout = gateway.read(id)?;
        self.repository.put(&workout)?;
        Ok(workout)
    }
}

fn validate_explanation(
    source: Option<&LibraryWorkout>,
    explanation: Option<&str>,
) -> Result<(), WorkoutError> {
    if source.is_some_and(|s| s.coach_plan_id.is_some()) && explanation.is_none() {
        return Err(WorkoutError::InvalidInput(
            "Coach drafts require an explanation of evidence, uncertainty and proposed changes",
        ));
    }
    if let Some(text) = explanation
        && (text.trim().is_empty() || text.len() > 4000 || text.chars().any(char::is_control))
    {
        return Err(WorkoutError::InvalidInput(
            "explanation must be 1..=4000 bytes without control characters",
        ));
    }
    Ok(())
}
