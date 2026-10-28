//! src/sync/sqlite.rs implements the full-sync journal on the profile's SQLite database.
//! Each call is one short transaction, so every recorded window survives a later crash.
//! Beginning a run prunes all but the newest runs. It exposes `SqliteSyncJournal`.

use super::{
    domain::{
        Outcome, Period, RunRecord, RunState, Step, StepReport, StepState, Window, WindowRecord,
    },
    service::SyncJournal,
};
use crate::{config::DatabaseSettings, health::Date, history::SqliteHistory};
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params};
use std::time::Duration;

/// Journaled runs kept for resume and status; older ones are deleted with their rows.
const KEPT_RUNS: i64 = 20;

pub struct SqliteSyncJournal {
    settings: DatabaseSettings,
}

impl SqliteSyncJournal {
    /// Bind to an initialized profile database at the current schema.
    pub fn open(settings: DatabaseSettings) -> Result<Self> {
        SqliteHistory::open(settings.clone())?;
        Ok(Self { settings })
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
        )
        .context("opening the sync journal")?;
        connection.busy_timeout(Duration::from_millis(self.settings.busy_timeout_ms))?;
        connection.pragma_update(None, "foreign_keys", true)?;
        Ok(connection)
    }
}

fn date(value: String) -> Result<Date> {
    value
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid date in the sync journal"))
}

fn count(value: i64) -> Result<usize> {
    usize::try_from(value).context("invalid count in the sync journal")
}

fn step(value: &str) -> Result<Step> {
    Step::parse(value).context("unknown step in the sync journal")
}

fn run(row: &Row<'_>) -> rusqlite::Result<(i64, String, String, String, Option<String>, String)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}

impl SyncJournal for SqliteSyncJournal {
    fn begin(&self, period: Period, at: &str) -> Result<i64> {
        let mut connection = self.connection(true)?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO sync_runs (start_date, end_date, started_at, state) VALUES (?1, ?2, ?3, 'running')",
            params![period.start.to_string(), period.end.to_string(), at],
        )?;
        let id = transaction.last_insert_rowid();
        transaction.execute(
            "DELETE FROM sync_runs WHERE id NOT IN (SELECT id FROM sync_runs ORDER BY id DESC LIMIT ?1)",
            [KEPT_RUNS],
        )?;
        transaction.commit().context("recording a new sync run")?;
        Ok(id)
    }

    fn latest(&self) -> Result<Option<RunRecord>> {
        let row = self
            .connection(false)?
            .query_row(
                "SELECT id, start_date, end_date, started_at, finished_at, state FROM sync_runs ORDER BY id DESC LIMIT 1",
                [],
                run,
            )
            .optional()
            .context("reading the latest sync run")?;
        row.map(|(id, start, end, started_at, finished_at, state)| {
            Ok(RunRecord {
                id,
                period: Period {
                    start: date(start)?,
                    end: date(end)?,
                },
                started_at,
                finished_at,
                state: RunState::parse(&state).context("unknown sync run state")?,
            })
        })
        .transpose()
    }

    fn windows(&self, run: i64) -> Result<Vec<WindowRecord>> {
        let connection = self.connection(false)?;
        let mut query = connection.prepare(
            "SELECT step, start_date, end_date, outcome, saved, reason FROM sync_run_windows WHERE run_id = ?1",
        )?;
        let rows = query.query_map([run], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;
        rows.map(|row| {
            let (name, start, end, outcome, saved, reason) = row?;
            Ok(WindowRecord {
                step: step(&name)?,
                window: Window {
                    start: date(start)?,
                    end: date(end)?,
                },
                outcome: match outcome.as_str() {
                    "done" => Outcome::Done {
                        saved: count(saved)?,
                    },
                    _ => Outcome::Failed {
                        reason: reason.unwrap_or_default(),
                    },
                },
            })
        })
        .collect()
    }

    fn steps(&self, run: i64) -> Result<Vec<StepReport>> {
        let connection = self.connection(false)?;
        let mut query = connection.prepare(
            "SELECT step, state, windows, done, reused, saved, reason FROM sync_run_steps WHERE run_id = ?1",
        )?;
        let rows = query.query_map([run], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })?;
        let mut steps = rows
            .map(|row| {
                let (name, state, windows, done, reused, saved, reason) = row?;
                let reason = reason.unwrap_or_default();
                Ok(StepReport {
                    step: step(&name)?,
                    windows: count(windows)?,
                    done: count(done)?,
                    reused: count(reused)?,
                    saved: count(saved)?,
                    state: match state.as_str() {
                        "complete" => StepState::Complete,
                        "partial" => StepState::Partial { reason },
                        "failed" => StepState::Failed { reason },
                        _ => StepState::Skipped { reason },
                    },
                })
            })
            .collect::<Result<Vec<_>>>()?;
        steps.sort_by_key(|report| Step::ORDER.iter().position(|s| *s == report.step));
        Ok(steps)
    }

    fn reopen(&self, run: i64) -> Result<()> {
        self.connection(true)?
            .execute(
                "UPDATE sync_runs SET state = 'running', finished_at = NULL WHERE id = ?1",
                [run],
            )
            .context("reopening the sync run")?;
        Ok(())
    }

    fn record_window(&self, run: i64, record: &WindowRecord, at: &str) -> Result<()> {
        let (outcome, saved, reason) = match &record.outcome {
            Outcome::Done { saved } => ("done", *saved, None),
            Outcome::Failed { reason } => ("failed", 0, Some(reason.as_str())),
        };
        self.connection(true)?
            .execute(
                "INSERT INTO sync_run_windows (run_id, step, start_date, end_date, outcome, saved, reason, recorded_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (run_id, step, start_date, end_date) DO UPDATE SET
                 outcome = excluded.outcome, saved = excluded.saved, reason = excluded.reason, recorded_at = excluded.recorded_at",
                params![
                    run,
                    record.step.key(),
                    record.window.start.to_string(),
                    record.window.end.to_string(),
                    outcome,
                    i64::try_from(saved)?,
                    reason,
                    at
                ],
            )
            .context("recording a sync window")?;
        Ok(())
    }

    fn record_step(&self, run: i64, report: &StepReport, at: &str) -> Result<()> {
        let (state, reason) = match &report.state {
            StepState::Complete => ("complete", None),
            StepState::Partial { reason } => ("partial", Some(reason.as_str())),
            StepState::Failed { reason } => ("failed", Some(reason.as_str())),
            StepState::Skipped { reason } => ("skipped", Some(reason.as_str())),
        };
        self.connection(true)?
            .execute(
                "INSERT INTO sync_run_steps (run_id, step, state, windows, done, reused, saved, reason, recorded_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT (run_id, step) DO UPDATE SET state = excluded.state, windows = excluded.windows,
                 done = excluded.done, reused = excluded.reused, saved = excluded.saved,
                 reason = excluded.reason, recorded_at = excluded.recorded_at",
                params![
                    run,
                    report.step.key(),
                    state,
                    i64::try_from(report.windows)?,
                    i64::try_from(report.done)?,
                    i64::try_from(report.reused)?,
                    i64::try_from(report.saved)?,
                    reason,
                    at
                ],
            )
            .context("recording a sync step result")?;
        Ok(())
    }

    fn finish(&self, run: i64, state: RunState, at: &str) -> Result<()> {
        self.connection(true)?
            .execute(
                "UPDATE sync_runs SET state = ?2, finished_at = ?3 WHERE id = ?1",
                params![run, state.key(), at],
            )
            .context("finishing the sync run")?;
        Ok(())
    }
}
