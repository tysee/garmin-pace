//! src/interface/terminal/status.rs shows the local state at a glance: the Status screen lists
//! the self-check findings grouped by area with a mark per row, Enter explains a row and names
//! the action that changes it, and App::status_context builds the main menu's one-line summary.
//! Everything here reads local files only; nothing is fetched from Garmin.

use super::{
    App,
    setup_status::{Check, Checks, Mark, moment},
    view::{Choice, Selection},
};
use crate::{
    config::Settings,
    history::{HistoryService, SqliteHistory},
};
use anyhow::Result;
use std::sync::Arc;

/// One Status row: its area heading, label and where the user acts on it.
struct Row<'a> {
    section: &'static str,
    label: &'static str,
    hint: &'static str,
    check: &'a Check,
}

const fn row<'a>(
    section: &'static str,
    label: &'static str,
    hint: &'static str,
    check: &'a Check,
) -> Row<'a> {
    Row {
        section,
        label,
        hint,
        check,
    }
}

/// Rows in reading order: local data, then Garmin access, then MCP. Three headings and nine
/// rows fit a 24-row terminal without scrolling. The server config is this profile's config
/// here, so the profile row already covers it.
fn rows(checks: &Checks) -> [Row<'_>; 9] {
    [
        row(
            "Local data",
            "Config and database",
            "Onboarding step 1 prepares them; Check configuration explains config errors",
            &checks.profile,
        ),
        row(
            "Local data",
            "Activities",
            "Sync activities imports a recent page or a date period",
            &checks.activities,
        ),
        row(
            "Local data",
            "Last sync",
            "Sync activities records it after a whole import succeeds",
            &checks.last_sync,
        ),
        row(
            "Local data",
            "Health days",
            "Onboarding step 5 imports a date range",
            &checks.health,
        ),
        row(
            "Garmin",
            "Personal API key",
            "Onboarding step 2 finds the saved key or takes a new one",
            &checks.key,
        ),
        row(
            "Garmin",
            "Connect login",
            "Garmin login signs in; readiness and workouts need it",
            &checks.login,
        ),
        row(
            "MCP",
            "MCP user",
            "Onboarding step 7 registers this profile",
            &checks.user,
        ),
        row(
            "MCP",
            "Access key",
            "Onboarding step 8 issues one; MCP users lists them",
            &checks.access_key,
        ),
        row(
            "MCP",
            "Serving",
            "Onboarding step 10 checks MCP and starts it",
            &checks.start,
        ),
    ]
}

impl App {
    /// Browse the local self-check; Enter on a row shows its full finding and remedy.
    pub(super) fn status(&mut self) -> Result<()> {
        let mut selection = Selection::default();
        loop {
            // Re-check on every return so the rows describe the current files.
            let config = self.config.clone();
            let checks = self.setup_checks(&config);
            let rows = rows(&checks);
            let choices = rows
                .iter()
                .map(|row| Choice {
                    icon: row.check.mark.icon(),
                    section: Some(row.section),
                    ..Choice::new(row.label, row.check.finding.clone())
                })
                .collect::<Vec<_>>();
            let ready = rows
                .iter()
                .filter(|row| row.check.mark == Mark::Ready)
                .count();
            self.view.context = format!(
                "Profile: {} | {ready}/{} ready | Esc back",
                self.config.display(),
                rows.len()
            );
            let Some(index) = self.view.select("Status", &choices, &mut selection, true)? else {
                return Ok(());
            };
            let row = &rows[index];
            self.view.result(
                "Status",
                &format!(
                    "{}\n{} {}\n\nNext: {}.\n\nStatus reads local files only; nothing was fetched from Garmin.",
                    row.label,
                    row.check.mark.icon(),
                    row.check.finding,
                    row.hint
                ),
            )?;
        }
    }

    /// The main menu's summary line: stored activities and the last successful sync.
    pub(super) fn status_context(&self) -> Result<String> {
        let settings = Settings::load(&self.config)?;
        let history = HistoryService::new(
            Arc::new(SqliteHistory::open(settings.database.clone())?),
            settings.history.max_page_size,
        );
        let summary = history.summary()?;
        let sync = summary
            .last_sync
            .map_or_else(|| "never".into(), |sync| moment(&sync.completed_at));
        Ok(format!(
            "{} activities | last sync {sync}",
            summary.activity_count
        ))
    }
}
