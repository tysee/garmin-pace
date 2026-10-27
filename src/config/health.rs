//! src/config/health.rs defines explicit optional resource tuning for wellness history.
//! Defaults bound existing configurations; validation rejects unbounded imports and reads.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

/// Health tuning is optional; endpoints and credentials remain required elsewhere.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HealthSettings {
    pub body_battery_chunks: usize,
    pub max_days: usize,
    pub max_samples_per_day: usize,
    pub max_sample_page: usize,
    pub max_records_per_metric: usize,
}
impl Default for HealthSettings {
    fn default() -> Self {
        Self {
            body_battery_chunks: 720,
            max_days: 31,
            max_samples_per_day: 20000,
            max_sample_page: 500,
            max_records_per_metric: 64,
        }
    }
}
impl HealthSettings {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            (1..=1440).contains(&self.body_battery_chunks),
            "health.body_battery_chunks must be 1..=1440"
        );
        ensure!(
            (1..=31).contains(&self.max_days),
            "health.max_days must be 1..=31"
        );
        ensure!(
            (1..=50000).contains(&self.max_samples_per_day),
            "health.max_samples_per_day must be 1..=50000"
        );
        ensure!(
            (1..=500).contains(&self.max_sample_page)
                && self.max_sample_page <= self.max_samples_per_day,
            "health.max_sample_page must be 1..=500 and fit daily bound"
        );
        ensure!(
            (1..=64).contains(&self.max_records_per_metric),
            "health.max_records_per_metric must be 1..=64"
        );
        Ok(())
    }
    /// Pass pure resource bounds inward without exposing configuration parsing.
    pub fn limits(&self) -> crate::health::Limits {
        crate::health::Limits {
            max_days: self.max_days,
            max_samples_per_day: self.max_samples_per_day,
            max_sample_page: self.max_sample_page,
            max_records_per_metric: self.max_records_per_metric,
        }
    }
}
