//! tests/cases/mcp_agent.rs exercises the agent's import, inspect, edit and save flow through
//! authenticated MCP with real SQLite. Only Garmin's external API is a scripted local server.

use crate::{
    benchmarks::{ACTIVITIES, CATALOG, SETS, activity, remote, sets},
    common::{Fixture, mcp::HttpMcp},
    garmin::{KEY, Server, TOKEN, TOKEN_PATH, configure, configure_connect},
    workouts::{candidate, remote_workout},
};
use anyhow::{Context, Result, ensure};
use garmin_pace::{
    history::{ActivityQuery, HistoryService, SqliteHistory},
    secrets::{Secrets, profile_key_path},
};
use serde_json::json;
use std::sync::Arc;

#[test]
fn agent_creates_revises_reviews_saves_and_delivers_a_new_workout_once() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let saved = remote_workout("New agent session", 1200.0).to_string();
    let devices = json!([{"unitId":99,"productDisplayName":"Test watch"}]).to_string();
    let g = Server::start(vec![
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
        // Duplicate sends may check the source/device, but must not queue another message.
        ("GET", "/workout-service/workout/777", 200, saved),
        (
            "GET",
            "/device-service/deviceregistration/devices",
            200,
            devices,
        ),
    ])?;
    configure_connect(&f, &g.base)?;
    let http = HttpMcp::start(&f, 10)?;
    let draft = http.content(
        "create_workout_draft_v1",
        json!({"workout":candidate("New agent session",900.0)}),
    )?;
    assert!(draft["source"].is_null());
    assert!(draft["remote_id"].is_null());
    let revision = http.content(
        "revise_workout_draft_v1",
        json!({"id":draft["id"],"revision":1,"workout":candidate("New agent session",1200.0)}),
    )?;
    assert_eq!(revision["revision"], 2);
    assert!(
        !http.call(
            "revise_workout_draft_v1",
            json!({"id":draft["id"],"revision":1,"workout":candidate("Stale edit",600.0)})
        )?["error"]
            .is_null()
    );
    let review = http.content("get_workout_draft_v1", json!({"id":draft["id"]}))?;
    assert_eq!(review["revision"], 2);
    assert!(
        review["review"]
            .as_str()
            .context("review")?
            .contains("1200 sec")
    );
    assert!(
        !http.call(
            "save_workout_draft_to_garmin_v1",
            json!({"id":draft["id"],"revision":1,"user_request":"Save this reviewed test workout"})
        )?["error"]
            .is_null()
    );
    let save =
        json!({"id":draft["id"],"revision":2,"user_request":"Save this reviewed test workout"});
    let verified = http.content("save_workout_draft_to_garmin_v1", save.clone())?;
    assert_eq!(verified["state"], "verified");
    assert_eq!(verified["remote_id"], 777);
    assert!(!http.call("save_workout_draft_to_garmin_v1", save)?["error"].is_null());
    let source = http.content("get_workout_v1", json!({"id":777}))?;
    assert!(source["coach_plan_id"].is_null());
    assert_eq!(source["definition"]["name"], "New agent session");
    let devices = http.content("list_garmin_devices_v1", json!({}))?;
    assert_eq!(devices["devices"][0]["id"], 99);
    let send = json!({"id":777,"device_id":99,"user_request":"Send this reviewed test workout to my watch"});
    let sent = http.content("send_workout_to_device_v1", send.clone())?;
    assert_eq!(sent["queued"], true);
    assert_eq!(sent["status_label"], "Sent to watch");
    assert_eq!(sent["watch_receipt"], "unconfirmed");
    let duplicate = http.call("send_workout_to_device_v1", send)?;
    assert!(
        duplicate["error"]["message"]
            .as_str()
            .context("duplicate")?
            .contains("already sent or has an unresolved send")
    );
    let journal = http.content(
        "get_workout_deliveries_v1",
        json!({"workout_id":777,"device_id":99}),
    )?;
    assert_eq!(
        journal["deliveries"]
            .as_array()
            .context("deliveries")?
            .len(),
        1
    );
    assert_eq!(journal["deliveries"][0]["revision"], source["revision"]);
    assert_eq!(journal["deliveries"][0]["state"], "queued");
    assert_eq!(journal["deliveries"][0]["status_label"], "Sent to watch");
    assert_eq!(journal["watch_receipt"], "unconfirmed");
    g.finish()?;
    http.stop()
}

#[test]
fn history_search_filters_pages_and_uses_sport_date_index() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    fixture.seed()?;
    let db = rusqlite::Connection::open(&fixture.settings.database.path)?;
    db.execute("INSERT INTO activities VALUES('strength','Strength','strength_training','2026-10-02T09:00:00.000Z',60,NULL)",[])?;
    let service = HistoryService::new(
        Arc::new(SqliteHistory::open(fixture.settings.database.clone())?),
        2,
    );
    let mut query = ActivityQuery {
        start: Some("2026-10-01".parse()?),
        end: Some("2026-10-02".parse()?),
        sport: None,
        name: None,
        limit: 1,
        offset: 0,
    };
    assert_eq!(service.search(&query)?.activities[0].garmin_id, "strength");
    query.offset = 1;
    assert_eq!(service.search(&query)?.activities[0].garmin_id, "fixture-2");
    query.sport = Some("running".into());
    query.offset = 0;
    assert_eq!(service.search(&query)?.activities[0].garmin_id, "fixture-2");
    query.start = query.end;
    query.offset = 1;
    assert!(service.search(&query)?.activities.is_empty());
    query.offset = 0;
    query.sport = Some("running' OR 1=1 --".into());
    assert!(service.search(&query)?.activities.is_empty());
    query.limit = 3;
    assert!(service.search(&query).is_err());
    query.limit = 1;
    query.start = Some("2026-10-03".parse()?);
    assert!(service.search(&query).is_err());
    let plan:String=db.query_row("EXPLAIN QUERY PLAN SELECT garmin_id FROM activities WHERE sport=?1 AND started_at>=?2 AND started_at<?3 ORDER BY started_at DESC,garmin_id DESC LIMIT ?4 OFFSET ?5",rusqlite::params!["running","2026-10-01T00:00:00","2026-10-03T00:00:00",1,0],|r|r.get(3))?;
    ensure!(plan.contains("activities_sport_chronological"), "{plan}");
    // Reproduce a real v7 database and prove the forward migration preserves rows.
    db.execute_batch("DROP TABLE coach_reschedules; DROP TABLE sync_run_steps; DROP TABLE sync_run_windows; DROP TABLE sync_runs; DROP INDEX workout_library_standalone; DROP TABLE coach_snapshots; ALTER TABLE workout_drafts DROP COLUMN explanation; DROP INDEX activities_sport_chronological; PRAGMA user_version=7;")?;
    assert!(SqliteHistory::open(fixture.settings.database.clone()).is_err());
    fixture.init()?;
    assert_eq!(service.summary()?.activity_count, 3);
    Ok(())
}

#[test]
fn granted_agent_syncs_all_sources_reads_evidence_and_updates_same_workout() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let run = json!({"activityId":502,"activityName":"Run","activityType":{"typeKey":"running"},"startTimeGMT":"2026-10-24T08:00:00Z","duration":600});
    let strength = activity()[1].clone();
    let original = remote_workout("Tempo session", 1200.0).to_string();
    let updated = remote_workout("Agent revised", 900.0).to_string();
    let token = || ("POST", TOKEN_PATH, 200, TOKEN.into());
    let mut script = vec![
        token(),
        (
            "GET",
            "/activitylist-service/activities/search/activities?start=0&limit=2",
            200,
            json!([run, strength]).to_string(),
        ),
        token(),
        (
            "GET",
            "/activitylist-service/activities/search/activities?start=2&limit=2",
            200,
            "[]".into(),
        ),
        token(),
    ];
    let mut health_script = crate::health::day_script("2026-10-24");
    health_script[1].3 = json!({"id":null,"calendarDate":null,"sleepTimeSeconds":null,"deviceRemCapable":false,"retro":false,"sleepNeed":{"calendarDate":"2026-10-24","actual":480}}).to_string();
    health_script[4].3 = json!([{"date":"2026-10-24","charged":null,"drained":null,"bodyBatteryValueDescriptorDTOList":null,"bodyBatteryValuesArray":[[1792800000000i64,null]]}]).to_string();
    script.extend(health_script);
    script.extend([
        token(),
        ("GET", CATALOG, 200, json!([remote(false, 0.0)]).to_string()),
        ("GET", ACTIVITIES, 200, activity().to_string()),
        ("GET", SETS, 200, sets().to_string()),
        ("GET", SETS, 200, sets().to_string()),
        (
            "GET",
            "/workout-service/workouts?start=0&limit=2",
            200,
            json!([{"workoutId":777}]).to_string(),
        ),
        ("GET", "/workout-service/workout/777", 200, original.clone()),
        ("GET", "/workout-service/workout/777", 200, original.clone()),
        ("GET", "/workout-service/workout/777", 200, original),
        ("PUT", "/workout-service/workout/777", 200, "{}".into()),
        ("GET", "/workout-service/workout/777", 200, updated),
    ]);
    let garmin = Server::start(script)?;
    configure(&fixture, &garmin.base)?;
    configure_connect(&fixture, &garmin.base)?;
    Secrets::save_personal_key(&profile_key_path(&fixture.settings.database.path)?, KEY)?;
    let http = HttpMcp::start(&fixture, 10)?;
    let ask = "Import my training data for this test";
    assert_eq!(
        http.content("get_capabilities_v1", json!({}))?["garmin_access"],
        true
    );
    let imported = http.content("sync_activities_v1", json!({"limit":2,"user_request":ask}))?;
    assert_eq!(imported["received"], 2);
    assert_eq!(imported["next_offset"], 2);
    assert!(
        http.content(
            "sync_activities_v1",
            json!({"limit":2,"offset":2,"user_request":ask})
        )?["next_offset"]
            .is_null()
    );
    let health = http.content(
        "sync_health_v1",
        json!({"date":"2026-10-24","user_request":ask}),
    )?;
    assert_eq!(
        health["datasets"]["daily"]["records"][0]["values"]["steps"]["value"].as_f64(),
        Some(1000.0)
    );
    assert_eq!(health["datasets"]["sleep"]["status"], "no_data");
    assert_eq!(health["datasets"]["body_battery"]["status"], "no_data");
    assert_eq!(
        http.content(
            "sync_strength_v1",
            json!({"start":"2026-10-20","end":"2026-10-24","user_request":ask})
        )?["sessions"],
        1
    );
    let matches = http.content(
        "search_activities_v1",
        json!({"start":"2026-10-20","end":"2026-10-24","sport":"strength_training","limit":10}),
    )?;
    assert_eq!(matches["activities"][0]["garmin_id"], "501");
    assert_eq!(matches["total"], 1);
    let undated = http.content(
        "search_activities_v1",
        json!({"sport":"strength_training","limit":10}),
    )?;
    assert_eq!(undated["total"], 1);
    assert_eq!(undated["activities"][0]["garmin_id"], "501");
    assert_eq!(
        http.content("get_activity_v1", json!({"garmin_id":"501"}))?["activity"]["sport"],
        "strength_training"
    );
    assert!(http.content("get_activity_v1", json!({"garmin_id":"absent"}))?["activity"].is_null());
    assert_eq!(
        http.content("get_activity_sets_v1", json!({"garmin_id":"501"}))?["sets"]["status"],
        "available"
    );
    // A per-activity import reads only that activity's sets: no token or search request.
    let single = http.content(
        "sync_activity_sets_v1",
        json!({"garmin_id":"501","user_request":ask}),
    )?;
    assert_eq!(single["activity"]["garmin_id"], "501");
    assert_eq!(single["sets"]["status"], "available");
    let evidence = http.content(
        "get_benchmark_context_v1",
        json!({"key":"BARBELL_ROW","start":"2026-10-20","end":"2026-10-24"}),
    )?;
    assert!(evidence["sample_count"].as_u64().context("sample count")? > 0);
    let exercises = http.content("search_exercises_v1", json!({"query":"barbell_row"}))?;
    assert_eq!(exercises["exercises"][0]["name"], "BARBELL_ROW");
    assert_eq!(
        http.content("sync_workouts_v1", json!({"limit":2,"user_request":ask}))?["received"],
        1
    );
    let source = http.content(
        "refresh_workout_v1",
        json!({"id":777,"user_request":"Read the latest workout before editing"}),
    )?;
    let draft=http.content("create_workout_draft_v1",json!({"workout":candidate("Agent revised",900.0),"source":{"id":777,"revision":source["revision"]}}))?;
    let saved=http.content("save_workout_draft_to_garmin_v1",json!({"id":draft["id"],"revision":draft["revision"],"user_request":"Save my revised workout to Garmin"}))?;
    assert_eq!(saved["state"], "verified");
    assert_eq!(saved["remote_id"], 777);
    assert_eq!(
        http.content("get_workout_v1", json!({"id":777}))?["name"],
        "Agent revised"
    );
    let empty = http.content(
        "get_workout_deliveries_v1",
        json!({"workout_id":777,"device_id":99}),
    )?;
    assert_eq!(empty["deliveries"], json!([]));
    assert_eq!(empty["watch_receipt"], "unconfirmed");
    let every_device = http.content("get_workout_deliveries_v1", json!({"workout_id":777}))?;
    assert_eq!(every_device["deliveries"], json!([]));
    garmin.finish()?;
    http.stop()
}

#[test]
fn sync_requires_profile_file_and_grant_and_rejects_invalid_inputs() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let garmin = Server::start(vec![])?;
    configure(&fixture, &garmin.base)?;
    configure_connect(&fixture, &garmin.base)?;
    let http = HttpMcp::start(&fixture, 10)?;
    let missing = http.call(
        "sync_activities_v1",
        json!({"limit":1,"user_request":"Import activities"}),
    )?;
    assert!(
        missing["error"]["message"]
            .as_str()
            .context("missing credential")?
            .contains("private garmin.env")
    );
    for (name, args) in [
        (
            "sync_activities_v1",
            json!({"limit":0,"user_request":"Import"}),
        ),
        (
            "sync_activities_v1",
            json!({"limit":1,"start":"2026-10-01","user_request":"Import"}),
        ),
        (
            "sync_activities_v1",
            json!({"limit":1,"start":"2026-01-01","end":"2026-10-01","user_request":"Import"}),
        ),
        (
            "sync_health_v1",
            json!({"date":"2026-02-30","user_request":"Import"}),
        ),
        (
            "sync_strength_v1",
            json!({"start":"2026-10-27","end":"2026-10-01","user_request":"Import"}),
        ),
        (
            "sync_workouts_v1",
            json!({"limit":101,"user_request":"Import"}),
        ),
        (
            "sync_activity_sets_v1",
            json!({"garmin_id":"","user_request":"Import"}),
        ),
        (
            "refresh_workout_v1",
            json!({"id":0,"user_request":"Refresh"}),
        ),
        ("sync_workouts_v1", json!({"limit":1,"user_request":"\n"})),
    ] {
        assert!(http.call(name, args)?["error"].is_object(), "{name}");
    }
    let long = http.call(
        "sync_activities_v1",
        json!({"limit":1,"start":"2026-01-01","end":"2026-10-01","user_request":"Import"}),
    )?;
    let message = long["error"]["message"].as_str().context("period error")?;
    assert!(message.contains("activity period"), "{message}");
    assert!(!message.contains("benchmark"), "{message}");
    let denied = fixture
        .command()
        .args(["users", "deny-garmin-writes", "owner"])
        .output()?;
    ensure!(denied.status.success(), "deny grant failed");
    assert_eq!(
        http.content("get_capabilities_v1", json!({}))?["garmin_access"],
        false
    );
    assert_eq!(
        http.call(
            "sync_workouts_v1",
            json!({"limit":1,"user_request":"Import"})
        )?["error"]["data"]["outcome"],
        "denied"
    );
    assert_eq!(http.content("get_status", json!({}))?["activity_count"], 0);
    garmin.finish()?;
    http.stop()
}
