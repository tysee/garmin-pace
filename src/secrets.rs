//! src/secrets.rs reads selected dotenv files as data and creates private credential files.
//! It exposes safe lookup, the profile's fixed key-file location beside its private database,
//! the shared key-file resolution used by every command, and Personal API key delivery
//! without secret-bearing diagnostics.

use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::HashMap,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const KEYS: [&str; 1] = ["GARMIN_PERSONAL_API_KEY"];
/// File name of a profile's saved Personal API key, kept in the private database directory.
const PROFILE_KEY_FILE: &str = "garmin.env";

/// Locate the profile key file beside the database, which is always private and writable.
pub fn profile_key_path(database: &Path) -> Result<PathBuf> {
    Ok(database
        .parent()
        .context("database path has no parent")?
        .join(PROFILE_KEY_FILE))
}

/// The key file every CLI and menu command reads: an explicit dotenv file, else the
/// profile's saved key when it exists, else none so only the environment applies.
pub fn profile_key_file(explicit: Option<&Path>, database: &Path) -> Result<Option<PathBuf>> {
    if let Some(path) = explicit {
        return Ok(Some(path.to_owned()));
    }
    let path = profile_key_path(database)?;
    Ok(path.try_exists()?.then_some(path))
}

/// Credentials from the environment, optionally overridden by an explicit dotenv file.
pub struct Secrets {
    values: HashMap<String, String>,
}
impl Secrets {
    /// Load known secret names, sanitizing all parser errors before they reach the CLI.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let mut values = HashMap::new();
        for key in KEYS {
            match std::env::var(key) {
                Ok(value) => {
                    values.insert(key.to_owned(), value);
                }
                Err(std::env::VarError::NotPresent) => (),
                Err(_) => bail!("credential environment variable must contain valid text"),
            }
        }
        Self::read_file(path, values)
    }
    /// Load only this profile's private file, never the server environment's credentials.
    pub fn from_file(path: &Path) -> Result<Self> {
        Self::read_file(Some(path), HashMap::new())
    }
    fn read_file(path: Option<&Path>, mut values: HashMap<String, String>) -> Result<Self> {
        if let Some(path) = path {
            let file = crate::private_fs::open(path, false, false).map_err(|_| {
                anyhow::anyhow!(
                    "env file must be an owned regular private file (0600), not a symlink"
                )
            })?;
            let mut data = Vec::new();
            file.take(65537)
                .read_to_end(&mut data)
                .map_err(|_| anyhow::anyhow!("cannot read selected env file"))?;
            ensure!(data.len() <= 65536, "env file exceeds size limit");
            let entries = dotenvy::from_read_iter(data.as_slice());
            let mut seen = std::collections::HashSet::new();
            for entry in entries {
                let (key, value) = entry
                    .map_err(|_| anyhow::anyhow!("invalid env file syntax; values are hidden"))?;
                ensure!(seen.insert(key.clone()), "duplicate entry in env file");
                if KEYS.contains(&key.as_str()) {
                    values.insert(key, value);
                }
            }
        }
        Ok(Self { values })
    }
    /// Hold an explicitly entered key in memory while checking access before persistence.
    pub fn personal_key(key: &str) -> Result<Self> {
        validate_personal_key(key)?;
        Ok(Self {
            values: HashMap::from([("GARMIN_PERSONAL_API_KEY".into(), key.into())]),
        })
    }

    /// Save an explicitly supplied Personal API key to a new owner-only dotenv file.
    pub fn save_personal_key(path: &Path, key: &str) -> Result<()> {
        validate_personal_key(key)?;
        let escaped = key
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('$', "\\$");
        let mut file = crate::private_fs::create_new(path)?;
        if let Err(error) =
            writeln!(file, "GARMIN_PERSONAL_API_KEY=\"{escaped}\"").and_then(|()| file.sync_all())
        {
            std::fs::remove_file(path).context("removing incomplete credential file")?;
            return Err(error).context("saving private credential file");
        }
        Ok(())
    }

    /// Read a required credential without reflecting its value in diagnostics.
    pub fn require(&self, key: &str) -> Result<&str> {
        self.optional(key)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| anyhow::anyhow!("{key} is required"))
    }
    /// Read an optional credential; explicitly supplied empty values stay visible to validation.
    pub fn optional(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }
}

// Each reason names the defect without reflecting any part of the key.
fn validate_personal_key(key: &str) -> Result<()> {
    const RULE: &str = "it must contain 1..=4096 printable ASCII characters without spaces";
    ensure!(!key.is_empty(), "Personal API key is empty; {RULE}");
    ensure!(key.len() <= 4096, "Personal API key is too long; {RULE}");
    ensure!(
        !key.bytes().any(|b| b.is_ascii_whitespace()),
        "Personal API key contains spaces or line breaks; {RULE}"
    );
    ensure!(
        key.bytes().all(|b| b.is_ascii_graphic()),
        "Personal API key contains non-ASCII or control characters; {RULE}"
    );
    Ok(())
}
