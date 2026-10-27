//! src/interface/workouts.rs exposes the library, model-authored drafts and explicit Garmin writes.
//! Files use the same bounded v1 contract as MCP; all decisions are delegated to WorkoutService.

use super::{output::Output, workout_view};
use crate::{
    config::Settings,
    garmin::ConnectApi,
    workouts::{
        SqliteWorkouts, Workout, WorkoutService,
        document::{LibraryDocument, WorkoutDocument},
    },
};
use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use serde_json::json;
use std::{io::Read, path::PathBuf, sync::Arc};

#[derive(Subcommand)]
pub(super) enum WorkoutCommand {
    /// Read the local delivery journal; queued never confirms physical watch receipt.
    Deliveries {
        #[arg(long)]
        id: u64,
        #[arg(long)]
        device_id: Option<u64>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Import one explicit page of reusable workouts from Garmin (not completed activities).
    Sync {
        #[arg(long, default_value_t = 0)]
        start: usize,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Browse the local workout library without contacting Garmin.
    List {
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Exclude workouts owned by Coach plans, as in the terminal Workouts menu.
        #[arg(long)]
        standalone: bool,
    },
    /// Read a stored definition and the revision needed for editing it.
    Show {
        #[arg(long)]
        id: u64,
    },
    /// Import a model-authored v1 JSON candidate as a local draft.
    Draft {
        #[arg(long)]
        file: PathBuf,
        #[arg(long, requires = "source_revision")]
        source_id: Option<u64>,
        #[arg(long, requires = "source_id")]
        source_revision: Option<String>,
        /// Evidence and specific changes; required for Coach source workouts.
        #[arg(long)]
        explanation: Option<String>,
    },
    /// Browse local drafts, including unresolved and verified saves.
    Drafts {
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show the complete before/after definition and draft revision.
    Review {
        #[arg(long)]
        id: u64,
    },
    /// Replace an unsaved draft with a new model-authored candidate.
    Revise {
        #[arg(long)]
        id: u64,
        #[arg(long)]
        revision: u64,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        explanation: Option<String>,
    },
    /// Save exactly the reviewed revision to Garmin, then read back and verify.
    Save {
        #[arg(long)]
        id: u64,
        #[arg(long)]
        revision: u64,
    },
    /// Resolve an uncertain save against an explicitly chosen Garmin workout ID.
    Reconcile {
        #[arg(long)]
        id: u64,
        #[arg(long)]
        workout_id: u64,
    },
    /// Discard an unsaved local draft; never delete from Garmin.
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
    /// Request delivery of a library workout to a registered device.
    Send {
        #[arg(long)]
        id: u64,
        #[arg(long)]
        device_id: u64,
    },
    /// Clear an unresolved delivery reservation after checking the watch and Garmin.
    ClearDelivery {
        #[arg(long)]
        id: u64,
        #[arg(long)]
        device_id: u64,
    },
}
pub(super) fn service(settings: &Settings) -> Result<WorkoutService> {
    Ok(WorkoutService::new(Arc::new(SqliteWorkouts::open(
        settings.database.clone(),
    )?)))
}
fn document(file: PathBuf) -> Result<Workout> {
    let mut bytes = Vec::new();
    std::fs::File::open(file)
        .context("opening model workout document")?
        .take(65_537)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 65_536, "workout document exceeds 64 KiB");
    let document: WorkoutDocument =
        serde_json::from_slice(&bytes).context("invalid v1 workout document")?;
    Ok(document.try_into()?)
}
pub(super) fn run(command: WorkoutCommand, settings: &Settings, output: Output<'_>) -> Result<()> {
    let service = service(settings)?;
    let connect = || ConnectApi::connect(&settings.garmin, &settings.garmin_auth);
    let draft = match command {
        WorkoutCommand::Deliveries {
            id,
            device_id,
            limit,
            offset,
        } => {
            let workout = service.get(id)?;
            let rows = match device_id {
                Some(device) => service.deliveries(id, device, limit, offset)?,
                None => service.delivery_history(id, limit, offset)?,
            };
            let mut text = format!("Workout {id}\nWatch receipt: unconfirmed.\n");
            if rows.is_empty() {
                text.push_str("No recorded requests on this page.");
            }
            for row in &rows {
                text.push_str(&format!(
                    "Device {} | {} | {} | revision {}\n",
                    row.device_id,
                    super::workout_view::delivery_label(row.queued),
                    if row.revision == workout.revision {
                        "current version"
                    } else {
                        "older version"
                    },
                    row.revision
                ));
            }
            return output.message(&json!({"workout_id":id,"watch_receipt":"unconfirmed","offset":offset,
                "deliveries":rows.iter().map(|d| json!({"device_id":d.device_id,"revision":d.revision,
                    "current_version":d.revision==workout.revision,"state":if d.queued {"queued"} else {"sending"},
                    "status_label":super::workout_view::delivery_label(d.queued)})).collect::<Vec<_>>()}), &text);
        }
        WorkoutCommand::List {
            offset,
            limit,
            standalone,
        } => {
            let rows = if standalone {
                service.standalone(limit, offset)?
            } else {
                service.list(limit, offset)?
            };
            let text = if rows.is_empty() && standalone {
                "No standalone workouts in the local library. Coach workouts are shown in Coach."
                    .into()
            } else if rows.is_empty() {
                "No local workouts. Sync the library from Garmin first.".into()
            } else {
                rows.iter()
                    .map(|r| {
                        format!(
                            "{} | {} | {}{}",
                            r.id,
                            r.name,
                            r.sport,
                            if r.editable { "" } else { " | read-only" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            return output.message(&json!({"workouts":rows.iter().map(workout_view::summary).collect::<Vec<_>>(),"offset":offset,"limit":limit}),&text);
        }
        WorkoutCommand::Show { id } => {
            let row = service.get(id)?;
            let text = workout_view::library(&row);
            return output.message(&LibraryDocument::from(row), &text);
        }
        WorkoutCommand::Sync { start, limit } => {
            let received = service.sync(&connect()?, start, limit)?;
            return output.message(&json!({"received":received,"start":start,"limit":limit}),&format!("Imported {received} workouts from Garmin (start {start}, limit {limit}).\nThis is one page; sync later pages to browse the rest."));
        }
        WorkoutCommand::Draft {
            file,
            source_id,
            source_revision,
            explanation,
        } => {
            let source = match (source_id, source_revision) {
                (Some(id), Some(rev)) => Some((id, rev)),
                (None, None) => None,
                _ => anyhow::bail!("source ID and revision must be supplied together"),
            };
            service.propose_explained(document(file)?, source, explanation.as_deref())?
        }
        WorkoutCommand::Revise {
            id,
            revision,
            file,
            explanation,
        } => service.revise_explained(id, revision, document(file)?, explanation.as_deref())?,
        WorkoutCommand::Drafts { offset, limit } => {
            let rows = service.drafts(limit, offset)?;
            let text = if rows.is_empty() {
                "No drafts. Ask your connected model to create_workout_draft_v1.".into()
            } else {
                rows.iter()
                    .map(|d| {
                        format!(
                            "{} | revision {} | {} | {}",
                            d.id,
                            d.revision,
                            d.workout.name,
                            workout_view::state(d.state)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            return output.message(&json!({"drafts":rows.iter().map(workout_view::draft_summary).collect::<Vec<_>>(),"offset":offset,"limit":limit}),&text);
        }
        WorkoutCommand::Review { id } => service.draft(id)?,
        WorkoutCommand::Save { id, revision } => service
            .save(&connect()?, id, revision, None)
            .with_context(|| {
                format!("saving draft {id}; inspect its state with workout review before retrying")
            })?,
        WorkoutCommand::Reconcile { id, workout_id } => {
            service.reconcile(&connect()?, id, workout_id, None)?
        }
        WorkoutCommand::Discard { id, revision } => {
            service.discard(id, revision)?;
            service.draft(id)?
        }
        WorkoutCommand::Abandon { id, revision } => {
            service.abandon(id, revision)?;
            service.draft(id)?
        }
        WorkoutCommand::Send { id, device_id } => {
            let sent = service.send(&connect()?, id, device_id, None)?;
            return output.message(&json!({"workout_id":sent.workout_id,"device_id":sent.device_id,"queued":sent.queued,
                "status_label":super::workout_view::delivery_label(sent.queued),
                "status_detail":super::workout_view::DELIVERY_RECEIPT_NOTE,"watch_receipt":"unconfirmed","mcp_request":sent.request}),
                &format!("{}\n{}",super::workout_view::delivery_label(sent.queued),super::workout_view::DELIVERY_RECEIPT_NOTE));
        }
        WorkoutCommand::ClearDelivery { id, device_id } => {
            service.clear_delivery(id, device_id)?;
            return output.message(
                &json!({"workout_id":id,"device_id":device_id,"delivery_cleared":true}),
                "Unresolved delivery reservation cleared. Garmin was not contacted.",
            );
        }
    };
    output.message(
        &workout_view::draft_json(&draft),
        &workout_view::review(&draft),
    )
}
