//! src/access/keys.rs generates opaque 256-bit MCP keys using the OS CSPRNG.
//! It exposes private-file delivery, a one-time interactive reveal and SHA-256 digests;
//! Debug and serialization are absent.

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
};
use std::{io::Write, path::Path};

/// A newly generated key, retained only long enough to write its private delivery file.
pub struct NewKey {
    token: String,
    pub id: String,
    pub digest: Vec<u8>,
}
impl NewKey {
    /// Generate 32 unpredictable bytes; no shared signing secret is necessary.
    pub fn generate() -> Result<Self> {
        let mut bytes = [0u8; 32];
        SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| anyhow::anyhow!("secure random generator unavailable"))?;
        let token = format!("pace_{}", URL_SAFE_NO_PAD.encode(bytes));
        let hash = digest::digest(&digest::SHA256, token.as_bytes())
            .as_ref()
            .to_vec();
        let id = hash[..16].iter().map(|b| format!("{b:02x}")).collect();
        Ok(Self {
            token,
            id,
            digest: hash,
        })
    }
    /// Deliver a key once to an owner-only new file. Never print it to stdout.
    pub fn write(&self, path: &Path) -> Result<()> {
        let mut file = crate::private_fs::create_new(path)?;
        let result = writeln!(file, "{}", self.token).and_then(|()| file.sync_all());
        if let Err(error) = result {
            std::fs::remove_file(path).context("removing incomplete key output")?;
            return Err(error).context("writing private key output");
        }
        Ok(())
    }
}

impl NewKey {
    /// Hand the key to an interactive screen exactly once; it is never logged or stored.
    pub fn reveal(self) -> String {
        self.token
    }
}

/// Validate the external key encoding before computing its registry lookup digest.
pub fn key_digest(token: &str) -> Result<Vec<u8>> {
    ensure!(
        token.len() == 48 && token.starts_with("pace_"),
        "invalid access key"
    );
    let bytes = URL_SAFE_NO_PAD
        .decode(&token[5..])
        .map_err(|_| anyhow::anyhow!("invalid access key"))?;
    ensure!(bytes.len() == 32, "invalid access key");
    Ok(digest::digest(&digest::SHA256, token.as_bytes())
        .as_ref()
        .to_vec())
}
