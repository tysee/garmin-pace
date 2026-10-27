//! src/main.rs is the process entry point. It parses CLI arguments and reports
//! an unhandled application error once, with a failing process exit status.

use std::process::ExitCode;

use clap::Parser;
use garmin_pace::interface::cli::{Cli, run};

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
