//! tests/cases/coach.rs drives Coach import, evidence review and revision-checked updates
//! through a running authenticated MCP server and real SQLite. Only external Garmin is scripted.

use crate::{
    common::{Fixture, mcp::HttpMcp},
    garmin::{Server, configure_connect},
};
use anyhow::{Context, Result};
use serde_json::{Value, json};

pub(super) const PLANS: &str = "/trainingplan-service/trainingplan/plans?limit=100&start=1";
pub(super) const WEEK: &str =
    "/workout-service/schedule/workoutScheduleByPeriod?startDate=2026-10-26&endDate=2026-11-01";
pub(super) fn plans() -> String {
    json!({"trainingPlanList":[{"trainingPlanId":42,"name":"Push, Pull, Legs","trainingPlanCategory":"STRENGTH","trainingStatus":{"statusKey":"Scheduled"},"startDate":"2026-10-27T00:00:00.0","endDate":"2026-12-05T00:00:00.0"}]}).to_string()
}
pub(super) fn workout() -> Value {
    json!({"workoutId":777,"trainingPlanId":42,"workoutName":"Push Day","description":null,"sportType":{"sportTypeKey":"strength_training"},"workoutSegments":[{"segmentOrder":1,"sportType":{"sportTypeKey":"strength_training"},"workoutSteps":[{"type":"ExecutableStepDTO","stepOrder":21,"stepType":{"stepTypeKey":"interval"},"endCondition":{"conditionTypeKey":"reps"},"endConditionValue":8.0,"isMinReps":true,"targetType":{"workoutTargetTypeKey":"no.target"},"category":"BENCH_PRESS","exerciseName":"BARBELL_BENCH_PRESS","relativeWeight":"HEAVY","weightValue":null}]}]})
}
pub(super) fn schedule(w: &Value) -> String {
    json!([{"workoutScheduleId":99,"calendarDate":"2026-10-27","workout":w,"associatedActivityId":null}]).to_string()
}
#[test]
fn agent_reads_coach_proposes_reviews_saves_and_delivers_same_plan_workout() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let original = workout();
    let mut updated = original.clone();
    let st = &mut updated["workoutSegments"][0]["workoutSteps"][0];
    st["relativeWeight"] = Value::Null;
    st["weightValue"] = json!(40.0);
    st["weightUnit"] = json!({"unitKey":"kilogram"});
    let g = Server::start(vec![
        ("GET", PLANS, 200, plans()),
        ("GET", WEEK, 200, schedule(&original)),
        (
            "GET",
            "/workout-service/workout/777",
            200,
            original.to_string(),
        ),
        (
            "GET",
            "/workout-service/workout/777",
            200,
            original.to_string(),
        ),
        ("PUT", "/workout-service/workout/777", 200, "{}".into()),
        (
            "GET",
            "/workout-service/workout/777",
            200,
            updated.to_string(),
        ),
        ("GET", PLANS, 200, plans()),
        ("GET", WEEK, 200, schedule(&updated)),
        (
            "GET",
            "/workout-service/workout/777",
            200,
            updated.to_string(),
        ),
        (
            "GET",
            "/workout-service/workout/777",
            200,
            updated.to_string(),
        ),
        (
            "GET",
            "/device-service/deviceregistration/devices",
            200,
            json!([{"unitId":99,"productDisplayName":"Test watch"}]).to_string(),
        ),
        (
            "POST",
            "/device-service/devicemessage/messages",
            200,
            "[]".into(),
        ),
    ])?;
    configure_connect(&f, &g.base)?;
    let http = HttpMcp::start(&f, 10)?;
    let dates = json!({"start":"2026-10-26","end":"2026-11-01"});
    assert_eq!(
        http.content("get_coach_v1", dates.clone())?["status"],
        "not_synced"
    );
    let imported = http.content(
        "sync_coach_v1",
        json!({"start":"2026-10-26","end":"2026-11-01","user_request":"Read my Coach week"}),
    )?;
    assert_eq!(imported["snapshot"]["plans"][0]["id"], 42);
    let source = http.content("get_workout_v1", json!({"id":777}))?;
    let mut candidate = source["definition"].clone();
    assert_eq!(
        candidate["segments"][0]["steps"][0]["duration"]["type"],
        "minimum_repetitions"
    );
    assert_eq!(
        candidate["segments"][0]["steps"][0]["exercise"]["relative_load"],
        "heavy"
    );
    candidate["segments"][0]["steps"][0]["exercise"]["relative_load"] = Value::Null;
    candidate["segments"][0]["steps"][0]["exercise"]["weight_kg"] = json!(40.0);
    let source_arg = json!({"id":777,"revision":source["revision"]});
    assert!(
        !http.call(
            "create_workout_draft_v1",
            json!({"source":source_arg,"workout":candidate})
        )?["error"]
            .is_null()
    );
    let explanation = "User-selected test load of 40 kg; no inferred conversion from heavy. Keep minimum eight repetitions.";
    let draft = http.content(
        "create_workout_draft_v1",
        json!({"source":source_arg,"workout":candidate,"explanation":explanation}),
    )?;
    assert_eq!(draft["source"]["coach_plan_id"], 42);
    let review = http.content("get_workout_draft_v1", json!({"id":draft["id"]}))?;
    let text = review["review"].as_str().context("review text")?;
    for needle in [
        "BEFORE",
        "AFTER",
        "heavy relative load",
        "40 kg",
        "at least 8 reps",
        explanation,
    ] {
        assert!(text.contains(needle), "{text}");
    }
    assert!(
        !http.call(
            "save_workout_draft_to_garmin_v1",
            json!({"id":draft["id"],"revision":2,"user_request":"Save this test"})
        )?["error"]
            .is_null()
    );
    let saved = http.content(
        "save_workout_draft_to_garmin_v1",
        json!({"id":draft["id"],"revision":draft["revision"],"user_request":"Save this test"}),
    )?;
    assert_eq!(saved["state"], "verified");
    assert_eq!(saved["remote_id"], 777);
    assert_eq!(saved["explanation"], explanation);
    let write = g
        .writes
        .lock()
        .map_err(|_| anyhow::anyhow!("write mutex"))?[0]
        .clone();
    assert_eq!(write["trainingPlanId"], 42);
    assert_eq!(
        write["workoutSegments"][0]["workoutSteps"][0]["weightValue"],
        40.0
    );
    assert_eq!(
        write["workoutSegments"][0]["workoutSteps"][0]["isMinReps"],
        true
    );
    let after = http.content(
        "sync_coach_v1",
        json!({"start":"2026-10-26","end":"2026-11-01","user_request":"Check my saved Coach week"}),
    )?;
    assert_eq!(after["snapshot"]["sessions"][0]["schedule_id"], 99);
    assert_eq!(
        after["snapshot"]["sessions"][0]["workout"]["coach_plan_id"],
        42
    );
    let sent = http.content("send_workout_to_device_v1",
        json!({"id":777,"device_id":99,"user_request":"Send this reviewed Coach workout to my test watch"}))?;
    assert_eq!(sent["queued"], true);
    assert_eq!(sent["watch_receipt"], "unconfirmed");
    let delivery = http.content(
        "get_workout_deliveries_v1",
        json!({"workout_id":777,"device_id":99}),
    )?;
    assert_eq!(delivery["deliveries"][0]["state"], "queued");
    let every_device = http.content("get_workout_deliveries_v1", json!({"workout_id":777}))?;
    assert_eq!(every_device["deliveries"][0]["device_id"], 99);
    let source = http.content("get_workout_v1", json!({"id":777}))?;
    assert_eq!(source["coach_plan_id"], 42);
    let library = http.content("list_workouts_v1", json!({}))?;
    assert_eq!(library["workouts"][0]["id"], 777);
    let standalone = http.content("list_workouts_v1", json!({"standalone":true}))?;
    assert_eq!(standalone["workouts"], json!([]));
    assert_eq!(delivery["deliveries"][0]["revision"], source["revision"]);
    g.finish()?;
    http.stop()
}
#[test]
fn failed_coach_import_keeps_previous_snapshot_and_window_bounds_are_enforced() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let original = workout();
    let mut invalid = original.clone();
    invalid["trainingPlanId"] = json!(43);
    let g = Server::start(vec![
        ("GET", PLANS, 200, plans()),
        ("GET", WEEK, 200, schedule(&original)),
        (
            "GET",
            "/workout-service/workout/777",
            200,
            original.to_string(),
        ),
        ("GET", PLANS, 200, plans()),
        ("GET", WEEK, 200, schedule(&original)),
        (
            "GET",
            "/workout-service/workout/777",
            200,
            invalid.to_string(),
        ),
    ])?;
    configure_connect(&f, &g.base)?;
    let http = HttpMcp::start(&f, 10)?;
    let args = json!({"start":"2026-10-26","end":"2026-11-01","user_request":"Import my week"});
    let before = http.content("sync_coach_v1", args.clone())?;
    assert!(
        !http.call(
            "sync_coach_v1",
            json!({"start":"2026-10-26","end":"2026-11-02","user_request":"Import my week"})
        )?["error"]
            .is_null()
    );
    assert!(!http.call("sync_coach_v1", args)?["error"].is_null());
    assert_eq!(
        http.content(
            "get_coach_v1",
            json!({"start":"2026-10-26","end":"2026-11-01"})
        )?["snapshot"],
        before["snapshot"]
    );
    g.finish()?;
    http.stop()
}

#[test]
fn coach_snapshot_and_library_rollback_together_on_storage_failure() -> Result<()> {
    use garmin_pace::workouts::{
        SqliteWorkouts, WorkoutRepository,
        coach::{Repository, document::SnapshotDocument},
    };
    let f = Fixture::new()?;
    f.init()?;
    let original = workout();
    let g = Server::start(vec![
        ("GET", PLANS, 200, plans()),
        ("GET", WEEK, 200, schedule(&original)),
        (
            "GET",
            "/workout-service/workout/777",
            200,
            original.to_string(),
        ),
    ])?;
    configure_connect(&f, &g.base)?;
    let http = HttpMcp::start(&f, 10)?;
    let before = http.content(
        "sync_coach_v1",
        json!({"start":"2026-10-26","end":"2026-11-01","user_request":"Import my week"}),
    )?;
    let doc: SnapshotDocument = serde_json::from_value(before["snapshot"].clone())?;
    let mut snapshot: garmin_pace::workouts::coach::Snapshot = doc.try_into()?;
    let repo = SqliteWorkouts::open(f.settings.database.clone())?;
    let original_source = repo.get(777)?;
    snapshot.sessions[0].workout.name = "Must roll back".into();
    let db = rusqlite::Connection::open(&f.settings.database.path)?;
    db.execute_batch("CREATE TRIGGER reject_coach BEFORE INSERT ON coach_snapshots BEGIN SELECT RAISE(ABORT,'fixture failure'); END;")?;
    assert!(repo.put_coach(&snapshot).is_err());
    assert_eq!(repo.get(777)?, original_source);
    assert_eq!(
        serde_json::to_value(SnapshotDocument::from(
            repo.get_coach(snapshot.window)?
                .context("snapshot retained")?
        ))?,
        before["snapshot"]
    );
    g.finish()?;
    http.stop()
}

#[test]
fn coach_save_with_lost_plan_link_stays_unverified() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let original = workout();
    let mut detached = original.clone();
    detached["trainingPlanId"] = Value::Null;
    let g = Server::start(vec![
        ("GET", PLANS, 200, plans()),
        ("GET", WEEK, 200, schedule(&original)),
        (
            "GET",
            "/workout-service/workout/777",
            200,
            original.to_string(),
        ),
        (
            "GET",
            "/workout-service/workout/777",
            200,
            original.to_string(),
        ),
        ("PUT", "/workout-service/workout/777", 200, "{}".into()),
        (
            "GET",
            "/workout-service/workout/777",
            200,
            detached.to_string(),
        ),
    ])?;
    configure_connect(&f, &g.base)?;
    let http = HttpMcp::start(&f, 10)?;
    http.content(
        "sync_coach_v1",
        json!({"start":"2026-10-26","end":"2026-11-01","user_request":"Import my week"}),
    )?;
    let source = http.content("get_workout_v1", json!({"id":777}))?;
    let draft=http.content("create_workout_draft_v1",json!({"source":{"id":777,"revision":source["revision"]},"workout":source["definition"],"explanation":"Test retention of Coach identity without changing exercise content."}))?;
    let result = http.call(
        "save_workout_draft_to_garmin_v1",
        json!({"id":draft["id"],"revision":draft["revision"],"user_request":"Run this test"}),
    )?;
    assert_eq!(result["error"]["data"]["outcome"], "uncertain");
    assert_eq!(
        http.content("get_workout_draft_v1", json!({"id":draft["id"]}))?["state"],
        "saved"
    );
    g.finish()?;
    http.stop()
}

#[test]
fn agent_edits_coach_percentage_workout_with_null_warmup_targets() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let original: Value = serde_json::from_str(include_str!("../fixtures/coach_push_5x5.json"))?;
    let mut updated = original.clone();
    updated["description"] = json!("Reviewed 5x5 session");
    let g = Server::start(vec![
        (
            "GET",
            "/workout-service/workout/777",
            200,
            original.to_string(),
        ),
        (
            "GET",
            "/workout-service/workout/777",
            200,
            original.to_string(),
        ),
        ("PUT", "/workout-service/workout/777", 200, "{}".into()),
        (
            "GET",
            "/workout-service/workout/777",
            200,
            updated.to_string(),
        ),
    ])?;
    configure_connect(&f, &g.base)?;
    let http = HttpMcp::start(&f, 10)?;
    http.content(
        "refresh_workout_v1",
        json!({"id":777,"user_request":"Read my Coach workout for editing"}),
    )?;
    let source = http.content("get_workout_v1", json!({"id":777}))?;
    assert!(source["read_only_reason"].is_null(), "{source}");
    let definition = source["definition"].clone();
    assert!(definition.is_object());
    let draft = http.content(
        "create_workout_draft_v1",
        json!({
        "source":{"id":777,"revision":source["revision"]},
        "workout":definition,"explanation":"Keep the original 5x5 at 70% and warmup."}),
    )?;
    let mut candidate = definition.clone();
    candidate["description"] = updated["description"].clone();
    let revised = http.content(
        "revise_workout_draft_v1",
        json!({
        "id":draft["id"],"revision":draft["revision"],"workout":candidate,
        "explanation":"Change only the description; preserve all loads and repetitions."}),
    )?;
    assert_eq!(revised["revision"], 2);
    let review = http.content("get_workout_draft_v1", json!({"id":draft["id"]}))?;
    assert_eq!(review["source"]["definition"], definition);
    let saved = http.content(
        "save_workout_draft_to_garmin_v1",
        json!({
        "id":draft["id"],"revision":2,"user_request":"Save this reviewed description"}),
    )?;
    assert_eq!(saved["state"], "verified");
    let after = http.content("get_workout_v1", json!({"id":777}))?;
    assert_eq!(after["definition"], candidate);
    assert_eq!(after["coach_plan_id"], 42);
    let writes = g
        .writes
        .lock()
        .map_err(|_| anyhow::anyhow!("write mutex"))?;
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0]["workoutId"], 777);
    assert_eq!(writes[0]["trainingPlanId"], 42);
    let steps = writes[0]["workoutSegments"][0]["workoutSteps"]
        .as_array()
        .context("steps")?;
    assert_eq!(steps[0]["targetType"]["workoutTargetTypeKey"], "no.target");
    let groups: Vec<_> = steps
        .iter()
        .filter(|s| s["numberOfIterations"] == 5)
        .collect();
    assert_eq!(groups.len(), 2);
    for group in groups {
        let lift = &group["workoutSteps"][0];
        assert_eq!(lift["endConditionValue"], 5);
        assert_eq!(lift["benchmarkPercentage"], 70);
        assert!(lift["weightValue"].is_null());
        assert_eq!(group["workoutSteps"][1]["endConditionValue"], 180.0);
    }
    drop(writes);
    http.stop()?;
    g.finish()?;
    Ok(())
}
