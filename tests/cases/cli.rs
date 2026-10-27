//! tests/cases/cli.rs drives configuration, initialization, and history queries through
//! the built executable, checking exit statuses, JSON output, and persistence.

use anyhow::Result;
use serde_json::{Value, json};

use crate::common::Fixture;

#[test]
fn interactive_cli_requires_a_terminal_and_rejects_json() -> Result<()> {
    let fixture = Fixture::new()?;
    for args in [
        vec![],
        vec!["ui"],
        vec!["ui", "--compact"],
        vec!["setup"],
        vec!["setup", "--compact"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_pace"))
            .arg("--config")
            .arg(&fixture.config)
            .args(args)
            .output()?;
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("requires a terminal"));
        assert!(output.stdout.is_empty());
    }
    let output = fixture.command().arg("ui").output()?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--json"));
    assert!(!fixture.settings.database.path.exists());
    Ok(())
}

#[test]
fn cli_configuration_and_history_flow() -> Result<()> {
    let fixture = Fixture::new()?;
    let check = std::process::Command::new(env!("CARGO_BIN_EXE_pace"))
        .env("GARMIN_PACE_CONFIG", &fixture.config)
        .args(["config", "check"])
        .output()?;
    assert!(check.status.success());
    let explicit_config = fixture
        .command()
        .env(
            "GARMIN_PACE_CONFIG",
            fixture.directory.path().join("missing.toml"),
        )
        .args(["config", "check"])
        .output()?;
    assert!(explicit_config.status.success());
    assert!(!fixture.settings.database.path.exists());
    let missing = fixture.command().arg("status").output()?;
    assert!(!missing.status.success());
    let init = fixture.command().arg("init").output()?;
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let status: Value = serde_json::from_slice(&fixture.command().arg("status").output()?.stdout)?;
    assert_eq!(status["activity_count"], 0);
    assert_eq!(status["garmin_integration"], "not_synced");
    fixture.seed()?;
    let list = fixture
        .command()
        .args(["activities", "list", "--limit", "1"])
        .output()?;
    assert!(list.status.success());
    let page: Value = serde_json::from_slice(&list.stdout)?;
    assert_eq!(page["activities"].as_array().map(Vec::len), Some(1));
    assert_eq!(page["activities"][0]["garmin_id"], "fixture-2");
    let invalid = fixture
        .command()
        .args(["activities", "list", "--limit", "101"])
        .output()?;
    assert!(!invalid.status.success());
    Ok(())
}

#[test]
fn config_init_never_overwrites_and_show_excludes_credentials() -> Result<()> {
    let fixture = Fixture::new()?;
    let overwrite = fixture.command().args(["config", "init"]).output()?;
    assert!(!overwrite.status.success());
    std::fs::remove_file(&fixture.config)?;
    assert!(
        fixture
            .command()
            .args(["config", "init"])
            .output()?
            .status
            .success()
    );
    let show = fixture
        .command()
        .env("GARMIN_PACE_HTTP_TOKEN", "do-not-print-this-credential")
        .args(["config", "show"])
        .output()?;
    assert!(show.status.success());
    assert!(!String::from_utf8_lossy(&show.stdout).contains("do-not-print"));
    let value: Value = serde_json::from_slice(&show.stdout)?;
    assert_eq!(value["http"]["max_in_flight"], json!(4));
    Ok(())
}

#[test]
fn http_refuses_to_start_without_a_registry() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let output = fixture
        .command()
        .env_remove("GARMIN_PACE_HTTP_TOKEN")
        .args(["serve", "--transport", "http"])
        .output()?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("access registry"));
    Ok(())
}

#[test]
fn dotenv_is_data_and_syntax_errors_do_not_echo_secrets() -> Result<()> {
    use std::fs;
    let fixture = Fixture::new()?;
    fixture.init()?;
    let env = fixture.directory.path().join("private.env");
    let marker = fixture.directory.path().join("must-not-exist");
    fs::write(
        &env,
        format!(
            "GARMIN_PERSONAL_API_KEY= fixture-key\nGARMIN_PACE_HTTP_TOKEN='$(touch {})'\n",
            marker.display()
        ),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&env, fs::Permissions::from_mode(0o600))?;
    }
    // Only credential-bearing commands read the dotenv file; health reads it without any network use.
    let output = fixture
        .command()
        .arg("--env-file")
        .arg(&env)
        .args(["health", "show", "--date", "2026-10-24"])
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!marker.exists());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("fixture-key"));
    fs::write(&env, "GARMIN_PERSONAL_API_KEY=\"unclosed-sensitive-value")?;
    let output = fixture
        .command()
        .arg("--env-file")
        .arg(&env)
        .args(["health", "show", "--date", "2026-10-24"])
        .output()?;
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("unclosed-sensitive-value"));
    Ok(())
}

#[test]
fn human_cli_initializes_and_reads_history_with_units() -> Result<()> {
    let fixture = Fixture::new()?;
    let human = |args: &[&str]| -> Result<std::process::Output> {
        Ok(std::process::Command::new(env!("CARGO_BIN_EXE_pace"))
            .arg("--config")
            .arg(&fixture.config)
            .args(args)
            .output()?)
    };
    let initialized = human(&["init"])?;
    assert!(initialized.status.success());
    assert!(String::from_utf8(initialized.stdout)?.contains("Database initialized."));
    let empty = human(&["activities", "list"])?;
    assert!(String::from_utf8(empty.stdout)?.contains("No stored activities."));
    fixture.seed()?;
    let status = human(&["status"])?;
    let text = String::from_utf8(status.stdout)?;
    assert!(text.contains("Stored activities: 2") && text.contains("Last sync: never"));
    let list = human(&["activities", "list", "--limit", "1"])?;
    let text = String::from_utf8(list.stdout)?;
    assert!(text.contains("Test run") && text.contains("30.0 min") && text.contains("5.00 km"));
    assert!(!text.contains("fixture-1"));
    let machine = human(&["status", "--json"])?;
    let value: Value = serde_json::from_slice(&machine.stdout)?;
    assert_eq!(value["activity_count"], 2);
    Ok(())
}
