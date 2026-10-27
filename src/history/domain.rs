//! src/history/domain.rs defines local activity summaries, import periods and typed
//! history failures independently of transport, configuration, and storage frameworks.

use chrono::{DateTime, Days, NaiveDate};

/// Summary of a recorded activity. Timestamp text uses normalized UTC RFC3339.
#[derive(Clone, Debug, PartialEq)]
pub struct Activity {
    pub garmin_id: String,
    pub name: String,
    pub sport: String,
    pub started_at: String,
    pub duration_seconds: f64,
    pub distance_meters: Option<f64>,
}

/// The number of activity summaries currently stored locally.
#[derive(Debug, PartialEq)]
pub struct HistorySummary {
    pub activity_count: u64,
    pub last_sync: Option<SyncSummary>,
}

/// Failures that callers can distinguish without matching error strings.
#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error("limit must be between 1 and {max}")]
    InvalidLimit { max: usize },
    #[error("start date must not be after end date")]
    InvalidPeriod,
    #[error("{0}")]
    InvalidQuery(&'static str),
    #[error("Garmin request failed: {0}")]
    Upstream(String),
    #[error("local history storage is unavailable")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Optional inclusive UTC dates, exact sport and literal name substring for local searches.
#[derive(Clone, Debug)]
pub struct ActivityQuery {
    pub start: Option<NaiveDate>,
    pub end: Option<NaiveDate>,
    pub sport: Option<String>,
    pub name: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

/// One bounded page and its matching count, read from the same local snapshot.
#[derive(Debug)]
pub struct ActivitySearchPage {
    pub activities: Vec<Activity>,
    pub total: u64,
}

/// Scope of the last fully successful import: one recent page, or a whole period whose
/// pages were requested `requested_limit` at a time.
#[derive(Clone, Debug, PartialEq)]
pub struct SyncSummary {
    pub completed_at: String,
    pub requested_limit: u32,
    pub received_count: u32,
}

/// Progress of a period import after each committed page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PeriodProgress {
    pub pages: u32,
    pub saved: u64,
}

/// Outcome of a period import: every page up to `progress` is saved; `completed` is set
/// only when the whole period was imported and recorded as the last successful sync.
#[derive(Debug, PartialEq)]
pub struct PeriodSync {
    pub progress: PeriodProgress,
    pub completed: Option<SyncSummary>,
}

/// An inclusive range of activity local start dates, the way Garmin filters them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActivityPeriod {
    start: NaiveDate,
    end: NaiveDate,
}

impl ActivityPeriod {
    pub fn new(start: NaiveDate, end: NaiveDate) -> Result<Self, HistoryError> {
        if end < start {
            return Err(HistoryError::InvalidPeriod);
        }
        Ok(Self { start, end })
    }
    pub fn start(&self) -> NaiveDate {
        self.start
    }
    pub fn end(&self) -> NaiveDate {
        self.end
    }
    /// Whether a stored UTC start can belong to this local-date period; local time is
    /// within one calendar day of UTC everywhere.
    pub fn admits(&self, started_at: &str) -> bool {
        let Ok(started) = DateTime::parse_from_rfc3339(started_at) else {
            return false;
        };
        let date = started.date_naive();
        let first = self
            .start
            .checked_sub_days(Days::new(1))
            .unwrap_or(self.start);
        let last = self.end.checked_add_days(Days::new(1)).unwrap_or(self.end);
        (first..=last).contains(&date)
    }
}
