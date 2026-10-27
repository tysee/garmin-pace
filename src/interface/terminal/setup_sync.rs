//! src/interface/terminal/setup_sync.rs lets the menu import activities by count or by period.
//! App::sync_mode asks which; App::import_period reads START [END] (END defaults to today,
//! UTC) and imports the period page by page with progress. Esc stops before the next page.

use super::{
    App,
    setup_health::parse_range,
    tty::{ExitRequested, Key},
    view::{Choice, Selection, Style},
};
use crate::{
    config::Settings,
    history::{ActivityPeriod, PeriodProgress},
    interface::garmin,
    secrets::Secrets,
};
use anyhow::{Context, Result};
use std::{io, ops::ControlFlow};

impl App {
    /// Ask how to import activities: Some(true) for a period, Some(false) for recent ones.
    pub(super) fn sync_mode(&mut self, title: &str) -> Result<Option<bool>> {
        let max = Settings::load(&self.config)?.history.max_page_size;
        let choices = [
            Choice::new(
                "Recent activities",
                format!("The newest activities, up to {max} in one request"),
            ),
            Choice::new(
                "Activities in a period",
                "Every activity between two dates, read page by page",
            ),
        ];
        Ok(self
            .view
            .select(title, &choices, &mut Selection::default(), false)?
            .map(|choice| choice == 1))
    }

    pub(super) fn import_period(&mut self, title: &str) -> Result<()> {
        let settings = Settings::load(&self.config)?;
        let today = chrono::Utc::now().date_naive();
        let label = format!(
            "Activity dates: START, or START END (YYYY-MM-DD). END defaults to today, {today} (UTC). Pace reads up to {} activities per request, paced to stay under Garmin's rate limit.",
            settings.history.max_page_size
        );
        let Some(input) = self.view.prompt(title, &label, false)? else {
            return Ok(());
        };
        let (start, end) = parse_range(&input, today)?;
        let period = ActivityPeriod::new(start, end)?;
        let secrets = Secrets::load(self.env_file.as_deref())?;
        let _reporting = self.report_rate_waits()?;
        self.period_progress(title, period, PeriodProgress { pages: 0, saved: 0 })?;
        let mut last = PeriodProgress { pages: 0, saved: 0 };
        let mut failure = None;
        let result = garmin::sync_period(&settings, &secrets, period, |progress| {
            last = progress;
            let step = self
                .period_progress(title, period, progress)
                .and_then(|()| {
                    Ok(match self.view.terminal.pending_key()? {
                        Key::Escape => ControlFlow::Break(()),
                        _ => ControlFlow::Continue(()),
                    })
                });
            // Drawing errors and Ctrl-C stop the import and are reported after it.
            step.unwrap_or_else(|error| {
                failure = Some(error);
                ControlFlow::Break(())
            })
        });
        // Discard typing during the import so it cannot confirm a later action.
        rustix::termios::tcflush(io::stdin(), rustix::termios::QueueSelector::IFlush)?;
        if let Some(error) = failure {
            return Err(error);
        }
        let outcome = match result {
            Err(error) if error.is::<ExitRequested>() => return Err(error),
            Err(error) => {
                return Err(error).context(format!(
                    "{} activities saved before the failure are kept; import the period again to finish",
                    last.saved
                ));
            }
            Ok(outcome) => outcome,
        };
        let (start, end) = (period.start(), period.end());
        let text = match outcome.completed {
            Some(_) if outcome.progress.saved == 0 => {
                format!("No activities between {start} and {end}.")
            }
            Some(_) => format!(
                "Imported {} activities from {start} to {end} ({} pages).",
                outcome.progress.saved, outcome.progress.pages
            ),
            None => format!(
                "Stopped with Esc after {} pages: {} activities saved.\nThe last successful sync time was not updated. Import the period again to finish; saved activities are updated, not duplicated.",
                outcome.progress.pages, outcome.progress.saved
            ),
        };
        self.view.result(title, &text)?;
        Ok(())
    }

    fn period_progress(
        &mut self,
        title: &str,
        period: ActivityPeriod,
        progress: PeriodProgress,
    ) -> Result<()> {
        let lines = [
            format!("Period: {} to {}", period.start(), period.end()),
            format!(
                "Pages read: {}; activities saved: {}",
                progress.pages, progress.saved
            ),
            String::new(),
            "Requests are paced to stay under Garmin's rate limit.".into(),
            "Esc stops before the next page".into(),
        ]
        .map(|line| (line, Style::Plain));
        self.view.frame(title, &lines, "Waiting for Garmin")
    }
}
