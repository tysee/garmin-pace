//! src/interface/terminal/setup_access.rs maps onboarding inputs to local access commands.
//! It tabulates MCP users and their keys, issues a key shown once on screen (only its
//! hash is stored), and explains client deployment.

use super::{
    App, capture,
    setup_status::Mark,
    view::{Choice, Column, Selection, Width},
};
use crate::{
    access::KeyInfo,
    config::Settings,
    interface::{
        cli::Command,
        users::{self, UserKeys, UsersCommand},
    },
};
use anyhow::{Result, ensure};
use std::path::{Path, PathBuf};

const USERS: &[Column] = &[
    Column::new("User", Width::Fill),
    Column::new("State", Width::Fit(8)),
    Column::new("Keys", Width::Fit(36)),
];
const KEYS: &[Column] = &[
    Column::new("Key ID", Width::Fill),
    Column::new("State", Width::Fit(7)),
    Column::new("Expires", Width::Fit(10)),
];

fn date(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0).map_or_else(
        || "unknown date".into(),
        |d| d.format("%Y-%m-%d").to_string(),
    )
}

/// Whether a user can connect now, and a one-line description of its keys.
fn key_state(entry: &UserKeys, now: i64) -> (Mark, String) {
    if !entry.user.enabled {
        return (Mark::Attention, "disabled; its keys are blocked".into());
    }
    let active: Vec<_> = entry
        .keys
        .iter()
        .filter(|key| !key.revoked && key.expires_at > now)
        .collect();
    match active.iter().map(|key| key.expires_at).min() {
        Some(next) => (
            Mark::Ready,
            format!(
                "{} active key(s); next expires {}",
                active.len(),
                date(next)
            ),
        ),
        None => (Mark::Attention, "no active key".into()),
    }
}

impl App {
    pub(super) fn server_config(&mut self, server: &mut PathBuf) -> Result<()> {
        if !self.view.result("MCP server config", &format!("Current server config: {}\n\nFor your first user, this may be the same as the current profile. For multiple users, select the same server config for each separate Garmin profile.\n\nEnter: select an existing server config; Esc: keep current.", server.display()))? { return Ok(()); }
        let Some(path) = self.setup_path("MCP server config", "Existing server config path")?
        else {
            return Ok(());
        };
        Settings::load(&path)?;
        *server = path;
        self.view.result(
            "MCP server config",
            &format!("Server config selected: {}", server.display()),
        )?;
        Ok(())
    }

    pub(super) fn register_profile(&mut self, server: &Path) -> Result<()> {
        let Some(user) = self.view.prompt(
            "Register MCP user",
            "Username (letters, digits, _ or -)",
            false,
        )?
        else {
            return Ok(());
        };
        let details = format!(
            "User: {user}\nGarmin profile: {}\nServer config: {}\n\nOnly this profile's saved data will be accessible. Connect the intended Garmin account before registration. Existing users should proceed to Issue access key.",
            self.config.display(),
            server.display()
        );
        if !self.confirm(
            "Register MCP user",
            "Register this profile for MCP access?",
            &details,
        )? {
            return Ok(());
        }
        capture(
            server,
            None,
            Command::Users {
                action: UsersCommand::Init,
            },
        )?;
        let text = capture(
            server,
            None,
            Command::Users {
                action: UsersCommand::Add {
                    user,
                    profile: self.config.clone(),
                },
            },
        )?;
        self.view.result("Register MCP user", &text)?;
        Ok(())
    }

    pub(super) fn issue_access_key(&mut self, server: &Path) -> Result<()> {
        let settings = Settings::load(server)?;
        let entries = users::overview(&settings)?;
        ensure!(
            !entries.is_empty(),
            "no MCP users yet; register one in step 7 first"
        );
        let now = chrono::Utc::now().timestamp();
        let choices: Vec<_> = entries
            .iter()
            .map(|entry| {
                let (mark, state) = key_state(entry, now);
                Choice {
                    icon: mark.icon(),
                    ..Choice::new(entry.user.name.clone(), state)
                }
            })
            .collect();
        let Some(index) = self.view.select(
            "Issue access key",
            &choices,
            &mut Selection::default(),
            false,
        )?
        else {
            return Ok(());
        };
        let name = entries[index].user.name.clone();
        let details = format!(
            "User: {name}\nThe new key expires in {} days. It is shown once on the next screen and saved nowhere; Pace keeps only its hash. Existing keys stay valid until they expire or are revoked.",
            settings.access.key_lifetime_days
        );
        if !self.confirm(
            "Issue access key",
            &format!("Issue a new key for {name}?"),
            &details,
        )? {
            return Ok(());
        }
        let key = users::issue_revealed_key(&settings, &name)?;
        let text = format!(
            "New MCP key for {name}. Copy it now: it is shown only once.\n\n{}\n\nKey ID: {}\nExpires: {}\n\nSet it in the client as the header Authorization: Bearer <key>. Never put it in a URL. If it is lost, issue a new key and revoke this one: pace users revoke-key {}",
            key.token,
            key.id,
            date(key.expires),
            key.id
        );
        self.view.result("Issue access key", &text)?;
        // Drop the revealed frame so the key does not linger in the redraw cache.
        self.view.invalidate();
        Ok(())
    }

    /// Every MCP user of this registry as a table; a user opens the table of its keys.
    pub(super) fn mcp_users(&mut self, server: &Path) -> Result<()> {
        let settings = Settings::load(server)?;
        let entries = users::overview(&settings)?;
        let now = chrono::Utc::now().timestamp();
        if entries.is_empty() {
            self.view.result(
                "MCP users",
                &format!(
                    "No MCP users yet. Register one in Onboarding, step 7.\n\nRegistry: {}",
                    settings.access.registry_path.display()
                ),
            )?;
            return Ok(());
        }
        let choices: Vec<_> = entries
            .iter()
            .map(|entry| {
                let (mark, state) = key_state(entry, now);
                Choice::row(
                    vec![
                        entry.user.name.clone(),
                        if entry.user.enabled {
                            "enabled"
                        } else {
                            "disabled"
                        }
                        .into(),
                        format!("{} {state}", mark.icon()),
                    ],
                    format!(
                        "Open this user's keys; profile database {}",
                        entry.user.database_path.display()
                    ),
                )
            })
            .collect();
        let mut state = Selection::default();
        while let Some(index) = self
            .view
            .table("MCP users", USERS, &choices, &mut state, true)?
        {
            self.user_keys(&entries[index], now)?;
        }
        Ok(())
    }

    fn user_keys(&mut self, entry: &UserKeys, now: i64) -> Result<()> {
        let status = |key: &KeyInfo| {
            if key.revoked {
                "revoked"
            } else if key.expires_at <= now {
                "expired"
            } else {
                "active"
            }
        };
        let mut choices = vec![Choice::new(
            "Profile",
            format!(
                "Issue keys in Onboarding step 8; revoke with pace users revoke-key KEY_ID; database {}",
                entry.user.database_path.display()
            ),
        )];
        choices.extend(entry.keys.iter().map(|key| {
            Choice::row(
                vec![key.id.clone(), status(key).into(), date(key.expires_at)],
                "Open key details",
            )
        }));
        let title = format!("{}: keys", entry.user.name);
        let mut state = Selection::default();
        while let Some(index) = self.view.table(&title, KEYS, &choices, &mut state, true)? {
            let text = if index == 0 {
                format!(
                    "User: {}\nState: {}\nGarmin writes by its model: {}\nProfile database: {}\nGarmin session: {}\n\nIssue keys in Onboarding, step 8. Revoke with: pace users revoke-key KEY_ID\nGrant or deny Garmin writes with: pace users allow-garmin-writes USER / deny-garmin-writes USER",
                    entry.user.name,
                    if entry.user.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    },
                    if entry.user.garmin_writes {
                        "allowed"
                    } else {
                        "denied"
                    },
                    entry.user.database_path.display(),
                    entry.user.session_path.display()
                )
            } else {
                let key = &entry.keys[index - 1];
                format!(
                    "Key ID: {}\nState: {}\nExpires: {}\nRevoke with: pace users revoke-key {}",
                    key.id,
                    status(key),
                    date(key.expires_at),
                    key.id
                )
            };
            self.view.result("MCP key", &text)?;
        }
        Ok(())
    }

    pub(super) fn connection_details(&mut self, server: &Path) -> Result<()> {
        let settings = Settings::load(server)?;
        let text = format!(
            "Server config: {}\nHTTP listener: {}\nMCP path: /api/v1/mcp\nHealth path: /api/v1/health (also requires the key)\n\nClient: use Streamable HTTP and Authorization: Bearer <key from step 8>. The client must support manual Bearer headers. OAuth-only clients are not supported.\nUse the listener address locally; wildcard bind addresses are not client destinations. For Internet access use https://YOUR_DOMAIN/api/v1/mcp through a TLS proxy.\n\nNew keys expire after {} days. Rotate by issuing a replacement and revoking the old key ID with users revoke-key KEY_ID. users disable USER blocks all of that user's keys.\n\nStart MCP runs in the foreground until Ctrl-C. For continuous Linux operation, use deploy/garmin-pace.service and deploy/config.toml under a dedicated OS account, plus an existing HTTPS proxy (deploy/Caddyfile). Enable the service after local provisioning and check its journal. Keep port 8080 private.\n\nThe wizard does not install services or TLS. Synchronization remains explicit; MCP serves saved data. Read README: Access administration and continuous deployment.",
            server.display(),
            settings.http.bind,
            settings.access.key_lifetime_days
        );
        self.view.result("Connection details", &text)?;
        Ok(())
    }
}
