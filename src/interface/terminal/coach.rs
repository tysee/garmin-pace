//! src/interface/terminal/coach.rs presents Coach plans and sessions separately from the library.
//! Week navigation and details read local snapshots; explicit sync and the confirmed move of
//! one session invoke the shared CLI, and existing workout/draft screens own step review, save
//! and delivery actions.

use super::{
    App, Choice, Selection,
    view::{Column, Width},
};
use crate::{
    config::Settings,
    health::Date,
    interface::{
        cli::{self, Command},
        coach::{CoachCommand, reschedule_line, reschedule_note},
        workouts,
    },
    workouts::coach::{Reschedule, RescheduleState, Session, Snapshot, Window},
};
use anyhow::Result;

const COLUMNS: &[Column] = &[
    Column::new("Date", Width::Fit(10)),
    Column::new("Workout", Width::Fill),
    Column::new("Result", Width::Fit(12)),
    Column::new("Plan", Width::Fit(12)),
];
fn choices(window: Window, snapshot: Option<&Snapshot>, moves: &[Reschedule]) -> Vec<Choice> {
    let status = match snapshot {
        None => "not synced".into(),
        Some(s) => format!("{} sessions; synced {}", s.sessions.len(), s.synced_at),
    };
    let mut rows = vec![
        Choice::new(
            "Sync this week",
            format!("{} to {} | {status}", window.start, window.end),
        ),
        Choice::new(
            "Plan details",
            "Plan identities, dates, status and snapshot time",
        ),
        Choice::new(
            "Adjust with model",
            "Read results, propose changes, review before saving",
        ),
        Choice::new(
            "Review drafts",
            "Shared review queue; Coach updates are labelled Coach",
        ),
        Choice::new(
            "Choose week",
            "Enter a Garmin calendar date; initial week uses today's UTC date",
        ),
        Choice::new("Previous week", "Read the preceding week locally"),
        Choice::new("Next week", "Read the following week locally"),
    ];
    if let Some(s) = snapshot {
        rows.extend(s.sessions.iter().map(|row| {
            Choice::row(
                vec![
                    row.date.to_string(),
                    row.workout.name.clone(),
                    if row.associated_activity_id.is_some() {
                        "activity linked"
                    } else {
                        "no link"
                    }
                    .into(),
                    row.workout
                        .coach_plan_id
                        .map_or("standalone".into(), |v| v.to_string()),
                ],
                format!(
                    "Workout {} | schedule {}{}. Open steps, recorded result or reschedule",
                    row.workout.id,
                    row.schedule_id,
                    move_hint(moves, row.schedule_id)
                ),
            )
        }));
    }
    rows
}
/// The newest journaled move of a session, since snapshot dates are never rewritten.
fn move_hint(moves: &[Reschedule], schedule_id: u64) -> String {
    moves
        .iter()
        .find(|r| r.schedule_id == schedule_id)
        .map(|r| match r.state {
            RescheduleState::Moving => format!(" | move to {} unresolved", r.to),
            RescheduleState::Moved => format!(" | moved to {}", r.to),
            RescheduleState::NotApplied => " | move not applied".into(),
            RescheduleState::Missing => " | entry missing in Garmin".into(),
        })
        .unwrap_or_default()
}
fn plan_details(window: Window, snapshot: Option<&Snapshot>) -> String {
    let mut text = format!(
        "Garmin calendar: {} to {}\nInitial selection uses the current UTC week; Choose week changes it.\n",
        window.start, window.end
    );
    match snapshot {
        None => text.push_str(
            "\nImport this week explicitly with Sync this week. No network request was made.",
        ),
        Some(s) => {
            text.push_str(&format!(
                "\nSynced: {}\nScheduled sessions: {}\n",
                s.synced_at,
                s.sessions.len()
            ));
            if s.plans.is_empty() {
                text.push_str("No plans returned for this window.\n");
            }
            for p in &s.plans {
                text.push_str(&format!(
                    "\n{}\nPlan: {} | {} | {}\n{} to {}\n",
                    p.name, p.id, p.category, p.status, p.start, p.end
                ));
            }
            if s.sessions.is_empty() {
                text.push_str(
                    "\nNo scheduled workouts were returned. This does not establish rest days.",
                );
            }
        }
    }
    text
}
impl App {
    fn coach_message(&mut self, title: &str, text: &str) -> Result<()> {
        self.view.result(title, text).map(|_| ())
    }
    pub(super) fn coach_menu(&mut self) -> Result<()> {
        let service = workouts::service(&Settings::load(&self.config)?)?;
        let mut window = Window::week_containing(chrono::Utc::now().date_naive().into())?;
        let mut state = Selection::default();
        loop {
            let snapshot = service.coach(window)?;
            let moves = service.coach_reschedules(window)?;
            let rows = choices(window, snapshot.as_ref(), &moves);
            let title = if snapshot.is_none() {
                "Coach: not synced"
            } else {
                "Coach week"
            };
            let selected = self.view.table(title, COLUMNS, &rows, &mut state, true)?;
            let outcome = match selected {
                None => return Ok(()),
                Some(0) => self.sync_coach_week(window),
                Some(1) => {
                    self.coach_message("Coach plan", &plan_details(window, snapshot.as_ref()))
                }
                Some(2) => self.coach_model_help(window),
                Some(3) => self.workout_drafts(),
                Some(4) => {
                    let result = self.choose_coach_week(&mut window);
                    if result.is_ok() {
                        state = Selection::default();
                    }
                    result
                }
                Some(5) => {
                    window = window.adjacent(false)?;
                    state = Selection::default();
                    Ok(())
                }
                Some(6) => {
                    window = window.adjacent(true)?;
                    state = Selection::default();
                    Ok(())
                }
                Some(index) => {
                    if let Some(row) = snapshot.as_ref().and_then(|s| s.sessions.get(index - 7)) {
                        self.coach_session(row)
                    } else {
                        Ok(())
                    }
                }
            };
            self.recover("Coach", outcome)?;
        }
    }
    fn choose_coach_week(&mut self, window: &mut Window) -> Result<()> {
        if let Some(value) =
            self.view
                .prompt("Choose Coach week", "Garmin calendar date YYYY-MM-DD", true)?
        {
            *window = Window::week_containing(value.trim().parse()?)?;
        }
        Ok(())
    }
    fn sync_coach_week(&mut self, window: Window) -> Result<()> {
        let _reporting = self.report_rate_waits()?;
        let result = self.run_command(
            "Sync Coach",
            "Reading plans and scheduled workouts...",
            "Waiting for Garmin",
            Command::Coach {
                action: CoachCommand::Sync {
                    start: window.start.to_string(),
                    end: window.end.to_string(),
                },
            },
        )?;
        self.coach_message("Coach imported", &result?)
    }
    fn coach_model_help(&mut self, window: Window) -> Result<()> {
        self.coach_message("Adjust Coach with model",&format!("Ask your connected model:\nRead my Coach plan and schedule for {} through {} and my recorded results and recovery. Propose adjustments for [my goal or request], including recommended weights. Explain evidence and uncertainty, show BEFORE/AFTER and affected dates, and create source-linked drafts. Do not save until I explicitly ask.\n\nReview drafts here or in Workout drafts. A workout ID can be shared by multiple scheduled dates. Changes affect that definition; they do not change Coach preferences. Moving one session to another date is a separate explicit request: open the session and choose Reschedule, or ask the model to call the reschedule tool with your exact instruction.",window.start,window.end))
    }
    fn coach_session(&mut self, session: &Session) -> Result<()> {
        let mut state = Selection::default();
        let rows = vec![
            Choice::new(
                "Workout steps",
                "Open the latest local definition, model editing and delivery",
            ),
            Choice::new(
                "Schedule details",
                "Date, plan, workout identity and activity link",
            ),
            Choice::new(
                "Recorded result",
                "Read the linked activity and strength sets locally",
            ),
            Choice::new(
                "Reschedule",
                "Move this session to another Garmin calendar date after review",
            ),
            Choice::new("Review drafts", "Open the shared before/after review queue"),
        ];
        let title = format!("Coach: {}", session.workout.name);
        loop {
            let outcome = match self.view.select(&title, &rows, &mut state, true)? {
                None => return Ok(()),
                Some(0) => {
                    let service = workouts::service(&Settings::load(&self.config)?)?;
                    self.library_entry(service.get(session.workout.id)?)
                }
                Some(1) => self.coach_schedule_details(session),
                Some(2) => self.coach_result(session),
                Some(3) => self.coach_reschedule(session),
                Some(_) => self.workout_drafts(),
            };
            self.recover("Coach session", outcome)?;
        }
    }
    fn coach_schedule_details(&mut self, session: &Session) -> Result<()> {
        let service = workouts::service(&Settings::load(&self.config)?)?;
        let mut text = format!(
            "{}\nDate: {}\nWorkout: {}\nPlan: {}\nSchedule: {}\nLinked activity: {}\nA missing link does not prove no workout was performed.",
            session.workout.name,
            session.date,
            session.workout.id,
            session
                .workout
                .coach_plan_id
                .map_or("none".into(), |v| v.to_string()),
            session.schedule_id,
            session
                .associated_activity_id
                .as_deref()
                .unwrap_or("no link")
        );
        let moves = service.session_reschedules(session.schedule_id)?;
        if !moves.is_empty() {
            text.push_str("\n\nJournaled moves (the snapshot date above is not rewritten):\n");
            for r in &moves {
                text.push_str(&reschedule_line(r));
                text.push('\n');
            }
        }
        self.coach_message("Coach session", &text)
    }
    /// Move one session after review and confirmation, or settle a move Garmin left unresolved.
    fn coach_reschedule(&mut self, session: &Session) -> Result<()> {
        let service = workouts::service(&Settings::load(&self.config)?)?;
        let moves = service.session_reschedules(session.schedule_id)?;
        if let Some(pending) = moves.iter().find(|r| r.state == RescheduleState::Moving) {
            return self.coach_pending_move(session, pending);
        }
        let Some(value) = self.view.prompt(
            "Reschedule session",
            "New Garmin calendar date YYYY-MM-DD",
            true,
        )?
        else {
            return Ok(());
        };
        let date: Date = value.trim().parse()?;
        let question = format!(
            "Move {} from {} to {} in Garmin?",
            session.workout.name, session.date, date
        );
        let details = format!(
            "Workout: {} (ID {})\nPlan: {}\nSchedule ID: {}\nFrom: {}\nTo: {}\n\nGarmin decides whether this plan allows the move and how far. Pace journals the request, re-reads the entry, sends only the new date and reads it back. Nothing else in the plan is shifted. Sync both affected weeks afterwards.",
            session.workout.name,
            session.workout.id,
            session
                .workout
                .coach_plan_id
                .map_or("none".into(), |v| v.to_string()),
            session.schedule_id,
            session.date,
            date
        );
        if !self.confirm("Reschedule session", &question, &details)? {
            return Ok(());
        }
        let _reporting = self.report_rate_waits()?;
        let result = self.run_command(
            "Reschedule session",
            "Moving the session in Garmin...",
            "Waiting for Garmin",
            Command::Coach {
                action: CoachCommand::Reschedule {
                    schedule_id: session.schedule_id,
                    date,
                },
            },
        )?;
        self.coach_message("Reschedule result", &result?)
    }
    fn coach_pending_move(&mut self, session: &Session, pending: &Reschedule) -> Result<()> {
        let text = format!(
            "{}\n{}",
            reschedule_line(pending),
            reschedule_note(pending.state)
        );
        let options = [
            Choice::new("Back", "Keep the unresolved move"),
            Choice::new(
                "Reconcile move",
                "Read the calendar entry back from Garmin; never writes",
            ),
            Choice::new(
                "Clear unresolved move",
                "Only after checking the Garmin calendar yourself; closes it as not applied",
            ),
        ];
        match self.view.select(
            "Unresolved move",
            &options,
            &mut Selection::default(),
            false,
        )? {
            Some(1) => {
                let _reporting = self.report_rate_waits()?;
                let result = self.run_command(
                    "Reconcile move",
                    "Reading the calendar entry...",
                    "Waiting for Garmin",
                    Command::Coach {
                        action: CoachCommand::ReconcileReschedule {
                            schedule_id: session.schedule_id,
                        },
                    },
                )?;
                self.coach_message("Reconcile result", &result?)
            }
            Some(2) => {
                if self.confirm(
                    "Clear unresolved move",
                    "I checked the Garmin calendar; close this move as not applied?",
                    &text,
                )? {
                    return self
                        .show_command(
                            "Clear unresolved move",
                            Command::Coach {
                                action: CoachCommand::ClearReschedule {
                                    schedule_id: session.schedule_id,
                                },
                            },
                        )
                        .map(|_| ());
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    fn coach_result(&mut self, session: &Session) -> Result<()> {
        let Some(id) = session.associated_activity_id.as_deref() else {
            return self.coach_message("Coach result","No activity is linked in this calendar snapshot. This does not prove no training occurred. Check Activities; explicitly sync results if needed.");
        };
        let settings = Settings::load(&self.config)?;
        let history = cli::history(&settings)?;
        let Some(activity) = history.find(id)? else {
            return self.coach_message("Coach result",&format!("Linked activity {id} has not been imported into local history. Use Sync activities explicitly."));
        };
        let benchmarks = crate::interface::benchmarks::service(&settings)?;
        let text = super::activities::details(&activity, &benchmarks)?;
        self.coach_message("Coach result", &text)
    }
}
