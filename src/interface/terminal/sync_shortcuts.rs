//! src/interface/terminal/sync_shortcuts.rs adds explicitly confirmed date presets to normal menus.
//! Presets differ from custom periods only in how dates are chosen: both run the same
//! activity and health imports. Sync everything reuses the same presets.

use super::{App, Choice, Selection};
use crate::config::Settings;
use anyhow::{Context, Result};
use chrono::{Days, NaiveDate};

pub(super) const PRESETS: [&str; 3] = ["Today", "Yesterday", "Last 7 days"];
pub(super) fn period(today: NaiveDate, index: usize) -> Result<(NaiveDate, NaiveDate)> {
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
        if is_health {
            self.import_health_range(start.into(), end.into())
        } else {
            self.import_activity_period(title, start, end)
        }
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
