//! src/interface/terminal/workouts.rs shows the workout library and drafts as tables and
//! opens each workout as a table of its steps with its actions. Models author through MCP;
//! explicit sync, save and device menus invoke the public workout service.

use super::{
    App, Choice, Selection, Style, capture,
    view::{Column, Width},
};
use crate::{
    config::Settings,
    garmin::ConnectApi,
    interface::{
        cli::Command,
        workout_view::{self, StepRow},
        workouts::{self, WorkoutCommand},
    },
    workouts::{Draft, DraftState, LibraryWorkout, Workout},
};
use anyhow::Result;

pub(super) const PAGE_SIZE: usize = 20;
const LIBRARY: &[Column] = &[
    Column::new("Workout", Width::Fill),
    Column::new("State", Width::Fit(9)),
    Column::new("Sport", Width::Fit(17)),
    Column::new("Synced", Width::Fit(10)),
    Column::new("Garmin ID", Width::Fit(12)),
];
const DRAFTS: &[Column] = &[
    Column::new("#", Width::Fit(6)),
    Column::new("Workout", Width::Fill),
    Column::new("Change", Width::Fit(6)),
    Column::new("Rev", Width::Fit(4)),
    Column::new("State", Width::Fit(9)),
];
const STEPS: &[Column] = &[
    Column::new("#", Width::Fit(5)),
    Column::new("Step", Width::Fit(10)),
    Column::new("Exercise", Width::Fill),
    Column::new("Duration", Width::Fit(10)),
    Column::new("Target", Width::Fit(18)),
    Column::new("Note", Width::Fit(40)),
];

fn choice(label: impl Into<String>, description: impl Into<String>) -> Choice {
    Choice::new(label, description)
}
/// Date part of a stored RFC 3339 timestamp, for narrow table cells.
pub(super) fn day(timestamp: &str) -> String {
    timestamp.chars().take(10).collect()
}
fn step_choices(workout: &Workout) -> (Vec<StepRow>, Vec<Choice>) {
    let rows = workout_view::step_rows(workout);
    let choices = rows
        .iter()
        .map(|row| {
            Choice::row(
                vec![
                    row.number.clone(),
                    row.kind.clone(),
                    row.exercise.clone(),
                    row.duration.clone(),
                    row.target.clone(),
                    row.note.clone(),
                ],
                "Open this step",
            )
        })
        .collect();
    (rows, choices)
}
/// Page actions appended after table rows: only the directions that exist.
pub(super) fn paging(
    choices: &mut Vec<Choice>,
    count: usize,
    offset: usize,
    what: &str,
) -> (usize, usize) {
    let next = if count == PAGE_SIZE {
        choices.push(choice(
            "Next page",
            format!("{what} from {}", offset + PAGE_SIZE + 1),
        ));
        choices.len() - 1
    } else {
        usize::MAX
    };
    let previous = if offset > 0 {
        choices.push(choice(
            "Previous page",
            format!("{what} before {}", offset + 1),
        ));
        choices.len() - 1
    } else {
        usize::MAX
    };
    (next, previous)
}

impl App {
    pub(super) fn workout_library(&mut self) -> Result<()> {
        let settings = Settings::load(&self.config)?;
        let service = workouts::service(&settings)?;
        let mut offset = 0;
        // Garmin pages are imported in order; the next sync continues where the last ended.
        let mut synced = 0;
        let mut state = Selection::default();
        loop {
            let rows = service.standalone(PAGE_SIZE, offset)?;
            let mut choices = vec![
                choice(
                    if synced == 0 {
                        "Sync from Garmin"
                    } else {
                        "Sync next page"
                    },
                    format!(
                        "Import Garmin library workouts {}-{} into this list",
                        synced + 1,
                        synced + PAGE_SIZE
                    ),
                ),
                choice(
                    "New with model",
                    "Create a workout with your connected MCP model",
                ),
            ];
            let first = choices.len();
            choices.extend(rows.iter().map(|row| {
                Choice::row(
                    vec![
                        row.name.clone(),
                        if row.editable {
                            "editable"
                        } else {
                            "read-only"
                        }
                        .into(),
                        row.sport.clone(),
                        day(&row.synced_at),
                        row.id.to_string(),
                    ],
                    if row.editable {
                        "Open steps, model editing and watch delivery"
                    } else {
                        "Open steps and watch delivery; edit the definition in Garmin Connect"
                    },
                )
            }));
            let (next, previous) = paging(&mut choices, rows.len(), offset, "Local workouts");
            let title = match (rows.is_empty(), offset) {
                (true, 0) => "Workouts: no standalone entries",
                (true, _) => "Workouts: no more entries",
                (false, _) => "Workouts",
            };
            match self
                .view
                .table(title, LIBRARY, &choices, &mut state, true)?
            {
                None => return Ok(()),
                Some(0) => {
                    let result = self.sync_workouts(synced);
                    let Some(received) = self.recover("Sync from Garmin", result)? else {
                        continue;
                    };
                    synced = if received < PAGE_SIZE {
                        0
                    } else {
                        synced + received
                    };
                }
                Some(1) => self.model_help(None)?,
                Some(index) if index == next => {
                    offset += PAGE_SIZE;
                    state = Selection::default();
                }
                Some(index) if index == previous => {
                    offset = offset.saturating_sub(PAGE_SIZE);
                    state = Selection::default();
                }
                Some(index) => {
                    let result = self.library_entry(service.get(rows[index - first].id)?);
                    self.recover("Workout", result)?;
                }
            }
        }
    }
    /// Import one Garmin page starting at `start`; returns how many workouts Garmin sent.
    fn sync_workouts(&mut self, start: usize) -> Result<usize> {
        self.view.frame(
            "Sync from Garmin",
            &[("Reading the Garmin workout library...".into(), Style::Plain)],
            "Waiting for Garmin",
        )?;
        let settings = Settings::load(&self.config)?;
        let service = workouts::service(&settings)?;
        let _reporting = self.report_rate_waits()?;
        let api = ConnectApi::connect(&settings.garmin, &settings.garmin_auth)?;
        let received = service.sync(&api, start, PAGE_SIZE)?;
        rustix::termios::tcflush(std::io::stdin(), rustix::termios::QueueSelector::IFlush)?;
        let text = if received == 0 {
            format!(
                "Garmin returned no workouts from position {}.\nThe library list below is complete for what Garmin has.",
                start + 1
            )
        } else {
            format!(
                "Imported {received} workouts (Garmin positions {}-{}).\n{}",
                start + 1,
                start + received,
                if received == PAGE_SIZE {
                    "Choose Sync next page for more."
                } else {
                    "That was the last page."
                }
            )
        };
        self.view.result("Sync from Garmin", &text)?;
        Ok(received)
    }
    /// One workout: its actions, then every step as a table row.
    pub(super) fn library_entry(&mut self, row: LibraryWorkout) -> Result<()> {
        let mut choices = vec![choice(
            "Full text",
            "Complete definition with units, sync time and read-only notes",
        )];
        let edit = row.definition.is_some().then(|| {
            choices.push(choice(
                "Edit with model",
                "Ask your model to prepare changes to this Garmin ID",
            ));
            choices.len() - 1
        });
        choices.push(choice(
            "Send to watch",
            "Choose a registered device; Garmin sync is required",
        ));
        let send = choices.len() - 1;
        choices.push(choice(
            "Delivery status",
            "Inspect recorded devices, versions and unresolved requests",
        ));
        let delivery = choices.len() - 1;
        let summary = row.definition.as_ref().map(|_| {
            choices.push(choice(
                "Summary",
                "Working sets, repetitions, loads and timer totals",
            ));
            choices.len() - 1
        });
        let first = choices.len();
        let (steps, step_choices) = match row.definition.as_ref().or(row.outline.as_ref()) {
            Some(workout) => step_choices(workout),
            None => (Vec::new(), Vec::new()),
        };
        if step_choices.is_empty() {
            choices.push(choice(
                "No step details stored",
                "Choose Sync from Garmin again to read this workout's steps",
            ));
        }
        choices.extend(step_choices);
        let title = format!(
            "{} ({})",
            row.name,
            if row.definition.is_some() {
                "editable"
            } else {
                "read-only"
            }
        );
        let mut state = Selection::default();
        loop {
            match self.view.table(&title, STEPS, &choices, &mut state, true)? {
                None => return Ok(()),
                Some(0) => {
                    self.view.result("Workout", &workout_view::library(&row))?;
                }
                Some(index) if Some(index) == summary => {
                    if let Some(workout) = &row.definition {
                        self.view.result(
                            "Workout summary",
                            &crate::interface::workout_overview::summary(workout),
                        )?;
                    }
                }
                Some(index) if Some(index) == edit => self.model_help(Some(&row))?,
                Some(index) if index == send => {
                    let result = self.send_to_watch(&row);
                    self.recover("Send to watch", result)?;
                }
                Some(index) if index == delivery => {
                    let result = self.delivery_status(&row);
                    self.recover("Delivery status", result)?;
                }
                Some(index) => {
                    let Some(step) = steps.get(index - first) else {
                        self.view
                            .result("No step details", &workout_view::library(&row))?;
                        continue;
                    };
                    self.view
                        .result("Step", &workout_view::step_details(step))?;
                }
            }
        }
    }
    pub(super) fn send_to_watch(&mut self, row: &LibraryWorkout) -> Result<()> {
        let Some(device) = self.device()? else {
            return Ok(());
        };
        let detail = format!(
            "{} (Garmin ID {})\nDevice: {} (ID {})\nWatch receipt remains unconfirmed.",
            row.name, row.id, device.model, device.id
        );
        if self.confirm(
            "Send to watch",
            "Request delivery of this saved workout?",
            &detail,
        )? {
            self.workout_command(
                "Send to watch",
                WorkoutCommand::Send {
                    id: row.id,
                    device_id: device.id,
                },
            )?;
            self.delivery_status(row)?;
        }
        Ok(())
    }
    pub(super) fn model_help(&mut self, source: Option<&LibraryWorkout>) -> Result<()> {
        let request=match source {
            Some(row) if row.coach_plan_id.is_some()=>format!("Ask your model:\nAdjust Coach workout {} ({}) for [your request]. Import my current Coach week and read the recorded results and health first. Keep source ID/revision, include an explanation with evidence and uncertainty, and show BEFORE/AFTER including recommended weights and affected dates. Save only on my separate explicit instruction.\n\nCoach plan: {}",row.id,row.name,row.coach_plan_id.ok_or_else(|| anyhow::anyhow!("missing Coach identity"))?),
            Some(row)=>format!("Ask your model:\nUpdate my Garmin workout {} ({}) with: [your changes]. Read it with get_workout_v1 and create a draft using its source ID and revision.",row.id,row.name),
            None=>"Ask your model:\nCreate a workout for [your goal, sport and constraints]. Use my saved activities and health as context and create a local draft in Pace.".into(),
        };
        self.view.result("Create / edit with model",&format!("Use Claude, Codex or another client connected to Pace through MCP. This terminal does not run a model.\n\n{request}\n\nThe model uses create_workout_draft_v1. Further corrections use revise_workout_draft_v1.\n\nThen open Workout drafts here, review all steps and choose Save to Garmin. Nothing is sent automatically. If the administrator granted Garmin writes for your MCP user, you can instead tell the model to save it; its quote of your instruction appears under the draft."))?;
        Ok(())
    }
    pub(super) fn workout_drafts(&mut self) -> Result<()> {
        let settings = Settings::load(&self.config)?;
        let service = workouts::service(&settings)?;
        let mut offset = 0;
        let mut state = Selection::default();
        loop {
            let rows = service.drafts(PAGE_SIZE, offset)?;
            let mut choices = vec![
                choice(
                    "Refresh drafts",
                    "Read drafts created or corrected by your connected model",
                ),
                choice(
                    "New with model",
                    "How to create a workout draft through MCP",
                ),
            ];
            let first = choices.len();
            choices.extend(rows.iter().map(|draft| {
                Choice::row(
                    vec![
                        draft.id.to_string(),
                        draft.workout.name.clone(),
                        if draft
                            .source
                            .as_ref()
                            .is_some_and(|s| s.coach_plan_id.is_some())
                        {
                            "Coach"
                        } else if draft.source.is_some() {
                            "update"
                        } else {
                            "create"
                        }
                        .into(),
                        draft.revision.to_string(),
                        draft.state.as_str().into(),
                    ],
                    workout_view::state(draft.state),
                )
            }));
            let (next, previous) = paging(&mut choices, rows.len(), offset, "Drafts");
            let title = match (rows.is_empty(), offset) {
                (true, 0) => "Drafts: none yet",
                (true, _) => "Drafts: no more entries",
                (false, _) => "Workout drafts",
            };
            match self.view.table(title, DRAFTS, &choices, &mut state, true)? {
                None => return Ok(()),
                Some(0) => {}
                Some(1) => self.model_help(None)?,
                Some(index) if index == next => {
                    offset += PAGE_SIZE;
                    state = Selection::default();
                }
                Some(index) if index == previous => {
                    offset = offset.saturating_sub(PAGE_SIZE);
                    state = Selection::default();
                }
                Some(index) => self.draft_entry(&rows[index - first])?,
            }
        }
    }
    /// One draft: review and state-specific actions first, then the proposed steps.
    pub(super) fn draft_entry(&mut self, selected: &Draft) -> Result<()> {
        let service = workouts::service(&Settings::load(&self.config)?)?;
        let mut state = Selection::default();
        let mut shown_state = selected.state;
        loop {
            let current = service.draft(selected.id)?;
            if current.state != shown_state {
                state = Selection::default();
                shown_state = current.state;
            }
            let result = self.draft_entry_once(&current, &mut state);
            if self.recover("Workout draft", result)? == Some(false) {
                return Ok(());
            }
        }
    }

    fn draft_entry_once(&mut self, draft: &Draft, state: &mut Selection) -> Result<bool> {
        let review = workout_view::review(draft);
        let mut choices = vec![choice(
            "Review before/after",
            "Read the complete draft, with the current Garmin version for updates",
        )];
        let (save, discard, reconcile) = match draft.state {
            DraftState::Draft => {
                choices.push(choice(
                    "Save to Garmin",
                    "Save this exact revision after confirmation",
                ));
                choices.push(choice(
                    "Discard draft",
                    "Remove this unsaved candidate; Garmin is unchanged",
                ));
                (1, 2, usize::MAX)
            }
            DraftState::Saving | DraftState::Saved => {
                choices.push(choice(
                    "Reconcile save",
                    "Verify against an existing Garmin workout ID; do not save again",
                ));
                (usize::MAX, usize::MAX, 1)
            }
            DraftState::Verified | DraftState::Discarded => (usize::MAX, usize::MAX, usize::MAX),
        };
        let summary = choices.len();
        choices.push(choice(
            "Summary",
            "Working sets, repetitions, loads and timer totals",
        ));
        let changes = choices.len();
        choices.push(choice(
            "Change overview",
            "Only changed fields, additions and removals",
        ));
        let first = choices.len();
        let (steps, step_choices) = step_choices(&draft.workout);
        choices.extend(step_choices);
        let title = format!("Draft #{}: {}", draft.id, draft.workout.name);
        loop {
            match self.view.table(&title, STEPS, &choices, state, true)? {
                None => return Ok(false),
                Some(0) => {
                    self.view.result("Review workout", &review)?;
                }
                Some(index) if index == summary => {
                    self.view.result(
                        "Workout summary",
                        &crate::interface::workout_overview::summary(&draft.workout),
                    )?;
                }
                Some(index) if index == changes => {
                    self.view.result(
                        "Change overview",
                        &crate::interface::workout_overview::changes(draft),
                    )?;
                }
                Some(index) if index == save => {
                    if self.confirm("Save to Garmin", "Save the reviewed revision?", &review)? {
                        let saved = self.workout_command(
                            "Save workout",
                            WorkoutCommand::Save {
                                id: draft.id,
                                revision: draft.revision,
                            },
                        )?;
                        if saved {
                            self.after_workout_save(draft.id)?;
                            return Ok(false);
                        }
                        return Ok(true);
                    }
                }
                Some(index) if index == discard => {
                    if self.confirm(
                        "Discard draft",
                        "Discard this local draft?",
                        &draft.workout.name,
                    )? {
                        self.workout_command(
                            "Discard draft",
                            WorkoutCommand::Discard {
                                id: draft.id,
                                revision: draft.revision,
                            },
                        )?;
                        return Ok(true);
                    }
                }
                Some(index) if index == reconcile => {
                    let id = if let Some(id) = draft.remote_id {
                        id
                    } else {
                        let Some(id) = self.workout_id_input(draft.id)? else {
                            continue;
                        };
                        id
                    };
                    let verified = self.workout_command(
                        "Reconcile save",
                        WorkoutCommand::Reconcile {
                            id: draft.id,
                            workout_id: id,
                        },
                    )?;
                    if verified {
                        self.after_workout_save(draft.id)?;
                        return Ok(false);
                    }
                    return Ok(true);
                }
                Some(index) => {
                    self.view
                        .result("Step", &workout_view::step_details(&steps[index - first]))?;
                }
            }
        }
    }
    pub(super) fn workout_command(&mut self, title: &str, command: WorkoutCommand) -> Result<bool> {
        self.view.frame(
            title,
            &[("Working...".into(), Style::Plain)],
            "Waiting for the selected operation",
        )?;
        let result = capture(
            &self.config,
            self.env_file.as_deref(),
            Command::Workout { action: command },
        );
        rustix::termios::tcflush(std::io::stdin(), rustix::termios::QueueSelector::IFlush)?;
        let Some(text) = self.recover(title, result)? else {
            return Ok(false);
        };
        self.view.result(title, &text)?;
        Ok(true)
    }
}
