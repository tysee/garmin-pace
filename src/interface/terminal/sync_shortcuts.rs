//! src/interface/terminal/sync_shortcuts.rs adds explicitly confirmed date presets to normal menus.
//! Existing custom-period and onboarding imports keep their original prompts and execution.

use super::{App, Choice, Selection, Style, tty::Key};
use crate::{
    config::Settings,
    health::Date,
    history::ActivityPeriod,
    interface::{garmin, health},
    secrets::Secrets,
};
use anyhow::{Context, Result};
use chrono::{Days, NaiveDate};
use std::ops::ControlFlow;

const PRESETS: [&str; 3] = ["Today", "Yesterday", "Last 7 days"];
fn period(today: NaiveDate, index: usize) -> Result<(NaiveDate, NaiveDate)> {
    match index {
        0 => Ok((today, today)),
        1 => {
            let yesterday = today
                .checked_sub_days(Days::new(1))
                .context("date underflow")?;
            Ok((yesterday, yesterday))
        }
        2 => Ok((
            today
                .checked_sub_days(Days::new(6))
                .context("date underflow")?,
            today,
        )),
        _ => anyhow::bail!("unknown date preset"),
    }
}
fn presets() -> Result<Vec<Choice>> {
    let today = chrono::Utc::now().date_naive();
    PRESETS
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let (start, end) = period(today, index)?;
            Ok(Choice::new(*name, format!("{start} to {end}, inclusive")))
        })
        .collect()
}
impl App {
    /// None means cancelled or completed preset; false requests a count, true custom dates.
    pub(super) fn activity_sync_shortcuts(&mut self, title: &str) -> Result<Option<bool>> {
        let max = Settings::load(&self.config)?.history.max_page_size;
        let mut choices = vec![
            Choice::new(
                "Recent activities",
                format!("The newest activities, up to {max} in one request"),
            ),
            Choice::new("Activities in a period", "Enter custom inclusive dates"),
        ];
        choices.extend(presets()?);
        match self
            .view
            .select(title, &choices, &mut Selection::default(), false)?
        {
            None => Ok(None),
            Some(0) => Ok(Some(false)),
            Some(1) => Ok(Some(true)),
            Some(index) => {
                self.sync_preset(false, index - 2)?;
                Ok(None)
            }
        }
    }
    pub(super) fn health_sync_shortcuts(&mut self) -> Result<()> {
        let mut choices = vec![Choice::new(
            "Custom period",
            "Enter START or START END; longer ranges are split safely",
        )];
        choices.extend(presets()?);
        match self
            .view
            .select("Import health", &choices, &mut Selection::default(), false)?
        {
            None => Ok(()),
            Some(0) => self.import_health(),
            Some(index) => self.sync_preset(true, index - 1),
        }
    }
    fn sync_preset(&mut self, is_health: bool, index: usize) -> Result<()> {
        let (start, end) = period(chrono::Utc::now().date_naive(), index)?;
        let title = if is_health {
            "Import health"
        } else {
            "Sync activities"
        };
        let details = format!(
            "{}: {start} to {end}, inclusive.\nToday is based on UTC; Garmin filters calendar dates.\n{}\nRequests are paced. Esc stops between committed days/pages; saved data is retained.",
            PRESETS[index],
            if is_health {
                "All eight health datasets; saved snapshots for these dates will be replaced."
            } else {
                "All activity pages in this period will be imported."
            }
        );
        if !self.confirm(title, "Import this period from Garmin?", &details)? {
            return Ok(());
        }
        let settings = Settings::load(&self.config)?;
        let secrets = Secrets::load(self.env_file.as_deref())?;
        let _reporting = self.report_rate_waits()?;
        if self
            .sync_progress(title, "Starting explicit import")?
            .is_break()
        {
            return Ok(());
        }
        let result = if is_health {
            self.health_preset(&settings, &secrets, start.into(), end.into())
        } else {
            self.activity_preset(&settings, &secrets, start, end)
        };
        rustix::termios::tcflush(std::io::stdin(), rustix::termios::QueueSelector::IFlush)?;
        self.view.result(title, &result?)?;
        Ok(())
    }
    fn sync_progress(&mut self, title: &str, text: &str) -> Result<ControlFlow<()>> {
        self.view.frame(
            title,
            &[(text.into(), Style::Plain)],
            "Esc stops after this day/page",
        )?;
        Ok(if self.view.terminal.pending_key()? == Key::Escape {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        })
    }
    fn health_preset(
        &mut self,
        settings: &Settings,
        secrets: &Secrets,
        start: Date,
        end: Date,
    ) -> Result<String> {
        let service = health::service(settings)?;
        let mut saved = 0;
        for (first, last) in start.windows(end, settings.health.max_days)? {
            let flow = health::sync_window(&service, settings, secrets, first, last, |day| {
                saved += 1;
                self.sync_progress(
                    "Import health",
                    &format!("Saved {saved} days; last {}", day.date),
                )
            })
            .with_context(|| {
                format!("Health import failed after {saved} saved days; committed data retained")
            })?;
            if flow.is_break() {
                return Ok(format!(
                    "Stopped after {saved} saved days. Committed data retained."
                ));
            }
        }
        Ok(format!(
            "Health sync complete: {saved} days, all 8 datasets checked.\n{start} to {end}."
        ))
    }
    fn activity_preset(
        &mut self,
        settings: &Settings,
        secrets: &Secrets,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<String> {
        let mut failure = None;
        let mut saved = 0;
        let result = garmin::sync_period(
            settings,
            secrets,
            ActivityPeriod::new(start, end)?,
            |progress| {
                saved = progress.saved;
                self.sync_progress(
                    "Sync activities",
                    &format!(
                        "Saved {} activities in {} pages",
                        progress.saved, progress.pages
                    ),
                )
                .unwrap_or_else(|error| {
                    failure = Some(error);
                    ControlFlow::Break(())
                })
            },
        );
        if let Some(error) = failure {
            return Err(error);
        }
        let result = result.with_context(|| {
            format!(
                "Activity import failed after {saved} saved activities; committed data retained"
            )
        })?;
        Ok(format!(
            "{}: {} activities, {} pages.\n{start} to {end}.",
            if result.completed.is_some() {
                "Import complete"
            } else {
                "Stopped; last complete sync unchanged"
            },
            result.progress.saved,
            result.progress.pages
        ))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn presets_are_inclusive_and_cross_month_and_year_boundaries() -> Result<()> {
        let today = "2026-01-02".parse::<NaiveDate>()?;
        assert_eq!(period(today, 0)?, (today, today));
        let (start, end) = period(today, 2)?;
        assert_eq!(start.to_string(), "2025-12-27");
        assert_eq!(end.signed_duration_since(start).num_days(), 6);
        assert_eq!(period(today, 1)?.0.to_string(), "2026-01-01");
        assert!(period(today, 3).is_err());
        Ok(())
    }
}
