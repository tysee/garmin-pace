//! src/health/sqlite.rs persists health snapshots and indexed samples in the profile database.
//! It implements bounded reads, coverage summaries computed in SQL over the stored documents,
//! and explicit transactions owned by HealthService.

use super::{
    Date, HealthDay, HealthError, HealthRepository, HealthSummary, HealthWrite, Metric,
    MetricCoverage, Sample, SamplePage, document::DayDocument,
};
use crate::{config::DatabaseSettings, history::SqliteHistory};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::time::Duration;

/// SQLite adapter using the exact profile path selected by CLI or HTTP auth.
pub struct SqliteHealth {
    settings: DatabaseSettings,
}
impl SqliteHealth {
    /// Require an already migrated, private profile database.
    pub fn open(settings: DatabaseSettings) -> Result<Self> {
        SqliteHistory::open(settings.clone())?;
        Ok(Self::attach(settings))
    }
    /// Bind to a profile database that the caller already validated with `SqliteHistory::open`.
    pub fn attach(settings: DatabaseSettings) -> Self {
        Self { settings }
    }
    fn connection(&self, write: bool) -> Result<Connection> {
        crate::private_fs::database(&self.settings.path, false)?;
        let flags = if write {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let connection = Connection::open_with_flags(
            &self.settings.path,
            flags | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        connection.busy_timeout(Duration::from_millis(self.settings.busy_timeout_ms))?;
        connection.pragma_update(None, "cache_size", -i64::from(self.settings.cache_kib))?;
        connection.pragma_update(None, "foreign_keys", true)?;
        Ok(connection)
    }
    fn read_summary(&self) -> Result<HealthSummary> {
        let (count, latest): (i64, Option<String>) = self.connection(false)?.query_row(
            "SELECT count(*), max(date) FROM health_days",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(HealthSummary {
            day_count: u64::try_from(count).context("invalid stored health day count")?,
            latest: latest.map(|date| date.parse()).transpose()?,
        })
    }
    /// Aggregate dataset status per metric with SQLite's JSON functions, never by loading
    /// every day document into memory. Unknown dataset names in a document are skipped.
    fn read_coverage(&self) -> Result<Vec<MetricCoverage>> {
        let connection = self.connection(false)?;
        connection.execute_batch("BEGIN")?;
        let mut samples =
            connection.prepare("SELECT metric, count(*) FROM health_samples GROUP BY metric")?;
        let samples = samples
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut query = connection.prepare(
            "SELECT dataset.key, count(*), min(days.date), max(days.date) \
             FROM health_days AS days, json_each(days.document, '$.datasets') AS dataset \
             WHERE json_extract(dataset.value, '$.status') = 'available' \
             GROUP BY dataset.key",
        )?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        let mut coverage = Vec::new();
        for row in rows {
            let (name, count, earliest, latest) = row?;
            let Ok(metric) = name.parse::<Metric>() else {
                continue;
            };
            let sample_count = samples
                .iter()
                .find(|(sampled, _)| *sampled == name)
                .map_or(Ok(0), |(_, count)| u64::try_from(*count))
                .context("invalid stored sample count")?;
            coverage.push(MetricCoverage {
                metric,
                day_count: u64::try_from(count).context("invalid stored coverage count")?,
                earliest: earliest.map(|date| date.parse()).transpose()?,
                latest: latest.map(|date| date.parse()).transpose()?,
                sample_count,
            });
        }
        Ok(coverage)
    }
    fn read_days(&self, start: Date, end: Date) -> Result<Vec<HealthDay>> {
        let connection = self.connection(false)?;
        let mut query = connection.prepare(
            "SELECT document FROM health_days WHERE date >= ?1 AND date <= ?2 ORDER BY date",
        )?;
        let rows = query.query_map([start.to_string(), end.to_string()], |row| {
            row.get::<_, String>(0)
        })?;
        rows.map(|r| -> Result<HealthDay> { serde_json::from_str::<DayDocument>(&r?)?.try_into() })
            .collect()
    }
    fn read_samples(
        &self,
        date: Date,
        metric: Metric,
        limit: usize,
        offset: usize,
    ) -> Result<SamplePage> {
        let connection = self.connection(false)?;
        connection.execute_batch("BEGIN")?;
        let synced_at = connection
            .query_row(
                "SELECT synced_at FROM health_days WHERE date = ?1",
                [date.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        let total = connection.query_row(
            "SELECT count(*) FROM health_samples WHERE date = ?1 AND metric = ?2",
            params![date.to_string(), metric.as_str()],
            |r| r.get::<_, i64>(0),
        )?;
        let total = usize::try_from(total).context("invalid sample count")?;
        let mut query = connection.prepare("SELECT timestamp, value, unit FROM health_samples WHERE date = ?1 AND metric = ?2 ORDER BY sequence LIMIT ?3 OFFSET ?4")?;
        let samples = query
            .query_map(
                params![
                    date.to_string(),
                    metric.as_str(),
                    limit as i64,
                    offset as i64
                ],
                |r| {
                    Ok(Sample {
                        timestamp: r.get(0)?,
                        value: r.get(1)?,
                        unit: r.get(2)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(SamplePage {
            date,
            metric,
            synced_at,
            total,
            offset,
            samples,
        })
    }
}
fn storage(error: anyhow::Error) -> HealthError {
    HealthError::Storage(error.into())
}
impl HealthRepository for SqliteHealth {
    fn days(&self, start: Date, end: Date) -> Result<Vec<HealthDay>, HealthError> {
        self.read_days(start, end).map_err(storage)
    }
    fn samples(
        &self,
        date: Date,
        metric: Metric,
        limit: usize,
        offset: usize,
    ) -> Result<SamplePage, HealthError> {
        self.read_samples(date, metric, limit, offset)
            .map_err(storage)
    }
    fn summary(&self) -> Result<HealthSummary, HealthError> {
        self.read_summary().map_err(storage)
    }
    fn coverage(&self) -> Result<Vec<MetricCoverage>, HealthError> {
        self.read_coverage().map_err(storage)
    }
    fn begin(&self) -> Result<Box<dyn HealthWrite>, HealthError> {
        let connection = self.connection(true).map_err(storage)?;
        connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| storage(e.into()))?;
        Ok(Box::new(Write { connection }))
    }
}
struct Write {
    connection: Connection,
}
impl Write {
    fn save(&mut self, day: &HealthDay, samples: &[(Metric, Vec<Sample>)]) -> Result<()> {
        let document = serde_json::to_string(&DayDocument::from(day.clone()))?;
        ensure!(
            document.len() <= 262144,
            "health summary exceeds storage bound"
        );
        self.connection.execute("INSERT INTO health_days VALUES (?1, ?2, ?3) ON CONFLICT(date) DO UPDATE SET synced_at = excluded.synced_at, document = excluded.document",
            params![day.date.to_string(), day.synced_at, document])?;
        self.connection.execute(
            "DELETE FROM health_samples WHERE date = ?1",
            [day.date.to_string()],
        )?;
        let mut insert = self
            .connection
            .prepare("INSERT INTO health_samples VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?;
        for (metric, samples) in samples {
            for (index, sample) in samples.iter().enumerate() {
                insert.execute(params![
                    day.date.to_string(),
                    metric.as_str(),
                    index as i64,
                    sample.timestamp,
                    sample.value,
                    sample.unit
                ])?;
            }
        }
        Ok(())
    }
}
impl HealthWrite for Write {
    fn replace(
        &mut self,
        day: &HealthDay,
        samples: &[(Metric, Vec<Sample>)],
    ) -> Result<(), HealthError> {
        self.save(day, samples).map_err(storage)
    }
    fn commit(self: Box<Self>) -> Result<(), HealthError> {
        self.connection
            .execute_batch("COMMIT")
            .context("committing health snapshot")
            .map_err(storage)
    }
}
