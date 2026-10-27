//! src/interface/terminal/freshness.rs explains local observation freshness.
//! App::freshness separates observation dates from import times.

use super::App;
use crate::{
    config::Settings,
    interface::{cli, health, health_view},
};
use anyhow::Result;
use chrono::NaiveDate;

/// Express calendar age without conflating import time with the observation date.
fn age(date: NaiveDate, today: NaiveDate) -> String {
    match today.signed_duration_since(date).num_days() {
        0 => "today".into(),
        1 => "1 day ago".into(),
        n if n > 1 => format!("{n} days ago"),
        _ => "future date; check the source date and clock".into(),
    }
}

impl App {
    pub(super) fn freshness(&mut self) -> Result<()> {
        let settings = Settings::load(&self.config)?;
        let today = chrono::Utc::now().date_naive();
        let history = cli::history(&settings)?;
        let summary = history.summary()?;
        let latest = history.list(1, 0)?;
        let mut text = format!(
            "As of {today} (UTC); local data only.\n\nActivities: {} stored\nLast complete activity import: {}\n",
            summary.activity_count,
            summary
                .last_sync
                .map_or_else(|| "never".into(), |s| s.completed_at)
        );
        match latest.first() {
            Some(row) => {
                let date = chrono::DateTime::parse_from_rfc3339(&row.started_at)?.date_naive();
                text.push_str(&format!("Latest activity: {date} ({})\n", age(date, today)));
            }
            None => text.push_str("Latest activity: no data\n"),
        }
        let service = health::service(&settings)?;
        let summary = service.summary()?;
        text.push_str("\nHealth: observation dates use Garmin's calendar.\n");
        match summary.latest {
            Some(date) => {
                let day = service
                    .days(date, date)?
                    .into_iter()
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("latest health snapshot is missing"))?;
                text.push_str(&format!("Latest saved day: {date}; imported {}\nAn imported day may contain no observations.\n", day.synced_at));
            }
            None => text.push_str("No health snapshots imported.\n"),
        }
        for coverage in service.coverage()? {
            let date = coverage.latest.map_or_else(
                || "no observed data".into(),
                |date| format!("{date} ({})", age(date.into(), today)),
            );
            text.push_str(&format!("{}: {date}\n", health_view::name(coverage.metric)));
        }
        text.push_str("\nUse Sync activities or Health > Import health to refresh explicitly.");
        self.view.result("Data freshness", &text)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observation_age_handles_leap_days_and_future_dates() -> Result<()> {
        let today = "2024-03-01".parse::<NaiveDate>()?;
        assert_eq!(age(today, today), "today");
        assert_eq!(age("2024-02-29".parse()?, today), "1 day ago");
        assert!(age("2024-03-02".parse()?, today).starts_with("future date"));
        Ok(())
    }
}
