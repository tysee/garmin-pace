//! src/interface/terminal/actions.rs maps menu selections into existing CLI commands.
//! Action metadata, section grouping, icons and parameter prompts live here; domain validation stays in features.

use super::view::Choice;
use crate::interface::{
    cli::{Command, ConfigCommand},
    garmin::GarminCommand,
};
use anyhow::{Context, Result, bail};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Action {
    Onboarding,
    Status,
    Activities,
    FullSync,
    Sync,
    Health,
    Login,
    Devices,
    AuthStatus,
    Workouts,
    Coach,
    Drafts,
    Benchmarks,
    ConfigCheck,
    ConfigShow,
    ConfigInit,
    GarminNetwork,
    InternetAccess,
    Initialize,
    McpUsers,
    Logout,
    Exit,
}

/// One main-menu row: the CLI action it maps to, its section heading, icon and texts.
pub(super) struct Entry {
    pub action: Action,
    pub section: Option<&'static str>,
    pub icon: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

const fn entry(
    section: Option<&'static str>,
    icon: &'static str,
    action: Action,
    label: &'static str,
    description: &'static str,
) -> Entry {
    Entry {
        action,
        section,
        icon,
        label,
        description,
    }
}

const HISTORY: Option<&str> = Some("History");
const GARMIN: Option<&str> = Some("Garmin");
const TRAINING: Option<&str> = Some("Training");
const CONFIGURATION: Option<&str> = Some("Configuration");
const TERMINAL: Option<&str> = Some("Terminal");

// Icons stay single-cell symbols present in Linux console and desktop terminal fonts.
pub(super) const ACTIONS: &[Entry] = &[
    entry(
        None,
        "»",
        Action::Onboarding,
        "Onboarding",
        "Set up a profile, first imports and MCP access",
    ),
    entry(
        HISTORY,
        "≡",
        Action::Status,
        "Status",
        "Local data, Garmin access and MCP readiness at a glance",
    ),
    entry(
        HISTORY,
        "•",
        Action::Activities,
        "Activities",
        "Browse stored activities page by page",
    ),
    entry(
        HISTORY,
        "▼",
        Action::FullSync,
        "Sync everything",
        "Every Garmin dataset for a period in one resumable run",
    ),
    entry(
        HISTORY,
        "↓",
        Action::Sync,
        "Sync activities",
        "Import a recent page from Garmin",
    ),
    entry(
        HISTORY,
        "☼",
        Action::Health,
        "Health",
        "Saved health datasets, their days and samples",
    ),
    entry(
        GARMIN,
        "→",
        Action::Login,
        "Garmin login",
        "Sign in with email, password and MFA",
    ),
    entry(
        GARMIN,
        "♦",
        Action::Devices,
        "Garmin devices",
        "Registered devices and their IDs, read from Garmin",
    ),
    entry(
        GARMIN,
        "●",
        Action::AuthStatus,
        "Session status",
        "Inspect the saved session locally",
    ),
    entry(
        GARMIN,
        "←",
        Action::Logout,
        "Garmin logout",
        "Delete the local Garmin session",
    ),
    entry(
        TRAINING,
        "♥",
        Action::Workouts,
        "Workouts",
        "Standalone workout library; create or edit with a model",
    ),
    entry(
        TRAINING,
        "▦",
        Action::Coach,
        "Coach",
        "Plans and weekly schedule; review model adjustments",
    ),
    entry(
        TRAINING,
        "○",
        Action::Drafts,
        "Workout drafts",
        "Review model changes and save to Garmin",
    ),
    entry(
        TRAINING,
        "▲",
        Action::Benchmarks,
        "Benchmark exercises",
        "Observed strength averages and model estimates",
    ),
    entry(
        CONFIGURATION,
        "√",
        Action::ConfigCheck,
        "Check configuration",
        "Validate the configured TOML file",
    ),
    entry(
        CONFIGURATION,
        "§",
        Action::ConfigShow,
        "Show configuration",
        "Inspect non-secret settings",
    ),
    entry(
        CONFIGURATION,
        "⇄",
        Action::GarminNetwork,
        "Garmin network",
        "Proxy and extra CA certificates for Garmin requests",
    ),
    entry(
        CONFIGURATION,
        "@",
        Action::InternetAccess,
        "Internet access (HTTPS)",
        "Public HTTPS address for MCP and its certificate check",
    ),
    entry(
        CONFIGURATION,
        "+",
        Action::ConfigInit,
        "Create configuration",
        "Create a starter file without overwrite",
    ),
    entry(
        CONFIGURATION,
        "■",
        Action::Initialize,
        "Initialize database",
        "Create storage or apply migrations",
    ),
    entry(
        CONFIGURATION,
        "☺",
        Action::McpUsers,
        "MCP users",
        "Registered users and their access keys",
    ),
    entry(
        TERMINAL,
        "×",
        Action::Exit,
        "Exit",
        "Return to the terminal",
    ),
];

pub(super) fn choices() -> Vec<Choice> {
    ACTIONS
        .iter()
        .map(|entry| Choice {
            icon: entry.icon,
            section: entry.section,
            ..Choice::new(entry.label, entry.description)
        })
        .collect()
}

impl Action {
    pub(super) fn prompt(self) -> Option<&'static str> {
        match self {
            Self::Sync => Some("Count"),
            _ => None,
        }
    }

    pub(super) fn confirmation(self) -> Option<&'static str> {
        match self {
            Self::Logout => Some("Delete the local session? Garmin access is not revoked."),
            _ => None,
        }
    }

    pub(super) fn command(self, input: String) -> Result<Command> {
        let limit = || -> Result<Option<usize>> {
            if input.trim().is_empty() {
                Ok(None)
            } else {
                Ok(Some(
                    input
                        .trim()
                        .parse()
                        .context("limit must be a positive integer")?,
                ))
            }
        };
        Ok(match self {
            Self::Sync => Command::Garmin {
                action: GarminCommand::Sync {
                    limit: limit()?,
                    start: None,
                    end: None,
                },
            },
            Self::Login => Command::Garmin {
                action: GarminCommand::Login,
            },
            Self::AuthStatus => Command::Garmin {
                action: GarminCommand::AuthStatus,
            },
            Self::Logout => Command::Garmin {
                action: GarminCommand::Logout,
            },
            Self::ConfigCheck => Command::Config {
                action: ConfigCommand::Check,
            },
            Self::ConfigShow => Command::Config {
                action: ConfigCommand::Show,
            },
            Self::ConfigInit => Command::Config {
                action: ConfigCommand::Init,
            },
            Self::Initialize => Command::Init,
            Self::Exit
            | Self::Onboarding
            | Self::Status
            | Self::FullSync
            | Self::Activities
            | Self::Devices
            | Self::Coach
            | Self::Workouts
            | Self::Drafts
            | Self::Benchmarks
            | Self::Health
            | Self::GarminNetwork
            | Self::InternetAccess
            | Self::McpUsers => {
                bail!("this action opens a menu")
            }
        })
    }
}
