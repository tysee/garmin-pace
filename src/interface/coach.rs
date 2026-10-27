//! src/interface/coach.rs exposes explicit Coach imports and offline calendar reads in the CLI.
//! It composes the shared workout service and Garmin adapter; no plan decisions live here.

use super::{output::Output, workouts::service};
use crate::{
    config::Settings,
    garmin::ConnectApi,
    workouts::coach::document::{SnapshotDocument, window},
};
use anyhow::Result;
use clap::Subcommand;
use serde_json::json;

#[derive(Subcommand)]
pub(super) enum CoachCommand {
    /// Import active plan context and scheduled workouts for at most seven calendar days.
    Sync {
        #[arg(long)]
        start: String,
        #[arg(long)]
        end: String,
    },
    /// Read the exact previously imported local calendar window, without contacting Garmin.
    Show {
        #[arg(long)]
        start: String,
        #[arg(long)]
        end: String,
    },
}
pub(super) fn run(command: CoachCommand, settings: &Settings, output: Output) -> Result<()> {
    let service = service(settings)?;
    let snapshot = match command {
        CoachCommand::Sync { start, end } => Some(service.sync_coach(
            &ConnectApi::connect(&settings.garmin, &settings.garmin_auth)?,
            window(&start, &end)?,
        )?),
        CoachCommand::Show { start, end } => service.coach(window(&start, &end)?)?,
    };
    match snapshot {
        Some(s) => {
            let mut lines = vec![format!(
                "Coach {} through {} | synced {}",
                s.window.start, s.window.end, s.synced_at
            )];
            for p in &s.plans {
                lines.push(format!(
                    "Plan {}: {} | {} | {} through {}",
                    p.id, p.name, p.status, p.start, p.end
                ));
            }
            for row in &s.sessions {
                lines.push(format!(
                    "{} | {} | workout {} | schedule {}",
                    row.date, row.workout.name, row.workout.id, row.schedule_id
                ));
            }
            output.message(
                &json!({"status":"available","snapshot":SnapshotDocument::from(s)}),
                &lines.join("\n"),
            )
        }
        None => output.message(
            &json!({"status":"not_synced","snapshot":null}),
            "This Coach window has not been imported. Run coach sync explicitly.",
        ),
    }
}
