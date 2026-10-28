//! tests/cases/workouts.rs exercises model documents, real SQLite and the built CLI.
//! Garmin alone is a local HTTP fixture; writes are inspected independently of the decoder.

use crate::{
    common::Fixture,
    garmin::{Server, configure_connect},
};
use anyhow::{Context, Result, ensure};
use garmin_pace::workouts::{SqliteWorkouts, Workout, WorkoutService, document::WorkoutDocument};
use serde_json::{Value, json};
use std::{fs, sync::Arc};

pub(super) fn candidate(name: &str, seconds: f64) -> Value {
    json!({"version":1,"name":name,"sport":"running","description":"Controlled tempo session",
    "segments":[{"sport":"running","steps":[
        {"type":"step","kind":"warmup","duration":{"type":"time","seconds":600},"target":{"type":"none"}},
        {"type":"step","kind":"interval","duration":{"type":"time","seconds":seconds},"target":{"type":"range","unit":"pace_seconds_per_km","minimum":300,"maximum":330}},
        {"type":"step","kind":"cooldown","duration":{"type":"time","seconds":300},"target":{"type":"none"}}
    ]}]})
}
pub(super) fn remote_workout(name: &str, seconds: f64) -> Value {
    json!({"workoutId":777,"workoutName":name,"description":"Controlled tempo session","sportType":{"sportTypeKey":"running"},"ownerId":42,
    "workoutSegments":[{"segmentOrder":1,"sportType":{"sportTypeKey":"running"},"workoutSteps":[
        {"type":"ExecutableStepDTO","stepOrder":1,"stepType":{"stepTypeKey":"warmup"},"endCondition":{"conditionTypeKey":"time"},"endConditionValue":600,"targetType":{"workoutTargetTypeKey":"no.target"}},
        {"type":"ExecutableStepDTO","stepOrder":2,"stepType":{"stepTypeKey":"interval"},"endCondition":{"conditionTypeKey":"time"},"endConditionValue":seconds,"targetType":{"workoutTargetTypeKey":"pace.zone"},"targetValueOne":3.3333,"targetValueTwo":3.0303},
        {"type":"ExecutableStepDTO","stepOrder":3,"stepType":{"stepTypeKey":"cooldown"},"endCondition":{"conditionTypeKey":"time"},"endConditionValue":300,"targetType":{"workoutTargetTypeKey":"no.target"}}
    ]}]})
}
pub(super) fn seed_draft(fixture: &Fixture) -> Result<()> {
    let service = WorkoutService::new(Arc::new(SqliteWorkouts::open(
        fixture.settings.database.clone(),
    )?));
    let doc: WorkoutDocument = serde_json::from_value(candidate("Tempo session", 1200.0))?;
    service.propose(doc.try_into()?, None)?;
    Ok(())
}
fn invoke(fixture: &Fixture, args: &[&str]) -> Result<Value> {
    let output = fixture.command().arg("workout").args(args).output()?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}
fn file(fixture: &Fixture, value: &Value) -> Result<String> {
    let path = fixture.directory.path().join("model.json");
    fs::write(&path, serde_json::to_vec(value)?)?;
    Ok(path.to_str().context("file path")?.into())
}
#[test]
fn library_edit_review_revision_save_and_send_preserve_garmin_identity() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let original = remote_workout("Tempo session", 1200.0).to_string();
    let saved = remote_workout("Tempo adjusted", 1500.0).to_string();
    let mut unsupported = remote_workout("Swim with drill", 1200.0);
    unsupported["workoutId"] = json!(888);
    unsupported["workoutSegments"][0]["workoutSteps"][1]["newSemanticField"] =
        json!("must survive");
    let server = Server::start(vec![
        (
            "GET",
            "/workout-service/workouts?start=0&limit=20",
            200,
            json!([{"workoutId":777},{"workoutId":888}]).to_string(),
        ),
        ("GET", "/workout-service/workout/777", 200, original.clone()),
        (
            "GET",
            "/workout-service/workout/888",
            200,
            unsupported.to_string(),
        ),
        ("GET", "/workout-service/workout/777", 200, original),
        ("PUT", "/workout-service/workout/777", 200, "{}".into()),
        ("GET", "/workout-service/workout/777", 200, saved.clone()),
        ("GET", "/workout-service/workout/777", 200, saved),
        (
            "GET",
            "/device-service/deviceregistration/devices",
            200,
            json!([{"unitId":99,"productDisplayName":"fenix 8"}]).to_string(),
        ),
        (
            "POST",
            "/device-service/devicemessage/messages",
            200,
            "[]".into(),
        ),
    ])?;
    configure_connect(&fixture, &server.base)?;
    assert_eq!(invoke(&fixture, &["sync"])?["received"], 2);
    let library = invoke(&fixture, &["list"])?;
    assert_eq!(library["workouts"].as_array().context("workouts")?.len(), 2);
    let source = invoke(&fixture, &["show", "--id", "777"])?;
    assert!(source["definition"].is_object());
    let readonly = invoke(&fixture, &["show", "--id", "888"])?;
    assert!(readonly["definition"].is_null());
    assert!(
        readonly["read_only_reason"]
            .as_str()
            .context("reason")?
            .contains("newSemanticField")
    );
    let path = file(&fixture, &candidate("Tempo adjusted", 1500.0))?;
    let revision = source["revision"].as_str().context("revision")?;
    let draft = invoke(
        &fixture,
        &[
            "draft",
            "--file",
            &path,
            "--source-id",
            "777",
            "--source-revision",
            revision,
        ],
    )?;
    assert_eq!(draft["source"]["id"], 777);
    assert_eq!(draft["state"], "draft");
    let revised = invoke(
        &fixture,
        &["revise", "--id", "1", "--revision", "1", "--file", &path],
    )?;
    assert_eq!(revised["revision"], 2);
    assert!(
        !fixture
            .command()
            .args(["workout", "save", "--id", "1", "--revision", "1"])
            .output()?
            .status
            .success()
    );
    let saved = invoke(&fixture, &["save", "--id", "1", "--revision", "2"])?;
    assert_eq!(saved["state"], "verified");
    assert_eq!(saved["remote_id"], 777);
    assert!(
        !fixture
            .command()
            .args(["workout", "save", "--id", "1", "--revision", "2"])
            .output()?
            .status
            .success()
    );
    assert_eq!(
        invoke(&fixture, &["send", "--id", "777", "--device-id", "99"])?["watch_receipt"],
        "unconfirmed"
    );
    let writes = server
        .writes
        .lock()
        .map_err(|_| anyhow::anyhow!("writes lock"))?
        .clone();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0]["workoutId"], 777);
    assert_eq!(writes[0]["ownerId"], 42);
    assert_eq!(writes[0]["workoutName"], "Tempo adjusted");
    let step = &writes[0]["workoutSegments"][0]["workoutSteps"][1];
    assert_eq!(step["endConditionValue"], 1500.0);
    assert!((step["targetValueOne"].as_f64().context("pace")? - 1000.0 / 300.0).abs() < 1e-8);
    assert!((step["targetValueTwo"].as_f64().context("pace")? - 1000.0 / 330.0).abs() < 1e-8);
    server.finish()
}
#[test]
fn rejected_writes_return_to_draft_and_unresolved_states_can_be_closed() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let remote = remote_workout("Tempo session", 1200.0).to_string();
    let devices = json!([{"unitId":99,"productDisplayName":"fenix 8"}]).to_string();
    let server = Server::start(vec![
        ("POST", "/workout-service/workout", 400, "{}".into()),
        (
            "POST",
            "/workout-service/workout",
            200,
            json!({"workoutId":777}).to_string(),
        ),
        ("GET", "/workout-service/workout/777", 200, remote.clone()),
        ("GET", "/workout-service/workout/777", 200, remote.clone()),
        (
            "GET",
            "/device-service/deviceregistration/devices",
            200,
            devices.clone(),
        ),
        (
            "POST",
            "/device-service/devicemessage/messages",
            400,
            "{}".into(),
        ),
        ("GET", "/workout-service/workout/777", 200, remote),
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
        (
            "POST",
            "/workout-service/workout",
            503,
            "unavailable".into(),
        ),
    ])?;
    configure_connect(&fixture, &server.base)?;
    let path = file(&fixture, &candidate("Tempo session", 1200.0))?;
    invoke(&fixture, &["draft", "--file", &path])?;
    // A definite HTTP 400 is not an uncertain outcome: the draft stays editable and saveable.
    let rejected = fixture
        .command()
        .args(["workout", "save", "--id", "1", "--revision", "1"])
        .output()?;
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("HTTP 400"));
    assert_eq!(
        invoke(&fixture, &["review", "--id", "1"])?["state"],
        "draft"
    );
    assert_eq!(
        invoke(&fixture, &["save", "--id", "1", "--revision", "1"])?["state"],
        "verified"
    );
    // A rejected delivery request releases its reservation so the same revision can be sent again.
    assert!(
        !fixture
            .command()
            .args(["workout", "send", "--id", "777", "--device-id", "99"])
            .output()?
            .status
            .success()
    );
    assert_eq!(
        invoke(&fixture, &["send", "--id", "777", "--device-id", "99"])?["queued"],
        true
    );
    // An uncertain outcome still stays pending until the user explicitly closes it.
    invoke(&fixture, &["draft", "--file", &path])?;
    assert!(
        !fixture
            .command()
            .args(["workout", "save", "--id", "2", "--revision", "1"])
            .output()?
            .status
            .success()
    );
    assert_eq!(
        invoke(&fixture, &["review", "--id", "2"])?["state"],
        "saving"
    );
    assert_eq!(
        invoke(&fixture, &["abandon", "--id", "2", "--revision", "1"])?["state"],
        "discarded"
    );
    server.finish()
}
#[test]
fn new_workout_is_created_once_and_uncertain_write_requires_reconciliation() -> Result<()> {
    for failed in [false, true] {
        let fixture = Fixture::new()?;
        fixture.init()?;
        let mut script = vec![(
            "POST",
            "/workout-service/workout",
            if failed { 503 } else { 200 },
            if failed {
                "unavailable".into()
            } else {
                json!({"workoutId":777}).to_string()
            },
        )];
        script.push((
            "GET",
            "/workout-service/workout/777",
            200,
            remote_workout("Tempo session", 1200.0).to_string(),
        ));
        let server = Server::start(script)?;
        configure_connect(&fixture, &server.base)?;
        let path = file(&fixture, &candidate("Tempo session", 1200.0))?;
        let draft = invoke(&fixture, &["draft", "--file", &path])?;
        assert!(draft["source"].is_null());
        let output = fixture
            .command()
            .args(["workout", "save", "--id", "1", "--revision", "1"])
            .output()?;
        assert_eq!(
            output.status.success(),
            !failed,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !fixture
                .command()
                .args(["workout", "save", "--id", "1", "--revision", "1"])
                .output()?
                .status
                .success()
        );
        if failed {
            assert_eq!(
                invoke(&fixture, &["review", "--id", "1"])?["state"],
                "saving"
            );
            assert_eq!(
                invoke(&fixture, &["reconcile", "--id", "1", "--workout-id", "777"])?["state"],
                "verified"
            );
        }
        assert_eq!(
            invoke(&fixture, &["show", "--id", "777"])?["name"],
            "Tempo session"
        );
        assert_eq!(
            server
                .writes
                .lock()
                .map_err(|_| anyhow::anyhow!("writes lock"))?
                .len(),
            1
        );
        server.finish()?;
    }
    Ok(())
}
#[test]
fn changed_remote_source_blocks_update_and_keeps_draft_editable() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let server = Server::start(vec![
        (
            "GET",
            "/workout-service/workouts?start=0&limit=20",
            200,
            json!([{"workoutId":777}]).to_string(),
        ),
        (
            "GET",
            "/workout-service/workout/777",
            200,
            remote_workout("Tempo session", 1200.0).to_string(),
        ),
        (
            "GET",
            "/workout-service/workout/777",
            200,
            remote_workout("Edited on phone", 1800.0).to_string(),
        ),
    ])?;
    configure_connect(&fixture, &server.base)?;
    invoke(&fixture, &["sync"])?;
    let source = invoke(&fixture, &["show", "--id", "777"])?;
    let path = file(&fixture, &candidate("Tempo adjusted", 1500.0))?;
    invoke(
        &fixture,
        &[
            "draft",
            "--file",
            &path,
            "--source-id",
            "777",
            "--source-revision",
            source["revision"].as_str().context("revision")?,
        ],
    )?;
    assert!(
        !fixture
            .command()
            .args(["workout", "save", "--id", "1", "--revision", "1"])
            .output()?
            .status
            .success()
    );
    assert_eq!(
        invoke(&fixture, &["review", "--id", "1"])?["state"],
        "draft"
    );
    assert!(
        server
            .writes
            .lock()
            .map_err(|_| anyhow::anyhow!("writes lock"))?
            .is_empty()
    );
    server.finish()
}
#[test]
fn invalid_model_documents_and_stale_local_revisions_never_mutate_drafts() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    for mutate in 0..5 {
        let mut value = candidate("Tempo session", 1200.0);
        match mutate {
            0 => value["version"] = json!(2),
            1 => value["segments"][0]["steps"][1]["duration"]["seconds"] = json!(-1),
            2 => value["segments"][0]["steps"][1]["target"]["minimum"] = json!(500),
            3 => value["sport"] = json!("walking"),
            _ => value["segments"][0]["steps"][1]["surprise"] = json!(true),
        }
        let path = file(&fixture, &value)?;
        assert!(
            !fixture
                .command()
                .args(["workout", "draft", "--file", &path])
                .output()?
                .status
                .success()
        );
    }
    assert_eq!(invoke(&fixture, &["drafts"])?["drafts"], json!([]));
    seed_draft(&fixture)?;
    let path = file(&fixture, &candidate("Adjusted", 1500.0))?;
    invoke(
        &fixture,
        &["revise", "--id", "1", "--revision", "1", "--file", &path],
    )?;
    assert!(
        !fixture
            .command()
            .args(["workout", "discard", "--id", "1", "--revision", "1"])
            .output()?
            .status
            .success()
    );
    assert_eq!(
        invoke(&fixture, &["discard", "--id", "1", "--revision", "2"])?["state"],
        "discarded"
    );
    assert_eq!(invoke(&fixture, &["drafts"])?["drafts"], json!([]));
    for old in ["preview", "publish-test", "status"] {
        assert!(
            !fixture
                .command()
                .args(["workout", old, "--name", "old test"])
                .output()?
                .status
                .success()
        );
    }
    Ok(())
}
#[test]
fn repeated_step_contract_roundtrips_and_bounds_nesting() -> Result<()> {
    let mut value = candidate("Intervals", 600.0);
    let step = value["segments"][0]["steps"][1].clone();
    value["segments"][0]["steps"][1] =
        json!({"type":"repeat","count":4,"steps":[step],"skip_last_rest":false});
    let document: WorkoutDocument = serde_json::from_value(value.clone())?;
    let workout: Workout = document.try_into()?;
    let roundtrip: Workout = serde_json::from_value::<WorkoutDocument>(serde_json::to_value(
        WorkoutDocument::from(workout.clone()),
    )?)?
    .try_into()?;
    assert_eq!(workout, roundtrip);
    value["segments"][0]["steps"][1]["count"] = json!(0);
    assert!(Workout::try_from(serde_json::from_value::<WorkoutDocument>(value)?).is_err());
    Ok(())
}

#[test]
fn mismatched_readback_stays_unverified_and_cannot_be_saved_again() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    seed_draft(&fixture)?;
    let server = Server::start(vec![
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
            remote_workout("Unexpected contents", 1800.0).to_string(),
        ),
    ])?;
    configure_connect(&fixture, &server.base)?;
    assert!(
        !fixture
            .command()
            .args(["workout", "save", "--id", "1", "--revision", "1"])
            .output()?
            .status
            .success()
    );
    let draft = invoke(&fixture, &["review", "--id", "1"])?;
    assert_eq!(draft["state"], "saved");
    assert_eq!(draft["remote_id"], 777);
    assert!(
        !fixture
            .command()
            .args(["workout", "save", "--id", "1", "--revision", "1"])
            .output()?
            .status
            .success()
    );
    assert_eq!(invoke(&fixture, &["list"])?["workouts"], json!([]));
    server.finish()
}

#[test]
fn library_migration_preserves_existing_history_health_and_legacy_journal() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    fixture.seed()?;
    crate::health::seed_health(&fixture, 1234.0)?;
    let db = rusqlite::Connection::open(&fixture.settings.database.path)?;
    db.execute_batch("DROP TABLE sync_run_steps; DROP TABLE sync_run_windows; DROP TABLE sync_runs; DROP INDEX workout_library_standalone; DROP TABLE coach_snapshots; DROP INDEX activities_sport_chronological; DROP TABLE benchmark_drafts; DROP TABLE strength_imports; DROP TABLE strength_sessions; DROP TABLE benchmark_catalog; DROP TABLE workout_deliveries; DROP TABLE workout_drafts; DROP TABLE workout_library; PRAGMA user_version=4;")?;
    db.execute("INSERT INTO workout_attempts(name,state,workout_id,device_id) VALUES('legacy audit','verified',77,NULL)",[])?;
    fixture.init()?;
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
        11
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM activities", [], |r| r
            .get::<_, i64>(0))?,
        2
    );
    assert_eq!(
        db.query_row(
            "SELECT workout_id FROM workout_attempts WHERE name='legacy audit'",
            [],
            |r| r.get::<_, i64>(0)
        )?,
        77
    );
    assert_eq!(invoke(&fixture, &["drafts"])?["drafts"], json!([]));
    Ok(())
}

fn strength_candidate() -> Value {
    json!({"version":1,"name":"Back day","sport":"strength_training",
    "segments":[{"sport":"strength_training","steps":[
        {"type":"step","kind":"warmup","duration":{"type":"lap_button"},"target":{"type":"none"},
         "exercise":{"category":"CARDIO","name":""}},
        {"type":"repeat","count":3,"steps":[
            {"type":"step","kind":"interval","duration":{"type":"repetitions","count":10},"target":{"type":"none"},
             "exercise":{"category":"DEADLIFT","name":"BARBELL_DEADLIFT","benchmark_percentage":80,"benchmark_key":"BARBELL_DEADLIFT"}},
            {"type":"step","kind":"rest","duration":{"type":"lap_button"},"target":{"type":"none"}}
        ]}
    ]}]})
}
fn remote_strength_workout() -> Value {
    // Field shapes observed in Garmin Connect: null absolute load next to a benchmark share.
    json!({"workoutId":777,"workoutName":"Back day","description":"","sportType":{"sportTypeKey":"strength_training"},"ownerId":42,
    "workoutSegments":[{"segmentOrder":1,"sportType":{"sportTypeKey":"strength_training"},"workoutSteps":[
        {"type":"ExecutableStepDTO","stepOrder":1,"stepType":{"stepTypeKey":"warmup"},"endCondition":{"conditionTypeKey":"lap.button"},"endConditionValue":0.0,"endConditionCompare":"",
         "targetType":{"workoutTargetTypeKey":"no.target"},"category":"CARDIO","exerciseName":"","weightValue":-1.0,"weightUnit":{"unitKey":"kilogram"}},
        {"type":"RepeatGroupDTO","stepOrder":2,"stepType":{"stepTypeKey":"repeat"},"numberOfIterations":3,"endCondition":{"conditionTypeKey":"iterations"},"endConditionValue":3.0,"smartRepeat":false,"skipLastRestStep":false,
         "workoutSteps":[
            {"type":"ExecutableStepDTO","stepOrder":3,"stepType":{"stepTypeKey":"interval"},"endCondition":{"conditionTypeKey":"reps"},"endConditionValue":10.0,"endConditionCompare":"",
             "targetType":{"workoutTargetTypeKey":"no.target"},"category":"DEADLIFT","exerciseName":"BARBELL_DEADLIFT","weightValue":null,"weightUnit":{"unitKey":"kilogram"},
             "benchmarkPercentage":80,"benchmarkKey":"BARBELL_DEADLIFT"},
            {"type":"ExecutableStepDTO","stepOrder":4,"stepType":{"stepTypeKey":"rest"},"endCondition":{"conditionTypeKey":"lap.button"},"endConditionValue":0.0,"endConditionCompare":"",
             "targetType":{"workoutTargetTypeKey":"no.target"},"weightValue":-1.0,"weightUnit":{"unitKey":"kilogram"}}]}
    ]}]})
}
#[test]
fn benchmark_percentage_draft_saves_and_verifies_against_readback() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let server = Server::start(vec![
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
            remote_strength_workout().to_string(),
        ),
    ])?;
    configure_connect(&fixture, &server.base)?;
    let path = file(&fixture, &strength_candidate())?;
    let draft = invoke(&fixture, &["draft", "--file", &path])?;
    let lift = &draft["workout"]["segments"][0]["steps"][1]["steps"][0]["exercise"];
    assert_eq!(lift["benchmark_percentage"], 80);
    assert_eq!(lift["benchmark_key"], "BARBELL_DEADLIFT");
    assert!(lift["weight_kg"].is_null());
    let saved = invoke(&fixture, &["save", "--id", "1", "--revision", "1"])?;
    assert_eq!(saved["state"], "verified");
    assert_eq!(saved["remote_id"], 777);
    let shown = invoke(&fixture, &["show", "--id", "777"])?;
    assert!(shown["read_only_reason"].is_null());
    assert_eq!(
        shown["definition"]["segments"][0]["steps"][1]["steps"][0]["exercise"]["benchmark_percentage"],
        80
    );
    let writes = server
        .writes
        .lock()
        .map_err(|_| anyhow::anyhow!("writes lock"))?
        .clone();
    assert_eq!(writes.len(), 1);
    let step = &writes[0]["workoutSegments"][0]["workoutSteps"][1]["workoutSteps"][0];
    assert_eq!(step["benchmarkPercentage"], 80);
    assert_eq!(step["benchmarkKey"], "BARBELL_DEADLIFT");
    assert!(step.get("weightValue").is_some_and(Value::is_null));
    assert!(step["weightUnit"].is_null());
    // Half a benchmark load, or a load given twice, never becomes a draft.
    for mutate in 0..2 {
        let mut value = strength_candidate();
        let lift = &mut value["segments"][0]["steps"][1]["steps"][0]["exercise"];
        if mutate == 0 {
            lift["benchmark_key"] = Value::Null;
        } else {
            lift["weight_kg"] = json!(60.0);
        }
        let path = file(&fixture, &value)?;
        assert!(
            !fixture
                .command()
                .args(["workout", "draft", "--file", &path])
                .output()?
                .status
                .success()
        );
    }
    server.finish()
}
