//! src/access/sqlite.rs persists the private local user/key registry with an embedded migration.
//! It implements AccessRegistry using bound SQL and read-only connections for request checks.

use super::{AccessError, AccessRegistry, Grant, KeyInfo, User};
use crate::config::DatabaseSettings;
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::time::Duration;

/// SQLite adapter for a registry separate from all users' activity databases.
pub struct SqliteAccess {
    settings: DatabaseSettings,
}
impl SqliteAccess {
    /// Initialize the registry explicitly; normal reads never create a database.
    pub fn initialize(settings: &DatabaseSettings) -> Result<()> {
        crate::private_fs::database(&settings.path, true)?;
        let mut db = Connection::open_with_flags(
            &settings.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        db.busy_timeout(Duration::from_millis(settings.busy_timeout_ms))?;
        let tx = db.transaction()?;
        let version: u32 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        ensure!(
            version <= 2,
            "access registry schema is newer than this application"
        );
        if version == 0 {
            tx.execute_batch(include_str!("../../migrations/access/0001_registry.sql"))?;
        }
        if version < 2 {
            tx.execute_batch(include_str!(
                "../../migrations/access/0002_garmin_writes.sql"
            ))?;
            tx.pragma_update(None, "user_version", 2)?;
        }
        tx.commit().context("committing access registry migration")
    }
    /// Open an initialized registry without caching credentials or grants.
    pub fn open(settings: DatabaseSettings) -> Result<Self> {
        let registry = Self { settings };
        registry.connection(false)?;
        Ok(registry)
    }
    fn connection(&self, write: bool) -> Result<Connection> {
        crate::private_fs::database(&self.settings.path, false)?;
        let mode = if write {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let db = Connection::open_with_flags(
            &self.settings.path,
            mode | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        db.busy_timeout(Duration::from_millis(self.settings.busy_timeout_ms))?;
        db.pragma_update(None, "cache_size", -i64::from(self.settings.cache_kib))?;
        db.pragma_update(None, "foreign_keys", true)?;
        let version: u32 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        ensure!(version == 2, "unsupported access registry; run users init");
        Ok(db)
    }
}
fn storage(error: impl Into<anyhow::Error>) -> AccessError {
    AccessError::Storage(error.into().into())
}
fn write_error(error: rusqlite::Error) -> AccessError {
    if error.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation) {
        AccessError::Conflict
    } else {
        storage(error)
    }
}
fn read_user(row: &rusqlite::Row<'_>) -> rusqlite::Result<User> {
    Ok(User {
        name: row.get(0)?,
        database_path: row.get::<_, String>(1)?.into(),
        session_path: row.get::<_, String>(2)?.into(),
        enabled: row.get(3)?,
        garmin_writes: row.get(4)?,
    })
}
fn changed(count: usize) -> Result<(), AccessError> {
    if count == 0 {
        Err(AccessError::NotFound)
    } else {
        Ok(())
    }
}
impl AccessRegistry for SqliteAccess {
    fn add(&self, user: &User) -> Result<(), AccessError> {
        let db = self.connection(true).map_err(storage)?;
        let count = db
            .execute(
                "INSERT INTO users(name,database_path,session_path,enabled,garmin_writes) SELECT ?1,?2,?3,1,0 WHERE (SELECT count(*) FROM users) < 1000",
                params![
                    user.name,
                    user.database_path
                        .to_str()
                        .ok_or(AccessError::InvalidUser)?,
                    user.session_path.to_str().ok_or(AccessError::InvalidUser)?
                ],
            )
            .map_err(write_error)?;
        if count == 0 {
            return Err(AccessError::Conflict);
        }
        Ok(())
    }
    fn user(&self, name: &str) -> Result<Option<User>, AccessError> {
        self.connection(false)
            .map_err(storage)?
            .query_row(
                "SELECT name,database_path,session_path,enabled,garmin_writes FROM users WHERE name=?1",
                [name],
                read_user,
            )
            .optional()
            .map_err(storage)
    }
    fn users(&self) -> Result<Vec<User>, AccessError> {
        let db = self.connection(false).map_err(storage)?;
        let mut query = db.prepare("SELECT name,database_path,session_path,enabled,garmin_writes FROM users ORDER BY name LIMIT 1000").map_err(storage)?;
        query
            .query_map([], read_user)
            .map_err(storage)?
            .collect::<rusqlite::Result<_>>()
            .map_err(storage)
    }
    fn set_enabled(&self, name: &str, enabled: bool) -> Result<(), AccessError> {
        changed(
            self.connection(true)
                .map_err(storage)?
                .execute(
                    "UPDATE users SET enabled=?1 WHERE name=?2",
                    params![enabled, name],
                )
                .map_err(storage)?,
        )
    }
    fn set_garmin_writes(&self, name: &str, allowed: bool) -> Result<(), AccessError> {
        changed(
            self.connection(true)
                .map_err(storage)?
                .execute(
                    "UPDATE users SET garmin_writes=?1 WHERE name=?2",
                    params![allowed, name],
                )
                .map_err(storage)?,
        )
    }
    fn insert_key(
        &self,
        user: &str,
        id: &str,
        digest: &[u8],
        expires_at: i64,
    ) -> Result<(), AccessError> {
        let count = self.connection(true).map_err(storage)?.execute(
            "INSERT INTO access_keys SELECT ?1,?2,?3,?4,0 WHERE (SELECT count(*) FROM access_keys WHERE user_name=?2) < 1000",
            params![id,user,digest,expires_at]).map_err(write_error)?;
        if count == 0 {
            return Err(AccessError::Conflict);
        }
        Ok(())
    }
    fn grant(&self, digest: &[u8]) -> Result<Option<Grant>, AccessError> {
        self.connection(false).map_err(storage)?.query_row(
            "SELECT u.name,u.database_path,u.session_path,u.enabled,u.garmin_writes,k.expires_at,k.revoked FROM access_keys k JOIN users u ON u.name=k.user_name WHERE k.digest=?1", [digest],
            |r| Ok(Grant { user: read_user(r)?, expires_at:r.get(5)?, revoked:r.get(6)? })).optional().map_err(storage)
    }
    fn keys(&self, user: &str) -> Result<Vec<KeyInfo>, AccessError> {
        let db = self.connection(false).map_err(storage)?;
        let mut query = db.prepare("SELECT id,expires_at,revoked FROM access_keys WHERE user_name=?1 ORDER BY expires_at DESC LIMIT 1000").map_err(storage)?;
        query
            .query_map([user], |r| {
                Ok(KeyInfo {
                    id: r.get(0)?,
                    expires_at: r.get(1)?,
                    revoked: r.get(2)?,
                })
            })
            .map_err(storage)?
            .collect::<rusqlite::Result<_>>()
            .map_err(storage)
    }
    fn revoke(&self, id: &str) -> Result<(), AccessError> {
        changed(
            self.connection(true)
                .map_err(storage)?
                .execute("UPDATE access_keys SET revoked=1 WHERE id=?1", [id])
                .map_err(storage)?,
        )
    }
}
