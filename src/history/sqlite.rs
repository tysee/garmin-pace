//! src/history/sqlite.rs implements the history port using local SQLite. It
//! exposes explicit initialization and read-only queries with bounded caches.

use std::time::Duration;

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};

use super::{
    Activity, ActivityQuery, ActivitySearchPage, HistoryError, HistoryRepository, HistorySummary,
    HistoryWrite, SyncSummary,
};
use crate::config::DatabaseSettings;

const SCHEMA_VERSION: u32 = 11;
const INITIAL_SCHEMA: &str = include_str!("../../migrations/0001_history.sql");

/// SQLite history adapter. Connections are short-lived and never pooled.
pub struct SqliteHistory {
    settings: DatabaseSettings,
}

impl SqliteHistory {
    /// Open an already initialized database; do not create or migrate it.
    pub fn open(settings: DatabaseSettings) -> Result<Self> {
        let history = Self { settings };
        history.connection()?;
        Ok(history)
    }

    /// Apply known forward migrations atomically, preserving existing data.
    pub fn initialize(settings: &DatabaseSettings) -> Result<()> {
        crate::private_fs::database(&settings.path, true)?;
        let mut connection = Connection::open_with_flags(
            &settings.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .context("opening SQLite for initialization")?;
        connection.busy_timeout(Duration::from_millis(settings.busy_timeout_ms))?;
        let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        ensure!(
            version <= SCHEMA_VERSION,
            "database schema is newer than this application"
        );
        connection.pragma_update(None, "journal_mode", "WAL")?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: u32 =
            transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        ensure!(
            version <= SCHEMA_VERSION,
            "database schema is newer than this application"
        );
        if version == 0 {
            transaction
                .execute_batch(INITIAL_SCHEMA)
                .context("applying history migration")?;
            transaction.pragma_update(None, "user_version", 1)?;
        }
        if version < 2 {
            transaction
                .execute_batch(include_str!("../../migrations/0002_sync_state.sql"))
                .context("applying sync state migration")?;
            transaction.pragma_update(None, "user_version", 2)?;
        }
        if version < 3 {
            transaction
                .execute_batch(include_str!("../../migrations/0003_workout_attempts.sql"))
                .context("applying workout journal migration")?;
            transaction.pragma_update(None, "user_version", 3)?;
        }
        if version < 4 {
            transaction
                .execute_batch(include_str!("../../migrations/0004_health.sql"))
                .context("applying health metrics migration")?;
            transaction.pragma_update(None, "user_version", 4)?;
        }
        if version < 5 {
            transaction
                .execute_batch(include_str!("../../migrations/0005_workout_library.sql"))
                .context("applying workout library migration")?;
            transaction.pragma_update(None, "user_version", 5)?;
        }
        if version < 6 {
            transaction
                .execute_batch(include_str!("../../migrations/0006_benchmarks.sql"))
                .context("applying benchmark evidence migration")?;
            transaction.pragma_update(None, "user_version", 6)?;
        }
        if version < 7 {
            transaction
                .execute_batch(include_str!("../../migrations/0007_mcp_requests.sql"))
                .context("applying MCP request journal migration")?;
            transaction.pragma_update(None, "user_version", 7)?;
        }
        if version < 8 {
            transaction
                .execute_batch(include_str!("../../migrations/0008_activity_search.sql"))
                .context("applying activity search index migration")?;
            transaction.pragma_update(None, "user_version", 8)?;
        }
        if version < 9 {
            transaction
                .execute_batch(include_str!("../../migrations/0009_coach.sql"))
                .context("applying Coach snapshot migration")?;
            transaction.pragma_update(None, "user_version", 9)?;
        }
        if version < 10 {
            transaction
                .execute_batch(include_str!(
                    "../../migrations/0010_standalone_workouts.sql"
                ))
                .context("applying standalone workout index migration")?;
            transaction.pragma_update(None, "user_version", 10)?;
        }
        if version < 11 {
            transaction
                .execute_batch(include_str!("../../migrations/0011_sync_runs.sql"))
                .context("applying full sync journal migration")?;
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        transaction.commit().context("committing history migration")
    }

    pub(super) fn connection_with_flags(&self, flags: OpenFlags) -> Result<Connection> {
        crate::private_fs::database(&self.settings.path, false)?;
        let connection = Connection::open_with_flags(
            &self.settings.path,
            flags | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .context("opening history database; run init first")?;
        connection.busy_timeout(Duration::from_millis(self.settings.busy_timeout_ms))?;
        connection.pragma_update(None, "cache_size", -i64::from(self.settings.cache_kib))?;
        let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        ensure!(
            version == SCHEMA_VERSION,
            "unsupported database schema; run init with a compatible application"
        );
        Ok(connection)
    }

    fn connection(&self) -> Result<Connection> {
        self.connection_with_flags(OpenFlags::SQLITE_OPEN_READ_ONLY)
    }

    fn read_summary(&self) -> Result<HistorySummary> {
        let connection = self.connection()?;
        let count: i64 = connection
            .query_row("SELECT count(*) FROM activities", [], |row| row.get(0))
            .context("counting stored activities")?;
        let last_sync = connection
            .query_row(
                "SELECT completed_at, requested_limit, received_count FROM sync_state WHERE id = 1",
                [],
                |row| {
                    Ok(SyncSummary {
                        completed_at: row.get(0)?,
                        requested_limit: row.get(1)?,
                        received_count: row.get(2)?,
                    })
                },
            )
            .optional()
            .context("reading synchronization status")?;
        Ok(HistorySummary {
            last_sync,
            activity_count: u64::try_from(count).context("invalid stored activity count")?,
        })
    }

    fn read_activities(&self, limit: usize, offset: usize) -> Result<Vec<Activity>> {
        let connection = self.connection()?;
        let mut query = connection
            .prepare(
                "SELECT garmin_id, name, sport, started_at, duration_seconds, distance_meters
             FROM activities ORDER BY started_at DESC, garmin_id DESC LIMIT ?1 OFFSET ?2",
            )
            .context("preparing activity query")?;
        let rows = query
            .query_map([i64::try_from(limit)?, i64::try_from(offset)?], |row| {
                Ok(Activity {
                    garmin_id: row.get(0)?,
                    name: row.get(1)?,
                    sport: row.get(2)?,
                    started_at: row.get(3)?,
                    duration_seconds: row.get(4)?,
                    distance_meters: row.get(5)?,
                })
            })
            .context("reading activity page")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("decoding stored activities")
    }

    fn read_activity(&self, garmin_id: &str) -> Result<Option<Activity>> {
        self.connection()?
            .query_row(
                "SELECT garmin_id, name, sport, started_at, duration_seconds, distance_meters
             FROM activities WHERE garmin_id = ?1",
                [garmin_id],
                |row| {
                    Ok(Activity {
                        garmin_id: row.get(0)?,
                        name: row.get(1)?,
                        sport: row.get(2)?,
                        started_at: row.get(3)?,
                        duration_seconds: row.get(4)?,
                        distance_meters: row.get(5)?,
                    })
                },
            )
            .optional()
            .context("reading stored activity")
    }
}

impl HistoryRepository for SqliteHistory {
    fn search(&self, query: &ActivityQuery) -> Result<ActivitySearchPage, HistoryError> {
        let read = || -> Result<ActivitySearchPage> {
            let mut db = self.connection()?;
            let tx = db.transaction()?;
            let mut filters = vec!["1=1".to_owned()];
            let mut values: Vec<rusqlite::types::Value> = Vec::new();
            for (expression, value) in [
                (
                    "started_at >=",
                    query.start.map(|d| format!("{d}T00:00:00")),
                ),
                (
                    "started_at <",
                    query
                        .end
                        .and_then(|d| d.succ_opt())
                        .map(|d| format!("{d}T00:00:00")),
                ),
                ("sport =", query.sport.clone()),
            ] {
                if let Some(value) = value {
                    values.push(value.into());
                    filters.push(format!("{expression} ?{}", values.len()));
                }
            }
            if let Some(name) = &query.name {
                values.push(name.clone().into());
                filters.push(format!("instr(lower(name), lower(?{})) > 0", values.len()));
            }
            let filter = filters.join(" AND ");
            let total: i64 = tx.query_row(
                &format!("SELECT count(*) FROM activities WHERE {filter}"),
                rusqlite::params_from_iter(&values),
                |r| r.get(0),
            )?;
            let (limit, offset) = (values.len() + 1, values.len() + 2);
            values.extend([(query.limit as i64).into(), (query.offset as i64).into()]);
            let mut statement = tx.prepare(&format!("SELECT garmin_id,name,sport,started_at,duration_seconds,distance_meters FROM activities WHERE {filter} ORDER BY started_at DESC,garmin_id DESC LIMIT ?{limit} OFFSET ?{offset}"))?;
            let rows = statement.query_map(rusqlite::params_from_iter(values), |r| {
                Ok(Activity {
                    garmin_id: r.get(0)?,
                    name: r.get(1)?,
                    sport: r.get(2)?,
                    started_at: r.get(3)?,
                    duration_seconds: r.get(4)?,
                    distance_meters: r.get(5)?,
                })
            })?;
            Ok(ActivitySearchPage {
                activities: rows.collect::<rusqlite::Result<Vec<_>>>()?,
                total: u64::try_from(total)?,
            })
        };
        read().map_err(|e| HistoryError::Storage(e.into()))
    }
    fn begin_sync(&self) -> Result<Box<dyn HistoryWrite>, HistoryError> {
        super::sqlite_write::begin(self)
    }

    fn summary(&self) -> Result<HistorySummary, HistoryError> {
        self.read_summary()
            .map_err(|error| HistoryError::Storage(error.into()))
    }

    fn list(&self, limit: usize, offset: usize) -> Result<Vec<Activity>, HistoryError> {
        self.read_activities(limit, offset)
            .map_err(|error| HistoryError::Storage(error.into()))
    }

    fn find(&self, garmin_id: &str) -> Result<Option<Activity>, HistoryError> {
        self.read_activity(garmin_id)
            .map_err(|error| HistoryError::Storage(error.into()))
    }
}
