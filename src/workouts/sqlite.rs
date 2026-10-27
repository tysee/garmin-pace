//! src/workouts/sqlite.rs persists the profile's library, draft revisions and delivery reservations.
//! Single-statement compare-and-set transitions never reopen an uncertain write automatically.

use super::{
    Delivery, Draft, DraftState, LibraryWorkout, Workout, WorkoutError, WorkoutRepository,
    WorkoutSummary,
    document::{LibraryDocument, WorkoutDocument},
};
use crate::{config::DatabaseSettings, history::SqliteHistory};
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::time::Duration;

/// Private SQLite adapter bound only to the selected profile database.
pub struct SqliteWorkouts {
    settings: DatabaseSettings,
}
impl SqliteWorkouts {
    /// Validate the existing schema; library reads never create storage.
    pub fn open(settings: DatabaseSettings) -> Result<Self> {
        SqliteHistory::open(settings.clone())?;
        Ok(Self::attach(settings))
    }
    /// Bind to a profile database that the caller already validated with `SqliteHistory::open`.
    pub fn attach(settings: DatabaseSettings) -> Self {
        Self { settings }
    }
    pub(super) fn connection(&self, write: bool) -> Result<Connection> {
        crate::private_fs::database(&self.settings.path, false)?;
        let flags = if write {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let db = Connection::open_with_flags(
            &self.settings.path,
            flags | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        db.busy_timeout(Duration::from_millis(self.settings.busy_timeout_ms))?;
        db.pragma_update(None, "cache_size", -i64::from(self.settings.cache_kib))?;
        Ok(db)
    }
    fn read_draft(db: &Connection, id: u64) -> Result<Draft, WorkoutError> {
        let row = db.query_row("SELECT id,revision,definition,source_document,state,remote_id,mcp_request,explanation FROM workout_drafts WHERE id=?1", [sql_id(id)?], draft_row)
            .optional().map_err(storage)?.ok_or(WorkoutError::NotFound)?;
        decode_draft(row)
    }
}
pub(super) fn storage(error: impl Into<anyhow::Error>) -> WorkoutError {
    WorkoutError::Storage(error.into().into())
}
pub(super) fn changed(rows: usize) -> Result<(), WorkoutError> {
    if rows == 1 {
        Ok(())
    } else {
        Err(WorkoutError::Conflict)
    }
}
pub(super) fn mutation(error: rusqlite::Error) -> WorkoutError {
    if error.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation) {
        WorkoutError::Conflict
    } else {
        storage(error)
    }
}
type DraftRow = (
    i64,
    i64,
    String,
    Option<String>,
    String,
    Option<i64>,
    Option<String>,
    Option<String>,
);
fn draft_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DraftRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}
fn decode_draft(row: DraftRow) -> Result<Draft, WorkoutError> {
    let state: DraftState = row.4.parse()?;
    let definition: WorkoutDocument = serde_json::from_str(&row.2).map_err(storage)?;
    let source = row
        .3
        .map(|s| -> Result<LibraryWorkout, WorkoutError> {
            let document: LibraryDocument = serde_json::from_str(&s).map_err(storage)?;
            document.try_into()
        })
        .transpose()?;
    Ok(Draft {
        id: u64::try_from(row.0).map_err(storage)?,
        revision: u64::try_from(row.1).map_err(storage)?,
        workout: definition.try_into()?,
        source,
        state,
        remote_id: row.5.map(u64::try_from).transpose().map_err(storage)?,
        mcp_request: row.6,
        explanation: row.7,
    })
}
impl WorkoutRepository for SqliteWorkouts {
    fn deliveries(
        &self,
        workout_id: u64,
        device_id: Option<u64>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Delivery>, WorkoutError> {
        self.read_deliveries(workout_id, device_id, limit, offset)
    }
    fn exercises(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<super::benchmarks::ExerciseKey>, WorkoutError> {
        self.read_exercises(query, limit, offset)
    }
    fn list(
        &self,
        limit: usize,
        offset: usize,
        standalone: bool,
    ) -> Result<Vec<WorkoutSummary>, WorkoutError> {
        let db = self.connection(false).map_err(storage)?;
        let query = if standalone {
            "SELECT id,name,sport,revision,synced_at,json_type(document,'$.definition') != 'null' FROM workout_library WHERE json_extract(document,'$.coach_plan_id') IS NULL ORDER BY name COLLATE NOCASE,id LIMIT ?1 OFFSET ?2"
        } else {
            "SELECT id,name,sport,revision,synced_at,json_type(document,'$.definition') != 'null' FROM workout_library ORDER BY name COLLATE NOCASE,id LIMIT ?1 OFFSET ?2"
        };
        let mut q = db.prepare(query).map_err(storage)?;
        q.query_map(params![limit as i64, offset as i64], |r| {
            Ok(WorkoutSummary {
                id: r.get::<_, i64>(0)?.try_into().map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Integer,
                        Box::new(e),
                    )
                })?,
                name: r.get(1)?,
                sport: r.get(2)?,
                revision: r.get(3)?,
                synced_at: r.get(4)?,
                editable: r.get(5)?,
            })
        })
        .map_err(storage)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(storage)
    }
    fn get(&self, id: u64) -> Result<LibraryWorkout, WorkoutError> {
        let db = self.connection(false).map_err(storage)?;
        let text: String = db
            .query_row(
                "SELECT document FROM workout_library WHERE id=?1",
                [sql_id(id)?],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage)?
            .ok_or(WorkoutError::NotFound)?;
        let document: LibraryDocument = serde_json::from_str(&text).map_err(storage)?;
        document.try_into()
    }
    fn put(&self, workout: &LibraryWorkout) -> Result<(), WorkoutError> {
        put_workout(&self.connection(true).map_err(storage)?, workout)
    }

    fn drafts(&self, limit: usize, offset: usize) -> Result<Vec<Draft>, WorkoutError> {
        let db = self.connection(false).map_err(storage)?;
        let mut q = db.prepare("SELECT id,revision,definition,source_document,state,remote_id,mcp_request,explanation FROM workout_drafts WHERE state != 'discarded' ORDER BY id DESC LIMIT ?1 OFFSET ?2").map_err(storage)?;
        q.query_map(params![limit as i64, offset as i64], draft_row)
            .map_err(storage)?
            .map(|row| decode_draft(row.map_err(storage)?))
            .collect()
    }
    fn attention(&self) -> Result<super::DraftAttention, WorkoutError> {
        self.read_attention()
    }
    fn draft(&self, id: u64) -> Result<Draft, WorkoutError> {
        Self::read_draft(&self.connection(false).map_err(storage)?, id)
    }
    fn insert(
        &self,
        workout: &Workout,
        source: Option<&LibraryWorkout>,
        explanation: Option<&str>,
    ) -> Result<Draft, WorkoutError> {
        let definition =
            serde_json::to_string(&WorkoutDocument::from(workout.clone())).map_err(storage)?;
        let source_document = source
            .map(|s| serde_json::to_string(&LibraryDocument::from(s.clone())))
            .transpose()
            .map_err(storage)?;
        let db = self.connection(true).map_err(storage)?;
        db.execute(
            "INSERT INTO workout_drafts(definition,source_id,source_document,explanation) VALUES(?1,?2,?3,?4)",
            params![
                definition,
                source.map(|s| sql_id(s.id)).transpose()?,
                source_document,
                explanation
            ],
        )
        .map_err(storage)?;
        let id = u64::try_from(db.last_insert_rowid())
            .context("invalid draft ID")
            .map_err(storage)?;
        Self::read_draft(&db, id)
    }
    fn revise(
        &self,
        id: u64,
        revision: u64,
        workout: &Workout,
        explanation: Option<&str>,
    ) -> Result<Draft, WorkoutError> {
        let definition =
            serde_json::to_string(&WorkoutDocument::from(workout.clone())).map_err(storage)?;
        let db = self.connection(true).map_err(storage)?;
        changed(db.execute("UPDATE workout_drafts SET definition=?1,revision=revision+1,explanation=?4 WHERE id=?2 AND revision=?3 AND state='draft'", params![definition,sql_id(id)?,sql_id(revision)?,explanation]).map_err(storage)?)?;
        Self::read_draft(&db, id)
    }
    fn transition(
        &self,
        id: u64,
        revision: u64,
        from: DraftState,
        to: DraftState,
        remote_id: Option<u64>,
    ) -> Result<(), WorkoutError> {
        changed(self.connection(true).map_err(storage)?.execute("UPDATE workout_drafts SET state=?1,remote_id=?2 WHERE id=?3 AND revision=?4 AND state=?5",
            params![to.as_str(),remote_id.map(sql_id).transpose()?,sql_id(id)?,sql_id(revision)?,from.as_str()]).map_err(mutation)?)
    }
    fn record_request(&self, id: u64, revision: u64, request: &str) -> Result<(), WorkoutError> {
        changed(self.connection(true).map_err(storage)?.execute("UPDATE workout_drafts SET mcp_request=CASE WHEN mcp_request IS NULL THEN ?1 ELSE mcp_request || char(10) || ?1 END WHERE id=?2 AND revision=?3 AND state IN ('draft','saving','saved')", params![request,sql_id(id)?,sql_id(revision)?]).map_err(storage)?)
    }
    fn reserve_delivery(&self, d: &Delivery) -> Result<(), WorkoutError> {
        changed(self.connection(true).map_err(storage)?.execute("INSERT INTO workout_deliveries(workout_id,device_id,revision,state,mcp_request) VALUES(?1,?2,?3,'sending',?4) ON CONFLICT DO NOTHING", params![sql_id(d.workout_id)?,sql_id(d.device_id)?,d.revision,d.request]).map_err(storage)?)
    }
    fn complete_delivery(&self, d: &Delivery) -> Result<(), WorkoutError> {
        changed(self.connection(true).map_err(storage)?.execute("UPDATE workout_deliveries SET state='queued' WHERE workout_id=?1 AND device_id=?2 AND revision=?3 AND state='sending'", params![sql_id(d.workout_id)?,sql_id(d.device_id)?,d.revision]).map_err(storage)?)
    }
    fn release_delivery(&self, workout_id: u64, device_id: u64) -> Result<(), WorkoutError> {
        let removed = self
            .connection(true)
            .map_err(storage)?
            .execute(
                "DELETE FROM workout_deliveries WHERE workout_id=?1 AND device_id=?2 AND state='sending'",
                params![sql_id(workout_id)?, sql_id(device_id)?],
            )
            .map_err(storage)?;
        if removed == 0 {
            return Err(WorkoutError::NotFound);
        }
        Ok(())
    }
}

pub(super) fn sql_id(id: u64) -> Result<i64, WorkoutError> {
    if id == 0 {
        return Err(WorkoutError::InvalidInput(
            "ID and revision must be positive",
        ));
    }
    i64::try_from(id)
        .map_err(|_| WorkoutError::InvalidInput("ID or revision exceeds the supported range"))
}

/// Persist a canonical library source on the caller's connection or transaction.
pub(super) fn put_workout(db: &Connection, workout: &LibraryWorkout) -> Result<(), WorkoutError> {
    let document =
        serde_json::to_string(&LibraryDocument::from(workout.clone())).map_err(storage)?;
    db.execute("INSERT INTO workout_library VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(id) DO UPDATE SET name=excluded.name,sport=excluded.sport,revision=excluded.revision,synced_at=excluded.synced_at,document=excluded.document",
        params![sql_id(workout.id)?,workout.name,workout.sport,workout.revision,workout.synced_at,document]).map_err(storage)?;
    Ok(())
}
