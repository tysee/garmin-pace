//! src/interface/health.rs defines the health CLI and renders local snapshot responses.
//! It composes Garmin and SQLite adapters while domain services own validation and commits;
//! sync_window lets the terminal import long ranges window by window with per-day progress.

use super::{health_responses, output::Output};
use crate::{
    config::Settings,
    garmin::GarminHealth,
    health::{Date, HealthDay, HealthService, Metric, SqliteHealth, document::DayDocument},
    secrets::Secrets,
};
use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use clap::Subcommand;
use serde_json::{Value, json};
use std::{fmt::Write as _, ops::ControlFlow, sync::Arc};

#[derive(Subcommand)]
pub(super) enum HealthCommand {
    /// Import all health datasets for an inclusive Garmin calendar-date range.
    Sync {
        #[arg(long)]
        start: Date,
        #[arg(long)]
        end: Date,
    },
    /// Show locally saved health and readiness for one calendar day.
    Show {
        #[arg(long)]
        date: Date,
    },
    /// Read local daily health summaries over a bounded inclusive range.
    List {
        #[arg(long)]
        start: Date,
        #[arg(long)]
        end: Date,
    },
    /// Read a page of stored heart_rate, body_battery, hrv or spo2 samples.
    Samples {
        #[arg(long)]
        date: Date,
        #[arg(long)]
        metric: Metric,
        #[arg(long)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
}
pub(super) fn service(settings: &Settings) -> Result<HealthService> {
    Ok(HealthService::new(
        Arc::new(SqliteHealth::open(settings.database.clone())?),
        settings.health.limits(),
    ))
}
pub(super) fn run(
    command: HealthCommand,
    settings: &Settings,
    secrets: &Secrets,
    output: Output<'_>,
) -> Result<()> {
    let service = service(settings)?;
    let value = match command {
        HealthCommand::Sync { start, end } => {
            return sync(&service, settings, secrets, start, end, output);
        }
        HealthCommand::Show { date } => health_responses::day(&service, date)?,
        HealthCommand::List { start, end } => {
            health_responses::days(&service, start, end, settings.health.max_days)?
        }
        HealthCommand::Samples {
            date,
            metric,
            limit,
            offset,
        } => health_responses::samples(&service, date, metric, limit, offset)?,
    };
    let human = render(&value)?;
    output.message(&value, &human)
}
fn sync(
    service: &HealthService,
    settings: &Settings,
    secrets: &Secrets,
    start: Date,
    end: Date,
    output: Output<'_>,
) -> Result<()> {
    let mut saved = Vec::new();
    // The CLI never breaks early: its range is one bounded window.
    let _: ControlFlow<()> = sync_window(service, settings, secrets, start, end, |day| {
        saved.push(DayDocument::from(day));
        Ok(ControlFlow::Continue(()))
    })?;
    let value = json!({"contract_version":1, "scope":"local_snapshot", "saved_days":saved.len(), "days":saved});
    let mut human = format!(
        "Health sync complete: {} days, all 8 datasets checked.\n",
        saved.len()
    );
    human.push_str(&render(&value)?);
    output.message(&value, human.trim_end())
}
/// Import one bounded window over one Garmin connection, handing each committed day to
/// on_day; Break stops before the next day. Committed days stay saved on any stop.
pub(super) fn sync_window(
    service: &HealthService,
    settings: &Settings,
    secrets: &Secrets,
    start: Date,
    end: Date,
    mut on_day: impl FnMut(HealthDay) -> Result<ControlFlow<()>>,
) -> Result<ControlFlow<()>> {
    let dates = start.range(end, settings.health.max_days)?;
    let source = GarminHealth::connect(
        settings,
        secrets.require("GARMIN_PERSONAL_API_KEY")?,
        settings.health.limits(),
    )?;
    for (committed, date) in dates.into_iter().enumerate() {
        let day = service.sync_day(&source, date, || Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true))
            .with_context(|| format!("health sync stopped at {date}; {committed} earlier days committed; failing day's previous data preserved"))?;
        if on_day(day)?.is_break() {
            return Ok(ControlFlow::Break(()));
        }
    }
    Ok(ControlFlow::Continue(()))
}
fn render(value: &Value) -> Result<String> {
    let mut human = String::new();
    if let Some(days) = value.get("days").and_then(Value::as_array) {
        for day in days {
            render_day(day, &mut human)?;
        }
        if let Some(missing) = value.get("missing_dates").and_then(Value::as_array) {
            for date in missing {
                writeln!(human, "{}: not synced", date.as_str().context("date")?)?;
            }
        }
    } else if let Some(samples) = value.get("samples").and_then(Value::as_array) {
        writeln!(
            human,
            "{} | {} | {} | {} samples ({} total)",
            value["date"],
            value["metric"],
            value["status"],
            samples.len(),
            value["total"]
        )?;
        for sample in samples {
            writeln!(
                human,
                "{} | {} {}",
                sample["timestamp"], sample["value"], sample["unit"]
            )?;
        }
    } else {
        render_day(value, &mut human)?;
    }
    Ok(human.trim_end().into())
}
fn render_day(day: &Value, human: &mut String) -> Result<()> {
    let date = day["date"].as_str().context("date")?;
    if day["status"] == "not_synced" {
        writeln!(human, "{date}: not synced")?;
        return Ok(());
    }
    writeln!(
        human,
        "{} | Synced: {}",
        date,
        day["synced_at"].as_str().context("sync time")?
    )?;
    for (name, dataset) in day["datasets"].as_object().context("datasets")? {
        writeln!(
            human,
            "  {}: {} ({} stored samples)",
            name.replace('_', " "),
            dataset["status"].as_str().context("status")?,
            dataset["sample_count"]
        )?;
        for record in dataset["records"].as_array().context("records")? {
            if let Some(time) = record["observed_at"].as_str() {
                writeln!(human, "    At {time}")?;
            }
            for (key, metric) in record["values"].as_object().context("values")? {
                let value = match &metric["value"] {
                    Value::String(s) => s.chars().flat_map(char::escape_debug).collect(),
                    v => v.to_string(),
                };
                let unit = metric["unit"].as_str().unwrap_or("");
                writeln!(human, "    {}: {} {}", key.replace('_', " "), value, unit)?;
            }
        }
    }
    Ok(())
}
