//! src/garmin/health_samples.rs decodes descriptor-based Garmin time series.
//! Columns come from their descriptors; timestamps are explicitly normalized to UTC.

use crate::health::{Limits, Sample};
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use serde_json::Value;

pub(super) fn timestamp(value: &Value) -> Result<String> {
    let time = if let Some(ms) = value.as_i64() {
        DateTime::from_timestamp_millis(ms).context("invalid health timestamp")?
    } else {
        let text = value.as_str().context("expected health UTC timestamp")?;
        DateTime::parse_from_rfc3339(text)
            .map(|t| t.with_timezone(&Utc))
            .or_else(|_| {
                NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f").map(|t| t.and_utc())
            })
            .context("invalid health UTC timestamp")?
    };
    Ok(time.to_rfc3339_opts(SecondsFormat::Millis, true))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn series(
    payload: &Value,
    rows_key: &str,
    descriptors_key: &str,
    index_key: &str,
    name_key: &str,
    value_key: &str,
    unit: &str,
    limits: Limits,
) -> Result<Vec<Sample>> {
    let Some(rows) = payload.get(rows_key).filter(|v| !v.is_null()) else {
        return Ok(vec![]);
    };
    let rows = rows.as_array().context("expected sample array")?;
    ensure!(
        rows.len() <= limits.max_samples_per_day,
        "too many time-series samples"
    );
    if rows.is_empty() {
        return Ok(vec![]);
    }
    let descriptors = payload
        .get(descriptors_key)
        .and_then(Value::as_array)
        .context("missing sample descriptors")?;
    let column = |name: &str| -> Result<usize> {
        let matches: Vec<_> = descriptors
            .iter()
            .filter(|d| {
                d.get(name_key)
                    .and_then(Value::as_str)
                    .is_some_and(|key| key.eq_ignore_ascii_case(name))
            })
            .collect();
        ensure!(
            matches.len() == 1,
            "missing or duplicate sample column descriptor"
        );
        let index = matches[0]
            .get(index_key)
            .and_then(Value::as_u64)
            .context("invalid sample column index")?;
        ensure!(index <= 32, "sample column index exceeds bound");
        Ok(index as usize)
    };
    let time_index = column("timestamp")?;
    let value_index = column(value_key)?;
    ensure!(time_index != value_index, "duplicate sample column indexes");
    rows.iter()
        .map(|row| {
            let row = row.as_array().context("expected sample row")?;
            Ok(Sample {
                timestamp: timestamp(row.get(time_index).context("missing sample time")?)?,
                value: super::health_decode::number(
                    row.get(value_index).context("missing sample value")?,
                    unit,
                )?,
                unit: unit.into(),
            })
        })
        .collect()
}
