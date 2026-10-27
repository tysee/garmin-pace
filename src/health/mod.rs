//! src/health/mod.rs exposes health domain types, use cases and composition adapters.
//! CLI and MCP use the same bounded service and explicit snapshot contract.

pub mod document;
mod domain;
mod service;
mod sqlite;
pub use domain::*;
pub use service::*;
pub use sqlite::SqliteHealth;
