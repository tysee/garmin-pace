//! src/workouts/benchmarks/mod.rs exposes strength evidence and benchmark proposal use cases.
//! Models and terminal edges share this public boundary while adapters own persistence and Garmin.

pub mod document;
mod domain;
mod service;
mod sqlite;
pub use domain::{
    ActivitySets, Benchmark, BenchmarkDraft, BenchmarkDraftSummary, Context, Evidence, ExerciseKey,
    ObservedSet, Proposal, ProposalOrigin, StrengthImport, StrengthSession, Window, summarize,
    valid_key,
};
pub use service::{BenchmarkGateway, BenchmarkRepository, BenchmarkService};
pub use sqlite::SqliteBenchmarks;
