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
fn full_help_lists_every_visible_command_and_flag_without_a_config() -> Result<()> {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_pace"))
        .env_remove("GARMIN_PACE_CONFIG")
        .args(["help", "--all"])
        .output()?;
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout)?;
    for needle in [
        "--config <CONFIG> (required)",
        "[env: GARMIN_PACE_CONFIG]",
        "pace workout send",
        "--device-id <DEVICE_ID> (required)",
        "pace config network",
        "--clear-proxy",
        "pace activities list",
        "[default: 0]",
        "[values: stdio, http]",
    ] {
        assert!(text.contains(needle), "{needle}\n{text}");
    }
    for hidden in ["installation-check", "--key-handoff", "pace help"] {
        assert!(!text.contains(hidden), "{hidden}");
    }
    // Global options are listed once, not repeated under every command.
    assert_eq!(text.matches("--json").count(), 1);
    let short = std::process::Command::new(env!("CARGO_BIN_EXE_pace"))
        .env_remove("GARMIN_PACE_CONFIG")
        .arg("--help")
        .output()?;
    assert!(String::from_utf8(short.stdout)?.contains("pace help --all"));
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

#[test]
fn config_network_edits_only_the_garmin_egress_keys() -> Result<()> {
    let fixture = Fixture::new()?;
    let original = std::fs::read_to_string(&fixture.config)?;
    let network = |args: &[&str]| -> Result<(bool, Value, String)> {
        let output = fixture
            .command()
            .args(["config", "network"])
            .args(args)
            .output()?;
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
        Ok((output.status.success(), value, stderr))
    };
    let (ok, shown, stderr) = network(&[])?;
    assert!(ok, "{stderr}");
    assert_eq!(
        shown,
        json!({"proxy_url":null,"extra_ca_file":null,"changed":false})
    );

    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/egress-ca.pem"),
        fixture.config.with_file_name("egress-ca.pem"),
    )?;
    let (ok, saved, stderr) = network(&[
        "--proxy-url",
        "http://proxy.test:3128",
        "--extra-ca-file",
        "egress-ca.pem",
    ])?;
    assert!(ok, "{stderr}");
    assert_eq!(saved["changed"], true);
    let edited = std::fs::read_to_string(&fixture.config)?;
    // Exactly two lines were added; every original line, comment and path is unchanged.
    let added: Vec<_> = edited
        .lines()
        .filter(|line| !original.lines().any(|o| o == *line))
        .collect();
    assert_eq!(
        added,
        [
            "proxy_url = \"http://proxy.test:3128\"",
            "extra_ca_file = \"egress-ca.pem\""
        ]
    );
    let settings = garmin_pace::config::Settings::load(&fixture.config)?;
    assert_eq!(
        settings.garmin.proxy_url.as_deref(),
        Some("http://proxy.test:3128")
    );
    assert_eq!(
        settings.garmin.extra_ca_file,
        Some(
            fixture
                .config
                .with_file_name("egress-ca.pem")
                .canonicalize()?
        )
    );

    // Rejected candidates leave the file byte-identical and no temporary file behind.
    for args in [
        &["--proxy-url", "http://user:secret@proxy.test:3128"][..],
        &["--extra-ca-file", "missing.pem"][..],
        &["--proxy-url", "x", "--clear-proxy"][..],
    ] {
        let (ok, _, stderr) = network(args)?;
        assert!(!ok, "{args:?}");
        assert!(!stderr.contains("secret"), "{stderr}");
        assert_eq!(std::fs::read_to_string(&fixture.config)?, edited);
    }
    let leftovers = std::fs::read_dir(fixture.config.parent().unwrap_or(&fixture.config))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".pace-edit"))
        .count();
    assert_eq!(leftovers, 0);

    let (ok, cleared, _) = network(&["--clear-proxy", "--clear-ca"])?;
    assert!(ok);
    assert_eq!(cleared["proxy_url"], Value::Null);
    assert_eq!(std::fs::read_to_string(&fixture.config)?, original);
    Ok(())
}
