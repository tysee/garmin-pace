//! src/garmin/connect.rs adapts a native Garmin Connect session to workout ports.
//! It validates saved workouts and requests device delivery without claiming watch receipt.

use super::http::{GarminHttp, secret_header};
use crate::{
    config::GarminSettings,
    workouts::{Device, LibraryWorkout, Workout, WorkoutError, WorkoutGateway},
};
use anyhow::{Context, Result, ensure};
use reqwest::{
    Method,
    header::{ACCEPT, HeaderMap, HeaderValue},
};
use serde::Deserialize;
use serde_json::{Value, json};

/// Garmin Connect native-session adapter with renewal before network operations.
pub struct ConnectApi {
    pub(super) http: GarminHttp,
}
impl ConnectApi {
    /// Load the native session and automatically renew an expiring access token.
    pub fn connect(
        settings: &GarminSettings,
        auth: &crate::config::GarminAuthSettings,
    ) -> Result<Self> {
        let token = super::auth::access_token(auth, settings)?;
        Self::with_token(settings, &auth.api_user_agent, &token)
    }
    pub(super) fn with_token(
        settings: &GarminSettings,
        user_agent: &str,
        token: &str,
    ) -> Result<Self> {
        secret_header(token)?;
        let mut headers = HeaderMap::new();
        let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))?;
        authorization.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, authorization);
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(
            reqwest::header::USER_AGENT,
            HeaderValue::from_str(user_agent).context("invalid Garmin API user agent")?,
        );
        Ok(Self {
            http: GarminHttp::new(&settings.connect_base_url, headers, settings)?,
        })
    }

    fn read_devices(&self) -> Result<Vec<Device>> {
        let rows: Vec<RemoteDevice> = self.http.json(
            Method::GET,
            "/device-service/deviceregistration/devices",
            &[],
            None,
        )?;
        ensure!(rows.len() <= 100, "unexpected device count");
        rows.into_iter()
            .map(|row| {
                ensure!(
                    row.unit_id > 0 && !row.product_display_name.is_empty(),
                    "invalid Garmin device identity"
                );
                Ok(Device {
                    id: row.unit_id,
                    model: row.product_display_name,
                })
            })
            .collect()
    }
    fn send(&self, id: u64, name: &str, device: u64) -> Result<()> {
        let payload = json!([{"deviceId":device,"messageUrl":format!("workout-service/workout/FIT/{id}"),
            "messageType":"workouts","groupName":null,"messageName":name,"priority":1,"fileType":"FIT","metaDataId":id}]);
        self.http.request(
            Method::POST,
            "/device-service/devicemessage/messages",
            &[],
            Some(&payload),
        )?;
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteDevice {
    unit_id: u64,
    product_display_name: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Created {
    workout_id: u64,
}
/// Keep a definite Garmin refusal distinguishable from an uncertain network outcome.
pub(super) fn upstream(error: anyhow::Error) -> WorkoutError {
    match super::http::Rejected::definite(&error) {
        Some(status) => WorkoutError::Rejected(status),
        None => WorkoutError::Upstream(error.to_string()),
    }
}
impl WorkoutGateway for ConnectApi {
    fn devices(&self) -> Result<Vec<Device>, WorkoutError> {
        self.read_devices().map_err(upstream)
    }
    fn list(&self, start: usize, limit: usize) -> Result<Vec<u64>, WorkoutError> {
        let read = || -> Result<Vec<u64>> {
            let rows: Vec<Created> = self.http.json(
                Method::GET,
                "/workout-service/workouts",
                &[("start", start.to_string()), ("limit", limit.to_string())],
                None,
            )?;
            ensure!(
                rows.len() <= limit,
                "Garmin workout page exceeds requested limit"
            );
            let mut ids = std::collections::HashSet::new();
            rows.into_iter()
                .map(|row| {
                    ensure!(
                        row.workout_id > 0
                            && row.workout_id <= i64::MAX as u64
                            && ids.insert(row.workout_id),
                        "invalid or duplicate Garmin workout ID"
                    );
                    Ok(row.workout_id)
                })
                .collect()
        };
        read().map_err(upstream)
    }
    fn read(&self, id: u64) -> Result<LibraryWorkout, WorkoutError> {
        let read = || -> Result<LibraryWorkout> {
            let value: Value = self.http.json(
                Method::GET,
                &format!("/workout-service/workout/{id}"),
                &[],
                None,
            )?;
            super::workout_decode::snapshot(&value, id)
        };
        read().map_err(upstream)
    }
    fn create(&self, workout: &Workout) -> Result<u64, WorkoutError> {
        let create = || -> Result<u64> {
            let body = super::workout_encode::payload(workout)?;
            let response: Created =
                self.http
                    .json(Method::POST, "/workout-service/workout", &[], Some(&body))?;
            ensure!(
                response.workout_id > 0 && response.workout_id <= i64::MAX as u64,
                "invalid created workout ID; outcome is uncertain"
            );
            Ok(response.workout_id)
        };
        create().map_err(upstream)
    }
    fn update(&self, id: u64, revision: &str, workout: &Workout) -> Result<(), WorkoutError> {
        let update = || -> Result<(), WorkoutError> {
            let path = format!("/workout-service/workout/{id}");
            let mut previous: Value = self
                .http
                .json(Method::GET, &path, &[], None)
                .map_err(upstream)?;
            if super::workout_decode::snapshot(&previous, id)
                .map_err(upstream)?
                .revision
                != revision
            {
                return Err(WorkoutError::Conflict);
            }
            let Value::Object(fields) =
                super::workout_encode::payload(workout).map_err(upstream)?
            else {
                return Err(WorkoutError::InvalidInput("invalid workout payload"));
            };
            let target = previous
                .as_object_mut()
                .ok_or(WorkoutError::InvalidInput("invalid remote workout"))?;
            target.extend(fields);
            // Estimates are server-derived and cannot survive changes to the actual steps.
            for key in [
                "estimatedDurationInSecs",
                "estimatedDistanceInMeters",
                "avgTrainingSpeed",
                "estimateType",
            ] {
                target.remove(key);
            }
            if workout.pool_length_meters.is_none() {
                target.remove("poolLength");
                target.remove("poolLengthUnit");
            }
            self.http
                .request(Method::PUT, &path, &[], Some(&previous))
                .map_err(upstream)?;
            Ok(())
        };
        update()
    }
    fn queue(&self, id: u64, name: &str, device: u64) -> Result<(), WorkoutError> {
        self.send(id, name, device).map_err(upstream)
    }
}
