//! src/interface/cli_reference.rs renders `pace help --all`: every visible command with its
//! description and flags, walked from the same clap definition that parses arguments, so the
//! reference cannot drift from the CLI. Exposes render.

use clap::{Arg, Command};

/// Global options printed once at the top and left out of every command's flag list.
const GLOBAL: [&str; 4] = ["env_file", "json", "help", "version"];

/// Render the full command tree of `root`, depth first, in definition order.
pub(super) fn render(root: &Command) -> String {
    let mut out = String::new();
    let name = root.get_name();
    if let Some(about) = root.get_about() {
        out.push_str(&format!("{name}: {about}\n\n"));
    }
    out.push_str("Global options:\n");
    for arg in root.get_arguments().filter(|a| !a.is_hide_set()) {
        line(&mut out, arg);
    }
    for command in visible(root) {
        walk(&mut out, name, command);
    }
    out
}

fn visible(command: &Command) -> impl Iterator<Item = &Command> {
    command
        .get_subcommands()
        .filter(|c| !c.is_hide_set() && c.get_name() != "help")
}

fn walk(out: &mut String, parent: &str, command: &Command) {
    let path = format!("{parent} {}", command.get_name());
    out.push_str(&format!("\n{path}"));
    if let Some(about) = command.get_about() {
        out.push_str(&format!("  {about}"));
    }
    out.push('\n');
    for arg in command
        .get_arguments()
        .filter(|a| !a.is_hide_set() && !GLOBAL.contains(&a.get_id().as_str()))
    {
        line(out, arg);
    }
    for child in visible(command) {
        walk(out, &path, child);
    }
}

fn line(out: &mut String, arg: &Arg) {
    let mut flag = match (arg.get_long(), arg.get_short()) {
        (Some(long), Some(short)) => format!("-{short}, --{long}"),
        (Some(long), None) => format!("--{long}"),
        (None, Some(short)) => format!("-{short}"),
        (None, None) => arg.get_id().as_str().to_uppercase(),
    };
    if arg.get_action().takes_values() && arg.get_long().is_some() {
        let value = arg
            .get_value_names()
            .and_then(|names| names.first())
            .map_or_else(|| arg.get_id().as_str().to_uppercase(), ToString::to_string);
        flag.push_str(&format!(" <{value}>"));
    }
    if arg.is_required_set() {
        flag.push_str(" (required)");
    }
    let mut help = arg.get_help().map(ToString::to_string).unwrap_or_default();
    let defaults: Vec<_> = arg
        .get_default_values()
        .iter()
        .map(|v| v.to_string_lossy())
        .collect();
    let values: Vec<_> = arg
        .get_possible_values()
        .into_iter()
        .filter(|v| !v.is_hide_set())
        .map(|v| v.get_name().to_string())
        .collect();
    if arg.get_action().takes_values() && !values.is_empty() {
        help.push_str(&format!(" [values: {}]", values.join(", ")));
    }
    if !defaults.is_empty() {
        help.push_str(&format!(" [default: {}]", defaults.join(", ")));
    }
    if let Some(env) = arg.get_env() {
        help.push_str(&format!(" [env: {}]", env.to_string_lossy()));
    }
    let help = help.trim_start();
    if help.is_empty() {
        out.push_str(&format!("    {flag}\n"));
    } else {
        out.push_str(&format!("    {flag:<34} {help}\n"));
    }
}
