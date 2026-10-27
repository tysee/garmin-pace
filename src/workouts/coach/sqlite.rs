//! src/workouts/coach/sqlite.rs implements the Coach snapshot transaction on profile SQLite.
//! It replaces one complete window and its library definitions together, preserving old data
//! on failure. Local reads never migrate storage or contact Garmin.

use super::{Repository, Snapshot, Window, document::SnapshotDocument};
use crate::workouts::sqlite::{put_workout, storage};
use crate::workouts::{SqliteWorkouts, WorkoutError};
use rusqlite::{OptionalExtension, params};

impl Repository for SqliteWorkouts {
    fn put_coach(&self, snapshot: &Snapshot) -> Result<(), WorkoutError> {
        let text =
            serde_json::to_string(&SnapshotDocument::from(snapshot.clone())).map_err(storage)?;
        let mut db = self.connection(true).map_err(storage)?;
        let tx = db.transaction().map_err(storage)?;
        for session in &snapshot.sessions {
            put_workout(&tx, &session.workout)?;
        }
        tx.execute("INSERT INTO coach_snapshots VALUES(?1,?2,?3) ON CONFLICT(start_date,end_date) DO UPDATE SET document=excluded.document",
            params![snapshot.window.start.to_string(),snapshot.window.end.to_string(),text]).map_err(storage)?;
        tx.commit().map_err(storage)
    }
    fn get_coach(&self, window: Window) -> Result<Option<Snapshot>, WorkoutError> {
        let text: Option<String> = self
            .connection(false)
            .map_err(storage)?
            .query_row(
                "SELECT document FROM coach_snapshots WHERE start_date=?1 AND end_date=?2",
                params![window.start.to_string(), window.end.to_string()],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage)?;
        text.map(|s| {
            serde_json::from_str::<SnapshotDocument>(&s)
                .map_err(storage)?
                .try_into()
        })
        .transpose()
    }
}
