//! src/interface/terminal/https.rs shows the managed HTTPS ingress (bundled Caddy) in front of MCP.
//! Changing it needs root, which this menu lacks: a system install runs it as the service account.
//! The screen reads the installer's recorded domain and unit state, runs the existing certificate
//! and DNS-path check on request, validates a new domain and names the administrator command that
//! applies it. Nothing here writes system files or contacts the network unless the user asks.

use super::{App, Choice, Selection, Style, capture};
use crate::{
    installation::{self, HTTPS_UNIT_NAME, PublicDomain, SYSTEM_CONFIG, ServiceState, Supervisor},
    interface::cli::Command,
};
use anyhow::Result;
use std::path::Path;

const TITLE: &str = "Internet access (HTTPS)";
const APPLY: &str = "This menu runs as the Pace service account and cannot change system files.\nAs an administrator with sudo, in the unpacked release folder, run:\n\n  ./install.sh --https";
const PREREQUISITES: &str = "Before applying: DNS A/AAAA records point to this device, the router forwards TCP 80 and 443, and port 8080 stays private.";

/// The installed setup in the installer's terms; `state` is read only for a configured domain.
fn summary(domain: Option<&PublicDomain>, state: Option<&ServiceState>) -> String {
    match (domain, state) {
        (Some(domain), Some(state)) => format!(
            "Domain: {}\nMCP URL: {}\n{HTTPS_UNIT_NAME}: {}, {} at boot\n\nCaddy renews the certificate itself. Logs: journalctl -u {HTTPS_UNIT_NAME}",
            domain.as_str(),
            domain.mcp_url(),
            state.active,
            state.enabled
        ),
        _ => format!(
            "HTTPS is not configured on this host. MCP answers only on its loopback listener.\n\n{APPLY}\n\nand answer yes to Internet access. {PREREQUISITES}"
        ),
    }
}

impl App {
    pub(super) fn internet_access(&mut self) -> Result<()> {
        let supervisor = installation::detect_supervisor();
        let Some(supervisor) = self.recover(TITLE, supervisor)? else {
            return Ok(());
        };
        match supervisor {
            Supervisor::Systemd => self.https_screen(),
            Supervisor::Container { .. } => self.view.result(TITLE, "Not available in a container. Managed HTTPS needs the Linux system install from deploy/install.sh.\n\nHere, publish the container port only on 127.0.0.1 and put your own HTTPS reverse proxy in front of it.").map(drop),
            Supervisor::None => self.view.result(TITLE, "No Pace system service on this host. Managed HTTPS (bundled Caddy) needs the Linux system install from deploy/install.sh; a user-only install has no service and no HTTPS.\n\nPublic access always needs HTTPS in front of http.bind; never expose port 8080 directly.").map(drop),
        }
    }

    fn https_screen(&mut self) -> Result<()> {
        let mut selection = Selection::default();
        loop {
            let domain = installation::installed_https_domain();
            let Some(domain) = self.recover(TITLE, domain)? else {
                return Ok(());
            };
            let state = match &domain {
                Some(_) => {
                    let state = installation::service_state(HTTPS_UNIT_NAME);
                    let Some(state) = self.recover(TITLE, state)? else {
                        return Ok(());
                    };
                    Some(state)
                }
                None => None,
            };
            let current = domain
                .as_ref()
                .map_or("Not configured".into(), PublicDomain::mcp_url);
            let choices = [
                Choice::new("Current setup", current),
                Choice::new(
                    "Check HTTPS",
                    "Trusted certificate, local key check and DNS path; no key leaves this device",
                ),
                Choice::new(
                    if domain.is_some() {
                        "Change domain"
                    } else {
                        "Set up HTTPS"
                    },
                    "Validate a public domain; an administrator applies it with the installer",
                ),
            ];
            match self.view.select(TITLE, &choices, &mut selection, false)? {
                None => return Ok(()),
                Some(0) => {
                    self.view
                        .result(TITLE, &summary(domain.as_ref(), state.as_ref()))?;
                }
                Some(1) => match &domain {
                    Some(domain) => self.https_check(domain)?,
                    None => {
                        self.view.result(TITLE, &summary(None, None))?;
                    }
                },
                Some(_) => self.https_domain(domain.as_ref())?,
            }
        }
    }

    fn https_check(&mut self, domain: &PublicDomain) -> Result<()> {
        self.view.frame(
            "Check HTTPS",
            &[(
                format!("Checking https://{}...", domain.as_str()),
                Style::Plain,
            )],
            "Waiting for the certificate and DNS-path check",
        )?;
        let result = capture(
            Path::new(SYSTEM_CONFIG),
            self.env_file.as_deref(),
            Command::InstallationCheck {
                profile: self.config.clone(),
                https_domain: Some(domain.as_str().into()),
            },
        );
        rustix::termios::tcflush(std::io::stdin(), rustix::termios::QueueSelector::IFlush)?;
        if let Some(text) = self.recover("Check HTTPS", result)? {
            self.view.result(
                "Check HTTPS",
                &format!("√ {text}\nMCP URL: {}", domain.mcp_url()),
            )?;
        }
        Ok(())
    }

    fn https_domain(&mut self, current: Option<&PublicDomain>) -> Result<()> {
        let hint = current.map_or_else(
            || "for example mcp.your-domain.com".to_owned(),
            |domain| format!("current: {}", domain.as_str()),
        );
        let Some(value) = self.view.prompt("Public domain", &hint, false)? else {
            return Ok(());
        };
        let parsed = PublicDomain::parse(value.trim());
        let Some(domain) = self.recover("Public domain", parsed)? else {
            return Ok(());
        };
        if current == Some(&domain) {
            self.view.result(
                TITLE,
                &format!(
                    "{} is already configured. Choose Check HTTPS to verify it.",
                    domain.as_str()
                ),
            )?;
            return Ok(());
        }
        self.view.result(TITLE, &format!("√ {} is a valid public hostname.\nMCP URL after setup: {}\n\n{APPLY}\n\nand enter {} at the Domain prompt. {PREREQUISITES}\nThen return here and choose Check HTTPS.", domain.as_str(), domain.mcp_url(), domain.as_str()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_names_the_url_and_unit_or_the_installer_command() -> Result<()> {
        let domain = PublicDomain::parse("MCP.Example.org")?;
        let state = ServiceState {
            active: "active".into(),
            enabled: "enabled".into(),
        };
        let text = summary(Some(&domain), Some(&state));
        assert!(text.contains("MCP URL: https://mcp.example.org/api/v1/mcp"));
        assert!(text.contains("garmin-pace-https.service: active, enabled at boot"));
        let text = summary(None, None);
        assert!(text.contains("not configured") && text.contains("./install.sh --https"));
        Ok(())
    }
}
