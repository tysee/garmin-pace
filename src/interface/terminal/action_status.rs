//! src/interface/terminal/action_status.rs projects local findings onto the existing Actions menu.
//! App::action_choices refreshes row states and next steps without network calls or storage creation.

use super::{
    App, Choice,
    actions::{self, ACTIONS, Action},
    setup_status::Mark,
};
use crate::{
    config::Settings,
    interface::{benchmarks, workouts},
    workouts::coach::Window,
};
use anyhow::Result;

/// A failed local read remains visible alongside the action that can explain it.
fn finding(result: Result<String>) -> String {
    result.unwrap_or_else(|error| format!("Unavailable: {error:#}"))
}

impl App {
    pub(super) fn action_choices(&mut self) -> Vec<Choice> {
        let config = self.config.clone();
        let checks = self.setup_checks(&config);
        let attention = checks
            .steps()
            .iter()
            .filter(|c| c.mark == Mark::Attention)
            .count();
        let settings = Settings::load(&config);
        let config_state = match &settings {
            Ok(_) => "Valid configuration".into(),
            Err(error) => format!("Unavailable: {error:#}"),
        };
        let mut choices = actions::choices();
        for (entry, choice) in ACTIONS.iter().zip(&mut choices) {
            let (state, next): (String, String) = match entry.action {
                Action::Onboarding => (
                    format!("{attention} setup checks need attention"),
                    "Open the setup checklist".into(),
                ),
                Action::Status => (
                    format!("{attention} checks need attention; local checks only"),
                    "Inspect findings and remedies".into(),
                ),
                Action::Activities => (
                    checks.activities.finding.clone(),
                    if checks.activities.mark == Mark::Ready {
                        "Browse sessions and recorded sets"
                    } else {
                        "Use Sync activities after profile setup"
                    }
                    .into(),
                ),
                Action::Sync => (
                    format!("Last import: {}", checks.last_sync.finding),
                    if checks.key.mark == Mark::Ready {
                        "Choose today, 7 days or a custom period"
                    } else {
                        "Set the Personal API key in Onboarding"
                    }
                    .into(),
                ),
                Action::Health => (
                    checks.health.finding.clone(),
                    "Open metrics, freshness or Import health".into(),
                ),
                Action::Login => (
                    checks.login.finding.clone(),
                    if checks.login.mark == Mark::Ready {
                        "Sign in again only to replace this session"
                    } else {
                        "Sign in for workouts and readiness"
                    }
                    .into(),
                ),
                Action::Devices => (
                    "Live device list; not fetched by this menu".into(),
                    if checks.login.mark == Mark::Ready {
                        "Fetch registered devices from Garmin"
                    } else {
                        "Use Garmin login first"
                    }
                    .into(),
                ),
                Action::AuthStatus => (
                    checks.login.finding.clone(),
                    "Inspect local session expiry; no connection test".into(),
                ),
                Action::Logout => (
                    checks.login.finding.clone(),
                    "Confirm removal of the local session".into(),
                ),
                Action::Workouts => (
                    finding(
                        settings
                            .as_ref()
                            .map_err(|e| anyhow::anyhow!("{e:#}"))
                            .and_then(|s| {
                                let rows = workouts::service(s)?.standalone(100, 0)?;
                                Ok(format!(
                                    "{}{} standalone workouts saved",
                                    rows.len(),
                                    if rows.len() == 100 { "+" } else { "" }
                                ))
                            }),
                    ),
                    "Browse steps; Sync from Garmin to refresh".into(),
                ),
                Action::Coach => (
                    finding(
                        settings
                            .as_ref()
                            .map_err(|e| anyhow::anyhow!("{e:#}"))
                            .and_then(|s| {
                                let window = Window::week_containing(
                                    chrono::Utc::now().date_naive().into(),
                                )?;
                                Ok(match workouts::service(s)?.coach(window)? {
                                    Some(snapshot) => format!(
                                        "{} sessions, {} to {}; imported {}",
                                        snapshot.sessions.len(),
                                        window.start,
                                        window.end,
                                        snapshot.synced_at
                                    ),
                                    None => {
                                        format!("{} to {}: not synced", window.start, window.end)
                                    }
                                })
                            }),
                    ),
                    "Open the week; Sync this week to refresh".into(),
                ),
                Action::Drafts => draft_status(&settings),
                Action::Benchmarks => (
                    finding(
                        settings
                            .as_ref()
                            .map_err(|e| anyhow::anyhow!("{e:#}"))
                            .and_then(|s| {
                                let rows = benchmarks::service(s)?.catalog()?;
                                let set =
                                    rows.iter().filter(|r| r.one_rep_max_kg.is_some()).count();
                                Ok(format!("{} exercises saved; {set} with 1RM", rows.len()))
                            }),
                    ),
                    "Sync strength data, then inspect evidence".into(),
                ),
                Action::ConfigCheck => {
                    (config_state.clone(), "Run configuration validation".into())
                }
                Action::ConfigShow => (
                    config_state.clone(),
                    "Inspect paths and non-secret settings".into(),
                ),
                Action::ConfigInit => (
                    finding(
                        config
                            .try_exists()
                            .map(|exists| {
                                if exists {
                                    "Config file exists; overwrite refused".into()
                                } else {
                                    "Config file absent".into()
                                }
                            })
                            .map_err(Into::into),
                    ),
                    if config.exists() {
                        "Use Check configuration for the existing file"
                    } else {
                        "Create the starter file at the configured path"
                    }
                    .into(),
                ),
                Action::Initialize => (
                    checks.profile.finding.clone(),
                    "Create storage or apply pending migrations".into(),
                ),
                Action::McpUsers => (
                    format!("{}; {}", checks.user.finding, checks.access_key.finding),
                    "Inspect users, keys and CLI management commands".into(),
                ),
                Action::Exit => (
                    "Close this menu".into(),
                    "Return to the terminal; separate MCP server keeps running".into(),
                ),
            };
            choice.description = state;
            choice.next_step = format!("Next: {next}.");
            choice.detail = format!(
                "{}\n\nLocal snapshot; no network request made by this menu.",
                entry.description
            );
        }
        choices
    }
}

fn draft_status(settings: &Result<Settings>) -> (String, String) {
    let result = (|| -> Result<_> {
        let settings = settings.as_ref().map_err(|e| anyhow::anyhow!("{e:#}"))?;
        Ok(workouts::service(settings)?.attention()?)
    })();
    match result {
        Ok(a) => (
            format!(
                "{} unresolved saves | {} unsaved drafts",
                a.pending, a.ready
            ),
            match a.next_id() {
                Some(id) if a.pending > 0 => format!("Open draft #{id} and reconcile its save"),
                Some(id) => format!("Open draft #{id}; review changes before saving"),
                None => "Ask the model for a workout when needed".into(),
            },
        ),
        Err(error) => (
            format!("Unavailable: {error:#}"),
            "Inspect Status before reviewing drafts".into(),
        ),
    }
}
