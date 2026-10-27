//! src/history/service.rs owns the history repository port and enforces bounded
//! history reads. HistoryService is shared by CLI and MCP adapters.

use std::sync::Arc;

use super::domain::{
    Activity, ActivityPeriod, ActivityQuery, ActivitySearchPage, HistoryError, HistorySummary,
    PeriodProgress, PeriodSync, SyncSummary,
};
use std::ops::ControlFlow;

/// Storage operations required by the history use cases.
pub trait HistoryRepository: Send + Sync {
    /// Count persisted activities without fetching their contents.
    fn summary(&self) -> Result<HistorySummary, HistoryError>;
    /// Fetch at most the validated limit, newest first, skipping `offset` newer rows.
    fn list(&self, limit: usize, offset: usize) -> Result<Vec<Activity>, HistoryError>;
    /// Fetch one stored activity by its Garmin ID; `None` when it was never imported.
    fn find(&self, garmin_id: &str) -> Result<Option<Activity>, HistoryError>;
    /// Fetch one bounded UTC date/sport page using the chronological indexes.
    fn search(&self, query: &ActivityQuery) -> Result<ActivitySearchPage, HistoryError>;
    /// Start a transaction whose commit is controlled by the sync use case.
    fn begin_sync(&self) -> Result<Box<dyn HistoryWrite>, HistoryError>;
}

/// Paging never walks past this many rows; deeper reads belong to a date-scoped query.
const MAX_OFFSET: usize = 100_000;

/// Application boundary enforcing history query invariants.
#[derive(Clone)]
pub struct HistoryService {
    repository: Arc<dyn HistoryRepository>,
    max_page_size: usize,
}

impl HistoryService {
    /// Bind the repository and the validated pagination policy.
    pub fn new(repository: Arc<dyn HistoryRepository>, max_page_size: usize) -> Self {
        Self {
            repository,
            max_page_size,
        }
    }

    /// Report the persisted history size.
    pub fn summary(&self) -> Result<HistorySummary, HistoryError> {
        self.repository.summary()
    }

    /// Validate transport page bounds before constructing a network adapter.
    pub fn validate_page(&self, limit: usize, offset: usize) -> Result<(), HistoryError> {
        if !(1..=self.max_page_size).contains(&limit) || offset > MAX_OFFSET {
            return Err(HistoryError::InvalidLimit {
                max: self.max_page_size,
            });
        }
        Ok(())
    }

    /// Read a bounded page; invalid limits and unbounded offsets never reach storage.
    pub fn list(&self, limit: usize, offset: usize) -> Result<Vec<Activity>, HistoryError> {
        self.validate_page(limit, offset)?;
        self.repository.list(limit, offset)
    }

    /// Read one stored activity summary; absence means it was never imported locally.
    pub fn find(&self, garmin_id: &str) -> Result<Option<Activity>, HistoryError> {
        if garmin_id.is_empty() || garmin_id.len() > 100 || garmin_id.chars().any(char::is_control)
        {
            return Err(HistoryError::InvalidQuery("invalid activity ID"));
        }
        self.repository.find(garmin_id)
    }

    /// Search stored activities by inclusive UTC dates, with bounded pagination.
    pub fn search(&self, query: &ActivityQuery) -> Result<ActivitySearchPage, HistoryError> {
        if query
            .start
            .zip(query.end)
            .is_some_and(|(start, end)| start > end)
            || query.end.is_some_and(|end| end.succ_opt().is_none())
        {
            return Err(HistoryError::InvalidPeriod);
        }
        self.validate_page(query.limit, query.offset)?;
        if query
            .sport
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 100 || s.chars().any(char::is_control))
        {
            return Err(HistoryError::InvalidQuery("invalid sport filter"));
        }
        if query
            .name
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 200 || s.chars().any(char::is_control))
        {
            return Err(HistoryError::InvalidQuery("invalid name filter"));
        }
        self.repository.search(query)
    }

    /// Import one remote page atomically; a full page requires an explicit next call.
    pub fn sync_page(
        &self,
        source: &impl ActivityPageSource,
        period: Option<ActivityPeriod>,
        limit: usize,
        offset: usize,
        now: impl FnOnce() -> String,
    ) -> Result<SyncSummary, HistoryError> {
        self.validate_page(limit, offset)?;
        let activities = source.page(period, offset, limit)?;
        if activities.len() > limit
            || period.is_some_and(|p| activities.iter().any(|a| !p.admits(&a.started_at)))
        {
            return Err(HistoryError::Upstream(
                "activity page exceeds requested bounds".into(),
            ));
        }
        let summary = SyncSummary {
            completed_at: now(),
            requested_limit: limit as u32,
            received_count: activities.len() as u32,
        };
        let mut tx = self.repository.begin_sync()?;
        tx.upsert(&activities)?;
        tx.record_sync(&summary)?;
        tx.commit()?;
        Ok(summary)
    }
}

/// External activity pages, optionally restricted to Garmin's local calendar dates.
pub trait ActivityPageSource {
    /// Read one bounded remote page without implicitly walking subsequent pages.
    fn page(
        &self,
        period: Option<ActivityPeriod>,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Activity>, HistoryError>;
}

/// External activity source. The adapter validates remote records before returning.
pub trait ActivitySource {
    /// Read at most the requested recent activities.
    fn recent(&self, limit: usize) -> Result<Vec<Activity>, HistoryError>;
}

/// External source of activities filtered by local start date, read page by page.
pub trait ActivityPeriodSource {
    /// Read at most `limit` activities of the period, newest first, skipping `offset`.
    fn period_page(
        &self,
        period: ActivityPeriod,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Activity>, HistoryError>;
}

/// Write transaction owned by a single synchronization use case.
pub trait HistoryWrite {
    /// Upsert the page without committing partial results.
    fn upsert(&mut self, activities: &[Activity]) -> Result<(), HistoryError>;
    /// Persist the import scope in the same transaction.
    fn record_sync(&mut self, summary: &SyncSummary) -> Result<(), HistoryError>;
    /// Atomically publish the batch; dropping without commit rolls it back.
    fn commit(self: Box<Self>) -> Result<(), HistoryError>;
}

impl HistoryService {
    /// Import a bounded recent page atomically; failed fetches preserve stored history.
    pub fn sync(
        &self,
        source: &impl ActivitySource,
        limit: usize,
        completed_at: impl FnOnce() -> String,
    ) -> Result<SyncSummary, HistoryError> {
        if !(1..=self.max_page_size).contains(&limit) {
            return Err(HistoryError::InvalidLimit {
                max: self.max_page_size,
            });
        }
        let activities = source.recent(limit)?;
        if activities.len() > limit {
            return Err(HistoryError::Upstream(
                "activity page exceeded the requested limit".into(),
            ));
        }
        let summary = SyncSummary {
            completed_at: completed_at(),
            requested_limit: u32::try_from(limit).map_err(|_| HistoryError::InvalidLimit {
                max: self.max_page_size,
            })?,
            received_count: u32::try_from(activities.len()).map_err(|_| {
                HistoryError::InvalidLimit {
                    max: self.max_page_size,
                }
            })?,
        };
        let mut transaction = self.repository.begin_sync()?;
        transaction.upsert(&activities)?;
        transaction.record_sync(&summary)?;
        transaction.commit()?;
        Ok(summary)
    }

    /// Import every activity of a period one full page at a time. Each page commits on its
    /// own; the sync record is written only after the last page. `on_page` sees progress
    /// after every committed page and may stop the import before the next request.
    pub fn sync_period(
        &self,
        source: &impl ActivityPeriodSource,
        period: ActivityPeriod,
        mut on_page: impl FnMut(PeriodProgress) -> ControlFlow<()>,
        completed_at: impl FnOnce() -> String,
    ) -> Result<PeriodSync, HistoryError> {
        let size = self.max_page_size;
        let invalid = || HistoryError::InvalidLimit { max: size };
        let mut progress = PeriodProgress { pages: 0, saved: 0 };
        let mut offset = 0usize;
        loop {
            let page = source.period_page(period, offset, size)?;
            if page.len() > size {
                return Err(HistoryError::Upstream(
                    "activity page exceeded the requested limit".into(),
                ));
            }
            // A filter Garmin ignored would otherwise page through the whole archive.
            if page
                .iter()
                .any(|activity| !period.admits(&activity.started_at))
            {
                return Err(HistoryError::Upstream(
                    "Garmin returned an activity outside the requested period".into(),
                ));
            }
            let full = page.len() == size;
            if !page.is_empty() {
                let mut transaction = self.repository.begin_sync()?;
                transaction.upsert(&page)?;
                transaction.commit()?;
                progress.pages += 1;
                progress.saved += page.len() as u64;
                if on_page(progress).is_break() && full {
                    return Ok(PeriodSync {
                        progress,
                        completed: None,
                    });
                }
            }
            if !full {
                break;
            }
            offset = offset.checked_add(size).ok_or_else(invalid)?;
        }
        let summary = SyncSummary {
            completed_at: completed_at(),
            requested_limit: u32::try_from(size).map_err(|_| invalid())?,
            received_count: u32::try_from(progress.saved).map_err(|_| invalid())?,
        };
        let mut transaction = self.repository.begin_sync()?;
        transaction.record_sync(&summary)?;
        transaction.commit()?;
        Ok(PeriodSync {
            progress,
            completed: Some(summary),
        })
    }
}
