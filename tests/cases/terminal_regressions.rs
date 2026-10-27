//! tests/cases/terminal_regressions.rs exercises ordinary menu rendering and navigation
//! through real PTYs and SQLite profiles, including resize and complete review text.

use super::{Session, scroll_until};
use crate::common::Fixture;
use anyhow::{Result, ensure};
use rustix::termios::{Winsize, tcsetwinsize};

fn resize(session: &mut Session, width: u16, height: u16) -> Result<()> {
    session.received.clear();
    tcsetwinsize(
        &session.slave,
        Winsize {
            ws_col: width,
            ws_row: height,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )?;
    Ok(())
}

#[test]
fn zero_width_resize_does_not_crash_the_menu() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let mut session = Session::start(&fixture, 80, 24, false)?;
    session.until("Search actions")?;
    resize(&mut session, 0, 0)?;
    session.until("\x1b[H\x1b[2J")?;
    resize(&mut session, 40, 15)?;
    session.until("Search actions")?;
    session.exit()
}

#[test]
fn wide_review_preserves_text_beyond_the_frame_width() -> Result<()> {
    use garmin_pace::workouts::{SqliteWorkouts, WorkoutService, document::WorkoutDocument};
    use std::sync::Arc;
    let fixture = Fixture::new()?;
    fixture.init()?;
    let service = WorkoutService::new(Arc::new(SqliteWorkouts::open(
        fixture.settings.database.clone(),
    )?));
    let mut candidate = crate::workouts::candidate("Wide review", 1200.0);
    candidate["description"] =
        serde_json::json!(format!("{}COMPLETE_REVIEW_MARKER", "word ".repeat(26)));
    let document: WorkoutDocument = serde_json::from_value(candidate)?;
    service.propose(document.try_into()?, None)?;
    let mut session = Session::start(&fixture, 180, 30, false)?;
    session.until("Search actions")?;
    session.search("Workout drafts")?;
    session.until("Refresh drafts")?;
    session.search("Wide review")?;
    session.until("> Review before/after")?;
    session.send(b"\r")?;
    session.until("Enter/Esc back")?;
    super::scroll_until(&mut session, "COMPLETE_REVIEW_MARKER")?;
    ensure!(
        session.saw("COMPLETE_REVIEW_MARKER"),
        "review text was clipped: {}",
        session.received
    );
    session.exit()
}

#[test]
fn compact_date_prompt_keeps_the_input_visible() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let mut session = Session::start(&fixture, 32, 10, true)?;
    session.until("Search actions")?;
    session.search("Health")?;
    session.until("> Import health")?;
    session.send(b"\r")?;
    session.until("> Custom period")?;
    session.send(b"\r")?;
    session.until("Enter continue  Esc cancel")?;
    ensure!(
        session.saw("> _"),
        "date input hidden below instructions: {}",
        session.received
    );
    session.send(b"2026-10-24")?;
    session.until("> 2026-10-24_")?;
    session.send(b"\x1b")?;
    session.until("> Import health")?;
    session.exit()
}

#[test]
fn review_cannot_advance_while_the_window_is_too_small() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let mut session = Session::start(&fixture, 80, 24, false)?;
    session.until("Search actions")?;
    session.search("Garmin logout")?;
    session.until("Enter next  Esc cancel")?;
    resize(&mut session, 20, 8)?;
    session.until("Resize: need 32x10")?;
    session.send(b"\r")?;
    // A key must be processed at the small size before the window is restored.
    std::thread::sleep(std::time::Duration::from_millis(200));
    resize(&mut session, 80, 24)?;
    session.until("Pace /")?;
    session.until("Enter next  Esc cancel")?;
    ensure!(!session.saw("> Cancel"), "an invisible review was accepted");
    session.exit()
}

#[test]
fn empty_proposal_list_has_no_phantom_pages() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let mut session = Session::start(&fixture, 100, 24, false)?;
    session.until("Search actions")?;
    session.search("Benchmark exercises")?;
    session.until("Sync strength data")?;
    session.search("Model proposals")?;
    session.until("Proposals: none yet")?;
    session.until("Enter open Tab info Esc back")?;
    ensure!(
        !session.saw("Next page") && !session.saw("Previous page"),
        "empty list offers phantom pages"
    );
    session.exit()
}

#[test]
fn ordinary_prompts_accept_pasted_text_without_submitting_it() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let mut session = Session::start(&fixture, 100, 24, false)?;
    session.until("Search actions")?;
    session.search("Sync activities")?;
    session.until("> Recent activities")?;
    session.send(b"\r")?;
    session.until("Enter continue  Esc cancel")?;
    session.send(b"\x1b[200~999\r\x1b[201~")?;
    session.until("> 999_")?;
    ensure!(!session.saw("Action failed"));
    session.send(b"\r")?;
    session.until("Action failed")?;
    scroll_until(&mut session, "limit")?;
    session.exit()
}
