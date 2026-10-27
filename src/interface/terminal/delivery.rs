//! src/interface/terminal/delivery.rs connects verified saves to watch selection and
//! displays paged local delivery evidence. Only explicit send or clear actions mutate state.

use super::{
    App, Choice, Selection,
    workouts::{PAGE_SIZE, paging},
};
use crate::{
    config::Settings,
    interface::{
        workout_view,
        workouts::{self, WorkoutCommand},
    },
    workouts::{DraftState, LibraryWorkout},
};
use anyhow::{Context, Result};

impl App {
    pub(super) fn after_workout_save(&mut self, draft_id: u64) -> Result<()> {
        let service = workouts::service(&Settings::load(&self.config)?)?;
        let draft = service.draft(draft_id)?;
        if draft.state != DraftState::Verified {
            return Ok(());
        }
        let id = draft.remote_id.context("verified draft has no Garmin ID")?;
        let choices = [
            Choice::new("Back to drafts", "Workout saved; send later if needed"),
            Choice::new(
                "Send to watch",
                "Choose a device and confirm delivery of the saved workout",
            ),
        ];
        let mut state = Selection::default();
        loop {
            match self
                .view
                .select("Workout saved", &choices, &mut state, false)?
            {
                Some(1) => {
                    let result = self.send_to_watch(&service.get(id)?);
                    if self.recover("Send to watch", result)?.is_some() {
                        return Ok(());
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    pub(super) fn delivery_status(&mut self, workout: &LibraryWorkout) -> Result<()> {
        let service = workouts::service(&Settings::load(&self.config)?)?;
        let mut offset = 0;
        let mut state = Selection::default();
        loop {
            let records = service.delivery_history(workout.id, PAGE_SIZE, offset)?;
            let current = service.get(workout.id)?;
            let mut choices = vec![Choice::new(
                "Refresh status",
                "Read local journal; watch receipt remains unconfirmed",
            )];
            choices.extend(records.iter().map(|d| {
                Choice::new(
                    format!(
                        "Device {}: {}",
                        d.device_id,
                        workout_view::delivery_label(d.queued)
                    ),
                    format!(
                        "{}; revision {}",
                        if d.revision == current.revision {
                            "Current version"
                        } else {
                            "Older version"
                        },
                        d.revision
                    ),
                )
            }));
            let (next, previous) = paging(&mut choices, records.len(), offset, "Delivery records");
            let title = if records.is_empty() && offset == 0 {
                "Delivery status: no recorded request"
            } else {
                "Delivery status"
            };
            match self.view.select(title, &choices, &mut state, false)? {
                None => return Ok(()),
                Some(0) => {}
                Some(index) if index == next => {
                    offset += PAGE_SIZE;
                    state = Selection::default();
                }
                Some(index) if index == previous => {
                    offset = offset.saturating_sub(PAGE_SIZE);
                    state = Selection::default();
                }
                Some(index) => {
                    let d = &records[index - 1];
                    let text = format!(
                        "Workout: {}\nDevice ID: {}\nVersion: {}\nRevision: {}\nState: {}\nWatch receipt: unconfirmed.\n{}",
                        workout.id,
                        d.device_id,
                        if d.revision == current.revision {
                            "current"
                        } else {
                            "older"
                        },
                        d.revision,
                        if d.queued {
                            "Sent to watch. Garmin accepted the send request"
                        } else {
                            "Sending or outcome unknown; do not resend"
                        },
                        super::super::workout_view::mcp_requests(d.request.as_deref())
                    );
                    self.view.result("Delivery record", &text)?;
                    if !d.queued {
                        let options = [
                            Choice::new("Back", "Keep the unresolved reservation"),
                            Choice::new(
                                "Clear unresolved delivery",
                                "Only after checking Garmin and the watch; does not resend",
                            ),
                        ];
                        if self.view.select("Unresolved delivery", &options, &mut Selection::default(), false)? == Some(1)
                            && self.confirm("Clear unresolved delivery", "I checked Garmin and the watch; clear unresolved requests for this workout and device?", &text)? {
                            self.workout_command("Clear delivery", WorkoutCommand::ClearDelivery { id: workout.id, device_id: d.device_id })?;
                        }
                    }
                }
            }
        }
    }

    pub(super) fn workout_id_input(&mut self, draft_id: u64) -> Result<Option<u64>> {
        let title = format!("Reconcile draft #{draft_id}");
        loop {
            let Some(text) = self
                .view
                .prompt(&title, "Existing Garmin workout ID", false)?
            else {
                return Ok(None);
            };
            let parsed = text
                .trim()
                .parse::<u64>()
                .context("enter a positive Garmin workout ID")
                .and_then(|id| {
                    anyhow::ensure!(id > 0, "workout ID must be positive");
                    Ok(id)
                });
            if let Some(id) = self.recover("Reconcile save", parsed)? {
                return Ok(Some(id));
            }
        }
    }
}
