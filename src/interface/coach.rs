//! src/interface/coach.rs exposes explicit Coach imports, offline calendar reads and the
//! journaled move of one scheduled session in the CLI. It composes the shared workout service
//! and Garmin adapter; no plan or date decisions live here.

use super::{output::Output, workouts::service};
use crate::{
    config::Settings,
    garmin::ConnectApi,
    health::Date,
    workouts::coach::{
        Reschedule, RescheduleState,
        document::{RescheduleDocument, SnapshotDocument, window},
    },
};
use anyhow::Result;
use clap::Subcommand;
use serde_json::{Value, json};

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
    /// Move one imported session to another Garmin calendar date; writes to Garmin at once.
    Reschedule {
        /// Schedule ID from `coach show`, not the workout ID.
        #[arg(long)]
        schedule_id: u64,
        /// New Garmin calendar date (YYYY-MM-DD); Garmin decides whether it is allowed.
        #[arg(long)]
        date: Date,
    },
    /// Resolve an unresolved move by reading the calendar entry back; never writes.
    ReconcileReschedule {
        #[arg(long)]
        schedule_id: u64,
    },
    /// Close an unresolved move after checking the Garmin calendar yourself.
    ClearReschedule {
        #[arg(long)]
        schedule_id: u64,
    },
}
/// Human-readable journal line shared by the CLI and the menu.
pub(super) fn reschedule_line(r: &Reschedule) -> String {
    format!(
        "{} | schedule {} | workout {} | {} -> {} | requested {}",
        reschedule_label(r.state),
        r.schedule_id,
        r.workout_id,
        r.from,
        r.to,
        r.requested_at
    )
}
pub(super) fn reschedule_label(state: RescheduleState) -> &'static str {
    match state {
        RescheduleState::Moving => "Unresolved move",
        RescheduleState::Moved => "Moved",
        RescheduleState::NotApplied => "Not applied",
        RescheduleState::Missing => "Entry missing",
    }
}
pub(super) fn reschedule_note(state: RescheduleState) -> &'static str {
    match state {
        RescheduleState::Moving => {
            "Garmin's answer is unknown. Reconcile reads the entry back; clear it only after checking the calendar yourself."
        }
        RescheduleState::Moved => {
            "Garmin returned the entry on the new date. Sync both affected weeks before trusting their snapshots."
        }
        RescheduleState::NotApplied => "Garmin did not apply the move; the session kept its date.",
        RescheduleState::Missing => {
            "Garmin no longer returns this calendar entry. Check the plan in Garmin Connect."
        }
    }
}
fn reschedule_output(output: Output, r: Reschedule) -> Result<()> {
    let text = format!(
        "{}
{}
{}",
        reschedule_line(&r),
        reschedule_note(r.state),
        super::workout_view::mcp_requests(r.request.as_deref())
    );
    output.message(
        &json!({"reschedule": RescheduleDocument::from(r)}),
        text.trim_end(),
    )
}
pub(super) fn run(command: CoachCommand, settings: &Settings, output: Output) -> Result<()> {
    let service = service(settings)?;
    let connect = || ConnectApi::connect(&settings.garmin, &settings.garmin_auth);
    let (snapshot, dates) = match command {
        CoachCommand::Sync { start, end } => {
            let dates = window(&start, &end)?;
            (Some(service.sync_coach(&connect()?, dates)?), dates)
        }
        CoachCommand::Show { start, end } => {
            let dates = window(&start, &end)?;
            (service.coach(dates)?, dates)
        }
        CoachCommand::Reschedule { schedule_id, date } => {
            let moved = service.reschedule(&connect()?, schedule_id, date, None)?;
            return reschedule_output(output, moved);
        }
        CoachCommand::ReconcileReschedule { schedule_id } => {
            let settled = service.reconcile_reschedule(&connect()?, schedule_id, None)?;
            return reschedule_output(output, settled);
        }
        CoachCommand::ClearReschedule { schedule_id } => {
            service.clear_reschedule(schedule_id)?;
            return output.message(
                &json!({"schedule_id":schedule_id,"reschedule_cleared":true}),
                "Unresolved move closed as not applied. Garmin was not contacted.",
            );
        }
    };
    let moves = service.coach_reschedules(dates)?;
    let journal: Vec<Value> = moves
        .iter()
        .cloned()
        .map(|r| json!(RescheduleDocument::from(r)))
        .collect();
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
            if !moves.is_empty() {
                lines.push(
                    "Journaled moves touching this window (snapshot dates are not rewritten):"
                        .into(),
                );
                lines.extend(moves.iter().map(reschedule_line));
            }
            output.message(
                &json!({"status":"available","snapshot":SnapshotDocument::from(s),"reschedules":journal}),
                &lines.join("\n"),
            )
        }
        None => output.message(
            &json!({"status":"not_synced","snapshot":null,"reschedules":journal}),
            "This Coach window has not been imported. Run coach sync explicitly.",
        ),
    }
}
