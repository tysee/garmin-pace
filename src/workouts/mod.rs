//! src/workouts/mod.rs exposes reusable workout definitions, the library and model drafts.
//! CLI/MCP consume this boundary; Garmin and SQLite implement its inward-facing ports.

pub mod document;
mod domain;
mod overview;
mod service;
pub use overview::{WorkoutOverview, overview};
mod sqlite;
mod sqlite_queries;
pub use domain::{
    BenchmarkLoad, Delivery, Device, Draft, DraftAttention, DraftState, Duration, Exercise,
    LibraryWorkout, Segment, Step, Target, Workout, WorkoutError, WorkoutSummary, validate_request,
};
pub use service::{WorkoutGateway, WorkoutRepository, WorkoutService};
pub use sqlite::SqliteWorkouts;

pub mod benchmarks;

pub mod coach;
