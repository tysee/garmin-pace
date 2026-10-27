//! src/interface/users.rs exposes local administrator commands for per-profile MCP access.
//! It validates profile paths, renders safe metadata, and delivers keys only to private files.

use super::output::Output;
use crate::{
    access::{AccessService, KeyInfo, NewKey, SqliteAccess, User},
    config::Settings,
    history::SqliteHistory,
};
use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Subcommand)]
pub(super) enum UsersCommand {
    /// Initialize the private access registry explicitly.
    Init,
    /// Register an initialized, separate Garmin profile.
    Add {
        user: String,
        #[arg(long)]
        profile: PathBuf,
    },
    /// List registered users without credentials.
    List,
    /// Suspend every key belonging to a user immediately.
    Disable { user: String },
    /// Restore a suspended user; unrevoked, unexpired keys become usable again.
    Enable { user: String },
    /// Let this user's MCP client save, reconcile and deliver to Garmin on the user's explicit instruction.
    AllowGarminWrites { user: String },
    /// Return this user's MCP client to local drafts only.
    DenyGarminWrites { user: String },
    /// Issue a random key into a new private file, never stdout.
    IssueKey {
        user: String,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        days: Option<u32>,
    },
    /// List safe key identifiers, expiry and revocation state.
    Keys { user: String },
    /// Revoke a key immediately, without its secret value.
    RevokeKey { key_id: String },
}

pub(super) fn service(settings: &Settings) -> Result<AccessService> {
    Ok(AccessService::new(Arc::new(
        SqliteAccess::open(settings.registry_database())
            .context("opening access registry; run users init")?,
    )))
}

pub(super) fn run(command: UsersCommand, settings: &Settings, output: Output<'_>) -> Result<()> {
    // Only initialization runs without an existing registry.
    let access = || service(settings);
    match command {
        UsersCommand::Init => {
            SqliteAccess::initialize(&settings.registry_database())?;
            output.message(
                &json!({"registry_initialized":true}),
                "Access registry initialized.",
            )
        }
        UsersCommand::Enable { user } => {
            access()?.set_enabled(&user, true)?;
            output.message(
                &json!({"user":user,"enabled":true}),
                "User enabled; unrevoked keys restored.",
            )
        }
        UsersCommand::AllowGarminWrites { user } => {
            access()?.set_garmin_writes(&user, true)?;
            output.message(
                &json!({"user":user,"garmin_writes":true}),
                "Garmin writes allowed; the model must quote the user's instruction on every write.",
            )
        }
        UsersCommand::DenyGarminWrites { user } => {
            access()?.set_garmin_writes(&user, false)?;
            output.message(
                &json!({"user":user,"garmin_writes":false}),
                "Garmin writes denied; the model can only write local drafts.",
            )
        }
        UsersCommand::Add { user, profile } => {
            register_profile(settings, &user, &profile, false)?;
            output.message(
                &json!({"user":user,"enabled":true}),
                &format!("Registered user: {user}"),
            )
        }
        UsersCommand::List => {
            let users = access()?.users()?;
            let rows: Vec<_> = users
                .iter()
                .map(|u| json!({"user":u.name,"enabled":u.enabled,"garmin_writes":u.garmin_writes,"database_path":u.database_path}))
                .collect();
            let text = users
                .iter()
                .map(|u| {
                    format!(
                        "{}: {}, Garmin writes {}",
                        u.name,
                        if u.enabled { "enabled" } else { "disabled" },
                        if u.garmin_writes { "allowed" } else { "denied" }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            output.message(&json!({"users":rows}), &text)
        }
        UsersCommand::Disable { user } => {
            access()?.set_enabled(&user, false)?;
            output.message(&json!({"user":user,"enabled":false}), "User disabled.")
        }
        UsersCommand::IssueKey {
            user,
            output: path,
            days,
        } => {
            let issued = issue_key(settings, &user, KeyDestination::File(path), days)?;
            output.message(
                &json!({"user":user,"key_id":issued.id,"expires_at_unix":issued.expires,"key_file":issued.path}),
                &format!("Key ID: {}\nExpires at Unix time: {}\nKey saved to private file: {}", issued.id, issued.expires, issued.path.display()),
            )
        }
        UsersCommand::Keys { user } => {
            let keys = access()?.keys(&user)?;
            let rows: Vec<_> = keys
                .iter()
                .map(|k| json!({"key_id":k.id,"expires_at_unix":k.expires_at,"revoked":k.revoked}))
                .collect();
            let text = keys
                .iter()
                .map(|k| format!("{}: expires {}, revoked {}", k.id, k.expires_at, k.revoked))
                .collect::<Vec<_>>()
                .join("\n");
            output.message(&json!({"keys":rows}), &text)
        }
        UsersCommand::RevokeKey { key_id } => {
            access()?.revoke(&key_id)?;
            output.message(&json!({"key_id":key_id,"revoked":true}), "Key revoked.")
        }
    }
}

/// Find the registered user bound to this profile's canonical database, if any.
pub(super) fn profile_user(access: &AccessService, profile: &Settings) -> Result<Option<User>> {
    let database = fs::canonicalize(&profile.database.path).context("locating profile database")?;
    Ok(access
        .users()?
        .into_iter()
        .find(|user| user.database_path == database))
}

/// Resolve transport paths before the access service checks ownership invariants.
pub(super) fn register_profile(
    settings: &Settings,
    name: &str,
    path: &Path,
    resume: bool,
) -> Result<()> {
    let profile = Settings::load(path)?;
    SqliteHistory::open(profile.database.clone())?;
    let database_path =
        fs::canonicalize(&profile.database.path).context("locating profile database")?;
    let parent = profile
        .garmin_auth
        .session_path
        .parent()
        .context("session parent")?;
    crate::private_fs::directory(parent)?;
    let session_path = fs::canonicalize(parent)?.join(
        profile
            .garmin_auth
            .session_path
            .file_name()
            .context("session filename")?,
    );
    ensure!(
        !session_path.is_symlink(),
        "profile session cannot be a symlink"
    );
    let user = User {
        name: name.into(),
        database_path,
        session_path,
        enabled: true,
        garmin_writes: false,
    };
    let access = service(settings)?;
    if resume {
        access.add_or_match(&user)?;
    } else {
        access.add(&user)?;
    }
    Ok(())
}

pub(super) enum KeyDestination {
    File(PathBuf),
    Directory(PathBuf),
}
pub(super) struct IssuedKey {
    pub id: String,
    pub path: PathBuf,
    pub expires: i64,
}

/// A key registered by digest whose value is returned once for an interactive reveal.
pub(super) struct RevealedKey {
    pub id: String,
    pub expires: i64,
    pub token: String,
}

/// Register a new key and return its value instead of writing a delivery file.
pub(super) fn issue_revealed_key(settings: &Settings, user: &str) -> Result<RevealedKey> {
    let key = NewKey::generate()?;
    let expires = service(settings)?.issue(
        user,
        &key.id,
        &key.digest,
        chrono::Utc::now().timestamp(),
        settings.access.key_lifetime_days,
    )?;
    Ok(RevealedKey {
        id: key.id.clone(),
        expires,
        token: key.reveal(),
    })
}

/// One registered user with its safe key metadata, for administration screens.
pub(super) struct UserKeys {
    pub user: User,
    pub keys: Vec<KeyInfo>,
}

/// List every registered user with key metadata; an absent registry means no users yet.
pub(super) fn overview(settings: &Settings) -> Result<Vec<UserKeys>> {
    if !settings.access.registry_path.try_exists()? {
        return Ok(Vec::new());
    }
    let access = service(settings)?;
    access
        .users()?
        .into_iter()
        .map(|user| {
            let keys = access.keys(&user.name)?;
            Ok(UserKeys { user, keys })
        })
        .collect()
}

/// Deliver a key once and register its digest; errors remove unregistered delivery files.
pub(super) fn issue_key(
    settings: &Settings,
    user: &str,
    destination: KeyDestination,
    days: Option<u32>,
) -> Result<IssuedKey> {
    let access = service(settings)?;
    let key = NewKey::generate()?;
    let path = match destination {
        KeyDestination::File(path) => path,
        KeyDestination::Directory(path) => path.join(format!("mcp-{}.key", key.id)),
    };
    key.write(&path)?;
    let result = access.issue(
        user,
        &key.id,
        &key.digest,
        chrono::Utc::now().timestamp(),
        days.unwrap_or(settings.access.key_lifetime_days),
    );
    match result {
        Ok(expires) => Ok(IssuedKey {
            id: key.id,
            path,
            expires,
        }),
        Err(error) => {
            fs::remove_file(&path).context("removing unregistered key output")?;
            Err(error.into())
        }
    }
}
