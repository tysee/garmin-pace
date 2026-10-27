//! src/garmin/session.rs stores native OAuth tokens privately and atomically.
//! It locks refresh operations and exposes only expiry metadata outside auth adapters.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

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
        let parent = path.parent().context("session path has no parent")?;
        crate::private_fs::directory(parent)?;
        ensure!(!path.is_symlink(), "session file must not be a symlink");
        let lock = crate::private_fs::open(&path.with_extension("lock"), true, true)?;
        lock.try_lock()
            .map_err(|_| anyhow::anyhow!("another Garmin authentication operation is running"))?;
        Ok(Self {
            path: path.to_owned(),
            _lock: lock,
        })
    }
    pub(super) fn load(&self) -> Result<Session> {
        let file = crate::private_fs::open(&self.path, false, false)
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
