//! src/config/network.rs edits the two Garmin egress keys, `garmin.proxy_url` and
//! `garmin.extra_ca_file`, in a configuration file without touching anything else: comments,
//! order and relative paths stay byte-identical. A re-parse proves only those keys changed, the
//! candidate is loaded and checked beside the original, then atomically replaces it.
//! Exposes GarminNetwork, garmin_network and update_garmin_network.

use super::Settings;
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

const KEYS: [&str; 2] = ["proxy_url", "extra_ca_file"];
const MAX_CONFIG_BYTES: u64 = 1_048_576;

/// The stored egress keys exactly as written; `None` means the key is absent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GarminNetwork {
    pub proxy_url: Option<String>,
    /// Relative paths stay relative to the configuration file.
    pub extra_ca_file: Option<PathBuf>,
}

/// Read the stored egress keys without resolving paths or validating the rest of the file.
pub fn garmin_network(path: &Path) -> Result<GarminNetwork> {
    let table = parse(&read(path)?)?;
    let garmin = table
        .get("garmin")
        .and_then(toml::Value::as_table)
        .context("configuration has no [garmin] section")?;
    let text = |key: &str| -> Result<Option<String>> {
        garmin
            .get(key)
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .with_context(|| format!("garmin.{key} must be a string"))
            })
            .transpose()
    };
    Ok(GarminNetwork {
        proxy_url: text("proxy_url")?,
        extra_ca_file: text("extra_ca_file")?.map(PathBuf::from),
    })
}

/// Replace the egress keys after `check` accepts the fully loaded candidate settings.
pub fn update_garmin_network(
    path: &Path,
    network: &GarminNetwork,
    check: impl FnOnce(&Settings) -> Result<()>,
) -> Result<()> {
    ensure!(
        !fs::symlink_metadata(path)
            .context("reading configuration file")?
            .file_type()
            .is_symlink(),
        "configuration is a symbolic link; edit its target instead"
    );
    let text = read(path)?;
    let edited = edit(&text, network)?;
    verify(&parse(&text)?, &parse(&edited)?, network)?;
    // Reject an invalid proxy form before touching the filesystem.
    toml::from_str::<Settings>(&edited)
        .map_err(|_| anyhow::anyhow!("invalid configuration; check required fields and types"))?
        .garmin
        .validate()?;
    let name = path
        .file_name()
        .context("configuration path has no file name")?
        .to_string_lossy();
    let temp = path.with_file_name(format!(".{name}.pace-edit"));
    let mode = fs::metadata(path)?.permissions().mode() & 0o777;
    let staged = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temp)
            .context("creating a temporary configuration beside the original")?;
        file.write_all(edited.as_bytes())?;
        file.sync_all()?;
        check(&Settings::load(&temp)?)
    })();
    if let Err(error) = staged {
        // Never leave a half-written candidate; the original is untouched.
        if temp.exists() {
            fs::remove_file(&temp).context("removing the rejected temporary configuration")?;
        }
        return Err(error);
    }
    fs::rename(&temp, path).context("replacing configuration file")
}

fn read(path: &Path) -> Result<String> {
    let mut text = String::new();
    fs::File::open(path)
        .context("reading configuration file")?
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_string(&mut text)
        .context("configuration must be UTF-8 text")?;
    ensure!(
        text.len() as u64 <= MAX_CONFIG_BYTES,
        "configuration exceeds 1 MiB"
    );
    Ok(text)
}

fn parse(text: &str) -> Result<toml::Table> {
    // Do not include TOML parser diagnostics: they can reproduce file contents.
    toml::from_str(text).map_err(|_| anyhow::anyhow!("invalid configuration TOML"))
}

/// The key an active `key = value` line assigns, ignoring comments and blank lines.
fn assigned(line: &str) -> Option<&str> {
    let line = line.trim_start();
    if line.starts_with('#') {
        return None;
    }
    let (key, _) = line.split_once('=')?;
    Some(key.trim())
}

fn is_header(line: &str) -> bool {
    line.trim_start().starts_with('[')
}

fn edit(text: &str, network: &GarminNetwork) -> Result<String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let header = lines
        .iter()
        .position(|line| {
            line.split('#')
                .next()
                .is_some_and(|h| h.trim() == "[garmin]")
        })
        .context("configuration has no [garmin] section")?;
    let end = lines[header + 1..]
        .iter()
        .position(|line| is_header(line))
        .map_or(lines.len(), |i| header + 1 + i);
    let body: Vec<&str> = lines[header + 1..end]
        .iter()
        .copied()
        .filter(|line| !assigned(line).is_some_and(|key| KEYS.contains(&key)))
        .collect();
    // New keys go after the section's last non-blank line, before its trailing blank lines.
    let last = body
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map_or(0, |i| i + 1);
    let mut out = lines[..=header].concat();
    out.push_str(&body[..last].concat());
    if !out.ends_with('\n') {
        out.push('\n');
    }
    if let Some(proxy) = &network.proxy_url {
        out.push_str(&assignment("proxy_url", proxy)?);
    }
    if let Some(path) = &network.extra_ca_file {
        let path = path
            .to_str()
            .context("garmin.extra_ca_file must be a UTF-8 path")?;
        out.push_str(&assignment("extra_ca_file", path)?);
    }
    out.push_str(&body[last..].concat());
    out.push_str(&lines[end..].concat());
    Ok(out)
}

/// One `key = "value"` line, quoted and escaped by the TOML serializer.
fn assignment(key: &str, value: &str) -> Result<String> {
    let mut table = toml::Table::new();
    table.insert(key.into(), toml::Value::String(value.into()));
    Ok(toml::to_string(&table)?)
}

/// The edit must equal the original with exactly the two keys replaced.
fn verify(original: &toml::Table, edited: &toml::Table, network: &GarminNetwork) -> Result<()> {
    let mut expected = original.clone();
    let Some(toml::Value::Table(garmin)) = expected.get_mut("garmin") else {
        bail!("configuration has no [garmin] section");
    };
    for key in KEYS {
        garmin.remove(key);
    }
    if let Some(proxy) = &network.proxy_url {
        garmin.insert("proxy_url".into(), toml::Value::String(proxy.clone()));
    }
    if let Some(path) = &network.extra_ca_file {
        garmin.insert(
            "extra_ca_file".into(),
            toml::Value::String(path.to_string_lossy().into_owned()),
        );
    }
    ensure!(
        &expected == edited,
        "configuration layout is not supported for in-place editing; edit [garmin] by hand"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "# top comment\n[database]\npath = \"data/pace.sqlite3\"\n\n[garmin]\nretry_base_ms = 500 # keep\n# proxy_url = \"http://proxy.internal:3128\"\nproxy_url = \"http://old:1\"\n\n[garmin_auth]\nsession_path = \"s.json\"\n";

    fn network(proxy: Option<&str>, ca: Option<&str>) -> GarminNetwork {
        GarminNetwork {
            proxy_url: proxy.map(str::to_owned),
            extra_ca_file: ca.map(PathBuf::from),
        }
    }

    #[test]
    fn edit_replaces_only_the_egress_keys_and_keeps_comments() -> Result<()> {
        let wanted = network(Some("http://proxy:3128"), Some("ca \"x\".pem"));
        let edited = edit(TEXT, &wanted)?;
        assert_eq!(
            edited,
            "# top comment\n[database]\npath = \"data/pace.sqlite3\"\n\n[garmin]\nretry_base_ms = 500 # keep\n# proxy_url = \"http://proxy.internal:3128\"\nproxy_url = \"http://proxy:3128\"\nextra_ca_file = 'ca \"x\".pem'\n\n[garmin_auth]\nsession_path = \"s.json\"\n"
        );
        verify(&parse(TEXT)?, &parse(&edited)?, &wanted)?;
        let cleared = edit(&edited, &GarminNetwork::default())?;
        assert!(!cleared.contains("\nproxy_url") && !cleared.contains("extra_ca_file"));
        assert!(cleared.contains("# proxy_url = \"http://proxy.internal:3128\"\n\n[garmin_auth]"));
        verify(&parse(TEXT)?, &parse(&cleared)?, &GarminNetwork::default())
    }

    #[test]
    fn unsupported_layouts_are_refused_instead_of_rewritten() {
        assert!(edit("[database]\npath = \"x\"\n", &GarminNetwork::default()).is_err());
        assert!(
            edit(
                "garmin = { retry_base_ms = 1 }\n",
                &GarminNetwork::default()
            )
            .is_err()
        );
    }

    #[test]
    fn a_file_without_trailing_newline_gains_one_before_new_keys() -> Result<()> {
        let edited = edit(
            "[garmin]\nretry_base_ms = 1",
            &network(Some("http://p:1"), None),
        )?;
        assert_eq!(
            edited,
            "[garmin]\nretry_base_ms = 1\nproxy_url = \"http://p:1\"\n"
        );
        Ok(())
    }
}
