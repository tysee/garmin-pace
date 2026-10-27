//! src/interface/terminal/setup.rs guides explicit profile and first-import setup.
//! App's onboarding screen marks each step from a local self-check, finds the profile's saved
//! Garmin key automatically and otherwise takes a hidden pasted key, reusing CLI commands.

use super::{
    App,
    actions::Action,
    capture,
    setup_status::Mark,
    tty::ExitRequested,
    view::{Choice, Selection, Style},
};
use crate::{
    config::Settings,
    interface::{
        cli::{Command, ConfigCommand},
        garmin::GarminCommand,
    },
};
use anyhow::{Context, Result, ensure};
use std::{io, path::PathBuf};

const STEPS: &[(&str, &str)] = &[
    (
        "1. Prepare profile",
        "Create or validate this config, then initialize its database",
    ),
    (
        "2. Personal API key",
        "Find the saved Garmin key, or paste a new one hidden",
    ),
    (
        "3. Garmin login",
        "Connect session for readiness and workouts; separate from the API key",
    ),
    (
        "4. Import activities",
        "Load a bounded recent page, not the complete archive",
    ),
    (
        "5. Import health",
        "Load an explicit date range; requires API key and Garmin login",
    ),
    (
        "6. MCP server config",
        "Select the server config; initially uses this profile's config",
    ),
    (
        "7. Register MCP user",
        "Prepare the registry and link this profile to a new username",
    ),
    (
        "8. Issue access key",
        "Pick a registered user; a new expiring key is shown once to copy",
    ),
    (
        "9. Connection details",
        "Client setup, HTTPS, key rotation and continuous operation",
    ),
    (
        "10. Start MCP",
        "Check MCP; start the service or show how to run it without a terminal",
    ),
];

impl App {
    pub(super) fn onboarding(&mut self) -> Result<()> {
        let mut selection = Selection::default();
        let mut server = self.config.clone();
        loop {
            // Re-check after every step so the marks always describe the current files.
            let checks = self.setup_checks(&server);
            let steps = checks.steps();
            let choices = STEPS
                .iter()
                .zip(steps)
                .map(|((label, help), check)| Choice {
                    icon: check.mark.icon(),
                    ..Choice::new(*label, format!("{}. {help}", check.finding))
                })
                .collect::<Vec<_>>();
            let checked = steps.iter().filter(|c| c.mark != Mark::Info);
            let ready = checked.clone().filter(|c| c.mark == Mark::Ready).count();
            self.view.context = format!(
                "Profile: {} | {ready}/{} ready | Esc back",
                self.config.display(),
                checked.count()
            );
            let Some(step) = self
                .view
                .select("Onboarding", &choices, &mut selection, true)?
            else {
                return Ok(());
            };
            if let Err(error) = self.setup_step(step, &mut server) {
                if error.is::<ExitRequested>() {
                    return Err(error);
                }
                self.view.result("Setup step failed", &format!("{}\n\n{error:#}\n\nCompleted steps are retained. Fix the problem and retry this step.", STEPS[step].0))?;
            }
            if self.pending_server.is_some() {
                return Ok(());
            }
        }
    }

    fn setup_step(&mut self, step: usize, server: &mut PathBuf) -> Result<()> {
        match step {
            0 => self.prepare_profile(),
            1 => self.personal_key(),
            2 => self.perform(Action::Login, "Garmin login"),
            3 => self.perform(Action::Sync, "Import activities"),
            4 => self.import_health(),
            5 => self.server_config(server),
            6 => self.register_profile(server),
            7 => self.issue_access_key(server),
            8 => self.connection_details(server),
            9 => self.start_mcp(server),
            _ => unreachable!("selection comes from the fixed setup steps"),
        }
    }

    fn prepare_profile(&mut self) -> Result<()> {
        // Existing files are validated, never replaced; create_new handles races safely.
        if self.config.try_exists()? {
            Settings::load(&self.config)?;
        } else {
            capture(
                &self.config,
                None,
                Command::Config {
                    action: ConfigCommand::Init,
                },
            )?;
        }
        let text = capture(&self.config, None, Command::Init)?;
        self.view.result("Prepare profile", &format!("Profile ready.\nConfig: {}\n{text}\n\nNext: Personal API key, then import data.\nUse a separate profile directory for each Garmin account.", self.config.display()))?;
        Ok(())
    }

    fn personal_key(&mut self) -> Result<()> {
        self.resolve_credentials()?;
        let settings = Settings::load(&self.config).context("prepare the profile first")?;
        ensure!(
            settings.database.path.try_exists()?,
            "prepare the profile first; the key is saved beside its database"
        );
        match self.key_source()? {
            Some(source) => self.found_key(&source),
            None => {
                let path = crate::secrets::profile_key_path(&settings.database.path)?;
                self.enter_garmin_key(&settings, &path)?;
                if self.env_file.is_some() {
                    self.view.result("Personal API key", &format!("Garmin accepted the key. It was saved hidden to a private file:\n{}\n\nPace finds this file automatically on every launch.\nNext: import activities. For readiness and workouts, use Garmin login.", path.display()))?;
                }
                Ok(())
            }
        }
    }

    fn found_key(&mut self, source: &str) -> Result<()> {
        let replace = match &self.env_file {
            Some(path) if !self.explicit_env_file => format!(
                "To replace it, delete {} and open this step again.",
                path.display()
            ),
            Some(_) => "It was selected with --env-file for this launch.".into(),
            None => "Unset the variable to save a key for this profile instead.".into(),
        };
        let choices = [
            Choice::new("Keep this key", "Return to onboarding"),
            Choice::new(
                "Check with Garmin",
                "Read one recent activity to confirm access",
            ),
        ];
        if !self.view.result("Personal API key", &format!("Personal API key found.\nSource: {source}\n\n{replace}\n\nEnter: continue; Esc: back"))? {
            return Ok(());
        }
        if self.view.select(
            "Personal API key",
            &choices,
            &mut Selection::default(),
            false,
        )? != Some(1)
        {
            return Ok(());
        }
        self.view.frame(
            "Personal API key",
            &[("Checking access to Garmin...".into(), Style::Plain)],
            "Waiting for Garmin",
        )?;
        let result = capture(
            &self.config,
            self.env_file.as_deref(),
            Command::Garmin {
                action: GarminCommand::Check,
            },
        );
        rustix::termios::tcflush(io::stdin(), rustix::termios::QueueSelector::IFlush)?;
        self.view.result("Personal API key", &result?)?;
        Ok(())
    }

    pub(super) fn setup_path(&mut self, title: &str, label: &str) -> Result<Option<PathBuf>> {
        self.view
            .setup_input(title, label, false)?
            .map(|value| {
                ensure!(
                    !value.trim().is_empty(),
                    "an explicit file path is required"
                );
                Ok(PathBuf::from(value))
            })
            .transpose()
    }
}
