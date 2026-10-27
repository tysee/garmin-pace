//! src/installation/mod.rs exposes private setup receipts, HTTPS configuration and readiness checks.
//! The Linux installer uses its bounded key handoff and transport checks without exposing credentials;
//! the menu uses the keyless probe and supervisor detection to keep MCP running without a terminal.

mod https;
pub use https::{PublicDomain, proxy_config};
mod check;
pub use check::{ServiceProbe, check_https, check_service, probe_service};
mod supervisor;
pub use supervisor::{
    ServiceState, Supervisor, UNIT_NAME, detect_supervisor, service_state, start_service,
};

use crate::{access::key_digest, config::Settings, private_fs};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    key_file: PathBuf,
}

/// Persist only the key's location, never its value, using an atomic private file.
pub fn save_receipt(profile: &Path, key_file: &Path) -> Result<()> {
    let directory = profile
        .parent()
        .context("profile directory required")?
        .canonicalize()?;
    private_fs::check_directory(&directory)?;
    let destination = directory.join("installation.json");
    if destination.try_exists()? {
        private_fs::open(&destination, false, false)?;
    }
    let mut temporary = tempfile::NamedTempFile::new_in(&directory)?;
    serde_json::to_writer(
        temporary.as_file_mut(),
        &Receipt {
            key_file: key_file.canonicalize()?,
        },
    )?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(destination)
        .map_err(|e| e.error)
        .context("saving installation receipt")?;
    fs::File::open(directory)?
        .sync_all()
        .context("persisting installation receipt")
}

/// Copy into the installer's empty, private handoff descriptor without exposing stdout.
pub fn handoff_key(source: &Path, destination: &Path) -> Result<()> {
    let key = read_key(source)?;
    let mut output = private_fs::open(destination, true, false)?;
    ensure!(
        output.metadata()?.len() == 0,
        "key handoff must be an empty private file"
    );
    output.write_all(key.as_bytes())?;
    output.write_all(b"\n")?;
    output.sync_all().context("persisting key handoff")
}

fn read_key(path: &Path) -> Result<String> {
    let mut key = String::new();
    private_fs::open(path, false, false)?
        .take(50)
        .read_to_string(&mut key)?;
    ensure!(
        key.len() == 49 && key.ends_with('\n'),
        "invalid private key file"
    );
    let key = key.strip_suffix('\n').context("invalid private key file")?;
    key_digest(key)?;
    Ok(key.into())
}

fn receipt_key(profile: &Path) -> Result<String> {
    let directory = profile
        .parent()
        .context("profile directory required")?
        .canonicalize()?;
    private_fs::check_directory(&directory)?;
    let mut data = Vec::new();
    private_fs::open(&directory.join("installation.json"), false, false)?
        .take(4097)
        .read_to_end(&mut data)?;
    ensure!(
        data.len() <= 4096,
        "installation receipt exceeds size limit"
    );
    let receipt: Receipt = serde_json::from_slice(&data)
        .map_err(|_| anyhow::anyhow!("invalid installation receipt"))?;
    ensure!(
        receipt.key_file.parent() == Some(directory.as_path()),
        "installation key must belong to the selected profile directory"
    );
    read_key(&receipt.key_file)
}

/// Require a private local listener before the installer provisions or starts service access.
pub fn validate_listener(settings: &Settings) -> Result<()> {
    ensure!(
        settings.http.bind.ip().is_loopback() && settings.http.bind.port() != 0,
        "installer readiness requires a configured loopback listener"
    );
    Ok(())
}

/// Read non-secret installer metadata without reflecting TOML parser input in errors.
pub fn load_https_domain(path: &Path) -> Result<PublicDomain> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Metadata {
        domain: String,
    }
    let mut data = String::new();
    fs::File::open(path)?.take(4097).read_to_string(&mut data)?;
    ensure!(data.len() <= 4096, "HTTPS metadata exceeds size limit");
    let metadata: Metadata =
        toml::from_str(&data).map_err(|_| anyhow::anyhow!("invalid HTTPS metadata"))?;
    PublicDomain::parse(&metadata.domain)
}
