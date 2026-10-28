//! src/interface/terminal/coach.rs presents Coach plans and sessions separately from the library.
//! Week navigation and details read local snapshots; explicit sync invokes the shared CLI,
//! and existing workout/draft screens own step review, save and delivery actions.

use super::{
    App, Choice, Selection,
    view::{Column, Width},
};
use crate::{
    config::Settings,
    interface::{
        cli::{self, Command},
        coach::CoachCommand,
        workouts,
    },
    workouts::coach::{Session, Snapshot, Window},
};
use anyhow::Result;

const COLUMNS: &[Column] = &[
    Column::new("Date", Width::Fit(10)),
    Column::new("Workout", Width::Fill),
    Column::new("Result", Width::Fit(12)),
    Column::new("Plan", Width::Fit(12)),
];
fn choices(window: Window, snapshot: Option<&Snapshot>) -> Vec<Choice> {
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
                    "Workout {} | schedule {}. Open steps and recorded result",
                    row.workout.id, row.schedule_id
                ),
            )
        }));
    }
    rows
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
            let rows = choices(window, snapshot.as_ref());
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
        self.coach_message("Adjust Coach with model",&format!("Ask your connected model:\nRead my Coach plan and schedule for {} through {} and my recorded results and recovery. Propose adjustments for [my goal or request], including recommended weights. Explain evidence and uncertainty, show BEFORE/AFTER and affected dates, and create source-linked drafts. Do not save until I explicitly ask.\n\nReview drafts here or in Workout drafts. A workout ID can be shared by multiple scheduled dates. Changes affect that definition; they do not change Coach preferences or reschedule sessions.",window.start,window.end))
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
            Choice::new("Review drafts", "Open the shared before/after review queue"),
        ];
        let title = format!("Coach: {}", session.workout.name);
        loop {
            let outcome=match self.view.select(&title,&rows,&mut state,true)? {
                None=>return Ok(()),
                Some(0)=>{
                    let service=workouts::service(&Settings::load(&self.config)?)?;
                    self.library_entry(service.get(session.workout.id)?)
                },
                Some(1)=>self.coach_message("Coach session",&format!("{}\nDate: {}\nWorkout: {}\nPlan: {}\nSchedule: {}\nLinked activity: {}\nA missing link does not prove no workout was performed.",session.workout.name,session.date,session.workout.id,session.workout.coach_plan_id.map_or("none".into(),|v|v.to_string()),session.schedule_id,session.associated_activity_id.as_deref().unwrap_or("no link"))),
                Some(2)=>self.coach_result(session),
                Some(_)=>self.workout_drafts(),
            };
            self.recover("Coach session", outcome)?;
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
