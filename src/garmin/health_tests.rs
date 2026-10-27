//! src/garmin/health_tests.rs exercises health boundary validation against public API shapes.
//! Tests distinguish missing observations, reordered descriptors and malformed payloads.

use super::health_decode::decode;
use crate::health::{Limits, Metric, Value};
use anyhow::Result;
use serde_json::json;

fn limits() -> Limits {
    Limits {
        max_days: 31,
        max_samples_per_day: 20,
        max_sample_page: 10,
        max_records_per_metric: 4,
    }
}

#[test]
fn body_battery_columns_are_described_and_missing_samples_remain_null() -> Result<()> {
    let (section, samples) = decode(
        "2026-10-24".parse()?,
        Metric::BodyBattery,
        json!([{
            "date":"2026-10-24","charged":40,"drained":20,
            "bodyBatteryValueDescriptorDTOList":[
                {"bodyBatteryValueDescriptorIndex":2,"bodyBatteryValueDescriptorKey":"timestamp"},
                {"bodyBatteryValueDescriptorIndex":0,"bodyBatteryValueDescriptorKey":"bodyBatteryLevel"}],
            "bodyBatteryValuesArray":[[50,"MEASURED",1792800060000i64],[-1,"UNKNOWN",1792800000000i64]]
        }]),
        limits(),
    )?;
    assert_eq!(section.sample_count, 2);
    assert_eq!(samples[0].value, None);
    assert_eq!(samples[1].value, Some(50.0));
    assert!(samples[0].timestamp.ends_with('Z'));
    Ok(())
}

#[test]
fn weight_converts_garmin_grams_to_kilograms_and_preserves_all_entries() -> Result<()> {
    let (section, _) = decode(
        "2026-10-24".parse()?,
        Metric::Weight,
        json!({"dailyWeightSummaries":[{
        "summaryDate":"2026-10-24","allWeightMetrics":[
            {"calendarDate":"2026-10-24","timestampGMT":"2026-10-24T08:00:00+03:00","weight":75500.0,"bodyFat":18},
            {"calendarDate":"2026-10-24","timestampGMT":"2026-10-24T20:00:00+03:00","weight":76000.0}
        ]}]}),
        limits(),
    )?;
    assert_eq!(section.records.len(), 2);
    assert_eq!(
        section.records[0].values["weight"].value,
        Value::Number(75.5)
    );
    assert_eq!(
        section.records[0].values["weight"].unit.as_deref(),
        Some("kg")
    );
    assert_eq!(
        section.records[0].observed_at.as_deref(),
        Some("2026-10-24T05:00:00.000Z")
    );
    Ok(())
}

#[test]
fn malformed_health_never_becomes_no_data() -> Result<()> {
    let date = "2026-10-24".parse()?;
    for (metric, payload) in [
        (Metric::Daily, json!({"error":"unauthorized"})),
        (
            Metric::Sleep,
            json!({"calendarDate":"2026-10-23","sleepTimeSeconds":100}),
        ),
        (
            Metric::Daily,
            json!({"calendarDate":"2026-10-24","totalSteps":"many"}),
        ),
        (
            Metric::TrainingReadiness,
            json!({"calendarDate":"2026-10-24","timestamp":"bad","score":5}),
        ),
        (
            Metric::TrainingReadiness,
            json!({"calendarDate":"2026-10-24","timestamp":"2026-10-24T08:00:00Z","score":101}),
        ),
        (
            Metric::HeartRate,
            json!({"calendarDate":"2026-10-24","heartRateValues":[[1792800000000i64,50]]}),
        ),
        (
            Metric::Hrv,
            json!({"hrvSummary":null,"hrvReadings":[{"hrvValue":30}]}),
        ),
        (
            Metric::Spo2,
            json!({"calendarDate":"2026-10-24","averageSpO2":120}),
        ),
    ] {
        assert!(
            decode(date, metric, payload, limits()).is_err(),
            "{}",
            metric.as_str()
        );
    }
    Ok(())
}

#[test]
fn explicit_empty_datasets_and_bounds_are_distinct() -> Result<()> {
    let date = "2026-10-24".parse()?;
    for metric in Metric::ALL {
        let payload = if metric == Metric::BodyBattery {
            json!([])
        } else {
            json!(null)
        };
        let (section, samples) = decode(date, metric, payload, limits())?;
        assert!(section.records.is_empty() && samples.is_empty());
    }
    let row = json!({"calendarDate":"2026-10-24","timestamp":"2026-10-24T08:00:00Z","score":50});
    assert!(
        decode(
            date,
            Metric::TrainingReadiness,
            json!([row.clone(), row.clone(), row.clone(), row.clone(), row]),
            limits()
        )
        .is_err()
    );
    Ok(())
}
#[test]
fn empty_sleep_dto_is_not_an_undated_observation() -> Result<()> {
    let date = "2026-10-24".parse()?;
    let empty = json!({"id":null,"calendarDate":null,"sleepTimeSeconds":null,
        "sleepStartTimestampGMT":null,"sleepEndTimestampGMT":null,"deviceRemCapable":false,
        "retro":false,"sleepNeed":{"calendarDate":"2026-10-24","actual":480}});
    let (section, samples) = decode(date, Metric::Sleep, empty.clone(), limits())?;
    assert!(section.records.is_empty() && samples.is_empty());
    for (key, value) in [
        ("sleepTimeSeconds", json!(0)),
        ("sleepEndTimestampGMT", json!(1792800000000i64)),
        ("sleepScores", json!({"overall":{"value":70}})),
        ("unknownObservation", json!(1)),
        ("id", json!(123)),
    ] {
        let mut malformed = empty.clone();
        malformed[key] = value;
        assert!(
            decode(date, Metric::Sleep, malformed, limits()).is_err(),
            "{key}"
        );
    }
    assert!(decode(date, Metric::Sleep, json!({"calendarDate":null}), limits()).is_err());
    Ok(())
}
#[test]
fn empty_body_battery_placeholders_do_not_require_invented_columns() -> Result<()> {
    let date = "2026-10-24".parse()?;
    let empty = json!([{"date":"2026-10-24","charged":null,"drained":null,
        "startTimestampGMT":null,"endTimestampGMT":null,"bodyBatteryValueDescriptorDTOList":null,
        "bodyBatteryValuesArray":[[1792800000000i64,null],[1792800060000i64,null]]}]);
    let (section, samples) = decode(date, Metric::BodyBattery, empty.clone(), limits())?;
    assert!(section.records.is_empty() && samples.is_empty());
    for (field, value) in [
        ("charged", json!(40)),
        ("bodyBatteryValuesArray", json!([[1792800000000i64, 50]])),
        ("bodyBatteryValuesArray", json!([["bad time", null]])),
        (
            "bodyBatteryValuesArray",
            json!([[1792800000000i64, null, null]]),
        ),
        ("unknownMeasurement", json!(1)),
    ] {
        let mut bad = empty.clone();
        bad[0][field] = value;
        assert!(
            decode(date, Metric::BodyBattery, bad, limits()).is_err(),
            "{field}"
        );
    }
    let mut bounded = limits();
    bounded.max_samples_per_day = 1;
    assert!(decode(date, Metric::BodyBattery, empty, bounded).is_err());
    Ok(())
}
