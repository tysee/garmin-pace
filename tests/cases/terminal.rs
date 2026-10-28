//! tests/cases/terminal.rs drives the real menu through a PTY and temporary SQLite.
//! It checks setup, navigation, compact resizing, cancellation and terminal restoration.

use crate::common::Fixture;
use anyhow::{Context, Result, ensure};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt},
    termios::{Termios, Winsize, tcgetattr, tcsetwinsize},
};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::ffi::OsStrExt,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Session {
    child: Child,
    master: File,
    slave: File,
    original: Termios,
    received: String,
}

impl Session {
    fn start(fixture: &Fixture, width: u16, height: u16, explicit: bool) -> Result<Self> {
        Self::start_with_key(fixture, width, height, explicit, None)
    }

    fn start_with_key(
        fixture: &Fixture,
        width: u16,
        height: u16,
        explicit: bool,
        key: Option<&str>,
    ) -> Result<Self> {
        let mut session = Self::start_options(
            fixture,
            width,
            height,
            if explicit { &["ui", "--compact"] } else { &[] },
            key,
        )?;
        session.until("Pace / Actions")?;
        Ok(session)
    }

    fn start_args(fixture: &Fixture, width: u16, height: u16, args: &[&str]) -> Result<Self> {
        Self::start_options(fixture, width, height, args, None)
    }

    fn start_options(
        fixture: &Fixture,
        width: u16,
        height: u16,
        args: &[&str],
        key: Option<&str>,
    ) -> Result<Self> {
        let master = File::from(openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?);
        grantpt(&master)?;
        unlockpt(&master)?;
        let path = ptsname(&master, Vec::new())?;
        let slave = OpenOptions::new()
            .read(true)
            .write(true)
            .open(std::ffi::OsStr::from_bytes(path.to_bytes()))?;
        tcsetwinsize(
            &slave,
            Winsize {
                ws_col: width,
                ws_row: height,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )?;
        let original = tcgetattr(&slave)?;
        let mut command = Command::new(env!("CARGO_BIN_EXE_pace"));
        command
            .arg("--config")
            .arg(&fixture.config)
            .env("TERM", "xterm-256color")
            .env_remove("GARMIN_PERSONAL_API_KEY")
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave.try_clone()?));
        command.args(args);
        if let Some(key) = key {
            command.env("GARMIN_PERSONAL_API_KEY", key);
        }
        let child = command.spawn()?;
        Ok(Self {
            child,
            master,
            slave,
            original,
            received: String::new(),
        })
    }

    fn send(&mut self, bytes: &[u8]) -> Result<()> {
        self.received.clear();
        self.master.write_all(bytes)?;
        Ok(())
    }

    fn until(&mut self, expected: &str) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !self.saw(expected) {
            ensure!(
                Instant::now() < deadline,
                "terminal did not show {expected:?}: {}",
                self.received
            );
            let ready = poll(
                &mut [PollFd::new(&self.master, PollFlags::IN)],
                Some(&Timespec {
                    tv_sec: 0,
                    tv_nsec: 100_000_000,
                }),
            )?;
            if ready == 0 {
                continue;
            }
            let mut bytes = [0; 8192];
            let count = self.master.read(&mut bytes)?;
            ensure!(count != 0, "terminal closed before {expected}");
            self.received
                .push_str(&String::from_utf8_lossy(&bytes[..count]));
            ensure!(
                self.received.len() < 1_048_576,
                "terminal output exceeded test cap"
            );
        }
        Ok(())
    }

    fn saw(&self, expected: &str) -> bool {
        if self.received.contains(expected) {
            return true;
        }
        let mut plain = String::new();
        let mut chars = self.received.chars();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' && chars.next() == Some('[') {
                for code in chars.by_ref() {
                    if ('@'..='~').contains(&code) {
                        break;
                    }
                }
            } else {
                plain.push(c);
            }
        }
        plain
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains(expected)
    }

    fn search(&mut self, query: &str) -> Result<()> {
        self.send(format!("/{query}").as_bytes())?;
        self.until(&format!("/ {query}_"))?;
        self.send(b"\r")
    }

    fn home(&mut self) -> Result<()> {
        self.send(b"\r")?;
        self.until("Pace / Actions")?;
        self.send(b"\x1b")?;
        self.until("Search actions")
    }

    fn exit(&mut self) -> Result<()> {
        self.send(b"\x03")?;
        self.finish()
    }

    fn finish(&mut self) -> Result<()> {
        self.until("\x1b[?1049l")?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = self.child.try_wait()? {
                ensure!(status.success(), "menu exited with {status}");
                break;
            }
            ensure!(Instant::now() < deadline, "menu did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
        let restored = tcgetattr(&self.slave)?;
        assert_eq!(restored.local_modes, self.original.local_modes);
        assert_eq!(restored.input_modes, self.original.input_modes);
        assert_eq!(restored.output_modes, self.original.output_modes);
        assert_eq!(restored.control_modes, self.original.control_modes);
        Ok(())
    }
}

#[test]
fn every_menu_action_opens_at_desktop_and_cardputer_sizes() -> Result<()> {
    let cases = [
        ("Onboarding", None, "1. Prepare profile"),
        ("Status", None, "Config and database ready"),
        ("Activities", None, "Activities: none stored"),
        ("Sync activities", Some(""), "Action failed"),
        ("Health", None, "Import health"),
        ("Garmin login", None, "Action failed"),
        ("Garmin devices", None, "Action failed"),
        ("Session status", None, "Action failed"),
        ("Standalone workout library", None, "New with model"),
        ("Coach", None, "Sync this week"),
        ("Workout drafts", None, "Refresh drafts"),
        ("Benchmark exercises", None, "Sync strength data"),
        ("Check configuration", None, "Configuration is valid."),
        ("Show configuration", None, "[database]"),
        ("Create configuration", None, "Action failed"),
        ("Initialize database", None, "Database initialized."),
        ("MCP users", None, "No MCP users yet"),
        ("Garmin logout", None, "Enter next  Esc cancel"),
        ("Exit", None, ""),
    ];
    for (width, height) in [(100, 24), (40, 15), (32, 10)] {
        let fixture = Fixture::new()?;
        fixture.init()?;
        for (query, input, expected) in cases {
            let mut session = Session::start(&fixture, width, height, width < 76)?;
            session.until("Search actions")?;
            session.search(query)?;
            if query == "Exit" {
                session.finish()?;
                continue;
            }
            if query == "Garmin login" {
                session.until("Garmin email:")?;
                session.send(b"\x04")?;
            }
            if query == "Sync activities" {
                session.until("Recent activities")?;
                session.send(b"\r")?;
            }
            if let Some(input) = input {
                session.until("Enter continue  Esc cancel")?;
                session.send(format!("{input}\r").as_bytes())?;
            }
            session
                .until(expected)
                .with_context(|| format!("{query} at {width}x{height}"))?;
            if !matches!(
                query,
                "Onboarding"
                    | "Status"
                    | "Activities"
                    | "Health"
                    | "Standalone workout library"
                    | "Coach"
                    | "Workout drafts"
                    | "Garmin logout"
                    | "Benchmark exercises"
            ) {
                session.until("Enter/Esc back")?;
            }
            if query == "Show configuration" {
                session.until("path = \"")?;
            }
            session.exit()?;
        }
    }
    Ok(())
}

#[test]
fn menu_groups_actions_under_headings_and_searches_flat() -> Result<()> {
    for (width, height) in [(100, 24), (40, 15), (32, 10)] {
        let fixture = Fixture::new()?;
        fixture.init()?;
        let mut session = Session::start(&fixture, width, height, width < 76)?;
        session.until("> » Onboarding")?;
        ensure!(
            session.saw("History"),
            "heading missing at {width}x{height}"
        );
        // Down skips the heading row and lands on the first item of the next section.
        session.send(b"\x1b[B")?;
        session.until("> ≡ Status")?;
        session.send(b"\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B")?;
        session.until("> → Garmin login")?;
        session.until("7/22")?;
        session.send(b"/")?;
        session.until("/ _")?;
        session.send(b"config")?;
        session.until("/ config_")?;
        ensure!(session.saw("> √ Check configuration"));
        ensure!(
            !session.saw("History") && !session.saw("Configuration"),
            "headings shown during search at {width}x{height}"
        );
        session.exit()?;
    }
    Ok(())
}

#[test]
fn menu_draft_review_save_device_selection_send_and_logout_use_real_adapters() -> Result<()> {
    use crate::{
        garmin::{Server, configure_connect},
        workouts::{remote_workout, seed_draft},
    };
    use serde_json::json;
    for (width, height) in [(100, 24), (40, 15), (32, 10)] {
        let fixture = Fixture::new()?;
        fixture.init()?;
        seed_draft(&fixture)?;
        let devices = json!([
            {"unitId":42,"productDisplayName":"Other watch"},
            {"unitId":99,"productDisplayName":"Forerunner Sapphire"}
        ])
        .to_string();
        let saved = remote_workout("Tempo session", 1200.0).to_string();
        let server = Server::start(vec![
            (
                "POST",
                "/workout-service/workout",
                200,
                json!({"workoutId":777}).to_string(),
            ),
            ("GET", "/workout-service/workout/777", 200, saved.clone()),
            (
                "GET",
                "/device-service/deviceregistration/devices",
                200,
                devices.clone(),
            ),
            ("GET", "/workout-service/workout/777", 200, saved),
            (
                "GET",
                "/device-service/deviceregistration/devices",
                200,
                devices,
            ),
            (
                "POST",
                "/device-service/devicemessage/messages",
                200,
                "[]".into(),
            ),
        ])?;
        configure_connect(&fixture, &server.base)?;
        let mut session = Session::start(&fixture, width, height, width < 76)?;
        session.until("Search actions")?;
        session.search("Workout drafts")?;
        session.until("Refresh drafts")?;
        session.search("Tempo session")?;
        session.until("> Review before/after")?;
        // Small screens scroll to the step rows; the wide layout shows them at once.
        ensure!(
            width < 76 || session.saw("interval"),
            "draft steps missing at {width}x{height}"
        );
        session.send(b"\x1b[B")?;
        session.until("> Save to Garmin")?;
        session.send(b"\r")?;
        confirm(&mut session)?;
        session.until("Saved and verified in Garmin")?;
        session.exit()?;
        let mut session = Session::start(&fixture, width, height, width < 76)?;
        session.until("Search actions")?;
        session.search("Standalone workout library")?;
        session.until("New with model")?;
        session.search("Tempo session")?;
        session.until("> Full text")?;
        session.search("cooldown")?;
        session.until("Kind: cooldown")?;
        session.send(b"\x1b")?;
        session.until("/ cooldown_")?;
        session.send(b"\x1b")?;
        session.until("> Full text")?;
        session.send(b"\x1b[B\x1b[B")?;
        session.until("> Send to watch")?;
        session.send(b"\r")?;
        session.until("> Other watch")?;
        // Moving past the final choice must still return its original device index.
        session.send(b"\x1b[B\x1b[B")?;
        // The smallest layout clips the model name, so match its visible start.
        session.until("> Forerunner")?;
        session.send(b"\t")?;
        session.until("Device ID: 99")?;
        session.send(b"\r")?;
        session.until("> Forerunner")?;
        session.send(b"\r")?;
        confirm(&mut session)?;
        session.until("Sent to watch")?;
        session.exit()?;
        let mut session = Session::start(&fixture, width, height, width < 76)?;
        session.until("Search actions")?;
        session.search("Garmin logout")?;
        confirm(&mut session)?;
        session.until("Local Garmin session deleted.")?;
        session.exit()?;
        assert!(!fixture.settings.garmin_auth.session_path.exists());
        server.finish()?;
    }
    Ok(())
}

fn confirm(session: &mut Session) -> Result<()> {
    session.until("Enter next  Esc cancel")?;
    session.send(b"\r")?;
    session.until("> Cancel")?;
    session.send(b"\x1b[B")?;
    session.until("> Confirm")?;
    session.send(b"\r")
}

impl Drop for Session {
    fn drop(&mut self) {
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            other => {
                if let Err(error) = other {
                    eprintln!("checking terminal test child: {error}");
                }
                if let Err(error) = self.child.kill() {
                    eprintln!("stopping terminal test child: {error}");
                }
                if let Err(error) = self.child.wait() {
                    eprintln!("waiting for terminal test child: {error}");
                }
            }
        }
    }
}

#[test]
fn terminal_setup_history_and_draft_cancel_flow() -> Result<()> {
    let fixture = Fixture::new()?;
    std::fs::remove_file(&fixture.config)?;
    let mut session = Session::start(&fixture, 100, 28, false)?;
    session.until("Local status unavailable")?;
    assert!(session.received.contains("(for Garmin)"));
    assert!(!fixture.config.exists());
    assert!(!fixture.settings.database.path.exists());
    session.search("create configuration")?;
    session.until("Configuration created.")?;
    session.home()?;
    session.search("initialize database")?;
    session.until("Database initialized.")?;
    fixture.seed()?;
    session.home()?;
    session.until("2 activities | last sync never")?;
    crate::benchmarks::seed(&fixture, false)?;
    crate::benchmarks::seed_activity(&fixture)?;
    session.search("browse stored")?;
    session.until("Activities 1-3")?;
    session.until("Test run")?;
    assert!(session.received.contains("5.00 km"));
    assert!(!session.received.contains("fixture-1"));
    // Enter opens the newest activity: a strength session offering details or a sets import.
    session.send(b"\r")?;
    session.until("Import sets from Garmin")?;
    session.send(b"\r")?;
    session.until("2. barbell row | 60.0 kg | 4 reps")?;
    assert!(
        session
            .received
            .contains("6. row (category) | 80.0 kg | 10 reps")
    );
    assert!(session.received.contains("Rest sets: 1"));
    session.send(b"\x1b")?;
    session.until("> View details")?;
    session.send(b"\x1b")?;
    session.until("Activities 1-3")?;
    session.send(b"\x1b[B")?;
    session.send(b"\r")?;
    session.until("Garmin ID: fixture-2")?;
    assert!(!session.received.contains("Sets:"));
    session.send(b"\x1b")?;
    session.until("Activities 1-3")?;
    session.send(b"\x1b")?;
    session.until("Pace / Actions")?;
    session.send(b"\x1b")?;
    session.until("Search actions")?;
    crate::workouts::seed_draft(&fixture)?;
    session.search("Workout drafts")?;
    session.until("Refresh drafts")?;
    session.search("Tempo session")?;
    session.until("> Review before/after")?;
    session.send(b"\x1b[B")?;
    session.until("> Save to Garmin")?;
    session.send(b"\r")?;
    session.until("Review action")?;
    session.send(b"\r")?;
    session.until("> Cancel")?;
    session.send(b"\r")?;
    // Cancelling returns to the draft with Save still selected; nothing was written.
    session.until("> Save to Garmin")?;
    session.send(b"\x1b")?;
    session.until("/ Tempo session_")?;
    let connection = rusqlite::Connection::open(&fixture.settings.database.path)?;
    let state: String =
        connection.query_row("SELECT state FROM workout_drafts WHERE id=1", [], |row| {
            row.get(0)
        })?;
    assert_eq!(state, "draft");
    session.exit()
}

#[test]
fn compact_terminal_resizes_recovers_errors_and_ignores_paste() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let mut session = Session::start(&fixture, 40, 15, true)?;
    session.until("Pace / Actions")?;
    session.until("(for Garmin)")?;
    session.send(b"\x1b[B\x1b[B")?;
    session.until("> • Activities")?;
    session.send(b"\r")?;
    session.until("Activities: none stored")?;
    session.send(b"\x1b")?;
    session.until("Pace / Actions")?;
    session.send(b"\x1b[200~/garmin login\r\x1b[201~")?;
    session.send(b"/no such action")?;
    session.until("No matching actions")?;
    session.send(b"\x1b")?;
    session.until("Search actions")?;
    session.search("Sync activities")?;
    session.until("> Recent activities")?;
    session.send(b"\r")?;
    session.until("How many recent activities to import (1-100)? Enter = 20")?;
    session.send(b"999\r")?;
    session.until("Action failed")?;
    session.home()?;
    session.received.clear();
    tcsetwinsize(
        &session.slave,
        Winsize {
            ws_col: 20,
            ws_row: 8,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )?;
    session.until("Resize: need 32x10")?;
    session.received.clear();
    tcsetwinsize(
        &session.slave,
        Winsize {
            ws_col: 32,
            ws_row: 10,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )?;
    session.until("Pace / Actions")?;
    session.search("garmin login")?;
    // Do not submit credentials; cancel at the ordinary email prompt with EOF.
    session.until("Garmin email:")?;
    session.send(b"\x04")?;
    session.until("email is required")?;
    session.home()?;
    session
        .exit()
        .context("compact terminal must restore input mode after login error")
}

#[test]
fn smallest_terminal_scrolls_complete_workout_review_and_can_cancel() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    crate::workouts::seed_draft(&fixture)?;
    let mut session = Session::start(&fixture, 32, 10, true)?;
    session.until("Search actions")?;
    session.search("Workout drafts")?;
    session.until("Refresh drafts")?;
    session.search("Tempo session")?;
    session.until("> Review before/after")?;
    session.send(b"\r")?;
    session.until("1-4/")?;
    for _ in 0..16 {
        if session.saw("cooldown: 300 sec") {
            break;
        }
        session.send(b"\x1b[6~")?;
        session.until("Enter/Esc back")?;
    }
    session.until("cooldown: 300 sec")?;
    session.send(b"\x1b")?;
    session.until("> Review before/after")?;
    session.send(b"\x1b")?;
    session.until("/ Tempo session_")?;
    session.exit()
}

#[test]
fn benchmark_menu_reviews_evidence_and_cancels_save_at_all_sizes() -> Result<()> {
    for (width, height) in [(100, 24), (40, 15), (32, 10)] {
        let f = Fixture::new()?;
        f.init()?;
        let (service, p) = crate::benchmarks::seed(&f, false)?;
        service.propose(p)?;
        let mut session = Session::start(&f, width, height, width < 76)?;
        session.until("Search actions")?;
        session.search("Benchmark exercises")?;
        session.until("Sync strength data")?;
        session.search("Model proposals")?;
        session.until("Refresh list")?;
        session.search("barbell row")?;
        session.until("Enter next")?;
        for _ in 0..25 {
            if session.saw("Averages use the exact") {
                break;
            }
            session.send(b"\x1b[6~")?;
            session.until("Enter next")?;
        }
        session.until("Averages use the exact")?;
        session.send(b"\r")?;
        session.until("> Back")?;
        session.send(b"\x1b[B")?;
        session.until("> Save to Garmin")?;
        session.send(b"\r")?;
        session.until("Enter next")?;
        session.send(b"\r")?;
        session.until("> Cancel")?;
        session.send(b"\r")?;
        session.until("barbell row_")?;
        assert_eq!(
            service.draft(1)?.state,
            garmin_pace::workouts::DraftState::Draft
        );
        session.exit()?;
    }
    Ok(())
}
#[test]
fn empty_benchmark_proposals_explain_how_a_model_proposes_through_mcp() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let mut session = Session::start(&f, 100, 24, false)?;
    session.until("Search actions")?;
    session.search("Benchmark exercises")?;
    session.until("Sync strength data")?;
    session.search("Model proposals")?;
    session.until("Proposals: none yet")?;
    session.until("nothing is requested from it")?;
    session.search("How to get a proposal")?;
    session.until("Use your model connected to Pace through MCP")?;
    session.send(b"\x1b")?;
    session.until("Proposals: none yet")?;
    session.exit()
}
/// Page down through a result screen until the text is visible; small screens show few rows.
fn scroll_until(session: &mut Session, expected: &str) -> Result<()> {
    for _ in 0..10 {
        if session.saw(expected) {
            return Ok(());
        }
        session.send(b"\x1b[6~")?;
        session.until("Enter/Esc back")?;
    }
    session.until(expected)
}

#[test]
fn health_menu_drills_from_metric_coverage_into_days_and_samples() -> Result<()> {
    for (width, height) in [(100, 24), (40, 15), (32, 10)] {
        let fixture = Fixture::new()?;
        fixture.init()?;
        crate::health::seed_health(&fixture, 4321.0)?;
        let mut session = Session::start(&fixture, width, height, width < 76)?;
        session.until("Search actions")?;
        session.search("Health")?;
        // The narrowest screen clips list descriptions, so these checks use their prefixes.
        session.until("Saved days: 1")?;
        session.until("> Import health")?;
        // The selected row's coverage is shown below the list at every size.
        session.send(b"\x1b[B")?;
        session.until("> Daily summary")?;
        session.until("1 of 1 days | 2026-10-24")?;
        session.send(b"\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B")?;
        session.until("> Training readiness")?;
        session.until("No data on any of 1 saved")?;
        session.send(b"\x1b[A\x1b[A\x1b[A\x1b[A")?;
        session.until("> Heart rate")?;
        session.send(b"\r")?;
        session.until("> 2026-10-24")?;
        // The day below the seeded one was never imported.
        session.send(b"\x1b[B\r")?;
        session.until("2026-10-23: not synced.")?;
        session.send(b"\x1b")?;
        session.until("> 2026-10-23")?;
        session.send(b"\x1b[A\r")?;
        session.until("> Summary")?;
        session.send(b"\r")?;
        session.until("Source: Personal API key")?;
        scroll_until(&mut session, "Stored samples: 1")?;
        session.send(b"\x1b")?;
        session.until("> Summary")?;
        session.send(b"\x1b[B\r")?;
        session.until("samples 1-1 of 1")?;
        scroll_until(&mut session, "60 bpm")?;
        session.send(b"\x1b")?;
        session.until("> Samples")?;
        session.send(b"\x1b")?;
        session.until("> 2026-10-24")?;
        session.send(b"\x1b")?;
        session.until("> Heart rate")?;
        session.until("1 of 1 days | 2026-10-24")?;
        session.exit()?;
    }
    Ok(())
}

#[path = "onboarding.rs"]
mod onboarding;

#[path = "terminal_regressions.rs"]
mod regressions;

#[path = "terminal_flows.rs"]
mod flows;

#[path = "terminal_ux.rs"]
mod ux;

#[path = "terminal_delivery.rs"]
mod delivery;

#[path = "quick_installation.rs"]
mod quick_installation;

#[path = "terminal_overview.rs"]
mod overview;
