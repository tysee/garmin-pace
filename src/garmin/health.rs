//! src/garmin/health.rs reads the supported wellness datasets through existing Garmin clients.
//! It implements HealthSource with bounded, sequential requests and no hidden fallback.

use super::{ConnectApi, PersonalApi, health_decode};
use crate::{
    config::{GarminAuthSettings, GarminSettings, HealthSettings, Settings},
    health::{Dataset, Date, DayImport, HealthError, HealthSource, Limits, Metric},
};
use anyhow::{Context, Result, ensure};
use reqwest::Method;
use serde_json::Value;

/// One-shot health adapter: Personal API reads plus native Connect readiness.
pub struct GarminHealth {
    body_battery_chunks: usize,
    personal: PersonalApi,
    connect: ConnectApi,
    limits: Limits,
}
impl GarminHealth {
    /// Authenticate once per synchronization, reusing the native refresh-token flow.
    pub fn connect(settings: &Settings, key: &str, limits: Limits) -> Result<Self> {
        Self::connect_profile(
            &settings.garmin,
            &settings.garmin_auth,
            &settings.health,
            key,
            limits,
        )
    }
    /// Compose an authenticated profile without inheriting another profile's settings.
    pub fn connect_profile(
        garmin: &GarminSettings,
        auth: &GarminAuthSettings,
        health: &HealthSettings,
        key: &str,
        limits: Limits,
    ) -> Result<Self> {
        Ok(Self {
            body_battery_chunks: health.body_battery_chunks,
            connect: ConnectApi::connect(garmin, auth)?,
            personal: PersonalApi::connect(garmin, key)?,
            limits,
        })
    }
    fn read(&self, date: Date, metric: Metric) -> Result<Value> {
        let d = date.to_string();
        let (path, query) = match metric {
            Metric::Daily => (
                "/usersummary-service/usersummary/daily".into(),
                vec![("calendarDate", d)],
            ),
            Metric::Sleep => ("/sleep-service/sleep/dailySleep".into(), vec![("date", d)]),
            Metric::Hrv => (format!("/hrv-service/hrv/{d}"), vec![]),
            Metric::HeartRate => (
                "/wellness-service/wellness/dailyHeartRate".into(),
                vec![("date", d)],
            ),
            Metric::BodyBattery => (
                "/wellness-service/wellness/bodyBattery/reports/daily".into(),
                vec![
                    ("startDate", d.clone()),
                    ("endDate", d),
                    ("chunks", self.body_battery_chunks.to_string()),
                ],
            ),
            Metric::Spo2 => (
                format!("/wellness-service/wellness/daily/spo2acclimation/{d}"),
                vec![],
            ),
            Metric::Weight => (
                format!("/weight-service/weight/range/{d}/{d}"),
                vec![("includeAll", "true".into())],
            ),
            Metric::TrainingReadiness => (
                format!("/metrics-service/metrics/trainingreadiness/{d}"),
                vec![],
            ),
        };
        let http = if metric == Metric::TrainingReadiness {
            &self.connect.http
        } else {
            &self.personal.http
        };
        let data = http.request(Method::GET, &path, &query, None)?;
        // HRV documents 204 No Content; an empty body elsewhere is a contract error.
        if data.is_empty() && metric == Metric::Hrv {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&data).map_err(|_| anyhow::anyhow!("invalid health JSON"))
    }
    fn fetch(&self, date: Date) -> Result<DayImport> {
        let mut datasets = Vec::with_capacity(Metric::ALL.len());
        let mut count = 0;
        for metric in Metric::ALL {
            let payload = self
                .read(date, metric)
                .with_context(|| format!("reading {} for {date}", metric.as_str()))?;
            let (section, samples) = health_decode::decode(date, metric, payload, self.limits)
                .with_context(|| format!("validating {} for {date}", metric.as_str()))?;
            count += samples.len();
            ensure!(
                count <= self.limits.max_samples_per_day,
                "daily sample limit exceeded"
            );
            datasets.push(Dataset { section, samples });
        }
        let datasets = datasets
            .try_into()
            .map_err(|_| anyhow::anyhow!("incomplete health datasets"))?;
        Ok(DayImport { datasets })
    }
}
impl HealthSource for GarminHealth {
    fn day(&self, date: Date) -> Result<DayImport, HealthError> {
        self.fetch(date)
            .map_err(|e| HealthError::Upstream(format!("{e:#}")))
    }
}
