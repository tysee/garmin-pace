//! src/garmin/health_decode.rs validates Garmin wellness envelopes and normalizes known fields.
//! It never turns request errors, malformed records or unexpected dates into empty history.

use super::{health_fields as fields, health_samples};
use crate::health::{Date, Limits, Measurement, Metric, Record, Sample, Section, Value};
use anyhow::{Context, Result, ensure};
use serde_json::Value as Json;
use std::collections::BTreeMap;

pub(super) fn decode(
    date: Date,
    metric: Metric,
    payload: Json,
    limits: Limits,
) -> Result<(Section, Vec<Sample>)> {
    let (records, mut samples) = match metric {
        Metric::Daily => (
            single(&payload, date, fields::DAILY, Some("lastSyncTimestampGMT"))?,
            vec![],
        ),
        Metric::Sleep if empty_sleep_observation(&payload) => (vec![], vec![]),
        Metric::Sleep => {
            let mut records = single(&payload, date, fields::SLEEP, Some("sleepEndTimestampGMT"))?;
            if let Some(record) = records.first_mut() {
                for (field, name) in [
                    ("sleepStartTimestampGMT", "start_time"),
                    ("sleepEndTimestampGMT", "end_time"),
                ] {
                    if let Some(time) = payload.get(field).filter(|v| !v.is_null()) {
                        record.values.insert(
                            name.into(),
                            Measurement {
                                value: Value::Text(health_samples::timestamp(time)?),
                                unit: None,
                            },
                        );
                    }
                }
            }
            (records, vec![])
        }
        Metric::Hrv => hrv(&payload, date, limits)?,
        Metric::HeartRate => (
            single(&payload, date, fields::HEART_RATE, None)?,
            health_samples::series(
                &payload,
                "heartRateValues",
                "heartRateValueDescriptors",
                "index",
                "key",
                "heartrate",
                "bpm",
                limits,
            )?,
        ),
        Metric::BodyBattery => body_battery(&payload, date, limits)?,
        Metric::Spo2 => (
            single(&payload, date, fields::SPO2, Some("latestSpO2TimestampGMT"))?,
            health_samples::series(
                &payload,
                "spO2SingleValues",
                "spO2SingleValuesDescriptorList",
                "spo2ValueDescriptorIndex",
                "spo2ValueDescriptorKey",
                "spo2",
                "%",
                limits,
            )?,
        ),
        Metric::Weight => (weight(&payload, date, limits)?, vec![]),
        Metric::TrainingReadiness => (readiness(&payload, date, limits)?, vec![]),
    };
    ensure!(
        records.len() <= limits.max_records_per_metric,
        "too many health records"
    );
    ensure!(
        samples.len() <= limits.max_samples_per_day,
        "too many health samples"
    );
    samples.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    Ok((
        Section {
            metric,
            records,
            sample_count: samples.len(),
        },
        samples,
    ))
}

fn single(
    payload: &Json,
    date: Date,
    fields: &[fields::Field],
    time: Option<&str>,
) -> Result<Vec<Record>> {
    if empty(payload) {
        return Ok(vec![]);
    }
    check_date(payload, "calendarDate", date)?;
    let record = record(payload, fields, time)?;
    Ok(if record.values.is_empty() {
        vec![]
    } else {
        vec![record]
    })
}
pub(super) fn empty(value: &Json) -> bool {
    value.is_null() || value.as_object().is_some_and(|o| o.is_empty())
}
// Garmin can return a placeholder containing recommendations but no observed sleep.
// Never infer a date for a populated observation or ignore an unknown populated field.
fn empty_sleep_observation(payload: &Json) -> bool {
    payload.get("id") == Some(&Json::Null)
        && payload.get("calendarDate") == Some(&Json::Null)
        && payload.as_object().is_some_and(|fields| {
            fields.iter().all(|(key, value)| {
                value.is_null()
                    || match key.as_str() {
                        "deviceRemCapable" | "retro" => value.is_boolean(),
                        "sleepNeed" => value.is_object(),
                        _ => false,
                    }
            })
        })
}
fn check_date(payload: &Json, key: &str, date: Date) -> Result<()> {
    ensure!(payload.is_object(), "expected health object");
    let supplied = payload
        .get(key)
        .and_then(Json::as_str)
        .context("missing health calendar date")?;
    ensure!(
        supplied == date.to_string(),
        "health response calendar date differs from request"
    );
    Ok(())
}
fn record(payload: &Json, fields: &[fields::Field], time: Option<&str>) -> Result<Record> {
    ensure!(payload.is_object(), "expected health record");
    let mut values = BTreeMap::new();
    for &(path, name, unit) in fields {
        let value = if path.starts_with('/') {
            payload.pointer(path)
        } else {
            payload.get(path)
        };
        let Some(value) = value.filter(|v| !v.is_null()) else {
            continue;
        };
        let measurement = if unit == "text" {
            let text = value
                .as_str()
                .context("expected qualitative health value")?;
            ensure!(text.len() <= 512, "health label too long");
            Measurement {
                value: Value::Text(text.into()),
                unit: None,
            }
        } else {
            let Some(number) = number(value, unit)? else {
                continue;
            };
            // Garmin's weight service reports grams; the v1 contract exposes kilograms.
            let (number, unit) = if unit == "g" {
                (number / 1000.0, "kg")
            } else {
                (number, unit)
            };
            Measurement {
                value: Value::Number(number),
                unit: Some(unit.into()),
            }
        };
        values.insert(name.into(), measurement);
    }
    let observed_at = time
        .and_then(|key| payload.get(key))
        .filter(|v| !v.is_null())
        .map(health_samples::timestamp)
        .transpose()?;
    Ok(Record {
        observed_at,
        values,
    })
}
pub(super) fn number(value: &Json, unit: &str) -> Result<Option<f64>> {
    if value.is_null() {
        return Ok(None);
    }
    let n = value.as_f64().context("expected numeric health value")?;
    // Garmin uses -1/-2 for unmeasured/invalid readings; retain gaps as null samples.
    if n == -1.0 || n == -2.0 {
        return Ok(None);
    }
    let max = match unit {
        "score" | "%" => 100.0,
        "bpm" => 350.0,
        "kg" => 500.0,
        "g" => 500_000.0,
        _ => 1_000_000_000.0,
    };
    ensure!(
        n.is_finite() && n >= 0.0 && n <= max,
        "health value outside supported unit bounds"
    );
    Ok(Some(n))
}
fn hrv(payload: &Json, date: Date, limits: Limits) -> Result<(Vec<Record>, Vec<Sample>)> {
    if empty(payload) {
        return Ok((vec![], vec![]));
    }
    let summary = payload
        .get("hrvSummary")
        .context("missing HRV summary envelope")?;
    let records = single(summary, date, fields::HRV, None)?;
    let mut samples = Vec::new();
    if let Some(rows) = payload.get("hrvReadings").filter(|v| !v.is_null()) {
        let rows = rows.as_array().context("invalid HRV readings")?;
        ensure!(
            rows.len() <= limits.max_samples_per_day,
            "too many HRV readings"
        );
        for row in rows {
            samples.push(Sample {
                timestamp: health_samples::timestamp(
                    row.get("readingTimeGMT").context("missing HRV timestamp")?,
                )?,
                value: number(row.get("hrvValue").context("missing HRV value")?, "ms")?,
                unit: "ms".into(),
            });
        }
    }
    Ok((records, samples))
}
fn body_battery(payload: &Json, date: Date, limits: Limits) -> Result<(Vec<Record>, Vec<Sample>)> {
    if payload.is_null() {
        return Ok((vec![], vec![]));
    }
    let rows = payload
        .as_array()
        .context("expected Body Battery report array")?;
    ensure!(rows.len() <= 1, "unexpected Body Battery dates");
    let Some(row) = rows.first() else {
        return Ok((vec![], vec![]));
    };
    check_date(row, "date", date)?;
    if empty_battery_placeholders(row, limits)? {
        return Ok((vec![], vec![]));
    }
    let r = record(row, fields::BODY_BATTERY, None)?;
    let records = if r.values.is_empty() { vec![] } else { vec![r] };
    let samples = health_samples::series(
        row,
        "bodyBatteryValuesArray",
        "bodyBatteryValueDescriptorDTOList",
        "bodyBatteryValueDescriptorIndex",
        "bodyBatteryValueDescriptorKey",
        "bodyBatteryLevel",
        "score",
        limits,
    )?;
    Ok((records, samples))
}
// Empty Garmin reports contain minute placeholders without describing any measured columns.
fn empty_battery_placeholders(row: &Json, limits: Limits) -> Result<bool> {
    if row.get("bodyBatteryValueDescriptorDTOList") != Some(&Json::Null) {
        return Ok(false);
    }
    let fields = row.as_object().context("expected Body Battery object")?;
    if fields.iter().any(|(key, value)| {
        !value.is_null() && !matches!(key.as_str(), "date" | "bodyBatteryValuesArray")
    }) {
        return Ok(false);
    }
    let rows = row
        .get("bodyBatteryValuesArray")
        .and_then(Json::as_array)
        .context("expected Body Battery placeholders")?;
    ensure!(
        rows.len() <= limits.max_samples_per_day,
        "too many time-series samples"
    );
    for row in rows {
        let cells = row
            .as_array()
            .context("expected Body Battery placeholder")?;
        if cells.len() != 2 || !cells[1].is_null() {
            return Ok(false);
        }
        health_samples::timestamp(&cells[0])?;
    }
    Ok(true)
}
fn weight(payload: &Json, date: Date, limits: Limits) -> Result<Vec<Record>> {
    if empty(payload) {
        return Ok(vec![]);
    }
    let days = payload
        .get("dailyWeightSummaries")
        .and_then(Json::as_array)
        .context("missing weight summaries")?;
    ensure!(days.len() <= 1, "unexpected weight summary dates");
    let mut records = Vec::new();
    for day in days {
        check_date(day, "summaryDate", date)?;
        let rows = day
            .get("allWeightMetrics")
            .and_then(Json::as_array)
            .context("missing requested weight entries")?;
        ensure!(
            rows.len() <= limits.max_records_per_metric,
            "too many weight entries"
        );
        for row in rows {
            check_date(row, "calendarDate", date)?;
            let r = record(row, fields::WEIGHT, Some("timestampGMT"))?;
            if !r.values.is_empty() {
                records.push(r);
            }
        }
    }
    Ok(records)
}
fn readiness(payload: &Json, date: Date, limits: Limits) -> Result<Vec<Record>> {
    if empty(payload) {
        return Ok(vec![]);
    }
    // The unofficial endpoint has observed array and single-snapshot responses.
    let rows: Vec<&Json> = match payload {
        Json::Array(rows) => rows.iter().collect(),
        Json::Object(_) => vec![payload],
        _ => anyhow::bail!("expected readiness snapshot or array"),
    };
    ensure!(
        rows.len() <= limits.max_records_per_metric,
        "too many readiness snapshots"
    );
    rows.into_iter()
        .map(|row| {
            check_date(row, "calendarDate", date)?;
            let r = record(row, fields::READINESS, Some("timestamp"))?;
            ensure!(
                r.observed_at.is_some() && !r.values.is_empty(),
                "incomplete readiness snapshot"
            );
            Ok(r)
        })
        .collect()
}
