//! src/installation/supervisor.rs recognizes what keeps MCP running without a terminal.
//! detect_supervisor finds a Podman container or the installer's systemd unit; service_state
//! reads that unit through systemctl and start_service enables it now and at boot via sudo.

use anyhow::{Context, Result, bail};
use std::{fs, io::ErrorKind, path::Path, process::Command};

/// Podman writes this file into every container it starts.
const CONTAINER_ENV: &str = "/run/.containerenv";
/// The unit deploy/install.sh installs; keep in sync with its UNIT variable.
const UNIT_PATH: &str = "/etc/systemd/system/garmin-pace.service";
pub const UNIT_NAME: &str = "garmin-pace.service";

/// The process supervisor available to this menu.
#[derive(Debug, PartialEq)]
pub enum Supervisor {
    /// Running inside Podman; `image` is the image name Podman recorded, when present.
    Container { image: Option<String> },
    /// The installer's systemd unit exists on this host.
    Systemd,
    /// Nothing here restarts MCP; only foreground serving is possible.
    None,
}

pub fn detect_supervisor() -> Result<Supervisor> {
    match fs::read_to_string(CONTAINER_ENV) {
        Ok(text) => {
            let image = text.lines().find_map(|line| {
                line.strip_prefix("image=")
                    .map(|value| value.trim_matches('"').to_string())
                    .filter(|value| !value.is_empty())
            });
            return Ok(Supervisor::Container { image });
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("reading container environment"),
    }
    if Path::new(UNIT_PATH).try_exists()? {
        return Ok(Supervisor::Systemd);
    }
    Ok(Supervisor::None)
}

/// systemd's own words for the unit, e.g. "active" and "enabled".
pub struct ServiceState {
    pub active: String,
    pub enabled: String,
}

fn systemctl(verb: &str) -> Result<String> {
    // is-active and is-enabled report through stdout and exit non-zero for "inactive"/"disabled".
    let output = Command::new("systemctl")
        .args([verb, UNIT_NAME])
        .output()
        .context("running systemctl")?;
    let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if state.is_empty() {
        bail!("systemctl {verb} {UNIT_NAME} gave no state");
    }
    Ok(state)
}

pub fn service_state() -> Result<ServiceState> {
    Ok(ServiceState {
        active: systemctl("is-active")?,
        enabled: systemctl("is-enabled")?,
    })
}

/// Enable and start the unit with sudo on the caller's terminal, so sudo can ask for a password.
pub fn start_service() -> Result<()> {
    let status = match Command::new("sudo")
        .args(["systemctl", "enable", "--now", UNIT_NAME])
        .status()
    {
        Ok(status) => status,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            bail!(
                "sudo is not installed; as an administrator run: systemctl enable --now {UNIT_NAME}"
            )
        }
        Err(error) => return Err(error).context("running sudo"),
    };
    if !status.success() {
        bail!(
            "sudo systemctl enable --now {UNIT_NAME} failed; this account may lack sudo rights. Ask an administrator to run it"
        );
    }
    Ok(())
}
