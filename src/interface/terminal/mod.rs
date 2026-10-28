//! src/interface/terminal/mod.rs composes an interactive edge over the CLI dispatcher.
//! run opens menus and onboarding, picks up the profile's saved Garmin key unless --env-file
//! was given, displays command results and hands confirmed MCP to the CLI.

mod action_status;
mod actions;
mod activities;
mod benchmarks;
mod brand;
mod coach;
mod delivery;
mod freshness;
mod full_sync;
mod health_metrics;
mod https;
mod network;
mod quick_setup;
mod setup;
mod setup_access;
mod setup_health;
mod setup_serve;
mod setup_status;
mod setup_sync;
mod status;
mod sync_shortcuts;
mod text;
mod tty;
mod view;
mod workouts;

pub(super) use quick_setup::{quick_setup, quick_setup_batch};

use actions::{ACTIONS, Action};
use anyhow::{Result, ensure};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};
use tty::{ExitRequested, Terminal};
use view::{Choice, Column, Selection, Style, View, Width};

use super::{
    cli::{self, Command},
    output::Output,
};
use crate::{
    config::Settings,
    garmin::ConnectApi,
    workouts::{Device, WorkoutGateway},
};

// Cap captured command output independently of the configured activity page size.
const MAX_OUTPUT_BYTES: usize = 262_144;
const DEVICES: &[Column] = &[
    Column::new("Device", Width::Fill),
    Column::new("Device ID", Width::Fit(14)),
];

pub(super) fn run(
    config: PathBuf,
    env_file: Option<PathBuf>,
    compact: bool,
    setup: bool,
) -> Result<()> {
    let terminal = Terminal::open()?;
    let mut app = App {
        view: View::new(terminal, compact),
        config,
        explicit_env_file: env_file.is_some(),
        env_file,
        pending_server: None,
    };
    let result = if setup { app.onboarding() } else { app.menu() };
    app.view.terminal.suspend()?;
    match result {
        Err(error) if error.is::<ExitRequested>() => Ok(()),
        Ok(()) => match app.pending_server {
            Some(config) => cli::execute(
                &config,
                None,
                Command::Serve {
                    transport: cli::Transport::Http,
                },
                Output {
                    json: false,
                    writer: &mut io::stdout(),
                },
            ),
            None => Ok(()),
        },
        other => other,
    }
}

struct App {
    view: View,
    config: PathBuf,
    /// Credential file for Garmin commands; None reads only the process environment.
    env_file: Option<PathBuf>,
    /// An explicit --env-file is never replaced by the profile's saved key file.
    explicit_env_file: bool,
    pending_server: Option<PathBuf>,
}

impl App {
    fn menu(&mut self) -> Result<()> {
        let mut selection = Selection::default();
        let mut last_action = None;
        self.refresh_status();
        loop {
            let choices = self.action_choices();
            let Some(index) = self
                .view
                .select("Actions", &choices, &mut selection, true)?
            else {
                return Ok(());
            };
            let (action, title) = (ACTIONS[index].action, ACTIONS[index].label);
            if last_action != Some(action) {
                self.view.remember_inputs(action != Action::Onboarding);
                last_action = Some(action);
            }
            if action == Action::Exit {
                return Ok(());
            }
            if let Err(error) = self.perform_normal(action, title) {
                if error.is::<ExitRequested>() {
                    return Err(error);
                }
                self.view
                    .result("Action failed", &format!("{title}\n\n{error:#}"))?;
            }
            if self.pending_server.is_some() {
                return Ok(());
            }
            self.refresh_status();
        }
    }

    /// Select the profile's saved key file when it exists, unless --env-file was given.
    fn resolve_credentials(&mut self) -> Result<()> {
        if self.explicit_env_file {
            return Ok(());
        }
        self.env_file = None;
        if self.config.try_exists()? {
            let settings = Settings::load(&self.config)?;
            self.env_file = crate::secrets::profile_key_file(None, &settings.database.path)?;
        }
        Ok(())
    }

    fn refresh_status(&mut self) {
        // Config errors that block key lookup also block the summary; Status explains both.
        let status = self
            .resolve_credentials()
            .and_then(|()| self.status_context());
        self.view.context = match status {
            Ok(text) => text,
            Err(_) => {
                "Local status unavailable - select Status for details; Setup actions below".into()
            }
        };
    }

    fn perform(&mut self, action: Action, title: &str) -> Result<()> {
        if action == Action::Onboarding {
            return self.onboarding();
        }
        if action == Action::Status {
            return self.status();
        }
        if action == Action::McpUsers {
            let config = self.config.clone();
            return self.mcp_users(&config);
        }
        if action == Action::FullSync {
            return self.full_sync();
        }
        if action == Action::Sync {
            match self.sync_mode(title)? {
                None => return Ok(()),
                Some(true) => return self.import_period(title),
                Some(false) => {}
            }
        }
        self.execute_action(action, title)
    }

    fn perform_normal(&mut self, action: Action, title: &str) -> Result<()> {
        if action != Action::Sync {
            return self.perform(action, title);
        }
        match self.activity_sync_shortcuts(title)? {
            None => Ok(()),
            Some(true) => self.import_period(title),
            Some(false) => self.execute_action(action, title),
        }
    }

    fn execute_action(&mut self, action: Action, title: &str) -> Result<()> {
        let input = if let Some(label) = action.prompt() {
            let count = action == Action::Sync;
            let label = if count {
                let settings = Settings::load(&self.config)?;
                format!(
                    "How many recent activities to import (1-{})? Enter = {}",
                    settings.history.max_page_size, settings.history.default_page_size
                )
            } else {
                label.into()
            };
            let Some(input) = self.view.prompt(title, &label, !count)? else {
                return Ok(());
            };
            input
        } else {
            String::new()
        };
        if action == Action::Activities {
            return self.activities();
        }
        if action == Action::Devices {
            return self.garmin_devices();
        }
        if action == Action::Coach {
            return self.coach_menu();
        }
        if action == Action::Workouts {
            return self.workout_library();
        }
        if action == Action::Benchmarks {
            return self.benchmark_menu();
        }
        if action == Action::Health {
            return self.health_menu();
        }
        if action == Action::Drafts {
            return self.workout_drafts();
        }
        if action == Action::GarminNetwork {
            return self.garmin_network();
        }
        if action == Action::InternetAccess {
            return self.internet_access();
        }
        if let Some(question) = action.confirmation()
            && !self.confirm(title, question, question)?
        {
            return Ok(());
        }
        let command = action.command(input)?;
        self.view.frame(
            title,
            &[("Working...".into(), Style::Plain)],
            "Waiting for the selected operation",
        )?;
        let login = action == Action::Login;
        if login {
            self.view.terminal.suspend()?;
        }
        let result = capture(&self.config, self.env_file.as_deref(), command);
        if login {
            self.view.terminal.resume()?;
            self.view.invalidate();
        }
        // Discard typing during an operation so it cannot confirm a later action.
        rustix::termios::tcflush(io::stdin(), rustix::termios::QueueSelector::IFlush)?;
        self.view.result(title, &result?).map(|_| ())
    }

    fn confirm(&mut self, title: &str, question: &str, details: &str) -> Result<bool> {
        // Show all details in a scrollable preview before offering the two choices.
        let review = if question == details {
            question.into()
        } else {
            format!("{question}\n\n{details}")
        };
        if !self.view.review("Review action", &review)? {
            return Ok(false);
        }
        let choices = vec![
            Choice::new("Cancel", "Return without making changes"),
            Choice::new("Confirm", question),
        ];
        Ok(self
            .view
            .select(title, &choices, &mut Selection::default(), false)?
            == Some(1))
    }

    /// Run one dispatcher command behind a waiting frame, then discard typing made meanwhile
    /// so it cannot confirm a later action. The inner result is the command's own outcome.
    fn run_command(
        &mut self,
        title: &str,
        waiting: &str,
        footer: &str,
        command: Command,
    ) -> Result<Result<String>> {
        self.view
            .frame(title, &[(waiting.into(), Style::Plain)], footer)?;
        let result = capture(&self.config, self.env_file.as_deref(), command);
        rustix::termios::tcflush(io::stdin(), rustix::termios::QueueSelector::IFlush)?;
        Ok(result)
    }

    /// Run a command with the generic working frame and show its output, or its failure
    /// locally. Returns whether the command succeeded.
    fn show_command(&mut self, title: &str, command: Command) -> Result<bool> {
        let result = self.run_command(
            title,
            "Working...",
            "Waiting for the selected operation",
            command,
        )?;
        let Some(text) = self.recover(title, result)? else {
            return Ok(false);
        };
        self.view.result(title, &text)?;
        Ok(true)
    }

    /// Present an action failure locally; never retry work or swallow terminal exit.
    fn recover<T>(&mut self, title: &str, outcome: Result<T>) -> Result<Option<T>> {
        match outcome {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.is::<ExitRequested>() => Err(error),
            Err(error) => {
                rustix::termios::tcflush(io::stdin(), rustix::termios::QueueSelector::IFlush)?;
                self.view.result("Action failed", &format!("{title}\n\n{error:#}\n\nNo automatic retry. Return to inspect the saved state or correct the input."))?;
                Ok(None)
            }
        }
    }

    /// Read the registered devices once and show them as a table until the user leaves.
    fn garmin_devices(&mut self) -> Result<()> {
        let devices = self.read_devices()?;
        let choices = device_rows(&devices, "Open device details");
        let mut state = Selection::default();
        while let Some(index) =
            self.view
                .table("Garmin devices", DEVICES, &choices, &mut state, true)?
        {
            let device = &devices[index];
            self.view.result(
                "Garmin device",
                &format!(
                    "{}\nDevice ID: {}\nWorkouts are delivered to a device by its ID from a workout's Send to watch.",
                    device.model, device.id
                ),
            )?;
        }
        Ok(())
    }

    fn read_devices(&mut self) -> Result<Vec<Device>> {
        self.view.frame(
            "Garmin devices",
            &[("Reading Garmin devices...".into(), Style::Plain)],
            "Waiting for Garmin",
        )?;
        let settings = Settings::load(&self.config)?;
        let api = ConnectApi::connect(&settings.garmin, &settings.garmin_auth)?;
        let devices = api.devices()?;
        ensure!(!devices.is_empty(), "No registered Garmin devices.");
        Ok(devices)
    }

    fn device(&mut self) -> Result<Option<Device>> {
        let mut devices = self.read_devices()?;
        let choices = device_rows(&devices, "Deliver the workout to this device");
        Ok(self
            .view
            .table(
                "Select device",
                DEVICES,
                &choices,
                &mut Selection::default(),
                false,
            )?
            .map(|index| devices.swap_remove(index)))
    }
}

fn device_rows(devices: &[Device], description: &str) -> Vec<Choice> {
    devices
        .iter()
        .map(|device| {
            Choice::row(
                vec![device.model.clone(), device.id.to_string()],
                description,
            )
        })
        .collect()
}

fn capture(config: &Path, env_file: Option<&Path>, command: Command) -> Result<String> {
    let mut output = BoundedOutput(Vec::new());
    cli::execute(
        config,
        env_file,
        command,
        Output {
            json: false,
            writer: &mut output,
        },
    )?;
    Ok(String::from_utf8(output.0)?)
}

struct BoundedOutput(Vec<u8>);

impl Write for BoundedOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.len() + bytes.len() > MAX_OUTPUT_BYTES {
            return Err(io::Error::other(
                "terminal output limit exceeded; use a smaller page or a direct CLI command",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
