//! src/interface/garmin.rs defines the explicit network-facing Garmin CLI commands.
//! It composes adapters with history use cases and emits bounded status summaries;
//! sync_period lets the terminal import a date period page by page with progress.

use super::output::Output;
use crate::{
    config::Settings,
    garmin::PersonalApi,
    history::{
        ActivityPeriod, ActivitySource, HistoryService, PeriodProgress, PeriodSync, SqliteHistory,
    },
    secrets::Secrets,
};
use anyhow::{Context, Result, ensure};
use chrono::{NaiveDate, SecondsFormat, Utc};
use clap::Subcommand;
use serde_json::json;
use std::{
    io::{self, Write},
    ops::ControlFlow,
    sync::Arc,
};

/// Garmin network operations are explicit and separate from offline history reads.
#[derive(Subcommand)]
pub(super) enum GarminCommand {
    /// Check Personal API authentication and read access without saving activities.
    Check,
    /// Sign in interactively, verify device access and save refreshable tokens.
    Login,
    /// Report local native-session expiry without exposing tokens.
    AuthStatus,
    /// Delete the local native session (does not revoke the server session).
    Logout,
    /// List registered devices using the saved native Garmin Connect session.
    Devices,
    /// Import one recent activity page, or every activity of a date period with --start.
    Sync {
        #[arg(long, conflicts_with = "start")]
        limit: Option<usize>,
        /// First local start date to import (YYYY-MM-DD); the period is read page by page.
        #[arg(long)]
        start: Option<NaiveDate>,
        /// Last local start date to import (YYYY-MM-DD); defaults to today in UTC.
        #[arg(long, requires = "start")]
        end: Option<NaiveDate>,
    },
}

pub(super) fn run(
    command: GarminCommand,
    settings: &Settings,
    secrets: &crate::secrets::Secrets,
    output: Output<'_>,
) -> Result<()> {
    match command {
        GarminCommand::Login => {
            eprint!("Garmin email: ");
            io::stderr().flush()?;
            let mut email = String::new();
            io::stdin().read_line(&mut email)?;
            let email = email.trim();
            ensure!(!email.is_empty(), "email is required");
            let password = rpassword::prompt_password("Garmin password: ")?;
            crate::garmin::login(
                &settings.garmin_auth,
                &settings.garmin,
                email,
                &password,
                || Ok(rpassword::prompt_password("Garmin MFA code: ")?),
            )?;
            output.message(
                &json!({"authenticated":true,"session_saved":true}),
                "Garmin login successful.\nSession saved. Access tokens refresh automatically.",
            )
        }
        GarminCommand::AuthStatus => {
            let expires = crate::garmin::session_expiry(&settings.garmin_auth)?;
            let expiry =
                chrono::DateTime::from_timestamp(expires, 0).context("invalid session expiry")?;
            let state = if expires <= Utc::now().timestamp() {
                "expired; renewal required on next request"
            } else {
                "valid by local expiry"
            };
            output.message(
                &json!({"session_present":true,"expires_at_unix":expires,"expired":expires<=Utc::now().timestamp(),"live_access":"unchecked"}),
                &format!("Saved Garmin session: {state}\nAccess token expires: {expiry}\nLive Garmin access: not checked"),
            )
        }
        GarminCommand::Logout => {
            crate::garmin::logout(&settings.garmin_auth)?;
            output.message(
                &json!({"local_session_deleted":true,"server_session_revoked":false}),
                "Local Garmin session deleted. The server session has not been revoked.",
            )
        }
        GarminCommand::Devices => {
            use crate::workouts::WorkoutGateway;
            let api = crate::garmin::ConnectApi::connect(&settings.garmin, &settings.garmin_auth)?;
            output.devices(api.devices()?)
        }
        GarminCommand::Check => {
            let api = PersonalApi::connect(
                &settings.garmin,
                secrets.require("GARMIN_PERSONAL_API_KEY")?,
            )?;
            let page = api.recent(1)?;
            output.message(
                &json!({"authenticated":true, "read_access":true, "sample_count":page.len()}),
                &format!(
                    "Garmin Personal API: connected.\nActivity read access verified ({} returned).",
                    page.len()
                ),
            )
        }
        GarminCommand::Sync {
            start: Some(start),
            end,
            ..
        } => {
            let period =
                ActivityPeriod::new(start, end.unwrap_or_else(|| Utc::now().date_naive()))?;
            let result = sync_period(settings, secrets, period, |_| ControlFlow::Continue(()))?;
            let summary = result
                .completed
                .context("period import stopped before its last page")?;
            output.message(
                &json!({"saved_count":result.progress.saved, "pages":result.progress.pages,
                "start":period.start().to_string(), "end":period.end().to_string(),
                "last_successful_sync_at":summary.completed_at, "scope":"period"}),
                &format!("Imported {} activities from {} to {} ({} pages).\nLast sync: {}\nScope: period", result.progress.saved, period.start(), period.end(), result.progress.pages, summary.completed_at),
            )
        }
        GarminCommand::Sync { limit, .. } => {
            let limit = limit.unwrap_or(settings.history.default_page_size);
            ensure!(
                (1..=settings.history.max_page_size).contains(&limit),
                "sync limit is outside configured bounds"
            );
            let repository = SqliteHistory::open(settings.database.clone())?;
            let history = HistoryService::new(Arc::new(repository), settings.history.max_page_size);
            let api = PersonalApi::connect(
                &settings.garmin,
                secrets.require("GARMIN_PERSONAL_API_KEY")?,
            )?;
            let summary = history.sync(&api, limit, || {
                Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
            })?;
            output.message(
                &json!({"saved_count":summary.received_count, "requested_limit":summary.requested_limit,
                "last_successful_sync_at":summary.completed_at, "scope":"recent_page"}),
                &format!("Imported {} activities (requested up to {}).\nLast sync: {}\nScope: recent page", summary.received_count, summary.requested_limit, summary.completed_at),
            )
        }
    }
}

/// Import every activity of a period page by page, reporting progress after each page.
pub(super) fn sync_period(
    settings: &Settings,
    secrets: &Secrets,
    period: ActivityPeriod,
    on_page: impl FnMut(PeriodProgress) -> ControlFlow<()>,
) -> Result<PeriodSync> {
    let repository = SqliteHistory::open(settings.database.clone())?;
    let history = HistoryService::new(Arc::new(repository), settings.history.max_page_size);
    let api = PersonalApi::connect(
        &settings.garmin,
        secrets.require("GARMIN_PERSONAL_API_KEY")?,
    )?;
    Ok(history.sync_period(&api, period, on_page, || {
        Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
    })?)
}
