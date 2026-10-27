//! src/history/sqlite_write.rs implements the sync transaction port using SQLite.
//! The use case chooses the boundary; dropping the connection rolls back unfinished work.

use super::{Activity, HistoryError, HistoryWrite, SqliteHistory, SyncSummary};
use anyhow::Result;
use rusqlite::{Connection, OpenFlags, params};

pub(super) fn begin(history: &SqliteHistory) -> Result<Box<dyn HistoryWrite>, HistoryError> {
    let connection = history
        .connection_with_flags(OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(storage)?;
    connection
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| storage(e.into()))?;
    Ok(Box::new(WriteTransaction { connection }))
}

struct WriteTransaction {
    connection: Connection,
}

fn storage(error: anyhow::Error) -> HistoryError {
    HistoryError::Storage(error.into())
}

impl HistoryWrite for WriteTransaction {
    fn upsert(&mut self, activities: &[Activity]) -> Result<(), HistoryError> {
        let mut statement = self.connection.prepare_cached(
            "INSERT INTO activities (garmin_id, name, sport, started_at, duration_seconds, distance_meters)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(garmin_id) DO UPDATE SET
             name=excluded.name, sport=excluded.sport, started_at=excluded.started_at,
             duration_seconds=excluded.duration_seconds, distance_meters=excluded.distance_meters"
        ).map_err(|e| storage(e.into()))?;
        for activity in activities {
            statement
                .execute(params![
                    activity.garmin_id,
                    activity.name,
                    activity.sport,
                    activity.started_at,
                    activity.duration_seconds,
                    activity.distance_meters
                ])
                .map_err(|e| storage(e.into()))?;
        }
        Ok(())
    }
    fn record_sync(&mut self, summary: &SyncSummary) -> Result<(), HistoryError> {
        self.connection
            .execute(
                "INSERT INTO sync_state VALUES (1, ?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET
             completed_at=excluded.completed_at, requested_limit=excluded.requested_limit,
             received_count=excluded.received_count",
                params![
                    summary.completed_at,
                    summary.requested_limit,
                    summary.received_count
                ],
            )
            .map_err(|e| storage(e.into()))?;
        Ok(())
    }
    fn commit(self: Box<Self>) -> Result<(), HistoryError> {
        self.connection
            .execute_batch("COMMIT")
            .map_err(|e| storage(e.into()))
    }
}
