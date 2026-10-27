//! src/interface/terminal/quick_setup.rs runs the installer's sequential, path-free setup.
//! It composes profile initialization, optional Garmin imports and per-user key issuance.

use super::{
    App, BoundedOutput,
    actions::Action,
    capture,
    tty::{ExitRequested, Terminal},
    view::{Choice, Selection, Style, View},
};
use crate::{
    access::validate_name,
    config::Settings,
    interface::{
        cli::{Command, ConfigCommand},
        garmin::{self, GarminCommand},
        output::Output,
        users::{self, KeyDestination, UsersCommand},
    },
    secrets::Secrets,
};
use anyhow::{Context, Result, ensure};
use std::{
    io,
    path::{Path, PathBuf},
};

pub(in crate::interface) fn quick_setup(
    config: PathBuf,
    user: String,
    server: PathBuf,
    handoff: Option<PathBuf>,
    compact: bool,
) -> Result<()> {
    validate_name(&user)?;
    crate::installation::validate_listener(&Settings::load(&server)?)?;
    let mut app = App {
        view: View::new(Terminal::open()?, compact),
        config,
        env_file: None,
        explicit_env_file: false,
        pending_server: None,
    };
    let result = app.install_profile(&user, &server, handoff.as_deref());
    app.view.terminal.suspend()?;
    match result {
        Err(error) if error.is::<ExitRequested>() => {
            anyhow::bail!("setup cancelled; run the installer again to continue")
        }
        other => other,
    }
}

impl App {
    fn install_profile(&mut self, user: &str, server: &Path, handoff: Option<&Path>) -> Result<()> {
        self.view.context = format!("Profile: {user} | Installation wizard");
        ensure!(self.view.result("Easy setup", "Pace will prepare your profile and a private MCP access key.\n\nYou can connect Garmin now or do it later. Saved files are kept when you return. No file paths or system commands are needed.\n\nEnter to continue; Esc to cancel.")?, "setup cancelled");
        self.view.frame(
            "Prepare profile",
            &[("Preparing private storage...".into(), Style::Plain)],
            "Step 1 of 4",
        )?;
        let directory = self.prepare_install_profile()?;
        let settings = Settings::load(&self.config)?;
        self.quick_garmin(&settings)?;
        self.quick_imports()?;
        self.view.frame(
            "MCP access",
            &[(
                "Preparing your private connection key...".into(),
                Style::Plain,
            )],
            "Step 4 of 4",
        )?;
        let key_id = self.provision_access(user, server, &directory, handoff)?;
        ensure!(self.view.result("Profile configured", &format!("Your profile and private MCP key are ready.\n\nKey ID: {}\nThe key itself is never displayed.\n\nPress Enter to let the installer start and verify the service, then deliver your connection file. The installer can configure Internet HTTPS next.", key_id))?, "setup cancelled before service startup");
        Ok(())
    }

    fn prepare_install_profile(&self) -> Result<PathBuf> {
        let directory = self
            .config
            .parent()
            .context("profile directory required")?
            .to_path_buf();
        crate::private_fs::directory(&directory)?;
        if !self.config.try_exists()? {
            capture(
                &self.config,
                None,
                Command::Config {
                    action: ConfigCommand::Init,
                },
            )?;
        }
        capture(&self.config, None, Command::Init)?;
        Ok(directory)
    }

    fn provision_access(
        &self,
        user: &str,
        server: &Path,
        directory: &Path,
        handoff: Option<&Path>,
    ) -> Result<String> {
        capture(
            server,
            None,
            Command::Users {
                action: UsersCommand::Init,
            },
        )?;
        let server_settings = Settings::load(server)?;
        users::register_profile(&server_settings, user, &self.config, true)?;
        let issued = users::issue_key(
            &server_settings,
            user,
            KeyDestination::Directory(directory.canonicalize()?),
            None,
        )?;
        crate::installation::save_receipt(&self.config, &issued.path)?;
        if let Some(path) = handoff {
            crate::installation::handoff_key(&issued.path, path)?;
        }
        Ok(issued.id)
    }

    fn choose(&mut self, title: &str, yes: &str, no: &str, description: &str) -> Result<bool> {
        let choices = [
            Choice::new(yes, description),
            Choice::new(no, "Continue without this step; you can return later"),
        ];
        self.view
            .select(title, &choices, &mut Selection::default(), false)?
            .map(|choice| choice == 0)
            .context("setup cancelled")
    }

    fn quick_garmin(&mut self, settings: &Settings) -> Result<()> {
        let path = crate::secrets::profile_key_path(&settings.database.path)?;
        let saved = path.try_exists()?;
        if !self.choose(
            "Garmin connection",
            if saved {
                "Use saved Garmin key"
            } else {
                "Connect Garmin now"
            },
            "Connect later",
            "Use your Personal API key to read activities and health data",
        )? {
            return Ok(());
        }
        if saved {
            Secrets::load(Some(&path))?.require("GARMIN_PERSONAL_API_KEY")?;
            self.env_file = Some(path);
            return Ok(());
        }
        self.enter_garmin_key(settings, &path)
    }

    /// Take a hidden pasted key, check it with Garmin, then save it to a new private file.
    pub(super) fn enter_garmin_key(&mut self, settings: &Settings, path: &Path) -> Result<()> {
        loop {
            if !self.view.result("Get Garmin key", "On your phone or computer, open Garmin Connect settings and create a Personal API key:\nhttps://connect.garmin.com/app/settings/personalApiToken\n\nCopy it, press Enter here, then paste it into hidden input. Pace checks it before saving.\nThis is different from the MCP key Pace creates for your client.")? { return Ok(()); }
            let Some(key) =
                self.view
                    .setup_input("Garmin key", "Paste Personal API key (hidden)", true)?
            else {
                return Ok(());
            };
            // Copy artifacts around a pasted key are not part of it.
            let key = key.trim();
            let result = self.check_personal_key(settings, key);
            match result {
                Ok(()) => {
                    Secrets::save_personal_key(path, key)?;
                    self.env_file = Some(path.to_path_buf());
                    return Ok(());
                }
                Err(error) => {
                    self.view.result(
                        "Connection failed",
                        &format!("{error:#}\n\nThe key was not saved."),
                    )?;
                    if !self.choose(
                        "Garmin connection",
                        "Try another key",
                        "Connect later",
                        "Check the Garmin key and your network connection",
                    )? {
                        return Ok(());
                    }
                }
            }
        }
    }

    fn check_personal_key(&mut self, settings: &Settings, key: &str) -> Result<()> {
        let secrets = Secrets::personal_key(key)?;
        self.view.frame(
            "Garmin connection",
            &[("Checking access to Garmin...".into(), Style::Plain)],
            "Waiting for Garmin",
        )?;
        let mut output = BoundedOutput(Vec::new());
        let result = garmin::run(
            GarminCommand::Check,
            settings,
            &secrets,
            Output {
                json: false,
                writer: &mut output,
            },
        );
        rustix::termios::tcflush(io::stdin(), rustix::termios::QueueSelector::IFlush)?;
        result
    }

    fn quick_imports(&mut self) -> Result<()> {
        if self.env_file.is_none() {
            return Ok(());
        }
        let settings = Settings::load(&self.config)?;
        if self.choose(
            "First download",
            "Load recent activities",
            "Download later",
            &format!(
                "Read up to {} recent activities from Garmin",
                settings.history.default_page_size
            ),
        )? {
            self.initial_import(Command::Garmin {
                action: GarminCommand::Sync {
                    limit: None,
                    start: None,
                    end: None,
                },
            })?;
        }
        if self.choose(
            "Health and recovery",
            "Sign in to Garmin",
            "Set up health later",
            "A separate Garmin login is needed for readiness and workouts",
        )? {
            match self.perform(Action::Login, "Garmin login") {
                Ok(()) => {
                    if self.choose(
                        "Health download",
                        "Choose dates to download",
                        "Download later",
                        "Select a Garmin calendar-date range",
                    )? {
                        self.import_health()?;
                    }
                }
                Err(error) if error.is::<ExitRequested>() => return Err(error),
                Err(error) => {
                    self.view.result("Login incomplete", &format!("{error:#}\n\nHealth setup can be resumed later. Activity data is preserved."))?;
                }
            }
        }
        Ok(())
    }

    fn initial_import(&mut self, command: Command) -> Result<()> {
        self.view.frame(
            "First download",
            &[("Downloading recent activities...".into(), Style::Plain)],
            "Waiting for Garmin",
        )?;
        let result = capture(&self.config, self.env_file.as_deref(), command);
        rustix::termios::tcflush(io::stdin(), rustix::termios::QueueSelector::IFlush)?;
        match result {
            Ok(text) => {
                self.view.result("Activities saved", &text)?;
            }
            Err(error) => {
                self.view.result(
                    "Download incomplete",
                    &format!(
                        "{error:#}\n\nExisting data is preserved. You can retry the download later."
                    ),
                )?;
            }
        }
        Ok(())
    }
}
