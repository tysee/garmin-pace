//! src/interface/terminal/setup_status.rs computes the local self-check behind Onboarding
//! marks and the Status screen. App::setup_checks returns one mark and finding per area;
//! Checks::steps orders them by onboarding step. Checks read local files only: they never
//! initialize missing storage and never call the network.

use super::App;
use crate::{
    config::Settings,
    history::{HistoryService, SqliteHistory},
    interface::{health, users},
    secrets::Secrets,
};
use anyhow::Result;
use std::{path::Path, sync::Arc};

/// Whether a step needs the user's attention.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Mark {
    Ready,
    /// Usable but not fully established: a key no import has used yet, an expired access
    /// token, or a full sync that did not cover its whole period.
    Partial,
    Attention,
    /// Reference steps that have no state to check.
    Info,
}

impl Mark {
    // √ and × exist in Linux console fonts, unlike ✓ and ✗.
    pub(super) fn icon(self) -> &'static str {
        match self {
            Mark::Ready => "√",
            Mark::Partial => "~",
            Mark::Attention => "×",
            Mark::Info => " ",
        }
    }

    /// Whether the credential behind the check can be used, possibly after a renewal.
    pub(super) fn usable(self) -> bool {
        matches!(self, Mark::Ready | Mark::Partial)
    }
}

pub(super) struct Check {
    pub mark: Mark,
    pub finding: String,
}

/// Every local finding, named by area so screens can group them their own way.
pub(super) struct Checks {
    pub profile: Check,
    pub key: Check,
    pub login: Check,
    pub activities: Check,
    pub last_sync: Check,
    pub health: Check,
    /// The latest journaled full sync and whether every step covered its period.
    pub full_sync: Check,
    pub server: Check,
    pub user: Check,
    pub access_key: Check,
    /// The connection-details step has no state; it is listed for completeness only.
    pub reference: Check,
    pub start: Check,
}

impl Checks {
    /// Findings in onboarding step order; the sync finding belongs to the import step.
    pub(super) fn steps(&self) -> [&Check; 10] {
        [
            &self.profile,
            &self.key,
            &self.login,
            &self.activities,
            &self.health,
            &self.server,
            &self.user,
            &self.access_key,
            &self.reference,
            &self.start,
        ]
    }
}

fn ready(finding: impl Into<String>) -> Check {
    Check {
        mark: Mark::Ready,
        finding: finding.into(),
    }
}

fn partial(finding: impl Into<String>) -> Check {
    Check {
        mark: Mark::Partial,
        finding: finding.into(),
    }
}

fn attention(finding: impl Into<String>) -> Check {
    Check {
        mark: Mark::Attention,
        finding: finding.into(),
    }
}

/// A failed check is a finding the user must see, not a reason to hide the menu.
fn reported(result: Result<Check>) -> Check {
    result.unwrap_or_else(|error| attention(format!("{error:#}")))
}

fn date(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0).map_or_else(
        || "unknown date".into(),
        |d| d.format("%Y-%m-%d").to_string(),
    )
}

impl App {
    /// Check every onboarding area against local files only.
    pub(super) fn setup_checks(&mut self, server: &Path) -> Checks {
        let (profile_check, profile) = self.profile_check();
        let key = reported(self.key_check(profile.as_ref()));
        let (server_check, server_settings) = match Settings::load(server) {
            Ok(settings) if server == self.config => {
                (ready("Uses this profile's config"), Some(settings))
            }
            Ok(settings) => (ready(format!("Uses {}", server.display())), Some(settings)),
            Err(error) => (attention(format!("{error:#}")), None),
        };
        let [
            login,
            activities,
            last_sync,
            health,
            full_sync,
            user,
            access_key,
        ] = match &profile {
            None => std::array::from_fn(|_| attention("Prepare the profile first")),
            Some(profile) => {
                let (user, name) = match &server_settings {
                    None => (attention("Select a valid server config first"), None),
                    Some(server) => user_check(server, profile)
                        .unwrap_or_else(|error| (attention(format!("{error:#}")), None)),
                };
                let access_key = match (&server_settings, name) {
                    (Some(server), Some(name)) => reported(key_check(server, &name)),
                    _ => attention("Register an MCP user first"),
                };
                let (activities, last_sync) = history_checks(profile).unwrap_or_else(|error| {
                    let finding = format!("{error:#}");
                    (attention(finding.clone()), attention(finding))
                });
                [
                    reported(login_check(profile)),
                    activities,
                    last_sync,
                    reported(health_check(profile)),
                    reported(full_sync_check(profile)),
                    user,
                    access_key,
                ]
            }
        };
        let serving = [&profile_check, &server_check, &user, &access_key]
            .iter()
            .all(|check| check.mark == Mark::Ready);
        let start = match &server_settings {
            Some(settings) if serving => ready(format!("Ready to serve on {}", settings.http.bind)),
            _ => attention("Needs the profile, server config, an MCP user and an access key"),
        };
        Checks {
            profile: profile_check,
            key,
            login,
            activities,
            last_sync,
            health,
            full_sync,
            server: server_check,
            user,
            access_key,
            reference: Check {
                mark: Mark::Info,
                finding: "Client setup reference".into(),
            },
            start,
        }
    }

    fn profile_check(&self) -> (Check, Option<Settings>) {
        let result = (|| -> Result<(Check, Option<Settings>)> {
            if !self.config.try_exists()? {
                return Ok((attention("Config not created yet"), None));
            }
            let settings = Settings::load(&self.config)?;
            if !settings.database.path.try_exists()? {
                return Ok((attention("Database not initialized yet"), None));
            }
            SqliteHistory::open(settings.database.clone())?;
            Ok((ready("Config and database ready"), Some(settings)))
        })();
        result.unwrap_or_else(|error| (attention(format!("{error:#}")), None))
    }

    /// A saved key is verified once an activity import succeeded after it was last written;
    /// a key from the environment cannot be dated, so the last import stands for it.
    fn key_check(&mut self, profile: Option<&Settings>) -> Result<Check> {
        self.resolve_credentials()?;
        let Some(source) = self.key_source()? else {
            return Ok(attention("No key saved for this profile"));
        };
        let last_sync = match profile {
            Some(profile) => HistoryService::new(
                Arc::new(SqliteHistory::open(profile.database.clone())?),
                profile.history.max_page_size,
            )
            .summary()?
            .last_sync
            .map(|sync| sync.completed_at),
            None => None,
        };
        let Some(completed) = last_sync else {
            return Ok(partial(format!(
                "Found in {source}; no import has used it yet"
            )));
        };
        let accepted = chrono::DateTime::parse_from_rfc3339(&completed)?;
        let written = match &self.env_file {
            Some(path) => Some(chrono::DateTime::<chrono::Utc>::from(
                std::fs::metadata(path)?.modified()?,
            )),
            None => None,
        };
        Ok(match written {
            Some(written) if written > accepted => partial(format!(
                "Found in {source}; changed after the last import ({}), not used yet",
                moment(&completed)
            )),
            _ => ready(format!(
                "Found in {source}; accepted by Garmin in the import of {}",
                moment(&completed)
            )),
        })
    }

    /// Describe where Garmin commands will read the key, or None when it is missing.
    pub(super) fn key_source(&self) -> Result<Option<String>> {
        let secrets = Secrets::load(self.env_file.as_deref())?;
        if secrets
            .optional("GARMIN_PERSONAL_API_KEY")
            .is_none_or(str::is_empty)
        {
            return Ok(None);
        }
        Ok(Some(match &self.env_file {
            Some(path) => path.display().to_string(),
            None => "the GARMIN_PERSONAL_API_KEY environment variable".into(),
        }))
    }
}

fn login_check(profile: &Settings) -> Result<Check> {
    Ok(
        match crate::garmin::saved_session_expiry(&profile.garmin_auth)? {
            None => attention("Not signed in"),
            Some(expires) if expires > chrono::Utc::now().timestamp() => {
                ready(format!("Signed in; token valid until {}", date(expires)))
            }
            Some(_) => {
                partial("Signed in; access token expired, renews on the next Garmin request")
            }
        },
    )
}

/// Stored activities and the last successful sync, as two separate findings.
fn history_checks(profile: &Settings) -> Result<(Check, Check)> {
    let history = HistoryService::new(
        Arc::new(SqliteHistory::open(profile.database.clone())?),
        profile.history.max_page_size,
    );
    let summary = history.summary()?;
    let activities = if summary.activity_count == 0 {
        attention("No activities saved yet")
    } else {
        // The newest stored start date tells how fresh the local copy is.
        let newest = history.list(1, 0)?.into_iter().next().map(|a| a.started_at);
        ready(match newest {
            Some(started) => format!("{} saved; newest {}", summary.activity_count, day(&started)),
            None => format!("{} saved", summary.activity_count),
        })
    };
    let last_sync = match summary.last_sync {
        // Period imports store the page size as the limit, so only the count is shown.
        Some(sync) => ready(format!(
            "{}, {} activities received",
            moment(&sync.completed_at),
            sync.received_count
        )),
        None => attention("Never synced"),
    };
    Ok((activities, last_sync))
}

/// Calendar date of an RFC 3339 timestamp, which starts with it.
fn day(timestamp: &str) -> &str {
    timestamp.get(..10).unwrap_or(timestamp)
}

/// RFC 3339 timestamp as a readable UTC minute, falling back to the raw text.
pub(super) fn moment(timestamp: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(timestamp).map_or_else(
        |_| timestamp.to_string(),
        |t| t.to_utc().format("%Y-%m-%d %H:%M UTC").to_string(),
    )
}

fn health_check(profile: &Settings) -> Result<Check> {
    let summary = health::service(profile)?.summary()?;
    Ok(match summary.latest {
        Some(latest) => ready(format!("{} days saved; latest {latest}", summary.day_count)),
        None => attention("No health days saved yet"),
    })
}

/// The latest full sync: complete, or which step did not cover its period and why.
fn full_sync_check(profile: &Settings) -> Result<Check> {
    use crate::sync::{RunState, SqliteSyncJournal, StepState, SyncService};
    let journal = SqliteSyncJournal::open(profile.database.clone())?;
    let Some(overview) = SyncService::new(&journal).overview()? else {
        return Ok(attention("Never run"));
    };
    let run = &overview.run;
    let first_gap = overview.steps.iter().find_map(|step| match &step.state {
        StepState::Complete => None,
        StepState::Partial { reason }
        | StepState::Failed { reason }
        | StepState::Skipped { reason } => Some(format!("{}: {reason}", step.step.label())),
    });
    Ok(match (run.state, first_gap) {
        (RunState::Complete, _) => ready(format!(
            "{} complete, {}",
            run.period,
            moment(run.finished_at.as_deref().unwrap_or(&run.started_at))
        )),
        (RunState::Running, _) => partial(format!(
            "{} started {}; still running or interrupted",
            run.period,
            moment(&run.started_at)
        )),
        (RunState::Stopped, _) => partial(format!("{} stopped; continue it", run.period)),
        (RunState::Partial, gap) => partial(format!(
            "{} partial; {}",
            run.period,
            gap.unwrap_or_else(|| "a step did not finish".into())
        )),
    })
}

/// The registry is absent until the first user is registered; that is not an error.
fn user_check(server: &Settings, profile: &Settings) -> Result<(Check, Option<String>)> {
    if !server.access.registry_path.try_exists()? {
        return Ok((attention("No MCP users registered yet"), None));
    }
    let access = users::service(server)?;
    Ok(match users::profile_user(&access, profile)? {
        None => (attention("No MCP user linked to this profile"), None),
        Some(user) if !user.enabled => (attention(format!("User {} is disabled", user.name)), None),
        Some(user) => (ready(format!("User {}", user.name)), Some(user.name)),
    })
}

fn key_check(server: &Settings, user: &str) -> Result<Check> {
    let now = chrono::Utc::now().timestamp();
    let active: Vec<_> = users::service(server)?
        .keys(user)?
        .into_iter()
        .filter(|key| !key.revoked && key.expires_at > now)
        .collect();
    Ok(match active.iter().map(|key| key.expires_at).min() {
        Some(next) => ready(format!(
            "{} active; next expires {}",
            active.len(),
            date(next)
        )),
        None => attention("No active access key"),
    })
}
