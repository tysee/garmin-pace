//! src/workouts/sqlite_queries.rs reads bounded delivery journals and exact imported exercise
//! identities. It supplies the SQLite implementations of the workout service's read ports.

use super::{
    Delivery, SqliteWorkouts, WorkoutError,
    benchmarks::ExerciseKey,
    sqlite::{sql_id, storage},
};
use rusqlite::params;

impl SqliteWorkouts {
    pub(super) fn read_attention(&self) -> Result<super::DraftAttention, WorkoutError> {
        let db = self.connection(false).map_err(storage)?;
        db.query_row(
            "SELECT
            (SELECT count(*) FROM workout_drafts WHERE state='draft'),
            (SELECT count(*) FROM workout_drafts WHERE state IN ('saving','saved')),
            (SELECT min(id) FROM workout_drafts WHERE state='draft'),
            (SELECT min(id) FROM workout_drafts WHERE state IN ('saving','saved'))",
            [],
            |row| {
                Ok(super::DraftAttention {
                    ready: row.get::<_, i64>(0)? as u64,
                    pending: row.get::<_, i64>(1)? as u64,
                    oldest_ready: row.get::<_, Option<i64>>(2)?.map(|id| id as u64),
                    oldest_pending: row.get::<_, Option<i64>>(3)?.map(|id| id as u64),
                })
            },
        )
        .map_err(storage)
    }

    pub(super) fn read_deliveries(
        &self,
        id: u64,
        device: Option<u64>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Delivery>, WorkoutError> {
        let db = self.connection(false).map_err(storage)?;
        let mut q = db.prepare("SELECT workout_id,device_id,revision,state,mcp_request FROM workout_deliveries WHERE workout_id=?1 AND (?2 IS NULL OR device_id=?2) ORDER BY device_id,revision LIMIT ?3 OFFSET ?4").map_err(storage)?;
        q.query_map(
            params![
                sql_id(id)?,
                device.map(sql_id).transpose()?,
                limit as i64,
                offset as i64
            ],
            |r| {
                Ok(Delivery {
                    workout_id: id,
                    device_id: r.get::<_, i64>(1)? as u64,
                    revision: r.get(2)?,
                    queued: r.get::<_, String>(3)? == "queued",
                    request: r.get(4)?,
                })
            },
        )
        .map_err(storage)?
        .map(|r| r.map_err(storage))
        .collect()
    }
    pub(super) fn read_exercises(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ExerciseKey>, WorkoutError> {
        let db = self.connection(false).map_err(storage)?;
        let mut q = db.prepare("WITH identities AS (
            SELECT json_extract(j.value,'$.category') AS category,json_extract(j.value,'$.name') AS name
            FROM workout_library w,json_tree(w.document,'$.definition') j WHERE j.type='object' AND j.key='exercise'
            UNION
            SELECT json_extract(j.value,'$.category'),json_extract(j.value,'$.name')
            FROM benchmark_catalog b,json_tree(b.document) j WHERE j.type='object'
        ) SELECT category,name FROM identities WHERE typeof(category)='text' AND typeof(name)='text' AND category!='' AND name!=''
        AND instr(lower(category || ' ' || name),lower(?1))>0 ORDER BY category,name LIMIT ?2 OFFSET ?3").map_err(storage)?;
        q.query_map(params![query, limit as i64, offset as i64], |r| {
            Ok(ExerciseKey {
                category: r.get(0)?,
                name: r.get(1)?,
            })
        })
        .map_err(storage)?
        .map(|r| r.map_err(storage))
        .collect()
    }
}
