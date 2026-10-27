//! src/interface/mod.rs groups the application's CLI and MCP transport adapters.
//! Only these adapters serialize domain records and decide process responses.

pub mod cli;
mod garmin;
mod health;
mod health_responses;
mod health_view;
mod http;
mod http_docs;
mod mcp;
mod mcp_workouts;
mod output;
mod responses;
mod terminal;

mod workout_overview;
mod workout_view;
mod workouts;

mod listener;
mod users;

mod benchmark_view;
mod benchmarks;
mod mcp_benchmarks;
mod mcp_delivery;
mod mcp_garmin;
mod mcp_history;
mod mcp_sync;

mod coach;
mod mcp_coach;
