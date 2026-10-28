//! src/lib.rs exposes application configuration, the feature modules (history, health,
//! workouts, full sync, access) and interface boundaries so the executable and
//! real-database tests share the same implementation.

pub mod config;
pub mod garmin;
pub mod health;
pub mod history;
pub mod interface;

pub mod workouts;

pub mod secrets;
pub mod sync;

pub mod access;
mod private_fs;

pub mod installation;
