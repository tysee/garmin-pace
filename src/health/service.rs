//! src/health/service.rs owns bounded health reads and per-day atomic synchronization.
//! Its ports keep HTTP, SQLite, clocks and serialization outside the use cases.

use super::{
    Date, DayImport, HealthDay, HealthError, HealthSummary, Metric, MetricCoverage, Sample,
    SamplePage, Section,
};
use std::sync::Arc;

/// Local persistence required by the health use cases.
pub trait HealthRepository: Send + Sync {
    /// Read existing summaries within a validated inclusive range.
    fn days(&self, start: Date, end: Date) -> Result<Vec<HealthDay>, HealthError>;
    /// Read one indexed page and its synchronization metadata consistently.
    fn samples(
        &self,
        date: Date,
        metric: Metric,
        limit: usize,
        offset: usize,
    ) -> Result<SamplePage, HealthError>;
    /// Count stored days and find the latest one.
    fn summary(&self) -> Result<HealthSummary, HealthError>;
    /// Count, per dataset, the saved days holding data and the stored samples.
    fn coverage(&self) -> Result<Vec<MetricCoverage>, HealthError>;
    /// Begin a write transaction without committing any state.
    fn begin(&self) -> Result<Box<dyn HealthWrite>, HealthError>;
}
/// An explicit transaction committed by the application, rolled back on drop.
pub trait HealthWrite {
    /// Replace one complete day and its samples inside this transaction.
    fn replace(
        &mut self,
        day: &HealthDay,
        samples: &[(Metric, Vec<Sample>)],
    ) -> Result<(), HealthError>;
    /// Publish the replacement atomically.
    fn commit(self: Box<Self>) -> Result<(), HealthError>;
}
/// A validated remote health source, with its own timeout and retry policy.
pub trait HealthSource {
    /// Fetch and validate all supported datasets for exactly one calendar day.
    fn day(&self, date: Date) -> Result<DayImport, HealthError>;
}
/// Resource bounds used identically by CLI and MCP.
#[derive(Clone, Copy)]
pub struct Limits {
    pub max_days: usize,
    pub max_samples_per_day: usize,
    pub max_sample_page: usize,
    pub max_records_per_metric: usize,
}
/// Bounded application boundary for health history.
#[derive(Clone)]
pub struct HealthService {
    repository: Arc<dyn HealthRepository>,
    limits: Limits,
}
impl HealthService {
    /// Bind storage to the validated application resource policy.
    pub fn new(repository: Arc<dyn HealthRepository>, limits: Limits) -> Self {
        Self { repository, limits }
    }
    /// Report stored coverage without reading day documents.
    pub fn summary(&self) -> Result<HealthSummary, HealthError> {
        self.repository.summary()
    }
    /// Report per-dataset coverage, one entry per supported metric in `Metric::ALL` order.
    pub fn coverage(&self) -> Result<Vec<MetricCoverage>, HealthError> {
        let stored = self.repository.coverage()?;
        Ok(Metric::ALL
            .into_iter()
            .map(|metric| {
                stored
                    .iter()
                    .find(|c| c.metric == metric)
                    .cloned()
                    .unwrap_or(MetricCoverage {
                        metric,
                        day_count: 0,
                        earliest: None,
                        latest: None,
                        sample_count: 0,
                    })
            })
            .collect())
    }
    /// Read only the explicitly bounded local calendar range.
    pub fn days(&self, start: Date, end: Date) -> Result<Vec<HealthDay>, HealthError> {
        start.range(end, self.limits.max_days)?;
        self.repository.days(start, end)
    }
    /// Read a bounded sample page for a supported time-series dataset.
    pub fn samples(
        &self,
        date: Date,
        metric: Metric,
        limit: usize,
        offset: usize,
    ) -> Result<SamplePage, HealthError> {
        if !metric.has_samples() {
            return Err(HealthError::Invalid(
                "samples support heart_rate, hrv, body_battery and spo2".into(),
            ));
        }
        if limit == 0
            || limit > self.limits.max_sample_page
            || offset > self.limits.max_samples_per_day
        {
            return Err(HealthError::Invalid(
                "sample limit or offset exceeds configured bounds".into(),
            ));
        }
        self.repository.samples(date, metric, limit, offset)
    }
    /// Fetch completely before opening the transaction; a failed day changes nothing.
    pub fn sync_day(
        &self,
        source: &impl HealthSource,
        date: Date,
        now: impl FnOnce() -> String,
    ) -> Result<HealthDay, HealthError> {
        // DayImport holds exactly one dataset per metric; only the storage budget is checked here.
        let import = source.day(date)?;
        if import
            .datasets
            .iter()
            .map(|d| d.samples.len())
            .sum::<usize>()
            > self.limits.max_samples_per_day
        {
            return Err(HealthError::Upstream(
                "daily samples exceed configured bound".into(),
            ));
        }
        let (sections, samples): (Vec<Section>, Vec<(Metric, Vec<Sample>)>) = import
            .datasets
            .into_iter()
            .map(|d| {
                let metric = d.section.metric;
                (d.section, (metric, d.samples))
            })
            .unzip();
        let day = HealthDay {
            date,
            synced_at: now(),
            sections,
        };
        let mut transaction = self.repository.begin()?;
        transaction.replace(&day, &samples)?;
        transaction.commit()?;
        Ok(day)
    }
}
