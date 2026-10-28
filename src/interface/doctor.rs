//! src/interface/doctor.rs renders `pace doctor`: the Garmin network path checked without keys.
//! It prints the trust settings in effect and one line per endpoint with its route, and fails the
//! process when any endpoint gets no HTTP response. It exposes `run`.

use super::output::Output;
use crate::{
    config::Settings,
    garmin::{NetworkReport, ProbeOutcome},
};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::fmt::Write as _;

pub(super) fn run(settings: &Settings, output: Output<'_>) -> Result<()> {
    let report = crate::garmin::diagnose(&settings.garmin, &settings.garmin_auth)?;
    let failed = report
        .probes
        .iter()
        .filter(|probe| matches!(probe.outcome, ProbeOutcome::Failed { .. }))
        .count();
    output.message(&to_json(&report), &human(&report)?)?;
    ensure!(
        failed == 0,
        "Garmin network check failed for {failed} endpoint(s); no key was sent"
    );
    Ok(())
}

fn to_json(report: &NetworkReport) -> Value {
    json!({
        "extra_ca_file": report.extra_ca_file.as_ref().map(|path| path.display().to_string()),
        "certificate_variables": report.certificate_variables.iter().map(|variable| json!({
            "name": variable.name,
            "path": variable.path,
            "exists": variable.exists,
        })).collect::<Vec<_>>(),
        "endpoints": report.probes.iter().map(|probe| match &probe.outcome {
            ProbeOutcome::Reachable { status } => json!({
                "endpoint": probe.endpoint,
                "host": probe.host,
                "route": probe.route,
                "reachable": true,
                "http_status": status,
            }),
            ProbeOutcome::Failed { reason } => json!({
                "endpoint": probe.endpoint,
                "host": probe.host,
                "route": probe.route,
                "reachable": false,
                "reason": reason,
            }),
        }).collect::<Vec<_>>(),
    })
}

fn human(report: &NetworkReport) -> Result<String> {
    let mut text = String::from("Garmin network check (no keys sent)\n");
    if let Some(path) = &report.extra_ca_file {
        writeln!(text, "Extra trust roots: {}", path.display())?;
    }
    for variable in &report.certificate_variables {
        let state = if variable.exists { "exists" } else { "missing" };
        writeln!(text, "{}: {} ({state})", variable.name, variable.path)?;
    }
    for probe in &report.probes {
        match &probe.outcome {
            ProbeOutcome::Reachable { status } => writeln!(
                text,
                "ok    {} {} ({}): HTTP {status} (HTTPS path works; says nothing about keys)",
                probe.endpoint, probe.host, probe.route
            )?,
            ProbeOutcome::Failed { reason } => {
                writeln!(text, "FAIL  {} {}: {reason}", probe.endpoint, probe.host)?
            }
        }
    }
    Ok(text.trim_end().to_owned())
}
