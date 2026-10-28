//! src/interface/cli.rs defines and dispatches the CLI. It composes the history
//! feature with validated settings and starts a bounded runtime only for servers.

use anyhow::{Context, Result, ensure};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
};

use super::{
    http,
    mcp::{AuthenticatedHistory, GarminAccess, HistoryMcp},
    output::Output,
};
use crate::{
    config::{self, Settings},
    history::{HistoryService, SqliteHistory},
};

/// Command-line contract for local configuration, diagnostics, and MCP serving.
#[derive(Parser)]
#[command(
    name = "pace",
    version,
    about,
    after_help = "Run 'pace help --all' to list every command and its flags."
)]
pub struct Cli {
    #[arg(long, env = "GARMIN_PACE_CONFIG")]
    config: PathBuf,
    /// Read the Garmin key from this private dotenv file instead of the profile's garmin.env.
    #[arg(long, global = true)]
    env_file: Option<PathBuf>,
    /// Print structured JSON instead of human-readable command output.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

/// The full command reference for exactly `pace help --all`, read before parsing so it needs
/// no --config, like clap's own help.
pub fn full_help(args: impl IntoIterator<Item = OsString>) -> Option<String> {
    let args: Vec<OsString> = args.into_iter().skip(1).collect();
    (args == ["help", "--all"]).then(|| super::cli_reference::render(&Cli::command()))
}

#[derive(Subcommand)]
pub(super) enum Command {
    #[command(hide = true)]
    InstallationCheck {
        #[arg(long)]
        profile: PathBuf,
        #[arg(long)]
        https_domain: Option<String>,
    },
    #[command(hide = true)]
    InstallationHttpsConfig {
        #[arg(
            long,
            required_unless_present = "saved_domain",
            conflicts_with = "saved_domain"
        )]
        domain: Option<String>,
        #[arg(long, conflicts_with = "domain")]
        saved_domain: Option<PathBuf>,
        #[arg(long, requires = "saved_domain")]
        domain_only: bool,
    },
    /// Synchronize or inspect local health, sleep and training readiness.
    Health {
        #[command(subcommand)]
        action: super::health::HealthCommand,
    },
    /// Administer per-user MCP credentials locally.
    Users {
        #[command(subcommand)]
        action: super::users::UsersCommand,
    },
    /// Open guided profile, Garmin import and MCP access setup.
    Setup {
        /// Use the compact layout even in a wide terminal.
        #[arg(long)]
        compact: bool,
        /// Use the installer flow with automatic profile and key paths.
        #[arg(long, requires_all = ["user", "server_config"])]
        quick: bool,
        #[arg(long, requires = "quick")]
        user: Option<String>,
        #[arg(long, requires = "quick")]
        server_config: Option<PathBuf>,
        #[arg(long, requires = "quick", hide = true)]
        key_handoff: Option<PathBuf>,
        /// Provision the profile, registry and key without a terminal; no Garmin step.
        #[arg(long, requires_all = ["quick", "key_days"])]
        batch: bool,
        /// Key lifetime in days for --batch (the wizard asks instead).
        #[arg(long, requires = "batch")]
        key_days: Option<u32>,
    },
    /// Open the interactive terminal menu (also the default without a command).
    Ui {
        /// Use the compact menu even in a wide terminal.
        #[arg(long)]
        compact: bool,
    },
    /// Browse workouts, review model drafts and save changes to Garmin.
    Workout {
        #[command(subcommand)]
        action: super::workouts::WorkoutCommand,
    },
    /// Read and explicitly import Garmin Coach plans and weekly schedules.
    Coach {
        #[command(subcommand)]
        action: super::coach::CoachCommand,
    },
    /// Review model-estimated benchmark exercises from observed strength history.
    Benchmark {
        #[command(subcommand)]
        action: super::benchmarks::BenchmarkCommand,
    },
    /// Import activities, strength sets, health and Coach for a period in one journaled run.
    Sync {
        /// First Garmin calendar date to import (YYYY-MM-DD).
        #[arg(long, required_unless_present = "resume", conflicts_with = "resume")]
        start: Option<crate::health::Date>,
        /// Last date to import, inclusive; defaults to today in UTC.
        #[arg(long, requires = "start")]
        end: Option<crate::health::Date>,
        /// Continue the latest unfinished run, skipping the windows it already imported.
        #[arg(long)]
        resume: bool,
    },
    /// Authenticate with Garmin and synchronize recent activities.
    Garmin {
        #[command(subcommand)]
        action: super::garmin::GarminCommand,
    },
    /// Create the database and apply forward migrations.
    Init,
    /// Inspect or create the non-secret TOML configuration.
    Config {
        #[command(subcommand)]
        action: ConfigCommand,
    },
    /// Show local history and Garmin integration status.
    Status,
    /// Check HTTPS to every Garmin host through Pace's own client, without sending keys.
    Doctor,
    /// Read locally stored activities.
    Activities {
        #[command(subcommand)]
        action: ActivityCommand,
    },
    /// Start the MCP server with the selected transport.
    Serve {
        #[arg(long, value_enum)]
        transport: Transport,
    },
}

#[derive(Subcommand)]
pub(super) enum ConfigCommand {
    /// Create an example at --config without overwriting an existing file.
    Init,
    /// Validate settings without opening or creating the database.
    Check,
    /// Print the effective non-secret settings (TOML by default).
    Show,
    /// Show or change the Garmin proxy and extra CA file; other settings stay untouched.
    Network {
        /// Proxy for every Garmin request, http(s)://host[:port] without credentials.
        #[arg(long, conflicts_with = "clear_proxy")]
        proxy_url: Option<String>,
        /// Remove the proxy so HTTPS_PROXY/ALL_PROXY/NO_PROXY from the environment apply.
        #[arg(long)]
        clear_proxy: bool,
        /// PEM trust roots added to the platform store; relative to the configuration file.
        #[arg(long, conflicts_with = "clear_ca")]
        extra_ca_file: Option<PathBuf>,
        /// Remove the extra CA file and trust the platform store only.
        #[arg(long)]
        clear_ca: bool,
    },
}

#[derive(Subcommand)]
pub(super) enum ActivityCommand {
    /// Search all saved history by optional UTC dates, exact sport and literal name.
    Search {
        #[arg(long)]
        start: Option<chrono::NaiveDate>,
        #[arg(long)]
        end: Option<chrono::NaiveDate>,
        #[arg(long)]
        sport: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// List a bounded page, newest first (uses the configured default limit).
    List {
        #[arg(long)]
        limit: Option<usize>,
        /// Number of newer activities to skip before the page starts.
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
}

#[derive(Clone, ValueEnum)]
pub(super) enum Transport {
    Stdio,
    Http,
}

/// Execute a CLI command, leaving error reporting to the process entry point.
pub fn run(cli: Cli) -> Result<()> {
    if let Some(Command::Setup {
        quick: true,
        user,
        server_config,
        key_handoff,
        compact,
        batch,
        key_days,
    }) = cli.command
    {
        ensure!(
            cli.env_file.is_none(),
            "quick setup selects its own private credentials"
        );
        let user = user.context("profile name required")?;
        let server = server_config.context("server config required")?;
        if batch {
            return super::terminal::quick_setup_batch(
                cli.config,
                user,
                server,
                key_handoff,
                key_days.context("--key-days required with --batch")?,
                cli.json,
            );
        }
        ensure!(
            !cli.json,
            "the setup wizard requires a terminal and does not support --json"
        );
        return super::terminal::quick_setup(cli.config, user, server, key_handoff, compact);
    }
    let command = match cli.command {
        None | Some(Command::Ui { .. }) | Some(Command::Setup { .. }) => {
            ensure!(
                !cli.json,
                "interactive mode does not support --json; select a command"
            );
            let compact = matches!(
                cli.command,
                Some(Command::Ui { compact: true } | Command::Setup { compact: true, .. })
            );
            let setup = matches!(cli.command, Some(Command::Setup { .. }));
            return super::terminal::run(cli.config, cli.env_file, compact, setup);
        }
        Some(command) => command,
    };
    let mut stdout = std::io::stdout();
    execute(
        &cli.config,
        cli.env_file.as_deref(),
        command,
        Output {
            json: cli.json,
            writer: &mut stdout,
        },
    )
}

pub(super) fn execute(
    config_path: &Path,
    env_file: Option<&Path>,
    command: Command,
    output: Output<'_>,
) -> Result<()> {
    // Each command composes only what it needs: settings, credentials or the access registry.
    let settings = || Settings::load(config_path);
    // The key file resolves like the menu's: --env-file, else garmin.env beside the database.
    let secrets = |settings: &Settings| {
        crate::secrets::Secrets::load(
            crate::secrets::profile_key_file(env_file, &settings.database.path)?.as_deref(),
        )
    };
    let registry_only = || {
        ensure!(
            env_file.is_none(),
            "HTTP and access administration use the registry, not --env-file"
        );
        Ok::<(), anyhow::Error>(())
    };
    match command {
        Command::Config {
            action: ConfigCommand::Init,
        } => {
            config::create(config_path)?;
            output.message(
                &serde_json::json!({"configuration_created": true}),
                "Configuration created.",
            )
        }
        Command::Config {
            action: ConfigCommand::Check,
        } => {
            settings()?;
            output.message(
                &serde_json::json!({"valid": true}),
                "Configuration is valid.",
            )
        }
        Command::Config {
            action: ConfigCommand::Show,
        } => {
            let settings = settings()?;
            output.message(&settings, &toml::to_string_pretty(&settings)?)
        }
        Command::Config {
            action:
                ConfigCommand::Network {
                    proxy_url,
                    clear_proxy,
                    extra_ca_file,
                    clear_ca,
                },
        } => {
            let current = config::garmin_network(config_path)?;
            let wanted = config::GarminNetwork {
                proxy_url: if clear_proxy {
                    None
                } else {
                    proxy_url.or_else(|| current.proxy_url.clone())
                },
                extra_ca_file: if clear_ca {
                    None
                } else {
                    extra_ca_file.or_else(|| current.extra_ca_file.clone())
                },
            };
            let changed = wanted != current;
            if changed {
                config::update_garmin_network(config_path, &wanted, |s| {
                    crate::garmin::check_client(&s.garmin)
                })?;
            }
            output.message(
                &serde_json::json!({
                    "proxy_url": wanted.proxy_url,
                    "extra_ca_file": wanted.extra_ca_file,
                    "changed": changed,
                }),
                &network_text(&wanted, changed),
            )
        }
        Command::InstallationHttpsConfig {
            domain,
            saved_domain,
            domain_only,
        } => installation_https_config(&settings()?, domain, saved_domain, domain_only, output),
        Command::InstallationCheck {
            profile,
            https_domain,
        } => installation_check(&settings()?, &profile, https_domain.as_deref(), output),
        Command::Users { action } => {
            registry_only()?;
            super::users::run(action, &settings()?, output)
        }
        Command::Serve {
            transport: Transport::Http,
        } => {
            registry_only()?;
            serve_http(settings()?)
        }
        Command::Serve {
            transport: Transport::Stdio,
        } => serve_stdio(settings()?),
        Command::Health { action } => {
            let settings = settings()?;
            super::health::run(action, &settings, &secrets(&settings)?, output)
        }
        Command::Benchmark { action } => {
            let settings = settings()?;
            super::benchmarks::run(action, &settings, &secrets(&settings)?, output)
        }
        Command::Sync { start, end, resume } => {
            let settings = settings()?;
            super::full_sync::run(start, end, resume, &settings, &secrets(&settings)?, output)
        }
        Command::Garmin { action } => {
            let settings = settings()?;
            super::garmin::run(action, &settings, &secrets(&settings)?, output)
        }
        Command::Coach { action } => super::coach::run(action, &settings()?, output),
        Command::Workout { action } => super::workouts::run(action, &settings()?, output),
        Command::Init => {
            SqliteHistory::initialize(&settings()?.database)?;
            output.message(
                &serde_json::json!({"database_initialized": true}),
                "Database initialized.",
            )
        }
        Command::Doctor => super::doctor::run(&settings()?, output),
        Command::Status => output.status(history(&settings()?)?.summary()?),
        Command::Activities {
            action:
                ActivityCommand::Search {
                    start,
                    end,
                    sport,
                    name,
                    limit,
                    offset,
                },
        } => {
            let settings = settings()?;
            let query = crate::history::ActivityQuery {
                start,
                end,
                sport,
                name,
                limit: limit.unwrap_or(settings.history.default_page_size),
                offset,
            };
            let page = history(&settings)?.search(&query)?;
            output.activity_search(page, offset)
        }
        Command::Activities {
            action: ActivityCommand::List { limit, offset },
        } => {
            let settings = settings()?;
            let limit = limit.unwrap_or(settings.history.default_page_size);
            output.activities(history(&settings)?.list(limit, offset)?)
        }
        Command::Setup { .. } | Command::Ui { .. } => {
            anyhow::bail!("interactive commands require a terminal; run pace or pace setup")
        }
    }
}

/// Compose the local history service; the terminal shares it for paged activity reads.
pub(super) fn history(settings: &Settings) -> Result<HistoryService> {
    let repository = SqliteHistory::open(settings.database.clone())?;
    Ok(HistoryService::new(
        Arc::new(repository),
        settings.history.max_page_size,
    ))
}

fn network_text(network: &config::GarminNetwork, changed: bool) -> String {
    let proxy = network.proxy_url.as_deref().map_or_else(
        || "not set; HTTPS_PROXY/ALL_PROXY/NO_PROXY from the environment apply".into(),
        |url| format!("{url} (replaces the environment's proxy)"),
    );
    let ca = network.extra_ca_file.as_ref().map_or_else(
        || "not set; the platform trust store only".into(),
        |path| format!("{} (added to the platform trust store)", path.display()),
    );
    let mut text = format!("Proxy: {proxy}\nExtra CA file: {ca}");
    if changed {
        text.push_str("\n\nSaved. A running MCP server applies it after a restart.\nCheck the Garmin path with pace doctor.");
    }
    text
}
fn installation_https_config(
    settings: &Settings,
    domain: Option<String>,
    saved_domain: Option<PathBuf>,
    domain_only: bool,
    output: Output<'_>,
) -> Result<()> {
    let domain = match (domain, saved_domain) {
        (Some(value), None) => crate::installation::PublicDomain::parse(&value)?,
        (None, Some(path)) => crate::installation::load_https_domain(&path)?,
        _ => anyhow::bail!("one HTTPS domain source is required"),
    };
    if domain_only {
        return output.message(
            &serde_json::json!({"domain":domain.as_str()}),
            domain.as_str(),
        );
    }
    let text = crate::installation::proxy_config(
        &domain,
        settings.http.bind,
        settings.http.max_body_bytes,
        settings.http.request_timeout_seconds,
    )?;
    output.message(&serde_json::json!({"caddyfile":text}), text.trim_end())
}

fn installation_check(
    settings: &Settings,
    profile: &Path,
    https_domain: Option<&str>,
    output: Output<'_>,
) -> Result<()> {
    if let Some(value) = https_domain {
        let domain = crate::installation::PublicDomain::parse(value)?;
        crate::installation::check_https(settings, profile, &domain)?;
        return output.message(&serde_json::json!({"tls_verified":true,"dns_path_verified":true,"external_reachability":"unchecked"}), "HTTPS certificate, local authorization and DNS path verified. Test reachability from another network before sharing the URL.");
    }
    crate::installation::check_service(settings, profile)?;
    output.message(
        &serde_json::json!({"ready":true}),
        "Authenticated MCP readiness verified.",
    )
}

fn serve_stdio(settings: Settings) -> Result<()> {
    initialize_logging(&settings)?;
    observe_rate_limits()?;
    let personal_key_path = crate::secrets::profile_key_path(&settings.database.path)?;
    let server = HistoryMcp::new(
        AuthenticatedHistory {
            history: history(&settings)?,
            health: super::health::service(&settings)?,
            workouts: super::workouts::service(&settings)?,
            benchmarks: super::benchmarks::service(&settings)?,
            garmin: settings.runtime.stdio_garmin_writes.then(|| GarminAccess {
                settings: settings.garmin.clone(),
                auth: settings.garmin_auth.clone(),
                health: settings.health.clone(),
                personal_key_path,
            }),
        },
        settings.health.max_days,
        settings.history.default_page_size,
        settings.runtime.blocking_threads,
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(settings.runtime.blocking_threads)
        .build()
        .context("starting async runtime")?;
    runtime.block_on(super::mcp::serve_stdio(server))
}

fn initialize_logging(settings: &Settings) -> Result<()> {
    use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};
    let level = settings
        .runtime
        .log_level
        .parse::<tracing::level_filters::LevelFilter>()?;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_writer(std::io::stderr)
                .with_filter(
                    tracing_subscriber::filter::Targets::new().with_target("garmin_pace", level),
                ),
        )
        .try_init()
        .map_err(|_| anyhow::anyhow!("initializing application logging"))
}

/// Servers have no terminal footer; rate-limit waits inside Garmin tools go to the log.
fn observe_rate_limits() -> Result<()> {
    crate::garmin::observe(Some(Box::new(|wait: &crate::garmin::RateWait| {
        tracing::warn!(
            seconds = wait.seconds,
            attempt = wait.attempt,
            attempts = wait.attempts,
            "Garmin rate limit wait"
        );
        Ok(())
    })))
}

fn serve_http(settings: Settings) -> Result<()> {
    initialize_logging(&settings)?;
    observe_rate_limits()?;
    let access = super::users::service(&settings)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(settings.runtime.blocking_threads)
        .build()
        .context("starting HTTP runtime")?;
    runtime.block_on(http::serve(access, &settings))
}
