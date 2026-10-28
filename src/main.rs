//! src/main.rs is the process entry point. It parses CLI arguments and reports
//! an unhandled application error once, with a failing process exit status.

use std::process::ExitCode;

use clap::Parser;
use garmin_pace::interface::cli::{Cli, full_help, run};

fn main() -> ExitCode {
    if let Some(text) = full_help(std::env::args_os()) {
        print!("{text}");
        return ExitCode::SUCCESS;
    }
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
