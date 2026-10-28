//! src/sync/mod.rs exposes the full-sync feature: one ordered import of activities, strength
//! sets, health and Coach over a period, journaled per window so it can resume and report
//! partial results honestly. Interfaces supply the step adapters; the journal is SQLite.

mod domain;
mod service;
mod sqlite;
pub use domain::*;
pub use service::*;
pub use sqlite::SqliteSyncJournal;
