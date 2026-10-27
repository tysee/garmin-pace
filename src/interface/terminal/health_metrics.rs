//! src/interface/terminal/health_metrics.rs shows which health datasets are saved and lets
//! the user drill from a metric into its days, one day's records and its stored samples.
//! Every read is local and bounded by the configured limits; only Import health reaches Garmin.

use super::{App, Choice, Selection};
use crate::{
    config::Settings,
    health::{Date, HealthDay, HealthService, Metric, MetricCoverage, Section},
    interface::{health, health_view},
};
use anyhow::{Context, Result};

/// Samples shown per screen, further bounded by the configured sample page limit.
const SAMPLE_PAGE: usize = 100;

fn today() -> Date {
    chrono::Utc::now().date_naive().into()
}

impl App {
    pub(super) fn health_menu(&mut self) -> Result<()> {
        let mut state = Selection::default();
        loop {
            let settings = Settings::load(&self.config)?;
            let service = health::service(&settings)?;
            let summary = service.summary()?;
            let coverage = service.coverage()?;
            let mut choices = vec![Choice::new(
                "Import health",
                "Load a Garmin date range; each day replaces its saved snapshot",
            )];
            choices.extend(coverage.iter().map(|c| {
                Choice::new(
                    health_view::name(c.metric),
                    health_view::coverage(c, summary.day_count),
                )
            }));
            let freshness = choices.len();
            choices.push(Choice::new(
                "Data freshness",
                "Observation dates, ages and snapshot import times",
            ));
            self.view.context = match summary.latest {
                Some(latest) => format!(
                    "Saved days: {}, latest {latest} | Esc back",
                    summary.day_count
                ),
                None => "Saved days: none yet | Esc back".into(),
            };
            let title = if summary.day_count == 0 {
                "Health: import first"
            } else {
                "Health metrics"
            };
            match self.view.select(title, &choices, &mut state, true)? {
                None => return Ok(()),
                Some(0) => {
                    let result = self.health_sync_shortcuts();
                    self.recover("Import health", result)?;
                }
                Some(index) if index == freshness => {
                    let result = self.freshness();
                    self.recover("Data freshness", result)?;
                }
                Some(index) => {
                    let result =
                        self.metric_days(&service, &settings, &coverage[index - 1], summary.latest);
                    self.recover("Health data", result)?;
                }
            }
        }
    }

    /// List one window of calendar days, newest first, with the metric's headline values.
    fn metric_days(
        &mut self,
        service: &HealthService,
        settings: &Settings,
        coverage: &MetricCoverage,
        latest_saved: Option<Date>,
    ) -> Result<()> {
        let metric = coverage.metric;
        let window = settings.health.max_days;
        let span = u64::try_from(window)?.saturating_sub(1);
        let mut end = coverage.latest.or(latest_saved).unwrap_or_else(today);
        let mut state = Selection::default();
        loop {
            let start = end.back(span).context("health date underflow")?;
            let days = service.days(start, end)?;
            let dates = start.range(end, window)?;
            let rows: Vec<(Date, Option<(&HealthDay, &Section)>)> = dates
                .iter()
                .rev()
                .map(|date| {
                    let found = days.iter().find(|day| day.date == *date).and_then(|day| {
                        day.sections
                            .iter()
                            .find(|section| section.metric == metric)
                            .map(|section| (day, section))
                    });
                    (*date, found)
                })
                .collect();
            let mut choices: Vec<Choice> = rows
                .iter()
                .map(|(date, found)| {
                    Choice::new(
                        date.to_string(),
                        match found {
                            Some((_, section)) => health_view::day_line(section),
                            None => "Not synced".into(),
                        },
                    )
                })
                .collect();
            let earlier = choices.len();
            choices.push(Choice::new(
                "Earlier days",
                format!("Show the {window} days before {start}"),
            ));
            choices.push(Choice::new(
                "Later days",
                if end >= today() {
                    "Already at today".into()
                } else {
                    format!("Show the {window} days after {end}")
                },
            ));
            let title = format!("{} {start} to {end}", health_view::name(metric));
            match self.view.select(&title, &choices, &mut state, false)? {
                None => return Ok(()),
                Some(index) if index == earlier => {
                    end = start.back(1).context("health date underflow")?;
                    state = Selection::default();
                }
                Some(index) if index == earlier + 1 => {
                    let later = end.forward(span + 1).context("health date overflow")?;
                    end = later.min(today()).max(end);
                    state = Selection::default();
                }
                Some(index) => match rows[index].1 {
                    Some((day, section)) => self.metric_day(service, settings, day, section)?,
                    None => {
                        self.view.result(
                            &title,
                            &format!(
                                "{}: not synced.\n\nUse Import health to load this day.",
                                rows[index].0
                            ),
                        )?;
                    }
                },
            }
        }
    }

    /// Show one day's records; time-series datasets also offer their stored samples.
    fn metric_day(
        &mut self,
        service: &HealthService,
        settings: &Settings,
        day: &HealthDay,
        section: &Section,
    ) -> Result<()> {
        let title = format!("{} {}", health_view::name(section.metric), day.date);
        let text = health_view::day(section, day.date, &day.synced_at);
        if !section.metric.has_samples() {
            self.view.result(&title, &text)?;
            return Ok(());
        }
        let choices = [
            Choice::new("Summary", "Saved records for this day"),
            Choice::new(
                "Samples",
                format!("{} stored time-series values", section.sample_count),
            ),
        ];
        let mut state = Selection::default();
        loop {
            match self.view.select(&title, &choices, &mut state, false)? {
                None => return Ok(()),
                Some(0) => {
                    self.view.result(&title, &text)?;
                }
                Some(_) => self.metric_samples(service, settings, day.date, section.metric)?,
            }
        }
    }

    /// Page through stored samples; Enter shows the next page while one remains.
    fn metric_samples(
        &mut self,
        service: &HealthService,
        settings: &Settings,
        date: Date,
        metric: Metric,
    ) -> Result<()> {
        let page = SAMPLE_PAGE.min(settings.health.max_sample_page);
        let title = format!("{} samples {date}", health_view::name(metric));
        let mut offset = 0;
        loop {
            let samples = service.samples(date, metric, page, offset)?;
            let text = health_view::samples(&samples);
            let next = offset + samples.samples.len();
            if samples.samples.is_empty() || next >= samples.total {
                self.view.result(&title, &text)?;
                return Ok(());
            }
            if !self.view.review(&title, &text)? {
                return Ok(());
            }
            offset = next;
        }
    }
}
