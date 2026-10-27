//! src/history/mod.rs is the history feature's public boundary, exposing domain
//! records, bounded read and period-import use cases, and its SQLite adapter.

mod domain;
mod service;
mod sqlite;
mod sqlite_write;

pub use domain::{
    Activity, ActivityPeriod, ActivityQuery, ActivitySearchPage, HistoryError, HistorySummary,
    PeriodProgress, PeriodSync, SyncSummary,
};
pub use service::{
    ActivityPageSource, ActivityPeriodSource, ActivitySource, HistoryRepository, HistoryService,
    HistoryWrite,
};
pub use sqlite::SqliteHistory;
