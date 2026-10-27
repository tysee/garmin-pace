//! src/interface/cli.rs defines and dispatches the CLI. It composes the history
//! feature with validated settings and starts a bounded runtime only for servers.

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use std::{
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
#[command(name = "pace", version, about)]
pub struct Cli {
    #[arg(long, env = "GARMIN_PACE_CONFIG")]
    config: PathBuf,
    /// Read credentials from a private dotenv file without executing it.
    #[arg(long, global = true)]
    env_file: Option<PathBuf>,
    /// Print structured JSON instead of human-readable command output.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<Command>,
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
    let secrets = || crate::secrets::Secrets::load(env_file);
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
        Command::Health { action } => super::health::run(action, &settings()?, &secrets()?, output),
        Command::Benchmark { action } => {
            super::benchmarks::run(action, &settings()?, &secrets()?, output)
        }
        Command::Garmin { action } => super::garmin::run(action, &settings()?, &secrets()?, output),
        Command::Coach { action } => super::coach::run(action, &settings()?, output),
        Command::Workout { action } => super::workouts::run(action, &settings()?, output),
        Command::Init => {
            SqliteHistory::initialize(&settings()?.database)?;
            output.message(
                &serde_json::json!({"database_initialized": true}),
                "Database initialized.",
            )
        }
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
