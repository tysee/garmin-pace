//! src/interface/health_view.rs renders stored health datasets as plain terminal text.
//! The menu uses it to name metrics, describe their coverage, summarize one day in a
//! row and print a day's records or a sample page; the CLI keeps its JSON-backed renderer.

use crate::health::{Date, Measurement, Metric, MetricCoverage, SamplePage, Section, Value};
use std::fmt::Write as _;

/// Human name of a dataset, as shown in menus and titles.
pub(super) fn name(metric: Metric) -> &'static str {
    match metric {
        Metric::Daily => "Daily summary",
        Metric::Sleep => "Sleep",
        Metric::Hrv => "HRV",
        Metric::HeartRate => "Heart rate",
        Metric::BodyBattery => "Body Battery",
        Metric::Spo2 => "Pulse ox (SpO2)",
        Metric::Weight => "Weight",
        Metric::TrainingReadiness => "Training readiness",
    }
}

/// Which credential the import needs for this dataset, in the user's terms.
pub(super) fn source(metric: Metric) -> &'static str {
    if metric.source() == "garmin_connect" {
        "Garmin login"
    } else {
        "Personal API key"
    }
}

/// One menu row: on how many of the saved days the dataset has data, and over which span.
pub(super) fn coverage(coverage: &MetricCoverage, saved_days: u64) -> String {
    match (coverage.earliest, coverage.latest) {
        (Some(first), Some(last)) => {
            let span = if first == last {
                last.to_string()
            } else {
                format!("{first} to {last}")
            };
            let mut text = format!("{} of {saved_days} days | {span}", coverage.day_count);
            if coverage.sample_count > 0 {
                let _ = write!(text, " | {} samples", coverage.sample_count);
            }
            text
        }
        _ if saved_days == 0 => format!("No days saved yet; needs {}", source(coverage.metric)),
        _ => format!(
            "No data on any of {saved_days} saved days; needs {}",
            source(coverage.metric)
        ),
    }
}

/// Values worth a glance in a day list; other values stay in the day view.
fn headline(metric: Metric) -> &'static [&'static str] {
    match metric {
        Metric::Daily => &["steps", "heart_rate_resting", "stress_average"],
        Metric::Sleep => &["score", "duration", "quality"],
        Metric::Hrv => &["overnight_average", "status"],
        Metric::HeartRate => &["resting", "min", "max"],
        Metric::BodyBattery => &["charged", "drained"],
        Metric::Spo2 => &["average", "lowest"],
        Metric::Weight => &["weight", "body_fat"],
        Metric::TrainingReadiness => &["score", "level"],
    }
}

/// One day of a dataset in a single row: headline values, record and sample counts.
pub(super) fn day_line(section: &Section) -> String {
    if section.records.is_empty() && section.sample_count == 0 {
        return "No data".into();
    }
    let mut parts = Vec::new();
    if let Some(record) = section.records.first() {
        parts.extend(
            headline(section.metric)
                .iter()
                .filter_map(|key| record.values.get(*key).map(|m| brief(key, m))),
        );
        if parts.is_empty() {
            parts.extend(record.values.iter().take(3).map(|(k, m)| brief(k, m)));
        }
    }
    if section.records.len() > 1 {
        parts.push(format!("{} records", section.records.len()));
    }
    if section.sample_count > 0 {
        parts.push(format!("{} samples", section.sample_count));
    }
    parts.join(" | ")
}

/// Everything saved for one dataset on one day.
pub(super) fn day(section: &Section, date: Date, synced_at: &str) -> String {
    let mut text = format!(
        "{} | {date}\nSynced: {synced_at}\nSource: {}\n",
        name(section.metric),
        source(section.metric)
    );
    if section.records.is_empty() && section.sample_count == 0 {
        text.push_str("\nNo data for this day.");
        return text;
    }
    for (index, record) in section.records.iter().enumerate() {
        if section.records.len() > 1 {
            let _ = write!(text, "\nRecord {} of {}", index + 1, section.records.len());
        }
        if let Some(time) = &record.observed_at {
            let _ = write!(text, "\nAt {time}");
        }
        for (key, measurement) in &record.values {
            let _ = write!(
                text,
                "\n  {}: {} {}",
                label(key),
                scalar(&measurement.value),
                measurement.unit.as_deref().unwrap_or("")
            );
        }
    }
    if section.sample_count > 0 {
        let _ = write!(text, "\n\nStored samples: {}", section.sample_count);
    }
    text
}

/// One page of a time series; a missing value marks a recorded gap.
pub(super) fn samples(page: &SamplePage) -> String {
    let end = page.offset + page.samples.len();
    let mut text = format!(
        "{} | {} | samples {}-{end} of {}",
        name(page.metric),
        page.date,
        (page.offset + 1).min(end),
        page.total
    );
    if page.samples.is_empty() {
        text.push_str("\n\nNo samples stored for this day.");
    }
    for sample in &page.samples {
        match sample.value {
            Some(value) => {
                let _ = write!(
                    text,
                    "\n{} | {} {}",
                    sample.timestamp,
                    number(value),
                    sample.unit
                );
            }
            None => {
                let _ = write!(text, "\n{} | gap", sample.timestamp);
            }
        }
    }
    text
}

fn label(key: &str) -> String {
    key.replace('_', " ")
}

fn brief(key: &str, measurement: &Measurement) -> String {
    let unit = measurement.unit.as_deref().unwrap_or("");
    let value = match (&measurement.value, unit) {
        (Value::Number(seconds), "s") => duration(*seconds),
        (value, "score" | "text" | "count" | "") => scalar(value),
        (value, unit) if unit == key => scalar(value),
        (value, unit) => format!("{} {unit}", scalar(value)),
    };
    format!("{}: {value}", label(key))
}

fn scalar(value: &Value) -> String {
    match value {
        Value::Number(number) => self::number(*number),
        Value::Text(text) => text.clone(),
    }
}

fn number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn duration(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    if total >= 3600 {
        format!("{}h {:02}m", total / 3600, (total % 3600) / 60)
    } else {
        format!("{}m {:02}s", total / 60, total % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::{Record, Sample};
    use std::collections::BTreeMap;

    fn measurement(value: f64, unit: &str) -> Measurement {
        Measurement {
            value: Value::Number(value),
            unit: Some(unit.into()),
        }
    }

    #[test]
    fn day_line_shows_headline_values_durations_and_counts() {
        let section = Section {
            metric: Metric::Sleep,
            records: vec![Record {
                observed_at: None,
                values: BTreeMap::from([
                    ("score".to_string(), measurement(85.0, "score")),
                    ("duration".to_string(), measurement(27000.0, "s")),
                    ("deep".to_string(), measurement(5000.0, "s")),
                ]),
            }],
            sample_count: 0,
        };
        assert_eq!(day_line(&section), "score: 85 | duration: 7h 30m");
        let empty = Section {
            metric: Metric::Hrv,
            records: vec![],
            sample_count: 0,
        };
        assert_eq!(day_line(&empty), "No data");
        let sampled = Section {
            metric: Metric::HeartRate,
            records: vec![],
            sample_count: 2,
        };
        assert_eq!(day_line(&sampled), "2 samples");
    }

    #[test]
    fn coverage_rows_name_the_missing_credential() -> anyhow::Result<()> {
        let none = MetricCoverage {
            metric: Metric::TrainingReadiness,
            day_count: 0,
            earliest: None,
            latest: None,
            sample_count: 0,
        };
        assert_eq!(
            coverage(&none, 3),
            "No data on any of 3 saved days; needs Garmin login"
        );
        assert_eq!(coverage(&none, 0), "No days saved yet; needs Garmin login");
        let some = MetricCoverage {
            metric: Metric::HeartRate,
            day_count: 2,
            earliest: Some("2026-10-01".parse()?),
            latest: Some("2026-10-24".parse()?),
            sample_count: 900,
        };
        assert_eq!(
            coverage(&some, 3),
            "2 of 3 days | 2026-10-01 to 2026-10-24 | 900 samples"
        );
        Ok(())
    }

    #[test]
    fn sample_pages_mark_gaps_and_ranges() -> anyhow::Result<()> {
        let page = SamplePage {
            date: "2026-10-24".parse()?,
            metric: Metric::HeartRate,
            synced_at: Some("2026-10-24T12:00:00Z".into()),
            total: 3,
            offset: 1,
            samples: vec![
                Sample {
                    timestamp: "2026-10-24T08:00:00.000Z".into(),
                    value: Some(60.0),
                    unit: "bpm".into(),
                },
                Sample {
                    timestamp: "2026-10-24T08:01:00.000Z".into(),
                    value: None,
                    unit: "bpm".into(),
                },
            ],
        };
        assert_eq!(
            samples(&page),
            "Heart rate | 2026-10-24 | samples 2-3 of 3\n2026-10-24T08:00:00.000Z | 60 bpm\n2026-10-24T08:01:00.000Z | gap"
        );
        Ok(())
    }
}
