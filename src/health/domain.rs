//! src/health/domain.rs defines calendar-scoped health snapshots and measurements.
//! It exposes validated dates, dataset identities and storage-independent value types.

use chrono::{Days, NaiveDate};
use std::{collections::BTreeMap, fmt, str::FromStr};

/// A Garmin calendar date, independent of the host's timezone.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Date(NaiveDate);
impl FromStr for Date {
    type Err = HealthError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .map_err(|_| HealthError::Invalid("date must be YYYY-MM-DD".into()))?;
        if value.len() != 10 || date.to_string() != value {
            return Err(HealthError::Invalid("date must be YYYY-MM-DD".into()));
        }
        Ok(Self(date))
    }
}
impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
/// How much health history is stored locally, without reading any day documents.
#[derive(Debug, PartialEq)]
pub struct HealthSummary {
    pub day_count: u64,
    pub latest: Option<Date>,
}
/// On how many saved days one dataset holds data, and the span of those days.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricCoverage {
    pub metric: Metric,
    /// Saved days whose dataset has records or samples.
    pub day_count: u64,
    pub earliest: Option<Date>,
    pub latest: Option<Date>,
    /// Stored time-series samples over all days; zero for summary-only datasets.
    pub sample_count: u64,
}
impl From<NaiveDate> for Date {
    fn from(date: NaiveDate) -> Self {
        Self(date)
    }
}
impl From<Date> for NaiveDate {
    fn from(date: Date) -> Self {
        date.0
    }
}
impl Date {
    /// Validate and enumerate an inclusive bounded range before any I/O.
    pub fn range(self, end: Self, max_days: usize) -> Result<Vec<Self>, HealthError> {
        let count = (end.0 - self.0).num_days() + 1;
        if count <= 0 || count > max_days as i64 {
            return Err(HealthError::Invalid(format!(
                "range must contain 1..={max_days} days"
            )));
        }
        (0..count)
            .map(|offset| {
                self.0
                    .checked_add_days(Days::new(offset as u64))
                    .map(Self)
                    .ok_or_else(|| HealthError::Invalid("date overflow".into()))
            })
            .collect()
    }
    /// Split an inclusive range of any length into consecutive windows of at most max_days.
    pub fn windows(self, end: Self, max_days: usize) -> Result<Vec<(Self, Self)>, HealthError> {
        if end < self {
            return Err(HealthError::Invalid(
                "start date must not be after end date".into(),
            ));
        }
        let span = u64::try_from(max_days)
            .ok()
            .and_then(|days| days.checked_sub(1))
            .ok_or_else(|| HealthError::Invalid("window must contain at least one day".into()))?;
        let mut windows = Vec::new();
        let mut start = self;
        loop {
            let last = start
                .0
                .checked_add_days(Days::new(span))
                .map_or(end, |date| Self(date).min(end));
            windows.push((start, last));
            if last == end {
                return Ok(windows);
            }
            start = last
                .0
                .succ_opt()
                .map(Self)
                .ok_or_else(|| HealthError::Invalid("date overflow".into()))?;
        }
    }
    /// The following calendar day, if representable.
    pub fn next(self) -> Option<Self> {
        self.0.succ_opt().map(Self)
    }
    /// Count the days of an inclusive range; zero when end precedes start.
    pub fn days_through(self, end: Self) -> u64 {
        u64::try_from((end.0 - self.0).num_days() + 1).unwrap_or(0)
    }
    /// The calendar day `days` before this one, if representable.
    pub fn back(self, days: u64) -> Option<Self> {
        self.0.checked_sub_days(Days::new(days)).map(Self)
    }
    /// The calendar day `days` after this one, if representable.
    pub fn forward(self, days: u64) -> Option<Self> {
        self.0.checked_add_days(Days::new(days)).map(Self)
    }
}

/// The explicitly supported datasets; arbitrary remote paths are never accepted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Metric {
    Daily,
    Sleep,
    Hrv,
    HeartRate,
    BodyBattery,
    Spo2,
    Weight,
    TrainingReadiness,
}
impl Metric {
    /// Complete supported dataset set, also used to reject partial imports.
    pub const ALL: [Self; 8] = [
        Self::Daily,
        Self::Sleep,
        Self::Hrv,
        Self::HeartRate,
        Self::BodyBattery,
        Self::Spo2,
        Self::Weight,
        Self::TrainingReadiness,
    ];
    /// Stable v1 dataset identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Sleep => "sleep",
            Self::Hrv => "hrv",
            Self::HeartRate => "heart_rate",
            Self::BodyBattery => "body_battery",
            Self::Spo2 => "spo2",
            Self::Weight => "weight",
            Self::TrainingReadiness => "training_readiness",
        }
    }
    /// Whether the dataset stores a time series beside its summary records.
    pub fn has_samples(self) -> bool {
        matches!(
            self,
            Self::HeartRate | Self::Hrv | Self::BodyBattery | Self::Spo2
        )
    }
    /// Identify which credentialed upstream supplies this dataset.
    pub fn source(self) -> &'static str {
        if self == Self::TrainingReadiness {
            "garmin_connect"
        } else {
            "garmin_personal_api"
        }
    }
}
impl FromStr for Metric {
    type Err = HealthError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|metric| metric.as_str() == value)
            .ok_or_else(|| HealthError::Invalid("unknown health metric".into()))
    }
}

/// A measured scalar or vendor-supplied qualitative label; absence is not zero.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Number(f64),
    Text(String),
}
/// A value with an explicit unit when numeric.
#[derive(Clone, Debug, PartialEq)]
pub struct Measurement {
    pub value: Value,
    pub unit: Option<String>,
}
/// One summary or readiness/weight observation with an optional UTC timestamp.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub observed_at: Option<String>,
    pub values: BTreeMap<String, Measurement>,
}
/// One normalized time-series value; a null value preserves a recorded gap.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub timestamp: String,
    pub value: Option<f64>,
    pub unit: String,
}
/// One dataset; empty records and samples explicitly mean no data.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub metric: Metric,
    pub records: Vec<Record>,
    pub sample_count: usize,
}
/// All summaries for one atomically synchronized calendar day.
#[derive(Clone, Debug, PartialEq)]
pub struct HealthDay {
    pub date: Date,
    pub synced_at: String,
    pub sections: Vec<Section>,
}
/// One dataset's summary and its time series as read from the remote source.
pub struct Dataset {
    pub section: Section,
    pub samples: Vec<Sample>,
}
/// A remote day's validated datasets, exactly one per metric in `Metric::ALL` order.
pub struct DayImport {
    pub datasets: [Dataset; Metric::ALL.len()],
}
/// One indexed page from the local sample archive.
pub struct SamplePage {
    pub date: Date,
    pub metric: Metric,
    pub synced_at: Option<String>,
    pub total: usize,
    pub offset: usize,
    pub samples: Vec<Sample>,
}

/// Typed failures distinguish invalid input, upstream failure and local storage.
#[derive(Debug, thiserror::Error)]
pub enum HealthError {
    #[error("{0}")]
    Invalid(String),
    #[error("Garmin health request failed: {0}")]
    Upstream(String),
    #[error("local health storage is unavailable")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}

#[cfg(test)]
mod tests {
    use super::{Date, HealthError};

    fn date(value: &str) -> Result<Date, HealthError> {
        value.parse()
    }

    #[test]
    fn windows_cover_a_long_range_without_gaps_or_overlap() -> Result<(), HealthError> {
        let windows = date("2026-01-01")?.windows(date("2026-03-05")?, 31)?;
        assert_eq!(
            windows,
            vec![
                (date("2026-01-01")?, date("2026-01-31")?),
                (date("2026-02-01")?, date("2026-03-03")?),
                (date("2026-03-04")?, date("2026-03-05")?),
            ]
        );
        assert_eq!(date("2026-01-01")?.days_through(date("2026-03-05")?), 64);
        assert_eq!(date("2026-01-31")?.next(), Some(date("2026-02-01")?));
        Ok(())
    }

    #[test]
    fn windows_reject_reversed_ranges_and_empty_windows() -> Result<(), HealthError> {
        assert!(
            date("2026-01-02")?
                .windows(date("2026-01-01")?, 31)
                .is_err()
        );
        assert!(date("2026-01-01")?.windows(date("2026-01-01")?, 0).is_err());
        assert_eq!(
            date("2026-01-01")?.windows(date("2026-01-01")?, 31)?,
            vec![(date("2026-01-01")?, date("2026-01-01")?)]
        );
        Ok(())
    }
}
