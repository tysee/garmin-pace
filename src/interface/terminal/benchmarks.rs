//! src/interface/terminal/benchmarks.rs lists benchmark exercises and model proposals as
//! tables and lets users inspect strength evidence and review model estimates.
//! Network work runs only from explicit sync, save or reconciliation selections.

use super::{
    App, Choice, Selection, Style, capture,
    view::{Column, Width},
    workouts::{PAGE_SIZE, day, paging},
};
use crate::{
    config::Settings,
    interface::{
        benchmark_view,
        benchmarks::{self, BenchmarkCommand},
        cli::Command,
    },
    workouts::{
        DraftState,
        benchmarks::{Benchmark, BenchmarkDraft, Window},
    },
};
use anyhow::{Context, Result};
use chrono::{Days, Utc};

const CATALOG: &[Column] = &[
    Column::new("Exercise", Width::Fill),
    Column::new("1RM", Width::Fit(9)),
    Column::new("Set", Width::Fit(16)),
    Column::new("Synced", Width::Fit(10)),
];
const PROPOSALS: &[Column] = &[
    Column::new("#", Width::Fit(6)),
    Column::new("Exercise", Width::Fill),
    Column::new("Rev", Width::Fit(4)),
    Column::new("State", Width::Fit(9)),
];

fn choice(label: impl Into<String>, description: impl Into<String>) -> Choice {
    Choice::new(label, description)
}
impl App {
    pub(super) fn benchmark_menu(&mut self) -> Result<()> {
        let service = benchmarks::service(&Settings::load(&self.config)?)?;
        let mut state = Selection::default();
        loop {
            let rows = service.catalog()?;
            let mut choices = vec![
                choice(
                    "Sync strength data",
                    "Import benchmarks and recorded sets for a chosen date window",
                ),
                choice(
                    "Model proposals",
                    "Review estimates, their evidence and pending saves",
                ),
                choice(
                    "Estimate with model",
                    "Request an estimate only when you need one",
                ),
            ];
            choices.extend(rows.iter().map(|b| {
                let set = match (b.weight_kg, b.repetitions) {
                    (Some(kg), Some(reps)) => format!("{kg:.1} kg x {reps}"),
                    _ => String::new(),
                };
                Choice::row(
                    vec![
                        benchmark_view::name(&b.exercise.name),
                        b.one_rep_max_kg
                            .map_or("not set".into(), |kg| format!("{kg:.1} kg")),
                        set,
                        day(&b.synced_at),
                    ],
                    "Open saved values, observed averages and estimation; changes only on your request",
                )
            }));
            let title = if rows.is_empty() {
                "Benchmarks: sync first"
            } else {
                "Benchmark exercises"
            };
            match self
                .view
                .table(title, CATALOG, &choices, &mut state, true)?
            {
                None => return Ok(()),
                Some(0) => {
                    let result = self.benchmark_window("Sync strength data");
                    if let Some(Some(window)) = self.recover("Sync strength data", result)? {
                        self.benchmark_command(
                            "Sync strength data",
                            BenchmarkCommand::Sync {
                                start: window.start,
                                end: window.end,
                            },
                        )?;
                    }
                }
                Some(1) => {
                    let result = self.benchmark_proposals();
                    self.recover("Model proposals", result)?;
                }
                Some(2) => self.benchmark_help(None)?,
                Some(i) => {
                    let result = self.benchmark_entry(&rows[i - 3]);
                    self.recover("Benchmark", result)?;
                }
            }
        }
    }
    fn benchmark_window(&mut self, title: &str) -> Result<Option<Window>> {
        let end = Utc::now().date_naive();
        let start = end
            .checked_sub_days(Days::new(27))
            .context("evidence date underflow")?;
        let Some(from) = self
            .view
            .prompt(title, &format!("Start UTC: Enter = {start}"), false)?
        else {
            return Ok(None);
        };
        let Some(to) = self.view.prompt(
            title,
            &format!("End UTC: Enter = {end}; max 31 days"),
            false,
        )?
        else {
            return Ok(None);
        };
        let window = Window {
            start: if from.trim().is_empty() {
                start.to_string()
            } else {
                from.trim().into()
            }
            .parse()?,
            end: if to.trim().is_empty() {
                end.to_string()
            } else {
                to.trim().into()
            }
            .parse()?,
        };
        window.validate()?;
        Ok(Some(window))
    }
    fn benchmark_entry(&mut self, row: &Benchmark) -> Result<()> {
        let choices = vec![
            choice(
                "Current benchmark",
                "View saved values and synchronization time",
            ),
            choice(
                "Observed averages",
                "Choose a date window and inspect actual strength sets",
            ),
            choice(
                "Estimate with model",
                "How to request a model proposal for this exercise",
            ),
        ];
        let mut state = Selection::default();
        loop {
            match self.view.select(
                &benchmark_view::name(&row.exercise.name),
                &choices,
                &mut state,
                false,
            )? {
                None => return Ok(()),
                Some(0) => {
                    self.view
                        .result("Current benchmark", &benchmark_view::benchmark(row))?;
                }
                Some(1) => {
                    let result = self.benchmark_window("Observed averages");
                    if let Some(Some(w)) = self.recover("Observed averages", result)? {
                        self.benchmark_command(
                            "Observed averages",
                            BenchmarkCommand::Context {
                                key: row.exercise.name.clone(),
                                start: w.start,
                                end: w.end,
                            },
                        )?;
                    }
                }
                Some(_) => self.benchmark_help(Some(row))?,
            }
        }
    }
    fn benchmark_help(&mut self, row: Option<&Benchmark>) -> Result<()> {
        let exercise = row.map_or("[exercise]".into(), |b| {
            benchmark_view::name(&b.exercise.name)
        });
        self.view.result("Estimate with model",&format!("Use your model connected to Pace through MCP. Sync recent strength data first.\n\nAsk: Estimate my {exercise} benchmark from my recent observed loads and repetitions. Explain the averages, method and confidence.\n\nThe model reads the catalogue and observed evidence, considers available health data, and creates a local proposal.\n\nFilled benchmarks change only on your explicit request. A model may proactively propose an unset one only when a specific workout draft needs it.\n\nOpen Model proposals to review before/after values and save to Garmin. Nothing updates in the background. If the administrator granted Garmin writes for your MCP user, you can instead tell the model to save it; its quote of your instruction appears under the proposal."))?;
        Ok(())
    }
    fn benchmark_proposals(&mut self) -> Result<()> {
        let service = benchmarks::service(&Settings::load(&self.config)?)?;
        let mut offset = 0;
        let mut state = Selection::default();
        loop {
            let rows = service.drafts(PAGE_SIZE, offset)?;
            let mut choices = vec![
                choice(
                    "Refresh list",
                    "Re-read proposals your model saved locally; nothing is requested from it",
                ),
                choice(
                    "How to get a proposal",
                    "Ask your model through MCP; Pace never requests estimates itself",
                ),
            ];
            let first = choices.len();
            choices.extend(rows.iter().map(|d| {
                Choice::row(
                    vec![
                        d.id.to_string(),
                        benchmark_view::name(&d.key),
                        d.revision.to_string(),
                        d.state.as_str().into(),
                    ],
                    benchmark_view::state(d.state),
                )
            }));
            let (next, previous) = paging(&mut choices, rows.len(), offset, "Proposals");
            let title = match (rows.is_empty(), offset) {
                (true, 0) => "Proposals: none yet",
                (true, _) => "Proposals: no more entries",
                (false, _) => "Benchmark proposals",
            };
            match self
                .view
                .table(title, PROPOSALS, &choices, &mut state, true)?
            {
                None => return Ok(()),
                Some(0) => {}
                Some(1) => self.benchmark_help(None)?,
                Some(i) if i == next => {
                    offset += PAGE_SIZE;
                    state = Selection::default();
                }
                Some(i) if i == previous => {
                    offset = offset.saturating_sub(PAGE_SIZE);
                    state = Selection::default();
                }
                Some(i) => self.benchmark_proposal(&service.draft(rows[i - first].id)?)?,
            }
        }
    }
    fn benchmark_proposal(&mut self, draft: &BenchmarkDraft) -> Result<()> {
        let review = benchmark_view::review(draft);
        if !self.view.review("Review benchmark", &review)? {
            return Ok(());
        }
        let choices = match draft.state {
            DraftState::Draft => vec![
                choice("Back", "Keep the proposal for model corrections"),
                choice(
                    "Save to Garmin",
                    "Save the reviewed estimate after confirmation",
                ),
                choice("Discard proposal", "Garmin values remain unchanged"),
            ],
            DraftState::Saving | DraftState::Saved => vec![
                choice("Back", "Keep the unresolved state"),
                choice(
                    "Reconcile save",
                    "Read Garmin to check whether the estimate was saved",
                ),
            ],
            _ => return Ok(()),
        };
        match self.view.select(
            "Proposal actions",
            &choices,
            &mut Selection::default(),
            false,
        )? {
            None | Some(0) => {}
            Some(1) if draft.state == DraftState::Draft => {
                if self.confirm("Save benchmark", "Save the reviewed estimate?", &review)? {
                    self.benchmark_command(
                        "Save benchmark",
                        BenchmarkCommand::Save {
                            id: draft.id,
                            revision: draft.revision,
                        },
                    )?;
                }
            }
            Some(1) => self.benchmark_command(
                "Reconcile benchmark",
                BenchmarkCommand::Reconcile { id: draft.id },
            )?,
            Some(_) => {
                if self.confirm(
                    "Discard proposal",
                    "Discard this local estimate?",
                    &benchmark_view::name(&draft.proposal.key),
                )? {
                    self.benchmark_command(
                        "Discard proposal",
                        BenchmarkCommand::Discard {
                            id: draft.id,
                            revision: draft.revision,
                        },
                    )?;
                }
            }
        }
        Ok(())
    }
    fn benchmark_command(&mut self, title: &str, command: BenchmarkCommand) -> Result<()> {
        self.view.frame(
            title,
            &[("Working...".into(), Style::Plain)],
            "Waiting for the selected operation",
        )?;
        let result = capture(
            &self.config,
            self.env_file.as_deref(),
            Command::Benchmark { action: command },
        );
        rustix::termios::tcflush(std::io::stdin(), rustix::termios::QueueSelector::IFlush)?;
        if let Some(text) = self.recover(title, result)? {
            self.view.result(title, &text)?;
        }
        Ok(())
    }
}
