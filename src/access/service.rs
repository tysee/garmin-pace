//! src/access/service.rs owns principal validation and key authorization policy.
//! AccessService consumes a registry port and checks expiry, revocation and user state.

use super::{AccessError, Grant, KeyInfo, User};
use std::sync::Arc;

/// Persistence operations consumed by the access use cases.
pub trait AccessRegistry: Send + Sync {
    fn add(&self, user: &User) -> Result<(), AccessError>;
    fn user(&self, name: &str) -> Result<Option<User>, AccessError>;
    fn users(&self) -> Result<Vec<User>, AccessError>;
    fn set_enabled(&self, name: &str, enabled: bool) -> Result<(), AccessError>;
    fn set_garmin_writes(&self, name: &str, allowed: bool) -> Result<(), AccessError>;
    fn insert_key(
        &self,
        user: &str,
        id: &str,
        digest: &[u8],
        expires_at: i64,
    ) -> Result<(), AccessError>;
    fn grant(&self, digest: &[u8]) -> Result<Option<Grant>, AccessError>;
    fn keys(&self, user: &str) -> Result<Vec<KeyInfo>, AccessError>;
    fn revoke(&self, id: &str) -> Result<(), AccessError>;
}

/// Live authorization without a cache, so revocations apply on the next request.
#[derive(Clone)]
pub struct AccessService {
    registry: Arc<dyn AccessRegistry>,
}
impl AccessService {
    /// Compose the registry port.
    pub fn new(registry: Arc<dyn AccessRegistry>) -> Self {
        Self { registry }
    }
    /// Register a validated profile supplied by the local administrator.
    pub fn add(&self, user: &User) -> Result<(), AccessError> {
        validate_name(&user.name)?;
        self.registry.add(user)
    }
    /// Resume provisioning only when an enabled principal owns exactly the same profile.
    pub fn add_or_match(&self, user: &User) -> Result<(), AccessError> {
        validate_name(&user.name)?;
        match self.registry.user(&user.name)? {
            None => self.registry.add(user),
            Some(existing) if !existing.enabled => Err(AccessError::Denied),
            Some(existing)
                if existing.database_path == user.database_path
                    && existing.session_path == user.session_path =>
            {
                Ok(())
            }
            Some(_) => Err(AccessError::Conflict),
        }
    }

    /// List bounded administrator-visible principals.
    pub fn users(&self) -> Result<Vec<User>, AccessError> {
        self.registry.users()
    }
    /// Grant or withdraw model-initiated Garmin writes; the user's keys stay unchanged.
    pub fn set_garmin_writes(&self, name: &str, allowed: bool) -> Result<(), AccessError> {
        validate_name(name)?;
        self.registry.set_garmin_writes(name, allowed)
    }
    /// Suspend or restore a principal; suspension immediately blocks all its keys.
    pub fn set_enabled(&self, name: &str, enabled: bool) -> Result<(), AccessError> {
        validate_name(name)?;
        self.registry.set_enabled(name, enabled)
    }
    /// Persist only a key digest after validating the principal and lifetime.
    pub fn issue(
        &self,
        user: &str,
        id: &str,
        digest: &[u8],
        now: i64,
        days: u32,
    ) -> Result<i64, AccessError> {
        validate_name(user)?;
        if !(1..=365).contains(&days) {
            return Err(AccessError::InvalidLifetime);
        }
        let principal = self.registry.user(user)?.ok_or(AccessError::NotFound)?;
        if !principal.enabled {
            return Err(AccessError::Denied);
        }
        let expires = now
            .checked_add(i64::from(days) * 86400)
            .ok_or(AccessError::InvalidLifetime)?;
        self.registry.insert_key(user, id, digest, expires)?;
        Ok(expires)
    }
    /// Accept only a live key belonging to an enabled principal.
    pub fn authorize(&self, digest: &[u8], now: i64) -> Result<User, AccessError> {
        let grant = self.registry.grant(digest)?.ok_or(AccessError::Denied)?;
        if grant.revoked || !grant.user.enabled || grant.expires_at <= now {
            return Err(AccessError::Denied);
        }
        Ok(grant.user)
    }
    /// Inspect safe metadata for one user's keys.
    pub fn keys(&self, user: &str) -> Result<Vec<KeyInfo>, AccessError> {
        validate_name(user)?;
        self.registry.user(user)?.ok_or(AccessError::NotFound)?;
        self.registry.keys(user)
    }
    /// Revoke a key idempotently without requiring its secret value.
    pub fn revoke(&self, id: &str) -> Result<(), AccessError> {
        self.registry.revoke(id)
    }
}

/// Validate an externally supplied local principal name.
pub fn validate_name(name: &str) -> Result<(), AccessError> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(AccessError::InvalidUser);
    }
    Ok(())
}
