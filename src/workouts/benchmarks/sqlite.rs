//! src/workouts/benchmarks/sqlite.rs persists bounded strength evidence and benchmark drafts.
//! Imports are atomic; revision tokens cover the exact local evidence read for a proposal.

use super::{document::*, *};
use crate::{
    config::DatabaseSettings,
    health::Date,
    workouts::{
        DraftState, SqliteWorkouts, WorkoutError, WorkoutRepository,
        sqlite::{changed, mutation, sql_id, storage},
    },
};
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};

/// Benchmark adapter reusing the canonical profile-private workout connection.
pub struct SqliteBenchmarks {
    storage: SqliteWorkouts,
}
impl SqliteBenchmarks {
    /// Validate an initialized private profile before any reads.
    pub fn open(settings: DatabaseSettings) -> Result<Self> {
        Ok(Self {
            storage: SqliteWorkouts::open(settings)?,
        })
    }
    /// Bind to a profile database that the caller already validated with `SqliteHistory::open`.
    pub fn attach(settings: DatabaseSettings) -> Self {
        Self {
            storage: SqliteWorkouts::attach(settings),
        }
    }
    fn read_draft(db: &Connection, id: u64) -> Result<BenchmarkDraft, WorkoutError> {
        let row = db
            .query_row(
                "SELECT id,revision,proposal,context,state,mcp_request FROM benchmark_drafts WHERE id=?1",
                [sql_id(id)?],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(storage)?
            .ok_or(WorkoutError::NotFound)?;
        DraftDocument {
            id: u64::try_from(row.0).map_err(storage)?,
            revision: u64::try_from(row.1).map_err(storage)?,
            proposal: serde_json::from_str(&row.2).map_err(storage)?,
            context: serde_json::from_str(&row.3).map_err(storage)?,
            state: row.4,
            mcp_request: row.5,
        }
        .try_into()
    }
}
fn catalog(db: &Connection, rows: &[Benchmark]) -> Result<()> {
    ensure!(rows.len() <= 128, "benchmark catalogue exceeds bound");
    db.execute("DELETE FROM benchmark_catalog", [])?;
    for row in rows {
        db.execute(
            "INSERT INTO benchmark_catalog VALUES(?1,?2)",
            params![
                row.exercise.name,
                serde_json::to_string(&BenchmarkDocument::from(row.clone()))?
            ],
        )?;
    }
    Ok(())
}
fn upsert_session(db: &Connection, session: &StrengthSession) -> Result<(), WorkoutError> {
    let doc = serde_json::to_string(&SessionDocument::from(session.clone())).map_err(storage)?;
    db.execute("INSERT INTO strength_sessions VALUES(?1,?2,?3) ON CONFLICT(activity_id) DO UPDATE SET date=excluded.date,document=excluded.document",params![sql_id(session.activity_id)?,session.date.to_string(),doc]).map_err(storage)?;
    Ok(())
}
impl BenchmarkRepository for SqliteBenchmarks {
    fn catalog(&self) -> Result<Vec<Benchmark>, WorkoutError> {
        let db = self.storage.connection(false).map_err(storage)?;
        let mut query = db
            .prepare("SELECT document FROM benchmark_catalog ORDER BY key LIMIT 129")
            .map_err(storage)?;
        let documents = query
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(storage)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage)?;
        if documents.len() > 128 {
            return Err(WorkoutError::InvalidInput(
                "benchmark catalogue exceeds bound",
            ));
        }
        documents
            .into_iter()
            .map(|s| {
                serde_json::from_str::<BenchmarkDocument>(&s)
                    .map(Into::into)
                    .map_err(storage)
            })
            .collect()
    }
    fn replace_catalog(&self, rows: &[Benchmark]) -> Result<(), WorkoutError> {
        let mut db = self.storage.connection(true).map_err(storage)?;
        let tx = db.transaction().map_err(storage)?;
        catalog(&tx, rows).map_err(storage)?;
        tx.commit().map_err(storage)
    }
    fn import(&self, rows: &[Benchmark], import: &StrengthImport) -> Result<(), WorkoutError> {
        let mut db = self.storage.connection(true).map_err(storage)?;
        let tx = db.transaction().map_err(storage)?;
        catalog(&tx, rows).map_err(storage)?;
        tx.execute(
            "DELETE FROM strength_sessions WHERE date>=?1 AND date<=?2",
            params![
                import.window.start.to_string(),
                import.window.end.to_string()
            ],
        )
        .map_err(storage)?;
        for session in &import.sessions {
            upsert_session(&tx, session)?;
        }
        tx.execute("INSERT INTO strength_imports VALUES(?1,?2,?3) ON CONFLICT(start,end) DO UPDATE SET synced_at=excluded.synced_at",params![import.window.start.to_string(),import.window.end.to_string(),import.synced_at]).map_err(storage)?;
        tx.commit().map_err(storage)
    }
    fn import_session(&self, session: &StrengthSession) -> Result<(), WorkoutError> {
        let mut db = self.storage.connection(true).map_err(storage)?;
        let tx = db.transaction().map_err(storage)?;
        upsert_session(&tx, session)?;
        tx.commit().map_err(storage)
    }
    fn evidence(&self, key: &str, window: Window) -> Result<Evidence, WorkoutError> {
        let db = self.storage.connection(false).map_err(storage)?;
        db.execute_batch("BEGIN").map_err(storage)?;
        let text: String = db
            .query_row(
                "SELECT document FROM benchmark_catalog WHERE key=?1",
                [key],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage)?
            .ok_or(WorkoutError::NotFound)?;
        let source = serde_json::from_str::<BenchmarkDocument>(&text).map_err(storage)?;
        let synced_at:Option<String>=db.query_row("SELECT synced_at FROM strength_imports WHERE start<=?1 AND end>=?2 ORDER BY synced_at DESC LIMIT 1",params![window.start.to_string(),window.end.to_string()],|r|r.get(0)).optional().map_err(storage)?;
        let mut query=db.prepare("SELECT document FROM strength_sessions WHERE date>=?1 AND date<=?2 ORDER BY date,activity_id LIMIT 101").map_err(storage)?;
        let documents = query
            .query_map(
                params![window.start.to_string(), window.end.to_string()],
                |r| r.get::<_, String>(0),
            )
            .map_err(storage)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage)?;
        if documents.len() > 100 {
            return Err(WorkoutError::InvalidInput(
                "more than 100 strength sessions; choose a shorter window",
            ));
        }
        let encoded = serde_json::to_vec(&(
            key,
            window.start.to_string(),
            window.end.to_string(),
            &source,
            &synced_at,
            &documents,
        ))
        .map_err(storage)?;
        let revision = ring::digest::digest(&ring::digest::SHA256, &encoded)
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let sessions = documents
            .into_iter()
            .map(|s| -> Result<StrengthSession, WorkoutError> {
                serde_json::from_str::<SessionDocument>(&s)
                    .map_err(storage)?
                    .try_into()
            })
            .collect::<Result<_, _>>()?;
        Ok(Evidence {
            source: source.into(),
            window,
            revision,
            synced_at,
            sessions,
        })
    }
    fn activity_sets(&self, activity_id: u64, date: Date) -> Result<ActivitySets, WorkoutError> {
        let db = self.storage.connection(false).map_err(storage)?;
        db.execute_batch("BEGIN").map_err(storage)?;
        let import_synced_at:Option<String>=db.query_row("SELECT synced_at FROM strength_imports WHERE start<=?1 AND end>=?1 ORDER BY synced_at DESC LIMIT 1",[date.to_string()],|r|r.get(0)).optional().map_err(storage)?;
        let document: Option<String> = db
            .query_row(
                "SELECT document FROM strength_sessions WHERE activity_id=?1",
                [sql_id(activity_id)?],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage)?;
        let session = document
            .map(|s| -> Result<StrengthSession, WorkoutError> {
                serde_json::from_str::<SessionDocument>(&s)
                    .map_err(storage)?
                    .try_into()
            })
            .transpose()?;
        Ok(ActivitySets {
            activity_id,
            date,
            import_synced_at,
            session,
        })
    }
    fn workout_draft(&self, id: u64) -> Result<crate::workouts::Draft, WorkoutError> {
        self.storage.draft(id)
    }
    fn drafts(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<BenchmarkDraftSummary>, WorkoutError> {
        let db = self.storage.connection(false).map_err(storage)?;
        let mut query=db.prepare("SELECT id,revision,key,state FROM benchmark_drafts WHERE state!='discarded' ORDER BY id DESC LIMIT ?1 OFFSET ?2").map_err(storage)?;
        query
            .query_map(params![limit as i64, offset as i64], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(storage)?
            .map(|r| {
                let (id, revision, key, s) = r.map_err(storage)?;
                Ok(BenchmarkDraftSummary {
                    id: u64::try_from(id).map_err(storage)?,
                    revision: u64::try_from(revision).map_err(storage)?,
                    key,
                    state: s.parse()?,
                })
            })
            .collect()
    }
    fn draft(&self, id: u64) -> Result<BenchmarkDraft, WorkoutError> {
        Self::read_draft(&self.storage.connection(false).map_err(storage)?, id)
    }
    fn insert(
        &self,
        proposal: &Proposal,
        context: &Context,
    ) -> Result<BenchmarkDraft, WorkoutError> {
        let db = self.storage.connection(true).map_err(storage)?;
        db.execute(
            "INSERT INTO benchmark_drafts(key,proposal,context) VALUES(?1,?2,?3)",
            params![
                proposal.key,
                serde_json::to_string(&ProposalDocument::from(proposal.clone()))
                    .map_err(storage)?,
                serde_json::to_string(&ContextDocument::from(context.clone())).map_err(storage)?
            ],
        )
        .map_err(storage)?;
        Self::read_draft(&db, u64::try_from(db.last_insert_rowid()).map_err(storage)?)
    }
    fn revise(
        &self,
        id: u64,
        revision: u64,
        proposal: &Proposal,
        context: &Context,
    ) -> Result<BenchmarkDraft, WorkoutError> {
        let db = self.storage.connection(true).map_err(storage)?;
        changed(db.execute("UPDATE benchmark_drafts SET revision=revision+1,proposal=?1,context=?2 WHERE id=?3 AND revision=?4 AND state='draft'",params![serde_json::to_string(&ProposalDocument::from(proposal.clone())).map_err(storage)?,serde_json::to_string(&ContextDocument::from(context.clone())).map_err(storage)?,sql_id(id)?,sql_id(revision)?]).map_err(storage)?)?;
        Self::read_draft(&db, id)
    }
    fn transition(
        &self,
        id: u64,
        revision: u64,
        from: DraftState,
        to: DraftState,
    ) -> Result<(), WorkoutError> {
        changed(
            self.storage
                .connection(true)
                .map_err(storage)?
                .execute(
                    "UPDATE benchmark_drafts SET state=?1 WHERE id=?2 AND revision=?3 AND state=?4",
                    params![to.as_str(), sql_id(id)?, sql_id(revision)?, from.as_str()],
                )
                .map_err(mutation)?,
        )
    }
    fn record_request(&self, id: u64, revision: u64, request: &str) -> Result<(), WorkoutError> {
        changed(self.storage.connection(true).map_err(storage)?.execute("UPDATE benchmark_drafts SET mcp_request=CASE WHEN mcp_request IS NULL THEN ?1 ELSE mcp_request || char(10) || ?1 END WHERE id=?2 AND revision=?3 AND state IN ('draft','saving','saved')", params![request,sql_id(id)?,sql_id(revision)?]).map_err(storage)?)
    }
}
