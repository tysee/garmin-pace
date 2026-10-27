//! tests/cases/terminal_delivery.rs verifies ambiguous delivery recovery through the menu.
//! A real CLI process persists its journal in SQLite while a local Garmin fixture times out.

use super::{Session, confirm};
use crate::{
    common::Fixture,
    garmin::{Server, configure_connect},
    workouts::remote_workout,
};
use anyhow::{Result, ensure};
use serde_json::json;

#[test]
fn delivery_timeout_opens_journal_without_retrying_the_write() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let saved = remote_workout("Tempo session", 1200.0).to_string();
    let devices = json!([{"unitId":99,"productDisplayName":"Test watch"}]).to_string();
    let server = Server::start(vec![
        (
            "GET",
            "/workout-service/workouts?start=0&limit=20",
            200,
            json!([{"workoutId":777}]).to_string(),
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
            504,
            "{}".into(),
        ),
    ])?;
    configure_connect(&f, &server.base)?;
    ensure!(
        f.command()
            .args(["workout", "sync"])
            .output()?
            .status
            .success()
    );
    let mut ui = Session::start(&f, 40, 15, true)?;
    ui.until("Search actions")?;
    ui.search("Standalone workout library")?;
    ui.until("New with model")?;
    ui.search("Tempo session")?;
    ui.until("> Full text")?;
    ui.search("Send to watch")?;
    ui.until("> Test watch")?;
    ui.send(b"\r")?;
    confirm(&mut ui)?;
    ui.until("Action failed")?;
    ui.send(b"\r")?;
    ui.until("Delivery status")?;
    ui.until("Device 99: Outcome unknown")?;
    ui.send(b"\x1b[B\r")?;
    ui.until("Version: current")?;
    ui.send(b"\x1b")?;
    ui.until("> Back")?;
    ui.send(b"\r")?;
    ui.until("Device 99: Outcome unknown")?;
    ui.exit()?;
    let output = f
        .command()
        .args(["workout", "deliveries", "--id", "777", "--device-id", "99"])
        .output()?;
    ensure!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(value["deliveries"][0]["state"], "sending");
    assert_eq!(value["watch_receipt"], "unconfirmed");
    server.finish()
}
