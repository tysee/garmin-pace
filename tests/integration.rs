//! tests/integration.rs runs the shared real-database and process test suite.
//! Each case owns its configuration, data directory, and child processes.

#[path = "cases/cli.rs"]
mod cli;
mod common;
#[path = "cases/health.rs"]
mod health;
#[path = "cases/history.rs"]
mod history;
#[path = "cases/http_docs.rs"]
mod http_docs;
#[path = "cases/mcp.rs"]
mod mcp;
#[path = "cases/mcp_agent.rs"]
mod mcp_agent;
#[path = "cases/mcp_concurrency.rs"]
mod mcp_concurrency;
#[path = "cases/mcp_garmin.rs"]
mod mcp_garmin;

#[path = "cases/garmin.rs"]
mod garmin;
#[path = "cases/workouts.rs"]
mod workouts;

#[cfg(unix)]
#[path = "cases/terminal.rs"]
mod terminal;

#[path = "cases/benchmarks.rs"]
mod benchmarks;
#[path = "cases/https.rs"]
mod https;

#[path = "cases/doctor.rs"]
mod doctor;

#[path = "cases/coach.rs"]
mod coach;

#[path = "cases/full_sync.rs"]
mod full_sync;
