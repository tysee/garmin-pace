//! src/interface/terminal/setup_health.rs imports Garmin health for any date range from the menu.
//! App::import_health reads one START [END] line (END defaults to today, UTC), splits long
//! ranges into configured windows and shows per-day progress. Esc stops after the current
//! day; every committed day stays saved. Hints explain Garmin's rate limit and the pacing
//! that avoids it; rate-limit waits appear on the footer row while they last.

use super::{
    App,
    tty::{ExitRequested, Key},
    view::Style,
};
use crate::{
    config::Settings,
    garmin::{self, RateWait},
    health::Date,
    interface::health,
    secrets::Secrets,
};
use anyhow::{Context, Result, ensure};
use std::{io, ops::ControlFlow, str::FromStr};

/// Each imported day reads eight Garmin datasets, one request each.
const REQUESTS_PER_DAY: u64 = 8;

/// Restores stderr reporting of rate-limit waits when the import screen closes.
pub(super) struct WaitReporting;
impl Drop for WaitReporting {
    fn drop(&mut self) {
        if let Err(error) = garmin::observe(None) {
            eprintln!("error: {error:#}");
        }
    }
}

/// Describe the pacing that keeps an import under Garmin's rate limit.
fn pace_hint(interval_ms: u64, days: u64) -> String {
    let seconds = (days * REQUESTS_PER_DAY * interval_ms).div_ceil(1000);
    let duration = if seconds < 120 {
        format!("{seconds} s")
    } else {
        format!("{} min", seconds.div_ceil(60))
    };
    format!(
        "Garmin limits request bursts, so Pace spaces requests {interval_ms} ms apart: at least {duration} for {days} days. If Garmin still answers 'too many requests', Pace waits as asked and slows down."
    )
}

/// Progress of one menu import, redrawn after every committed day.
struct Progress {
    part: usize,
    parts: usize,
    window: (Date, Date),
    saved: u64,
    total: u64,
    last: Option<Date>,
}

impl App {
    pub(super) fn import_health(&mut self) -> Result<()> {
        let today: Date = chrono::Utc::now().date_naive().into();
        let per_day = (REQUESTS_PER_DAY * Settings::load(&self.config)?.garmin.request_interval_ms)
            .div_ceil(1000);
        let label = format!(
            "Garmin dates: START, or START END (YYYY-MM-DD). END defaults to today, {today} (UTC). Each day takes about {per_day} s: requests are paced to stay under Garmin's rate limit."
        );
        let Some(input) = self.view.prompt("Import health", &label, false)? else {
            return Ok(());
        };
        let (start, end) = parse_range(&input, today)?;
        let settings = Settings::load(&self.config)?;
        let max_days = settings.health.max_days;
        let windows = start.windows(end, max_days)?;
        let total = start.days_through(end);
        if windows.len() > 1 {
            let details = format!(
                "From {start} to {end}: {total} days in {} parts of up to {max_days} days.\nEach day reads all 8 health datasets from Garmin and replaces that day's saved snapshot.\n{}\nEsc stops after the current day; days already saved are kept.",
                windows.len(),
                pace_hint(settings.garmin.request_interval_ms, total)
            );
            let question = format!("Import {total} days of health data?");
            if !self.confirm("Import health", &question, &details)? {
                return Ok(());
            }
        }
        let secrets = Secrets::load(self.env_file.as_deref())?;
        let service = health::service(&settings)?;
        let _reporting = self.report_rate_waits()?;
        let mut progress = Progress {
            part: 0,
            parts: windows.len(),
            window: (start, end),
            saved: 0,
            total,
            last: None,
        };
        let mut outcome = Ok(ControlFlow::Continue(()));
        for (part, window) in windows.into_iter().enumerate() {
            progress.part = part + 1;
            progress.window = window;
            self.health_progress(&progress)?;
            outcome =
                health::sync_window(&service, &settings, &secrets, window.0, window.1, |day| {
                    progress.saved += 1;
                    progress.last = Some(day.date);
                    self.health_progress(&progress)?;
                    Ok(match self.view.terminal.pending_key()? {
                        Key::Escape => ControlFlow::Break(()),
                        _ => ControlFlow::Continue(()),
                    })
                });
            if !matches!(outcome, Ok(ControlFlow::Continue(()))) {
                break;
            }
        }
        // Discard typing during the import so it cannot confirm a later action.
        rustix::termios::tcflush(io::stdin(), rustix::termios::QueueSelector::IFlush)?;
        let Progress { saved, last, .. } = progress;
        let text = match outcome {
            Ok(ControlFlow::Continue(())) => {
                format!("Health import complete: {saved} days saved from {start} to {end}.")
            }
            Ok(ControlFlow::Break(())) => match last.and_then(Date::next) {
                Some(next) => format!(
                    "Stopped with Esc. {saved} of {total} days saved, through {}.\nTo continue, import from {next}.",
                    last.map_or_else(String::new, |d| d.to_string())
                ),
                None => format!("Stopped with Esc. {saved} of {total} days saved."),
            },
            Err(error) if error.is::<ExitRequested>() => return Err(error),
            Err(error) => {
                let resume = last
                    .map_or(Some(start), Date::next)
                    .map_or_else(String::new, |d| {
                        format!("; to continue, import from {d} {end}")
                    });
                return Err(error.context(format!(
                    "{saved} of {total} days saved before the failure; they are kept{resume}"
                )));
            }
        };
        self.view.result("Import health", &text)?;
        Ok(())
    }

    /// Show Garmin rate-limit waits on the footer row until the returned guard drops.
    pub(super) fn report_rate_waits(&mut self) -> Result<WaitReporting> {
        let footer = self.view.footer_writer()?;
        garmin::observe(Some(Box::new(move |pause: &RateWait| {
            footer(&format!(
                "Garmin rate limit: waiting {} s as Garmin asked (retry {}/{}); later requests slow down",
                pause.seconds, pause.attempt, pause.attempts
            ))
        })))?;
        Ok(WaitReporting)
    }

    fn health_progress(&mut self, progress: &Progress) -> Result<()> {
        let (from, to) = progress.window;
        let mut lines = vec![
            (
                format!("Part {}/{}: {from} to {to}", progress.part, progress.parts),
                Style::Plain,
            ),
            (
                format!("Saved {} of {} days", progress.saved, progress.total),
                Style::Plain,
            ),
        ];
        if let Some(last) = progress.last {
            lines.push((format!("Last saved: {last}"), Style::Plain));
        }
        lines.push((String::new(), Style::Plain));
        lines.push((
            "Requests are paced to stay under Garmin's rate limit.".into(),
            Style::Plain,
        ));
        lines.push(("Esc stops after the current day".into(), Style::Plain));
        self.view
            .frame("Import health", &lines, "Waiting for Garmin")
    }
}

/// Parse "START" or "START END"; a missing END means today.
pub(super) fn parse_range<T: FromStr>(input: &str, today: T) -> Result<(T, T)>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let mut parts = input.split_whitespace();
    let start = parts
        .next()
        .context("enter a start date as YYYY-MM-DD")?
        .parse()
        .context("invalid start date; use YYYY-MM-DD")?;
    let end = match parts.next() {
        Some(value) => value.parse().context("invalid end date; use YYYY-MM-DD")?,
        None => today,
    };
    ensure!(parts.next().is_none(), "enter at most two dates: START END");
    Ok((start, end))
}
