//! tests/cases/https.rs verifies the installer HTTPS contract without network authority.
//! It drives config rendering through the CLI and rejects unsafe hostnames and listeners.

use crate::common::Fixture;
use anyhow::{Context, Result};
use garmin_pace::installation::{PublicDomain, proxy_config};

#[test]
fn https_hostname_boundary_rejects_config_and_url_injection() -> Result<()> {
    for value in [
        "",
        "localhost",
        "127.0.0.1",
        "[::1]",
        "mcp.local",
        "home.arpa",
        "mcp.internal",
        "https://mcp.example.com",
        "mcp.example.com:443",
        "*.example.com",
        "mcp.example.com/path",
        "mcp.example.com\n{ admin :9999 }",
        "mcp.example.com {$SECRET}",
        "user@mcp.example.com",
        "-mcp.example.com",
        "mcp..example.com",
        "mcp.example.com.",
    ] {
        assert!(PublicDomain::parse(value).is_err(), "accepted {value:?}");
    }
    assert!(PublicDomain::parse(&format!("{}.example.com", "a".repeat(64))).is_err());
    let domain = PublicDomain::parse("MCP.Example.COM")?;
    assert_eq!(domain.mcp_url(), "https://mcp.example.com/api/v1/mcp");
    assert!(proxy_config(&domain, "0.0.0.0:8080".parse()?, 16384, 15).is_err());
    assert!(proxy_config(&domain, "127.0.0.1:8080".parse()?, usize::MAX, 15).is_err());
    Ok(())
}

#[test]
fn https_config_cli_validates_and_renders_without_mutating_profile() -> Result<()> {
    let fixture = Fixture::new()?;
    let original = std::fs::read(&fixture.config)?;
    let output = fixture
        .command()
        .args(["installation-https-config", "--domain", "mcp.example.com"])
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reply: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let config = reply["caddyfile"]
        .as_str()
        .context("rendered Caddyfile is required")?;
    assert!(config.contains("https://mcp.example.com"));
    assert!(config.contains("header_up Host 127.0.0.1:8080"));
    assert!(config.contains("/api/v1/docs /api/v1/docs/init.js /api/v1/openapi.json"));
    assert!(config.contains("{http.request.header.Origin} != \"https://mcp.example.com\""));
    assert!(config.contains("respond @foreign_origin 403"));
    assert!(config.contains("header_up Origin http://127.0.0.1:8080"));
    assert!(config.contains("admin off") && config.contains("respond 404"));
    assert!(!config.contains("{$"));
    let invalid = fixture
        .command()
        .args([
            "installation-https-config",
            "--domain",
            "mcp.example.com\nimport /etc/shadow",
        ])
        .output()?;
    assert!(!invalid.status.success());
    assert_eq!(std::fs::read(&fixture.config)?, original);
    assert!(!fixture.settings.database.path.exists());
    Ok(())
}

#[test]
fn https_saved_domain_is_bounded_validated_and_reusable() -> Result<()> {
    let fixture = Fixture::new()?;
    let path = fixture.directory.path().join("https.toml");
    std::fs::write(&path, "domain = \"MCP.Example.COM\"\n")?;
    let output = fixture
        .command()
        .args(["installation-https-config", "--saved-domain"])
        .arg(&path)
        .arg("--domain-only")
        .output()?;
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(value["domain"], "mcp.example.com");
    for invalid in [
        "domain = \"https://mcp.example.com\"".to_owned(),
        "domain = \"mcp.example.com\"\nunknown = true".to_owned(),
        "x".repeat(4097),
    ] {
        std::fs::write(&path, invalid)?;
        assert!(garmin_pace::installation::load_https_domain(&path).is_err());
    }
    Ok(())
}
