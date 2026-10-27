//! src/interface/benchmarks.rs exposes explicit strength imports and model proposal review.
//! CLI and terminal delegate validation and journaled saves to the benchmark feature.

use super::{benchmark_view, output::Output};
use crate::{
    config::Settings,
    garmin::GarminBenchmarks,
    health::Date,
    history::Activity,
    secrets::Secrets,
    workouts::{
        WorkoutError,
        benchmarks::{
            ActivitySets, BenchmarkService, Proposal, SqliteBenchmarks, Window,
            document::{BenchmarkDocument, ContextDocument, DraftDocument, ProposalDocument},
        },
    },
};
use anyhow::{Context, Result, ensure};
use chrono::DateTime;
use clap::Subcommand;
use serde_json::json;
use std::{io::Read, path::PathBuf, sync::Arc};

#[derive(Subcommand)]
pub(super) enum BenchmarkCommand {
    /// Import benchmarks and actual strength sets for an inclusive UTC window (at most 31 days).
    Sync {
        #[arg(long)]
        start: Date,
        #[arg(long)]
        end: Date,
    },
    /// List known benchmarks, including unset entries; no network access.
    List,
    /// Read observed averages for an exact exercise and UTC evidence window.
    Context {
        #[arg(long)]
        key: String,
        #[arg(long)]
        start: Date,
        #[arg(long)]
        end: Date,
    },
    /// Import a model's v1 estimate, source revisions and authorized trigger as a local proposal.
    Propose {
        #[arg(long)]
        file: PathBuf,
    },
    /// List local proposals, including unresolved saves.
    Drafts {
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Review before/after values and the model's evidence and reasoning.
    Review {
        #[arg(long)]
        id: u64,
    },
    /// Correct an unsaved proposal using its current revision.
    Revise {
        #[arg(long)]
        id: u64,
        #[arg(long)]
        revision: u64,
        #[arg(long)]
        file: PathBuf,
    },
    /// Save exactly the reviewed revision to Garmin and verify its values.
    Save {
        #[arg(long)]
        id: u64,
        #[arg(long)]
        revision: u64,
    },
    /// Read Garmin to resolve a pending save; never retry the write.
    Reconcile {
        #[arg(long)]
        id: u64,
    },
    /// Discard an unsaved local proposal without changing Garmin.
    Discard {
        #[arg(long)]
        id: u64,
        #[arg(long)]
        revision: u64,
    },
    /// Give up on an unresolved save after checking Garmin yourself; nothing is retried.
    Abandon {
        #[arg(long)]
        id: u64,
        #[arg(long)]
        revision: u64,
    },
}
pub(super) fn service(settings: &Settings) -> Result<BenchmarkService> {
    Ok(BenchmarkService::new(Arc::new(SqliteBenchmarks::open(
        settings.database.clone(),
    )?)))
}
/// Read the imported sets of one stored strength activity by its Garmin ID and UTC start date.
pub(super) fn activity_sets(
    service: &BenchmarkService,
    activity: &Activity,
) -> Result<ActivitySets, WorkoutError> {
    if activity.sport != "strength_training" {
        return Err(WorkoutError::InvalidInput(
            "exercise sets exist only for strength training activities",
        ));
    }
    let id = activity
        .garmin_id
        .parse::<u64>()
        .map_err(|_| WorkoutError::InvalidInput("stored activity has a non-numeric Garmin ID"))?;
    let date = DateTime::parse_from_rfc3339(&activity.started_at)
        .map_err(|_| WorkoutError::InvalidInput("stored activity has an invalid start time"))?
        .date_naive();
    service.activity_sets(id, Date::from(date))
}
fn document(file: PathBuf) -> Result<Proposal> {
    let mut bytes = Vec::new();
    std::fs::File::open(file)
        .context("opening model benchmark proposal")?
        .take(65_537)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 65_536, "benchmark proposal exceeds 64 KiB");
    let doc: ProposalDocument =
        serde_json::from_slice(&bytes).context("invalid v1 benchmark proposal")?;
    Ok(doc.try_into()?)
}
pub(super) fn run(
    command: BenchmarkCommand,
    settings: &Settings,
    secrets: &Secrets,
    output: Output<'_>,
) -> Result<()> {
    let service = service(settings)?;
    let connect = || GarminBenchmarks::connect(&settings.garmin, &settings.garmin_auth, None);
    let draft = match command {
        BenchmarkCommand::Sync { start, end } => {
            let window = Window { start, end };
            window.validate()?;
            let gateway = GarminBenchmarks::connect(
                &settings.garmin,
                &settings.garmin_auth,
                Some(secrets.require("GARMIN_PERSONAL_API_KEY")?),
            )?;
            let count = service.sync(&gateway, window)?;
            return output.message(&json!({"sessions":count,"start":start.to_string(),"end":end.to_string()}),
                &format!("Imported benchmarks and {count} strength sessions for {start} to {end} UTC.\nNo benchmark values changed. Ask your model to review this evidence when needed."));
        }
        BenchmarkCommand::List => {
            let rows = service.catalog()?;
            let text = if rows.is_empty() {
                "No local benchmarks. Sync strength data first.".into()
            } else {
                rows.iter()
                    .map(benchmark_view::benchmark)
                    .collect::<Vec<_>>()
                    .join("\n\n")
            };
            return output.message(&json!({"benchmarks":rows.into_iter().map(BenchmarkDocument::from).collect::<Vec<_>>()}), &text);
        }
        BenchmarkCommand::Context { key, start, end } => {
            let context = service.context(&key, Window { start, end })?;
            let text = benchmark_view::context(&context);
            return output.message(&ContextDocument::from(context), &text);
        }
        BenchmarkCommand::Drafts { limit, offset } => {
            let rows = service.drafts(limit, offset)?;
            let text = if rows.is_empty() {
                "No benchmark proposals. Ask your MCP model when an estimate is needed.".into()
            } else {
                rows.iter()
                    .map(|d| {
                        format!(
                            "#{} | {} | revision {} | {}",
                            d.id,
                            benchmark_view::name(&d.key),
                            d.revision,
                            benchmark_view::state(d.state)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            return output.message(&json!({"drafts":rows.iter().map(benchmark_view::summary).collect::<Vec<_>>(),"limit":limit,"offset":offset}), &text);
        }
        BenchmarkCommand::Propose { file } => service.propose(document(file)?)?,
        BenchmarkCommand::Review { id } => service.draft(id)?,
        BenchmarkCommand::Revise { id, revision, file } => {
            service.revise(id, revision, document(file)?)?
        }
        BenchmarkCommand::Save { id, revision } => service
            .save(&connect()?, id, revision, None)
            .with_context(|| {
                format!("saving benchmark proposal {id}; inspect its state before retrying")
            })?,
        BenchmarkCommand::Reconcile { id } => service.reconcile(&connect()?, id, None)?,
        BenchmarkCommand::Discard { id, revision } => {
            service.discard(id, revision)?;
            service.draft(id)?
        }
        BenchmarkCommand::Abandon { id, revision } => {
            service.abandon(id, revision)?;
            service.draft(id)?
        }
    };
    let text = benchmark_view::review(&draft);
    output.message(&DraftDocument::from(draft), &text)
}
