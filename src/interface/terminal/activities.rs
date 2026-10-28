//! src/interface/terminal/activities.rs pages through stored activities as a table and opens
//! one activity's details, including the imported sets of a strength activity. Reads are
//! local except the explicit per-activity sets import; other imports stay with the Sync and
//! strength sync actions.

use super::{
    App, Choice, Selection,
    view::{Column, Width},
    workouts::day,
};
use crate::{
    config::Settings,
    history::{Activity, ActivityQuery, HistoryService},
    interface::{benchmark_view, benchmarks, benchmarks::BenchmarkCommand, cli},
    workouts::benchmarks::{BenchmarkService, SqliteBenchmarks},
};
use anyhow::Result;
use std::sync::Arc;

const COLUMNS: &[Column] = &[
    Column::new("Start (UTC)", Width::Fit(16)),
    Column::new("Activity", Width::Fill),
    Column::new("Sport", Width::Fit(16)),
    Column::new("Time", Width::Fit(7)),
    Column::new("Distance", Width::Fit(9)),
];

fn start(timestamp: &str) -> String {
    // Stored starts are normalized RFC 3339 UTC; keep the date and minute.
    let date = day(timestamp);
    let time: String = timestamp.chars().skip(11).take(5).collect();
    format!("{date} {time}")
}
fn duration(seconds: f64) -> String {
    let minutes = (seconds / 60.0).round() as u64;
    if minutes >= 60 {
        format!("{}h{:02}", minutes / 60, minutes % 60)
    } else {
        format!("{minutes} min")
    }
}
fn distance(meters: Option<f64>) -> String {
    meters.map_or(String::new(), |m| format!("{:.2} km", m / 1000.0))
}
pub(super) fn details(activity: &Activity, benchmarks: &BenchmarkService) -> Result<String> {
    let mut text = format!(
        "{}\nSport: {}\nStart: {}\nDuration: {} ({:.0} s)\nDistance: {}\nGarmin ID: {}",
        activity.name,
        activity.sport,
        activity.started_at,
        duration(activity.duration_seconds),
        activity.duration_seconds,
        if activity.distance_meters.is_some() {
            distance(activity.distance_meters)
        } else {
            "no data".into()
        },
        activity.garmin_id
    );
    if activity.sport == "strength_training" {
        let sets = benchmarks::activity_sets(benchmarks, activity)?;
        text.push_str("\n\n");
        text.push_str(&benchmark_view::activity_sets(&sets));
    }
    Ok(text)
}

impl App {
    pub(super) fn activities(&mut self) -> Result<()> {
        let settings = Settings::load(&self.config)?;
        let history = cli::history(&settings)?;
        // The history service already validated this profile database.
        let benchmarks = BenchmarkService::new(Arc::new(SqliteBenchmarks::attach(
            settings.database.clone(),
        )));
        let page = settings.history.default_page_size;
        let mut offset = 0;
        let mut state = Selection::default();
        let mut filter: Option<ActivityQuery> = None;
        loop {
            let (rows, total) = if let Some(query) = &mut filter {
                query.offset = offset;
                let found = history.search(query)?;
                (found.activities, Some(found.total))
            } else {
                (history.list(page, offset)?, None)
            };
            let has_next = total.map_or(rows.len() == page, |total| {
                ((offset + rows.len()) as u64) < total
            });
            let mut choices: Vec<_> = rows
                .iter()
                .map(|activity| {
                    Choice::row(
                        vec![
                            start(&activity.started_at),
                            activity.name.clone(),
                            activity.sport.clone(),
                            duration(activity.duration_seconds),
                            distance(activity.distance_meters),
                        ],
                        "Open activity details",
                    )
                })
                .collect();
            if has_next {
                choices.push(Choice::new(
                    "Next page",
                    format!("Older activities from {}", offset + page + 1),
                ));
            }
            let next = if has_next {
                choices.len() - 1
            } else {
                usize::MAX
            };
            if offset > 0 {
                choices.push(Choice::new(
                    "Previous page",
                    format!("Newer activities before {}", offset + 1),
                ));
            }
            let previous = if offset > 0 {
                choices.len() - 1
            } else {
                usize::MAX
            };
            let search = choices.len();
            choices.push(Choice::new(
                "Search all history",
                filter.as_ref().map_or_else(
                    || "Filter all saved activities by name, sport and UTC dates".into(),
                    |q| {
                        format!(
                            "Name: {}; sport: {}; UTC: {} to {}. Enter edits filters.",
                            q.name.as_deref().unwrap_or("any"),
                            q.sport.as_deref().unwrap_or("any"),
                            q.start.map_or("any".into(), |d| d.to_string()),
                            q.end.map_or("any".into(), |d| d.to_string())
                        )
                    },
                ),
            ));
            let clear = choices.len();
            if filter.is_some() {
                choices.push(Choice::new(
                    "Clear filters",
                    "Return to all saved activities",
                ));
            }
            let title = if let Some(total) = total {
                format!(
                    "Matches {}-{} of {total}",
                    if rows.is_empty() { 0 } else { offset + 1 },
                    offset + rows.len()
                )
            } else if rows.is_empty() && offset == 0 {
                "Activities: none stored; use Sync activities".into()
            } else if rows.is_empty() {
                // A page that ended exactly at the page size has nothing after it.
                "Activities: no more entries".into()
            } else {
                format!("Activities {}-{}", offset + 1, offset + rows.len())
            };
            match self
                .view
                .table(&title, COLUMNS, &choices, &mut state, true)?
            {
                None => return Ok(()),
                Some(index) if index == search => {
                    if let Some(query) = self.activity_filter(&history, page)? {
                        filter = Some(query);
                        offset = 0;
                        state = Selection::default();
                    }
                }
                Some(index) if index == clear && filter.is_some() => {
                    filter = None;
                    offset = 0;
                    state = Selection::default();
                    self.view.remember_inputs(true);
                }
                Some(index) if index == next => {
                    offset += page;
                    state = Selection::default();
                }
                Some(index) if index == previous => {
                    offset = offset.saturating_sub(page);
                    state = Selection::default();
                }
                Some(index) if rows[index].sport == "strength_training" => {
                    self.strength_activity(&rows[index], &benchmarks)?;
                }
                Some(index) => self.activity_details(&rows[index], &benchmarks)?,
            }
        }
    }

    fn activity_details(
        &mut self,
        activity: &Activity,
        benchmarks: &BenchmarkService,
    ) -> Result<()> {
        let result = details(activity, benchmarks);
        if let Some(text) = self.recover("Activity", result)? {
            self.view.result("Activity", &text)?;
        }
        Ok(())
    }

    /// A strength activity can also import its own sets; nothing else contacts Garmin here.
    fn strength_activity(
        &mut self,
        activity: &Activity,
        benchmarks: &BenchmarkService,
    ) -> Result<()> {
        let choices = [
            Choice::new("View details", "Activity summary and its imported sets"),
            Choice::new(
                "Import sets from Garmin",
                "Read this activity's sets by its ID; no activity search",
            ),
        ];
        let mut state = Selection::default();
        loop {
            match self
                .view
                .select(&activity.name, &choices, &mut state, false)?
            {
                None => return Ok(()),
                Some(0) => self.activity_details(activity, benchmarks)?,
                Some(_) => self.benchmark_command(
                    "Import sets from Garmin",
                    BenchmarkCommand::SyncActivity {
                        id: activity.garmin_id.clone(),
                    },
                )?,
            }
        }
    }

    fn activity_filter(
        &mut self,
        history: &HistoryService,
        limit: usize,
    ) -> Result<Option<ActivityQuery>> {
        loop {
            let mut fields = Vec::new();
            for label in [
                "Name contains (blank = any)",
                "Sport key (blank = any)",
                "Start UTC YYYY-MM-DD (blank = any)",
                "End UTC YYYY-MM-DD (blank = any)",
            ] {
                let Some(value) = self.view.prompt("Search all history", label, false)? else {
                    return Ok(None);
                };
                fields.push(value.trim().to_owned());
            }
            let parsed = (|| -> Result<ActivityQuery> {
                let query = ActivityQuery {
                    name: (!fields[0].is_empty()).then(|| fields[0].clone()),
                    sport: (!fields[1].is_empty()).then(|| fields[1].clone()),
                    start: if fields[2].is_empty() {
                        None
                    } else {
                        Some(fields[2].parse()?)
                    },
                    end: if fields[3].is_empty() {
                        None
                    } else {
                        Some(fields[3].parse()?)
                    },
                    limit,
                    offset: 0,
                };
                history.search(&query)?;
                Ok(query)
            })();
            if let Some(query) = self.recover("Search all history", parsed)? {
                return Ok(Some(query));
            }
        }
    }
}
