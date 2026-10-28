//! src/garmin/mod.rs exposes Garmin adapters for CLI composition.
//! HTTP details and remote DTOs stay inside this feature boundary.

mod doctor;
mod health;
mod health_decode;
mod health_fields;
mod health_samples;
#[cfg(test)]
mod health_tests;
mod http;
pub use http::check_client;
mod pacing;
mod personal;
pub use doctor::{CertificateVariable, NetworkReport, Probe, ProbeOutcome, diagnose};
pub use health::GarminHealth;
pub use personal::PersonalApi;

mod connect;
pub use connect::ConnectApi;
mod workout_decode;
mod workout_encode;

mod auth;
mod session;
pub use auth::{login, logout, saved_session_expiry, session_expiry};
pub use pacing::{RateWait, WaitObserver, observe};

#[cfg(test)]
mod workout_tests;

mod benchmark_decode;
mod benchmarks;
pub use benchmarks::GarminBenchmarks;

mod coach;
