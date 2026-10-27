//! src/garmin/personal.rs reads recent activities through Garmin's official Personal API.
//! It validates remote summaries and implements the history feature's recent-page and
//! date-period activity sources.

use super::http::{GarminHttp, secret_header};
use crate::{
    config::GarminSettings,
    history::{
        Activity, ActivityPageSource, ActivityPeriod, ActivityPeriodSource, ActivitySource,
        HistoryError,
    },
};
use anyhow::{Result, ensure};
use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use reqwest::{
    Method,
    header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde::Deserialize;

/// Authorized short-lived Personal API client; keys/tokens are never persisted.
pub struct PersonalApi {
    pub(super) http: GarminHttp,
}

impl PersonalApi {
    /// Exchange the environment API key once; redirects and automatic retries are disabled.
    pub fn connect(settings: &GarminSettings, key: &str) -> Result<Self> {
        secret_header(key)?;
        let mut headers = HeaderMap::new();
        let mut auth = HeaderValue::from_str(&format!("ApiKey {key}"))?;
        auth.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth);
        let http = GarminHttp::new(&settings.personal_base_url, headers, settings)?;
        let token = http.request(
            Method::POST,
            "/personalapitoken-service/personal-api/personal-api-key/token",
            &[],
            None,
        )?;
        let token = std::str::from_utf8(&token)
            .map_err(|_| anyhow::anyhow!("Garmin returned an invalid token"))?
            .trim();
        secret_header(token)?;
        let mut headers = HeaderMap::new();
        let mut auth = HeaderValue::from_str(&format!("Bearer {token}"))?;
        auth.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth);
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        Ok(Self {
            http: GarminHttp::new(&settings.personal_base_url, headers, settings)?,
        })
    }

    fn read_recent(&self, limit: usize) -> Result<Vec<Activity>> {
        self.read_page(
            &[("start", "0".into()), ("limit", limit.to_string())],
            limit,
        )
    }

    /// Read one search page; startDate and endDate filter by inclusive local start date.
    fn read_page(&self, query: &[(&str, String)], limit: usize) -> Result<Vec<Activity>> {
        let rows: Vec<RemoteActivity> = self.http.json(
            Method::GET,
            "/activitylist-service/activities/search/activities",
            query,
            None,
        )?;
        ensure!(
            rows.len() <= limit,
            "Garmin activity page exceeded requested limit"
        );
        rows.into_iter().map(TryInto::try_into).collect()
    }
}

impl ActivityPeriodSource for PersonalApi {
    fn period_page(
        &self,
        period: ActivityPeriod,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Activity>, HistoryError> {
        self.read_page(
            &[
                ("start", offset.to_string()),
                ("limit", limit.to_string()),
                ("startDate", period.start().to_string()),
                ("endDate", period.end().to_string()),
            ],
            limit,
        )
        .map_err(|error| HistoryError::Upstream(error.to_string()))
    }
}

impl ActivitySource for PersonalApi {
    fn recent(&self, limit: usize) -> Result<Vec<Activity>, HistoryError> {
        self.read_recent(limit)
            .map_err(|error| HistoryError::Upstream(error.to_string()))
    }
}

impl ActivityPageSource for PersonalApi {
    fn page(
        &self,
        period: Option<ActivityPeriod>,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Activity>, HistoryError> {
        let mut query = vec![("start", offset.to_string()), ("limit", limit.to_string())];
        if let Some(period) = period {
            query.extend([
                ("startDate", period.start().to_string()),
                ("endDate", period.end().to_string()),
            ]);
        }
        self.read_page(&query, limit)
            .map_err(|error| HistoryError::Upstream(error.to_string()))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteActivity {
    activity_id: u64,
    activity_name: String,
    activity_type: Sport,
    #[serde(rename = "startTimeGMT")]
    started_at: String,
    duration: f64,
    distance: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Sport {
    type_key: String,
}

impl TryFrom<RemoteActivity> for Activity {
    type Error = anyhow::Error;
    fn try_from(value: RemoteActivity) -> Result<Self> {
        ensure!(value.activity_id > 0, "invalid Garmin activity ID");
        ensure!(
            value.activity_name.len() <= 1024
                && !value.activity_type.type_key.is_empty()
                && value.activity_type.type_key.len() <= 100,
            "invalid Garmin activity text fields"
        );
        ensure!(
            value.duration.is_finite() && value.duration >= 0.0,
            "invalid Garmin activity duration"
        );
        ensure!(
            value.distance.is_none_or(|d| d.is_finite() && d >= 0.0),
            "invalid Garmin activity distance"
        );
        // Garmin's startTimeGMT is explicitly UTC even when it has no offset suffix.
        let timestamp = DateTime::parse_from_rfc3339(&value.started_at)
            .map(|d| d.with_timezone(&Utc))
            .or_else(|_| {
                NaiveDateTime::parse_from_str(&value.started_at, "%Y-%m-%d %H:%M:%S%.f")
                    .map(|d| d.and_utc())
            })
            .map_err(|_| anyhow::anyhow!("invalid Garmin activity UTC timestamp"))?;
        Ok(Activity {
            garmin_id: value.activity_id.to_string(),
            name: value.activity_name,
            sport: value.activity_type.type_key,
            started_at: timestamp.to_rfc3339_opts(SecondsFormat::Millis, true),
            duration_seconds: value.duration,
            distance_meters: value.distance,
        })
    }
}
