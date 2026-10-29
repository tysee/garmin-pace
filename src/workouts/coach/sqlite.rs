//! src/workouts/coach/sqlite.rs implements the Coach snapshot transaction and the reschedule
//! journal on profile SQLite. A snapshot replaces one complete window and its library
//! definitions together, preserving old data on failure; a move is journaled before Garmin is
//! contacted and closed by one compare-and-set update. Local reads never migrate storage.

use super::{
    Repository, Reschedule, RescheduleState, Session, Snapshot, Window,
    document::{SessionDocument, SnapshotDocument},
};
use crate::workouts::sqlite::{changed, put_workout, sql_id, storage};
use crate::workouts::{SqliteWorkouts, WorkoutError};
use rusqlite::{OptionalExtension, Row, params};

const RESCHEDULE_COLUMNS: &str =
    "id,schedule_id,workout_id,from_date,to_date,state,requested_at,mcp_request";

fn reschedule(row: &Row<'_>) -> rusqlite::Result<Reschedule> {
    let text = |index: usize| -> rusqlite::Result<String> { row.get(index) };
    let date = |index: usize| {
        text(index)?.parse().map_err(|_| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                "invalid journaled Coach date".into(),
            )
        })
    };
    let state = text(5)?;
    Ok(Reschedule {
        id: row.get::<_, i64>(0)? as u64,
        schedule_id: row.get::<_, i64>(1)? as u64,
        workout_id: row.get::<_, i64>(2)? as u64,
        from: date(3)?,
        to: date(4)?,
        state: RescheduleState::parse(&state).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                "unknown Coach reschedule state".into(),
            )
        })?,
        requested_at: text(6)?,
        request: row.get(7)?,
    })
}

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
    fn find_session(&self, schedule_id: u64) -> Result<Option<Session>, WorkoutError> {
        // Overlapping windows may hold the same entry; the newest import is the reference.
        let text: Option<String> = self
            .connection(false)
            .map_err(storage)?
            .query_row(
                "SELECT j.value FROM coach_snapshots s, json_each(s.document,'$.sessions') j WHERE json_extract(j.value,'$.schedule_id')=?1 ORDER BY json_extract(s.document,'$.synced_at') DESC LIMIT 1",
                params![sql_id(schedule_id)?],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage)?;
        text.map(|s| {
            serde_json::from_str::<SessionDocument>(&s)
                .map_err(storage)?
                .try_into()
        })
        .transpose()
    }
    fn begin_reschedule(&self, r: &Reschedule) -> Result<u64, WorkoutError> {
        let db = self.connection(true).map_err(storage)?;
        let inserted = db
            .execute(
                "INSERT INTO coach_reschedules(schedule_id,workout_id,from_date,to_date,state,requested_at,mcp_request) VALUES(?1,?2,?3,?4,'moving',?5,?6) ON CONFLICT DO NOTHING",
                params![
                    sql_id(r.schedule_id)?,
                    sql_id(r.workout_id)?,
                    r.from.to_string(),
                    r.to.to_string(),
                    r.requested_at,
                    r.request
                ],
            )
            .map_err(storage)?;
        changed(inserted)?;
        Ok(db.last_insert_rowid() as u64)
    }
    fn finish_reschedule(
        &self,
        schedule_id: u64,
        state: RescheduleState,
    ) -> Result<(), WorkoutError> {
        changed(
            self.connection(true)
                .map_err(storage)?
                .execute(
                    "UPDATE coach_reschedules SET state=?2 WHERE schedule_id=?1 AND state='moving'",
                    params![sql_id(schedule_id)?, state.as_str()],
                )
                .map_err(storage)?,
        )
    }
    fn pending_reschedule(&self, schedule_id: u64) -> Result<Option<Reschedule>, WorkoutError> {
        self.connection(false)
            .map_err(storage)?
            .query_row(
                &format!(
                    "SELECT {RESCHEDULE_COLUMNS} FROM coach_reschedules WHERE schedule_id=?1 AND state='moving'"
                ),
                params![sql_id(schedule_id)?],
                reschedule,
            )
            .optional()
            .map_err(storage)
    }
    fn reschedules_of(&self, schedule_id: u64) -> Result<Vec<Reschedule>, WorkoutError> {
        let db = self.connection(false).map_err(storage)?;
        let mut query = db
            .prepare(&format!(
                "SELECT {RESCHEDULE_COLUMNS} FROM coach_reschedules WHERE schedule_id=?1 ORDER BY id DESC LIMIT 20"
            ))
            .map_err(storage)?;
        let rows = query
            .query_map(params![sql_id(schedule_id)?], reschedule)
            .map_err(storage)?;
        rows.collect::<Result<_, _>>().map_err(storage)
    }
    fn reschedules(&self, window: Window) -> Result<Vec<Reschedule>, WorkoutError> {
        let db = self.connection(false).map_err(storage)?;
        let mut query = db
            .prepare(&format!(
                "SELECT {RESCHEDULE_COLUMNS} FROM coach_reschedules WHERE (from_date BETWEEN ?1 AND ?2) OR (to_date BETWEEN ?1 AND ?2) ORDER BY id DESC LIMIT 100"
            ))
            .map_err(storage)?;
        let rows = query
            .query_map(
                params![window.start.to_string(), window.end.to_string()],
                reschedule,
            )
            .map_err(storage)?;
        rows.collect::<Result<_, _>>().map_err(storage)
    }
}
