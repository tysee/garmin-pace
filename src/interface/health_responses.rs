//! src/interface/health_responses.rs shapes the shared v1 CLI and MCP health responses.
//! Missing synchronization, empty datasets and sample pagination remain explicit.

use crate::health::{Date, HealthError, HealthService, Metric, document::DayDocument};
use serde_json::{Value, json};

pub(super) fn day(service: &HealthService, date: Date) -> Result<Value, HealthError> {
    let days = service.days(date, date)?;
    match days.into_iter().next() {
        Some(day) => serde_json::to_value(DayDocument::from(day)).map_err(encode_error),
        None => Ok(
            json!({"contract_version":1,"scope":"local_snapshot","date":date.to_string(),"status":"not_synced","synced_at":null}),
        ),
    }
}
pub(super) fn days(
    service: &HealthService,
    start: Date,
    end: Date,
    max_days: usize,
) -> Result<Value, HealthError> {
    let dates = start.range(end, max_days)?;
    let days = service.days(start, end)?;
    let missing: Vec<_> = dates
        .into_iter()
        .filter(|d| !days.iter().any(|day| day.date == *d))
        .map(|d| d.to_string())
        .collect();
    let documents: Vec<_> = days.into_iter().map(DayDocument::from).collect();
    Ok(
        json!({"contract_version":1,"scope":"local_snapshot","days":documents,"missing_dates":missing}),
    )
}
pub(super) fn samples(
    service: &HealthService,
    date: Date,
    metric: Metric,
    limit: usize,
    offset: usize,
) -> Result<Value, HealthError> {
    let page = service.samples(date, metric, limit, offset)?;
    let status = if page.synced_at.is_none() {
        "not_synced"
    } else if page.total == 0 {
        "no_data"
    } else {
        "available"
    };
    let semantics = if metric == Metric::BodyBattery {
        "interval_extrema"
    } else {
        "reported_measurements"
    };
    let next = page.offset + page.samples.len();
    let next = (next < page.total).then_some(next);
    let points: Vec<_> = page
        .samples
        .into_iter()
        .map(|s| json!({"timestamp":s.timestamp,"value":s.value,"unit":s.unit}))
        .collect();
    Ok(
        json!({"contract_version":1,"scope":"local_snapshot","date":date.to_string(),"metric":metric.as_str(),"status":status,
        "synced_at":page.synced_at,"sample_semantics":semantics,"total":page.total,"offset":offset,"next_offset":next,"samples":points}),
    )
}
fn encode_error(error: serde_json::Error) -> HealthError {
    HealthError::Storage(Box::new(error))
}
