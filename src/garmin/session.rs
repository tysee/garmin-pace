//! src/garmin/session.rs stores native OAuth tokens privately and atomically.
//! It locks refresh and login across processes, waiting a bounded time for another holder,
//! and exposes only expiry metadata outside auth adapters; reads need no lock.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, TryLockError},
    io::{Read, Write},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

/// How long a refresh or login waits for another process holding the session lock: longer
/// than one token request, shorter than an interactive login, after which it fails explicitly.
const LOCK_WAIT: Duration = Duration::from_secs(30);
const LOCK_POLL: Duration = Duration::from_millis(100);

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Session {
    pub version: u32,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    pub client_id: String,
    pub token_url: String,
    pub api_base_url: String,
}

pub(super) struct SessionStore {
    path: PathBuf,
    _lock: File,
}
impl SessionStore {
    pub(super) fn lock(path: &Path) -> Result<Self> {
        Self::lock_within(path, LOCK_WAIT)
    }
    /// Take the exclusive lock, waiting up to `wait` so a concurrent refresh can finish first;
    /// the caller then reads the session that refresh saved instead of refreshing again.
    fn lock_within(path: &Path, wait: Duration) -> Result<Self> {
        let parent = path.parent().context("session path has no parent")?;
        crate::private_fs::directory(parent)?;
        ensure!(!path.is_symlink(), "session file must not be a symlink");
        let lock = crate::private_fs::open(&path.with_extension("lock"), true, true)?;
        let deadline = Instant::now() + wait;
        loop {
            match lock.try_lock() {
                Ok(()) => break,
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    thread::sleep(LOCK_POLL)
                }
                Err(TryLockError::WouldBlock) => anyhow::bail!(
                    "another Garmin authentication operation held the session for {} s; retry after it finishes",
                    wait.as_secs()
                ),
                Err(TryLockError::Error(error)) => {
                    return Err(error).context("locking the Garmin session");
                }
            }
        }
        Ok(Self {
            path: path.to_owned(),
            _lock: lock,
        })
    }
    pub(super) fn load(&self) -> Result<Session> {
        read(&self.path)
    }
    pub(super) fn save(&self, session: &Session) -> Result<()> {
        crate::private_fs::check_directory(self.path.parent().context("session parent")?)?;
        let mut file =
            tempfile::NamedTempFile::new_in(self.path.parent().context("session parent")?)
                .context("creating session replacement")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        serde_json::to_writer(&mut file, session)
            .map_err(|_| anyhow::anyhow!("encoding Garmin session failed"))?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(&self.path)
            .map_err(|_| anyhow::anyhow!("persisting Garmin session failed"))?;
        File::open(self.path.parent().context("session parent")?)?.sync_all()?;
        Ok(())
    }
    pub(super) fn remove(&self) -> Result<()> {
        fs::remove_file(&self.path).context("removing local Garmin session")
    }
}

/// Read the saved session without the lock: saves replace the file atomically, so a reader
/// sees either the old or the new session, never a partial one.
pub(super) fn read(path: &Path) -> Result<Session> {
    let file = crate::private_fs::open(path, false, false)
        .context("no private Garmin session; run garmin login")?;
    let mut data = Vec::new();
    file.take(32769)
        .read_to_end(&mut data)
        .context("reading Garmin session")?;
    ensure!(data.len() <= 32768, "invalid Garmin session file");
    let session: Session = serde_json::from_slice(&data)
        .map_err(|_| anyhow::anyhow!("invalid Garmin session; run garmin login"))?;
    ensure!(
        session.version == 1 && !session.refresh_token.is_empty(),
        "unsupported Garmin session; run garmin login"
    );
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_lock_is_awaited_and_then_fails_explicitly() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let private = directory.path().join("garmin");
        let path = private.join("session.json");
        let held = SessionStore::lock_within(&path, Duration::ZERO)?;
        let error = SessionStore::lock_within(&path, Duration::from_millis(200))
            .err()
            .context("a held lock must not be taken")?;
        assert!(format!("{error:#}").contains("held the session"));
        let release = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            drop(held);
        });
        SessionStore::lock_within(&path, Duration::from_secs(5))?;
        release
            .join()
            .map_err(|_| anyhow::anyhow!("release thread panicked"))?;
        Ok(())
    }
}
