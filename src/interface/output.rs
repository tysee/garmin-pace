//! src/interface/output.rs renders concise terminal output or explicit JSON for CLI consumers.
//! Formatting stays at the edge; MCP continues to use its own structured responses.

use super::responses::{ActivityPage, StatusResponse};
use crate::{
    history::{Activity, HistorySummary},
    workouts::Device,
};
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::json;
use std::fmt::Write as _;

pub(super) struct Output<'a> {
    pub json: bool,
    pub writer: &'a mut dyn std::io::Write,
}
impl Output<'_> {
    pub(super) fn activity_search(
        self,
        page: crate::history::ActivitySearchPage,
        offset: usize,
    ) -> Result<()> {
        let next = offset + page.activities.len();
        let mut human = Vec::new();
        Output {
            json: false,
            writer: &mut human,
        }
        .activities(page.activities.clone())?;
        let text = format!(
            "Matching saved activities: {}\n{}",
            page.total,
            String::from_utf8(human)?
        );
        let mut value = json!(ActivityPage::from(page.activities));
        value["total"] = json!(page.total);
        value["offset"] = json!(offset);
        value["next_offset"] = json!(((next as u64) < page.total).then_some(next));
        value["scope"] = json!("local_snapshot");
        self.message(&value, &text)
    }
    pub(super) fn message(self, value: &impl Serialize, human: &str) -> Result<()> {
        let mut output = self.writer;
        if self.json {
            serde_json::to_writer_pretty(&mut output, value).context("encoding CLI response")?;
            writeln!(output).context("writing CLI response")
        } else {
            writeln!(output, "{human}").context("writing CLI response")
        }
    }
    pub(super) fn status(self, summary: HistorySummary) -> Result<()> {
        let last = match &summary.last_sync {
            Some(sync) => format!(
                "{} ({} received, limit {})",
                sync.completed_at, sync.received_count, sync.requested_limit
            ),
            None => "never".into(),
        };
        let human = format!(
            "Stored activities: {}\nLast sync: {last}\nScope: imported recent pages",
            summary.activity_count
        );
        self.message(&StatusResponse::from(summary), &human)
    }
    pub(super) fn activities(self, activities: Vec<Activity>) -> Result<()> {
        let mut human = if activities.is_empty() {
            "No stored activities.".into()
        } else {
            format!("Activities: {}\n", activities.len())
        };
        for activity in &activities {
            let distance = match activity.distance_meters {
                Some(meters) => format!("{:.2} km", meters / 1000.0),
                None => "distance: no data".into(),
            };
            writeln!(
                human,
                "{} | {} | {:.1} min | {} | {}",
                activity.started_at,
                line(&activity.sport),
                activity.duration_seconds / 60.0,
                distance,
                line(&activity.name)
            )?;
        }
        self.message(&ActivityPage::from(activities), human.trim_end())
    }
    pub(super) fn devices(self, devices: Vec<Device>) -> Result<()> {
        let mut human = if devices.is_empty() {
            "No registered Garmin devices.".into()
        } else {
            format!("Registered devices: {}\n", devices.len())
        };
        for device in &devices {
            writeln!(human, "  {} (ID: {})", line(&device.model), device.id)?;
        }
        let rows: Vec<_> = devices
            .into_iter()
            .map(|d| json!({"device_id": d.id, "model": d.model}))
            .collect();
        self.message(&json!({"devices": rows}), human.trim_end())
    }
}

fn line(value: &str) -> String {
    value.chars().flat_map(char::escape_debug).collect()
}
