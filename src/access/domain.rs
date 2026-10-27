//! src/access/domain.rs defines local MCP principals and revocable key metadata.
//! These types contain no original tokens, transport objects or Garmin credentials.

use std::path::PathBuf;

/// An administrator-registered profile; paths are canonical and unique per user.
#[derive(Clone, Debug)]
pub struct User {
    pub name: String,
    pub database_path: PathBuf,
    pub session_path: PathBuf,
    pub enabled: bool,
    /// Whether this user's MCP client may save, reconcile and deliver to Garmin on request.
    pub garmin_writes: bool,
}

/// Safe key metadata shown to administrators without the credential or its hash.
#[derive(Debug)]
pub struct KeyInfo {
    pub id: String,
    pub expires_at: i64,
    pub revoked: bool,
}

/// Registry facts used by the authorization policy on every request.
pub struct Grant {
    pub user: User,
    pub expires_at: i64,
    pub revoked: bool,
}

/// Explicit access outcomes; storage errors never become anonymous access.
#[derive(Debug, thiserror::Error)]
pub enum AccessError {
    #[error("invalid user name; use 1..=64 ASCII letters, digits, underscores or hyphens")]
    InvalidUser,
    #[error("access denied")]
    Denied,
    #[error("user, profile or key already registered, or registry limit reached")]
    Conflict,
    #[error("unknown user or key")]
    NotFound,
    #[error("key lifetime must be 1..=365 days")]
    InvalidLifetime,
    #[error("access registry unavailable")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}
