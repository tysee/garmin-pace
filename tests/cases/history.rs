//! tests/cases/history.rs verifies migrations, persistence, query bounds, and config
//! rejection using real temporary SQLite databases and actual TOML files.

use anyhow::Result;
use garmin_pace::{
    config::Settings,
    history::{HistoryError, HistoryService, SqliteHistory},
};
use std::{fs, sync::Arc};

use crate::common::Fixture;

#[test]
fn initialization_is_repeatable_and_preserves_history() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    fixture.seed()?;
    fixture.init()?;
    let repository = SqliteHistory::open(fixture.settings.database)?;
    let history = HistoryService::new(Arc::new(repository), 100);
    assert_eq!(history.summary()?.activity_count, 2);
    let activities = history.list(1, 0)?;
    assert_eq!(activities.len(), 1);
    assert_eq!(activities[0].garmin_id, "fixture-2");
    // The offset skips newer rows so a page can continue where the previous one ended.
    let older = history.list(1, 1)?;
    assert_eq!(older[0].garmin_id, "fixture-1");
    assert!(history.list(1, 2)?.is_empty());
    for (limit, offset) in [(0, 0), (101, 0), (1, 100_001)] {
        assert!(matches!(
            history.list(limit, offset),
            Err(HistoryError::InvalidLimit { .. })
        ));
    }
    Ok(())
}

#[test]
fn reads_do_not_create_a_missing_database() -> Result<()> {
    let fixture = Fixture::new()?;
    assert!(SqliteHistory::open(fixture.settings.database.clone()).is_err());
    assert!(!fixture.settings.database.path.exists());
    Ok(())
}

#[test]
fn newer_schema_is_rejected_without_mutation() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let connection = rusqlite::Connection::open(&fixture.settings.database.path)?;
    connection.pragma_update(None, "user_version", 99)?;
    assert!(fixture.init().is_err());
    assert!(SqliteHistory::open(fixture.settings.database).is_err());
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(version, 99);
    Ok(())
}

#[test]
fn config_resolves_paths_and_rejects_unknown_or_unbounded_settings() -> Result<()> {
    let fixture = Fixture::new()?;
    assert_eq!(
        fixture.settings.database.path,
        fixture
            .directory
            .path()
            .canonicalize()?
            .join("data/garmin.sqlite3")
    );
    let valid = fs::read_to_string(&fixture.config)?;
    for invalid in [
        valid.replace("max_page_size = 100", "max_page_size = 0"),
        valid.replace("cache_kib = 2048", "cache_kib = 999999"),
        valid.replace("log_level = \"info\"", "unexpected = true"),
    ] {
        fs::write(&fixture.config, invalid)?;
        assert!(Settings::load(&fixture.config).is_err());
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn history_storage_is_private_and_rejects_permissive_files() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new()?;
    fixture.init()?;
    let path = &fixture.settings.database.path;
    assert_eq!(fs::metadata(path)?.permissions().mode() & 0o077, 0);
    assert_eq!(
        fs::metadata(
            path.parent()
                .ok_or_else(|| anyhow::anyhow!("missing parent"))?
        )?
        .permissions()
        .mode()
            & 0o077,
        0
    );
    fs::set_permissions(path, fs::Permissions::from_mode(0o644))?;
    assert!(SqliteHistory::open(fixture.settings.database.clone()).is_err());
    Ok(())
}

#[cfg(unix)]
#[test]
fn private_database_boundary_rejects_symlinks_and_sidecar_disclosure() -> Result<()> {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = Fixture::new()?;
    fixture.init()?;
    let path = &fixture.settings.database.path;
    let sidecar = path.with_file_name("garmin.sqlite3-wal");
    fs::write(&sidecar, b"sensitive sidecar")?;
    fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o644))?;
    assert!(SqliteHistory::open(fixture.settings.database.clone()).is_err());
    fs::remove_file(&sidecar)?;
    let target = path.with_file_name("private-copy");
    fs::rename(path, &target)?;
    symlink(&target, path)?;
    assert!(fixture.init().is_err());
    assert!(SqliteHistory::open(fixture.settings.database.clone()).is_err());
    fs::remove_file(path)?;
    fs::hard_link(&target, path)?;
    assert!(SqliteHistory::open(fixture.settings.database.clone()).is_err());
    Ok(())
}

#[cfg(unix)]
#[test]
fn private_file_checks_preserve_registry_locks() -> Result<()> {
    preserve_locks(false)
}

#[cfg(unix)]
#[test]
fn private_file_checks_preserve_wal_locks() -> Result<()> {
    preserve_locks(true)
}

#[cfg(unix)]
fn preserve_locks(wal: bool) -> Result<()> {
    use garmin_pace::access::SqliteAccess;
    let fixture = Fixture::new()?;
    let settings = if wal {
        fixture.init()?;
        fixture.seed()?;
        fixture.settings.database
    } else {
        let settings = fixture.settings.registry_database();
        SqliteAccess::initialize(&settings)?;
        settings
    };
    let reader = rusqlite::Connection::open(&settings.path)?;
    reader.execute_batch(if wal {
        "BEGIN; SELECT * FROM activities;"
    } else {
        "BEGIN; SELECT * FROM users;"
    })?;
    if wal {
        let writer = rusqlite::Connection::open(&settings.path)?;
        writer.execute("UPDATE activities SET name='new value'", [])?;
        SqliteHistory::open(settings.clone())?;
    } else {
        SqliteAccess::open(settings.clone())?;
    }
    let check = |expected: &str| -> Result<()> {
        let output = std::process::Command::new(std::env::current_exe()?)
            .args(["--exact", "history::sqlite_lock_probe", "--nocapture"])
            .env("PACE_TEST_LOCK_PATH", &settings.path)
            .env("PACE_TEST_LOCK_MODE", if wal { "wal" } else { "delete" })
            .env("PACE_TEST_EXPECT_LOCK", expected)
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    };
    check("held")?;
    reader.execute_batch("ROLLBACK")?;
    check("released")
}

#[cfg(unix)]
#[test]
fn sqlite_lock_probe() -> Result<()> {
    let Some(path) = std::env::var_os("PACE_TEST_LOCK_PATH") else {
        return Ok(());
    };
    let db = rusqlite::Connection::open(path)?;
    db.busy_timeout(std::time::Duration::from_millis(50))?;
    let expected = std::env::var("PACE_TEST_EXPECT_LOCK")? == "held";
    let blocked = if std::env::var("PACE_TEST_LOCK_MODE")? == "wal" {
        let busy: i32 = db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
        busy == 1
    } else {
        match db.execute_batch("BEGIN EXCLUSIVE; ROLLBACK;") {
            Ok(()) => false,
            Err(e) if e.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy) => true,
            Err(e) => return Err(e.into()),
        }
    };
    assert_eq!(
        blocked, expected,
        "a metadata check must not release another connection's SQLite locks"
    );
    Ok(())
}
