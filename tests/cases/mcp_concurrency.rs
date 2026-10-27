//! tests/cases/mcp_concurrency.rs holds Garmin responses to prove HTTP timeouts and client
//! disconnects cannot release the operation gate, duplicate writes or hide durable outcomes.

use crate::{
    common::{
        Fixture,
        mcp::{HttpMcp, request},
    },
    garmin::{Server, configure_connect},
    workouts::{remote_workout, seed_draft},
};
use anyhow::{Context, Result, ensure};
use serde_json::json;
use std::{
    thread,
    time::{Duration, Instant},
};

const ASK: &str = "Save this test workout to Garmin";

#[test]
fn timeout_keeps_write_owned_and_blocks_concurrent_garmin_calls() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    seed_draft(&fixture)?;
    let (garmin, entered, release) = Server::paused(
        vec![
            (
                "POST",
                "/workout-service/workout",
                200,
                json!({"workoutId":777}).to_string(),
            ),
            (
                "GET",
                "/workout-service/workout/777",
                200,
                remote_workout("Tempo session", 1200.0).to_string(),
            ),
        ],
        0,
    )?;
    configure_connect(&fixture, &garmin.base)?;
    let http = HttpMcp::start(&fixture, 1)?;
    let (client, base, token) = (http.client.clone(), http.base.clone(), http.token.clone());
    let writer = thread::spawn(move || {
        request(
            &client,
            &base,
            &token,
            "save_workout_draft_to_garmin_v1",
            json!({"id":1,"revision":1,"user_request":ASK}),
        )
    });
    entered.recv_timeout(Duration::from_secs(5))?;
    let outcome = writer
        .join()
        .map_err(|_| anyhow::anyhow!("writer panicked"))??;
    assert_eq!(outcome.status(), 408);
    assert_eq!(
        http.content("get_workout_draft_v1", json!({"id":1}))?["state"],
        "saving"
    );
    assert_eq!(
        http.content("get_capabilities_v1", json!({}))?["garmin_operation_running"],
        true
    );
    for (name, args) in [
        (
            "save_workout_draft_to_garmin_v1",
            json!({"id":1,"revision":1,"user_request":ASK}),
        ),
        ("list_garmin_devices_v1", json!({})),
        (
            "sync_workouts_v1",
            json!({"limit":1,"user_request":"Refresh the library"}),
        ),
    ] {
        assert_eq!(http.call(name, args)?["error"]["data"]["outcome"], "busy");
    }
    // SQLite reads remain available while the network worker holds the write gate.
    assert_eq!(http.content("get_status", json!({}))?["activity_count"], 0);
    release.send(())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if http.content("get_workout_draft_v1", json!({"id":1}))?["state"] == "verified" {
            break;
        }
        ensure!(
            Instant::now() < deadline,
            "save did not complete after HTTP timeout"
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        http.content("get_workout_draft_v1", json!({"id":1}))?["remote_id"],
        777
    );
    assert!(
        http.call(
            "save_workout_draft_to_garmin_v1",
            json!({"id":1,"revision":1,"user_request":ASK})
        )?["error"]
            .is_object()
    );
    assert_eq!(
        garmin
            .writes
            .lock()
            .map_err(|_| anyhow::anyhow!("writes poisoned"))?
            .len(),
        1
    );
    garmin.finish()?;
    http.stop()
}

#[test]
fn disconnected_delivery_finishes_and_its_journal_prevents_resend() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let saved = remote_workout("Tempo session", 1200.0).to_string();
    let devices = json!([{"unitId":99,"productDisplayName":"Test watch"}]).to_string();
    let (garmin, entered, release) = Server::paused(
        vec![
            ("GET", "/workout-service/workout/777", 200, saved.clone()),
            ("GET", "/workout-service/workout/777", 200, saved.clone()),
            (
                "GET",
                "/device-service/deviceregistration/devices",
                200,
                devices.clone(),
            ),
            (
                "POST",
                "/device-service/devicemessage/messages",
                200,
                "[]".into(),
            ),
            // A second explicit call may verify the remote source, but must never queue again.
            ("GET", "/workout-service/workout/777", 200, saved),
            (
                "GET",
                "/device-service/deviceregistration/devices",
                200,
                devices,
            ),
        ],
        3,
    )?;
    configure_connect(&fixture, &garmin.base)?;
    let http = HttpMcp::start(&fixture, 10)?;
    http.content(
        "refresh_workout_v1",
        json!({"id":777,"user_request":"Refresh this workout"}),
    )?;
    let base = http.base.clone();
    let token = http.token.clone();
    let sender = thread::spawn(move || -> Result<bool> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(300))
            .build()?;
        Ok(request(
            &client,
            &base,
            &token,
            "send_workout_to_device_v1",
            json!({"id":777,"device_id":99,"user_request":"Send this test workout to my watch"}),
        )
        .err()
        .is_some_and(|e| {
            e.downcast_ref::<reqwest::Error>()
                .is_some_and(|e| e.is_timeout())
                || e.chain().any(|e| {
                    e.downcast_ref::<reqwest::Error>()
                        .is_some_and(|e| e.is_timeout())
                })
        }))
    });
    entered.recv_timeout(Duration::from_secs(5))?;
    assert!(
        sender
            .join()
            .map_err(|_| anyhow::anyhow!("sender panicked"))??
    );
    let args = json!({"workout_id":777,"device_id":99});
    let pending = http.content("get_workout_deliveries_v1", args.clone())?;
    assert_eq!(pending["deliveries"][0]["state"], "sending");
    assert_eq!(pending["deliveries"][0]["status_label"], "Outcome unknown");
    assert_eq!(pending["watch_receipt"], "unconfirmed");
    assert_eq!(
        http.call("list_garmin_devices_v1", json!({}))?["error"]["data"]["outcome"],
        "busy"
    );
    release.send(())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let journal = http.content("get_workout_deliveries_v1", args.clone())?;
        if journal["deliveries"][0]["state"] == "queued"
            && http.content("get_capabilities_v1", json!({}))?["garmin_operation_running"] == false
        {
            break;
        }
        ensure!(
            Instant::now() < deadline,
            "delivery did not finish after client disconnect"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let duplicate = http.call(
        "send_workout_to_device_v1",
        json!({"id":777,"device_id":99,"user_request":"Send this test workout to my watch"}),
    )?;
    ensure!(
        duplicate["error"]["message"]
            .as_str()
            .context("duplicate error")?
            .contains("already sent or has an unresolved send"),
        "{duplicate}"
    );
    assert_eq!(
        http.content("get_workout_deliveries_v1", args)?["deliveries"]
            .as_array()
            .context("deliveries")?
            .len(),
        1
    );
    garmin.finish()?;
    http.stop()
}
