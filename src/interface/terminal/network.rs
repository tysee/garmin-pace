//! src/interface/terminal/network.rs shows and changes the Garmin proxy and extra CA file.
//! Saving runs `pace config network`, which edits only those keys; the screen also runs
//! `pace doctor` on request. Nothing here contacts Garmin unless the user asks for the check.

use super::{App, Choice, Selection};
use crate::{
    config::{self, GarminNetwork},
    interface::cli::{Command, ConfigCommand},
};
use anyhow::Result;
use std::path::PathBuf;

const TITLE: &str = "Garmin network";

fn proxy(network: &GarminNetwork) -> String {
    network.proxy_url.as_deref().map_or_else(
        || "Not set: HTTPS_PROXY/ALL_PROXY/NO_PROXY from the environment apply".into(),
        |url| format!("{url}; replaces the environment's proxy"),
    )
}
fn ca(network: &GarminNetwork) -> String {
    network.extra_ca_file.as_ref().map_or_else(
        || "Not set: the platform trust store only".into(),
        |path| format!("{}; added to the platform trust store", path.display()),
    )
}

impl App {
    pub(super) fn garmin_network(&mut self) -> Result<()> {
        let mut state = Selection::default();
        loop {
            let current = config::garmin_network(&self.config);
            let Some(current) = self.recover(TITLE, current)? else {
                return Ok(());
            };
            let choices = [
                Choice::new("Proxy URL", proxy(&current)),
                Choice::new("Extra CA file", ca(&current)),
                Choice::new(
                    "Check Garmin connection",
                    "Run pace doctor: route, trust and reachability per Garmin host",
                ),
            ];
            match self.view.select(TITLE, &choices, &mut state, false)? {
                None => return Ok(()),
                Some(0) => {
                    let Some(value) = self.view.prompt(
                        "Proxy URL",
                        "http(s)://host[:port], no credentials (blank = use environment)",
                        false,
                    )?
                    else {
                        continue;
                    };
                    let value = value.trim().to_owned();
                    self.network_command(ConfigCommand::Network {
                        clear_proxy: value.is_empty(),
                        proxy_url: (!value.is_empty()).then_some(value),
                        extra_ca_file: None,
                        clear_ca: false,
                    })?;
                }
                Some(1) => {
                    let Some(value) = self.view.prompt(
                        "Extra CA file",
                        "PEM file path, relative to the configuration (blank = none)",
                        false,
                    )?
                    else {
                        continue;
                    };
                    let value = value.trim().to_owned();
                    self.network_command(ConfigCommand::Network {
                        proxy_url: None,
                        clear_proxy: false,
                        clear_ca: value.is_empty(),
                        extra_ca_file: (!value.is_empty()).then(|| PathBuf::from(value)),
                    })?;
                }
                Some(_) => self.network_run("Check Garmin connection", Command::Doctor)?,
            }
        }
    }

    fn network_command(&mut self, action: ConfigCommand) -> Result<()> {
        self.network_run(TITLE, Command::Config { action })
    }

    fn network_run(&mut self, title: &str, command: Command) -> Result<()> {
        self.show_command(title, command).map(|_| ())
    }
}
