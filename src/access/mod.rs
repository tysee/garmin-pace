//! src/access/mod.rs exposes user and key administration and the live authorization boundary.
//! Cryptography and SQLite stay behind the feature API; original keys never enter domain records.

mod domain;
mod keys;
mod service;
mod sqlite;
pub use domain::{AccessError, Grant, KeyInfo, User};
pub use keys::{NewKey, key_digest};
pub use service::{AccessRegistry, AccessService, validate_name};
pub use sqlite::SqliteAccess;
