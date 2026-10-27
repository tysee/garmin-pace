//! tests/common/mod.rs supplies real temporary configuration and SQLite fixtures
//! plus process cleanup shared by the CLI, MCP, and history integration tests.

use anyhow::{Context, Result};
use garmin_pace::{config::Settings, history::SqliteHistory};
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Output},
};
use tempfile::TempDir;
pub mod mcp;

/// Isolated configuration and a real SQLite database, never a Garmin account.
pub struct Fixture {
    pub directory: TempDir,
    pub config: PathBuf,
    pub settings: Settings,
}

impl Fixture {
    pub fn new() -> Result<Self> {
        let directory = tempfile::tempdir()?;
        let config = directory.path().join("config.toml");
        fs::write(&config, include_str!("../../config.example.toml"))?;
        let settings = Settings::load(&config)?;
        Ok(Self {
            directory,
            config,
            settings,
        })
    }

    pub fn init(&self) -> Result<()> {
        SqliteHistory::initialize(&self.settings.database)
    }

    pub fn seed(&self) -> Result<()> {
        let connection = rusqlite::Connection::open(&self.settings.database.path)?;
        for (id, date) in [
            ("fixture-1", "2026-10-01T08:00:00Z"),
            ("fixture-2", "2026-10-02T08:00:00Z"),
        ] {
            connection.execute(
                "INSERT INTO activities VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![id, "Test run", "running", date, 1800.0, 5000.0],
            )?;
        }
        Ok(())
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pace"));
        command.arg("--config").arg(&self.config).arg("--json");
        command
    }
}

/// Owns a child so failed assertions cannot leak a running server.
pub struct Process(pub Option<Child>);

impl Process {
    pub fn stop(mut self) -> Result<Output> {
        let mut child = self.0.take().context("server already stopped")?;
        child.kill().context("stopping test server")?;
        child.wait_with_output().context("waiting for test server")
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            if let Err(error) = child.kill() {
                eprintln!("test child cleanup: {error}");
            }
            if let Err(error) = child.wait() {
                eprintln!("test child wait: {error}");
            }
        }
    }
}

/// Register a real isolated profile and issue a file-delivered test credential through the CLI.
pub fn register_user(server: &Fixture, profile: &Fixture, name: &str) -> Result<(String, String)> {
    for args in [
        vec!["users", "init"],
        vec![
            "users",
            "add",
            name,
            "--profile",
            profile.config.to_str().context("profile path")?,
        ],
    ] {
        let output = server.command().args(args).output()?;
        anyhow::ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let path = server.directory.path().join(format!("{name}.key"));
    let output = server
        .command()
        .args(["users", "issue-key", name, "--output"])
        .arg(&path)
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let token = fs::read_to_string(path)?.trim().to_owned();
    assert!(!String::from_utf8_lossy(&output.stdout).contains(&token));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(&token));
    Ok((
        token,
        metadata["key_id"].as_str().context("key id")?.to_owned(),
    ))
}
