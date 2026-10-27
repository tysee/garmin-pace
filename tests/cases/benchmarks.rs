//! tests/cases/benchmarks.rs verifies evidence, authorization triggers and durable Garmin saves.
//! Real SQLite and CLI processes run end to end; only Garmin's external HTTP API is substituted.

use crate::{
    common::Fixture,
    garmin::{KEY, Server, TOKEN, TOKEN_PATH, configure, configure_connect},
};
use anyhow::{Context as _, Result, ensure};
use garmin_pace::workouts::{
    DraftState, SqliteWorkouts, WorkoutError, WorkoutService,
    benchmarks::{
        document::{ContextDocument, ProposalDocument},
        *,
    },
    document::WorkoutDocument,
};
use serde_json::{Value, json};
use std::{fs, sync::Arc};

pub(super) const CATALOG: &str = "/workout-service/benchmarks";
pub(super) const ACTIVITIES: &str = "/activitylist-service/activities/search/activities?start=0&limit=50&startDate=2026-10-19&endDate=2026-10-25&activityType=fitness_equipment";
pub(super) const SETS: &str = "/activity-service/activity/501/exerciseSets";
pub(super) fn window() -> Result<Window> {
    Ok(Window {
        start: "2026-10-20".parse()?,
        end: "2026-10-24".parse()?,
    })
}
fn key() -> ExerciseKey {
    ExerciseKey {
        category: "ROW".into(),
        name: "BARBELL_ROW".into(),
    }
}
fn source(filled: bool) -> Benchmark {
    Benchmark {
        id: filled.then_some(91),
        exercise: key(),
        weight_kg: filled.then_some(40.0),
        repetitions: filled.then_some(8),
        one_rep_max_kg: filled.then_some(50.0),
        linked: vec![key()],
        revision: "a".repeat(64),
        synced_at: "2026-10-24T10:00:00Z".into(),
    }
}
fn set(position: usize, weight: f64, reps: u32) -> ObservedSet {
    ObservedSet {
        position,
        active: true,
        exercise: Some(key()),
        category: None,
        weight_kg: Some(weight),
        repetitions: Some(reps),
    }
}
fn observed() -> Result<StrengthImport> {
    let mut other = set(3, 100.0, 10);
    other.exercise = Some(ExerciseKey {
        category: "SQUAT".into(),
        name: "BARBELL_BACK_SQUAT".into(),
    });
    let mut rest = set(4, 0.0, 0);
    rest.active = false;
    rest.exercise = None;
    let mut ambiguous = set(5, 80.0, 10);
    ambiguous.exercise = None;
    ambiguous.category = Some("ROW".into());
    Ok(StrengthImport {
        window: window()?,
        synced_at: "2026-10-24T10:00:00Z".into(),
        sessions: vec![StrengthSession {
            activity_id: 501,
            date: "2026-10-23".parse()?,
            started_at: "2026-10-23T10:00:00Z".into(),
            synced_at: "2026-10-24T10:00:00Z".into(),
            sets: vec![
                set(0, 40.0, 8),
                set(1, 60.0, 4),
                set(2, 0.0, 8),
                other,
                rest,
                ambiguous,
                set(6, 50.0, 30),
            ],
        }],
    })
}
pub(super) fn proposal(c: &Context) -> Proposal {
    Proposal{origin:ProposalOrigin::UserRequest{request:"Estimate my barbell row benchmark from recent observed sets".into()},key:"BARBELL_ROW".into(),source_revision:c.evidence.source.revision.clone(),context_revision:c.evidence.revision.clone(),window:c.evidence.window,weight_kg:50.0,repetitions:6,one_rep_max_kg:58.1,method:"Brzycki from representative mean load and repetitions: 50 * 36 / (37 - 6), rounded to 0.1 kg".into(),rationale:"Two recent sets average 50 kg and 6 repetitions; limited evidence, no current recovery data available".into(),confidence:"low".into()}
}
/// Store the activity summary that the imported strength session 501 belongs to.
pub(super) fn seed_activity(f: &Fixture) -> Result<()> {
    rusqlite::Connection::open(&f.settings.database.path)?.execute(
        "INSERT INTO activities VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            "501",
            "Back day",
            "strength_training",
            "2026-10-23T10:00:00Z",
            3600.0,
            Option::<f64>::None
        ],
    )?;
    Ok(())
}
pub(super) fn seed(f: &Fixture, filled: bool) -> Result<(BenchmarkService, Proposal)> {
    let repo = Arc::new(SqliteBenchmarks::open(f.settings.database.clone())?);
    repo.import(&[source(filled)], &observed()?)?;
    let service = BenchmarkService::new(repo);
    let p = proposal(&service.context("BARBELL_ROW", window()?)?);
    Ok((service, p))
}
fn workout(f: &Fixture) -> Result<garmin_pace::workouts::Draft> {
    let doc: WorkoutDocument = serde_json::from_value(
        json!({"version":1,"name":"Strength session","sport":"strength_training","description":"Rows with reviewed loads",
        "segments":[{"sport":"strength_training","steps":[{"type":"step","kind":"interval","duration":{"type":"repetitions","count":6},"target":{"type":"none"},"exercise":{"category":"ROW","name":"BARBELL_ROW","weight_kg":50}}]}]}),
    )?;
    Ok(
        WorkoutService::new(Arc::new(SqliteWorkouts::open(f.settings.database.clone())?))
            .propose(doc.try_into()?, None)?,
    )
}
pub(super) fn invoke(f: &Fixture, args: &[&str]) -> Result<Value> {
    let output = f
        .command()
        .env("GARMIN_PERSONAL_API_KEY", KEY)
        .arg("benchmark")
        .args(args)
        .output()?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    ensure!(
        !String::from_utf8_lossy(&output.stderr).contains("ERROR"),
        "runtime error"
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}
fn save_file(f: &Fixture, p: &Proposal) -> Result<String> {
    let path = f.directory.path().join("benchmark.json");
    fs::write(
        &path,
        serde_json::to_vec(&ProposalDocument::from(p.clone()))?,
    )?;
    Ok(path.to_str().context("path")?.into())
}
#[test]
fn observed_means_are_exact_exercise_only_and_missing_is_not_zero() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let (s, p) = seed(&f, false)?;
    let c = s.context("BARBELL_ROW", window()?)?;
    assert_eq!(
        (
            c.sample_count,
            c.session_count,
            c.excluded_sets,
            c.unrelated_sets
        ),
        (2, 1, 4, 1)
    );
    assert_eq!(c.mean_weight_kg, Some(50.0));
    assert_eq!(c.mean_repetitions, Some(6.0));
    assert_eq!(c.evidence.sessions[0].sets.len(), 2);
    let d = s.propose(p.clone())?;
    assert_eq!(d.state, DraftState::Draft);
    assert_eq!(s.draft(d.id)?.context.excluded_sets, 4);
    let revised = s.revise(d.id, 1, p.clone())?;
    assert_eq!(revised.revision, 2);
    assert!(s.revise(d.id, 1, p).is_err());
    s.discard(d.id, 2)?;
    assert_eq!(s.draft(d.id)?.state, DraftState::Discarded);
    let w = Window {
        start: "2026-10-22".parse()?,
        end: "2026-10-22".parse()?,
    };
    let c = s.context("BARBELL_ROW", w)?;
    assert!(c.evidence.synced_at.is_some());
    assert_eq!(c.mean_weight_kg, None);
    assert_eq!(c.sample_count, 0);
    let mut p = proposal(&c);
    p.window = w;
    assert!(s.propose(p).is_err());
    let c = s.context(
        "BARBELL_ROW",
        Window {
            start: "2026-10-19".parse()?,
            end: window()?.end,
        },
    )?;
    assert!(c.evidence.synced_at.is_none());
    assert_eq!(c.sample_count, 2);
    let mut p = proposal(&c);
    p.window = c.evidence.window;
    assert!(s.propose(p).is_err());
    Ok(())
}
#[test]
fn activity_sets_report_import_coverage_instead_of_zero() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let (s, _) = seed(&f, false)?;
    let stored = s.activity_sets(501, "2026-10-23".parse()?)?;
    let session = stored.session.context("imported session")?;
    assert_eq!(session.sets.len(), 7);
    assert_eq!(session.sets[1].weight_kg, Some(60.0));
    assert!(!session.sets[4].active);
    assert_eq!(
        stored.import_synced_at.as_deref(),
        Some("2026-10-24T10:00:00Z")
    );
    // A covered date without a session for this activity is explicit, not an empty session.
    let covered = s.activity_sets(502, "2026-10-22".parse()?)?;
    assert!(covered.session.is_none() && covered.import_synced_at.is_some());
    let uncovered = s.activity_sets(502, "2026-10-10".parse()?)?;
    assert!(uncovered.session.is_none() && uncovered.import_synced_at.is_none());
    assert!(s.activity_sets(0, "2026-10-23".parse()?).is_err());
    let document = serde_json::to_value(
        garmin_pace::workouts::benchmarks::document::ActivitySetsDocument::from(uncovered),
    )?;
    assert_eq!(document["status"], "not_synced");
    Ok(())
}
#[test]
fn proactive_estimate_requires_unset_benchmark_and_matching_current_workout() -> Result<()> {
    for filled in [false, true] {
        let f = Fixture::new()?;
        f.init()?;
        let (s, mut p) = seed(&f, filled)?;
        let d = workout(&f)?;
        p.origin = ProposalOrigin::MissingForWorkout {
            draft_id: d.id,
            draft_revision: d.revision,
            reason: "This row session needs an estimated baseline to select its working load"
                .into(),
        };
        if filled {
            assert!(
                s.propose(p.clone())
                    .err()
                    .context("proactive filled proposal must fail")?
                    .to_string()
                    .contains("explicit user request")
            );
        } else {
            assert_eq!(s.propose(p.clone())?.state, DraftState::Draft);
        }
        p.origin = ProposalOrigin::UserRequest {
            request: "Update my existing benchmark from these recent sets".into(),
        };
        assert!(s.propose(p.clone()).is_ok());
        if filled {
            continue;
        }
        let w = WorkoutService::new(Arc::new(SqliteWorkouts::open(f.settings.database.clone())?));
        let revised = w.revise(d.id, d.revision, d.workout.clone())?;
        p.origin = ProposalOrigin::MissingForWorkout {
            draft_id: d.id,
            draft_revision: 1,
            reason: "Needed by this workout".into(),
        };
        assert!(matches!(s.propose(p.clone()), Err(WorkoutError::Conflict)));
        w.discard(d.id, revised.revision)?;
        p.origin = ProposalOrigin::MissingForWorkout {
            draft_id: d.id,
            draft_revision: revised.revision,
            reason: "Needed by this workout".into(),
        };
        assert!(s.propose(p.clone()).is_err());
        crate::workouts::seed_draft(&f)?;
        p.origin = ProposalOrigin::MissingForWorkout {
            draft_id: 2,
            draft_revision: 1,
            reason: "A run cannot justify a barbell benchmark".into(),
        };
        assert!(s.propose(p).is_err());
    }
    Ok(())
}
#[test]
fn stale_evidence_and_invalid_model_contract_are_rejected() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let (s, p) = seed(&f, false)?;
    let repo = SqliteBenchmarks::open(f.settings.database)?;
    let mut import = observed()?;
    import.sessions[0].sets[0].weight_kg = Some(70.0);
    repo.import(&[source(false)], &import)?;
    assert!(matches!(s.propose(p.clone()), Err(WorkoutError::Conflict)));
    let mut doc = serde_json::to_value(ProposalDocument::from(p))?;
    doc["origin"] = json!({"type":"automatic_refresh"});
    assert!(serde_json::from_value::<ProposalDocument>(doc.clone()).is_err());
    doc["origin"] = json!({"type":"user_request","request":""});
    assert!(Proposal::try_from(serde_json::from_value::<ProposalDocument>(doc.clone())?).is_err());
    doc["origin"] = json!({"type":"user_request","request":"Update benchmark"});
    doc["version"] = json!(2);
    assert!(Proposal::try_from(serde_json::from_value::<ProposalDocument>(doc.clone())?).is_err());
    doc["version"] = json!(1);
    doc["weight_kg"] = json!(0);
    assert!(Proposal::try_from(serde_json::from_value::<ProposalDocument>(doc)?).is_err());
    Ok(())
}
pub(super) fn remote(filled: bool, weight: f64) -> Value {
    if !filled {
        return json!({"benchmarkKey":"BARBELL_ROW","benchmarkCategoryKey":"ROW"});
    }
    json!({"id":91,"benchmarkKey":"BARBELL_ROW","benchmarkCategoryKey":"ROW","maxWeight":weight,"reps":6,"oneRepMax":58.1,"weightUnit":{"unitKey":"kilogram","unitId":8,"factor":1000},"linkedBenchmarkExercises":[{"categoryKey":"ROW","exerciseKey":"BARBELL_ROW"}]})
}
pub(super) fn activity() -> Value {
    json!([{"activityId":500,"activityName":"Rowing machine","activityType":{"typeKey":"indoor_rowing"},"startTimeGMT":"2026-10-22 10:00:00","duration":600,"distance":2000.0},
    {"activityId":501,"activityName":"Observed rows","activityType":{"typeKey":"strength_training"},"startTimeGMT":"2026-10-23 10:00:00","duration":600,"distance":null}])
}
pub(super) fn sets() -> Value {
    json!({"activityId":501,"exerciseSets":[
    {"setType":"ACTIVE","exercises":[{"category":"ROW","name":"BARBELL_ROW"}],"weight":40000,"repetitionCount":8},
    {"setType":"ACTIVE","exercises":[{"category":"ROW","name":"BARBELL_ROW"}],"weight":60000,"repetitionCount":4},
    {"setType":"ACTIVE","exercises":[{"category":"BENCH_PRESS","name":null}],"weight":20000,"repetitionCount":10},
    {"setType":"ACTIVE","exercises":[{"category":"ROW","name":"BARBELL_ROW"},{"category":"ROW","name":"DUMBBELL_ROW"}],"weight":30000,"repetitionCount":10},
    {"setType":"ACTIVE","exercises":[{"category":"ROW","name":"BARBELL_ROW"},{"category":"BENCH_PRESS","name":null}],"weight":30000,"repetitionCount":10},
    {"setType":"REST","exercises":[],"weight":-1,"repetitionCount":null}]})
}
#[test]
fn strength_pagination_advances_over_sibling_sports() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let siblings: Vec<_> = (0..50)
        .map(|id| {
            let mut row = activity()[0].clone();
            row["activityId"] = json!(1000 + id);
            row
        })
        .collect();
    let server = Server::start(vec![
        ("POST", TOKEN_PATH, 200, TOKEN.into()),
        ("GET", CATALOG, 200, json!([remote(false, 0.0)]).to_string()),
        ("GET", ACTIVITIES, 200, json!(siblings).to_string()),
        (
            "GET",
            "/activitylist-service/activities/search/activities?start=50&limit=50&startDate=2026-10-19&endDate=2026-10-25&activityType=fitness_equipment",
            200,
            json!([activity()[1].clone()]).to_string(),
        ),
        ("GET", SETS, 200, sets().to_string()),
    ])?;
    configure(&f, &server.base)?;
    configure_connect(&f, &server.base)?;
    assert_eq!(
        invoke(
            &f,
            &["sync", "--start", "2026-10-20", "--end", "2026-10-24"]
        )?["sessions"],
        1
    );
    server.finish()
}
#[test]
fn cli_import_model_proposal_create_and_update_preserve_units_and_links() -> Result<()> {
    for filled in [false, true] {
        let f = Fixture::new()?;
        f.init()?;
        let mut before = remote(filled, 40.0);
        let mut after = remote(true, 50.0);
        if filled {
            before["dateSet"] = json!("2026-09-01T10:00:00.0");
            let extra = json!({"categoryKey":"ROW","exerciseKey":"WIDE_GRIP_BARBELL_ROW"});
            before["linkedBenchmarkExercises"]
                .as_array_mut()
                .context("benchmark links")?
                .push(extra.clone());
            after["linkedBenchmarkExercises"]
                .as_array_mut()
                .context("benchmark links")?
                .push(extra);
            after["linkedBenchmarkExercises"]
                .as_array_mut()
                .context("links")?
                .reverse();
        }
        let server = Server::start(vec![
            ("POST", TOKEN_PATH, 200, TOKEN.into()),
            ("GET", CATALOG, 200, json!([before.clone()]).to_string()),
            ("GET", ACTIVITIES, 200, activity().to_string()),
            ("GET", SETS, 200, sets().to_string()),
            ("GET", CATALOG, 200, json!([before]).to_string()),
            (
                if filled { "PUT" } else { "POST" },
                if filled {
                    "/workout-service/benchmarks/91"
                } else {
                    CATALOG
                },
                if filled { 204 } else { 200 },
                if filled { String::new() } else { "91".into() },
            ),
            ("GET", CATALOG, 200, json!([after]).to_string()),
        ])?;
        configure(&f, &server.base)?;
        configure_connect(&f, &server.base)?;
        assert_eq!(
            invoke(
                &f,
                &["sync", "--start", "2026-10-20", "--end", "2026-10-24"]
            )?["sessions"],
            1
        );
        let context = invoke(
            &f,
            &[
                "context",
                "--key",
                "BARBELL_ROW",
                "--start",
                "2026-10-20",
                "--end",
                "2026-10-24",
            ],
        )?;
        assert_eq!(context["mean_weight_kg"], 50.0);
        assert_eq!(context["mean_repetitions"], 6.0);
        assert_eq!(context["excluded_sets"], 4);
        // Sets without one exact exercise keep an agreed category, or nothing when mixed.
        let imported = SqliteBenchmarks::open(f.settings.database.clone())?
            .activity_sets(501, "2026-10-23".parse()?)?
            .session
            .context("imported session")?;
        let categories: Vec<_> = imported
            .sets
            .iter()
            .map(|s| (s.exercise.is_some(), s.category.as_deref()))
            .collect();
        assert_eq!(
            categories,
            [
                (true, None),
                (true, None),
                (false, Some("BENCH_PRESS")),
                (false, Some("ROW")),
                (false, None),
                (false, None)
            ]
        );
        let c: Context = serde_json::from_value::<ContextDocument>(context)?.try_into()?;
        let path = save_file(&f, &proposal(&c))?;
        let d = invoke(&f, &["propose", "--file", &path])?;
        assert_eq!(d["state"], "draft");
        assert_eq!(
            invoke(&f, &["review", "--id", "1"])?["proposal"]["one_rep_max_kg"],
            58.1
        );
        assert_eq!(
            invoke(&f, &["save", "--id", "1", "--revision", "1"])?["state"],
            "verified"
        );
        let writes = server
            .writes
            .lock()
            .map_err(|_| anyhow::anyhow!("writes lock"))?;
        assert_eq!(writes.len(), 1);
        assert!(writes[0].get("id").is_none());
        assert!(writes[0].get("dateSet").is_none());
        assert_eq!(writes[0]["maxWeight"], 50.0);
        assert_eq!(writes[0]["oneRepMax"], 58.1);
        assert_eq!(writes[0]["weightUnit"]["unitKey"], "kilogram");
        let mut expected_links = vec![json!({"categoryKey":"ROW","exerciseKey":"BARBELL_ROW"})];
        if filled {
            expected_links.push(json!({"categoryKey":"ROW","exerciseKey":"WIDE_GRIP_BARBELL_ROW"}));
        }
        assert_eq!(writes[0]["linkedBenchmarkExercises"], json!(expected_links));
        drop(writes);
        server.finish()?;
    }
    Ok(())
}
#[test]
fn uncertain_save_is_never_retried_and_can_be_reconciled() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let before = remote(false, 0.0);
    let server = Server::start(vec![
        ("POST", TOKEN_PATH, 200, TOKEN.into()),
        ("GET", CATALOG, 200, json!([before.clone()]).to_string()),
        ("GET", ACTIVITIES, 200, activity().to_string()),
        ("GET", SETS, 200, sets().to_string()),
        ("GET", CATALOG, 200, json!([before]).to_string()),
        ("POST", CATALOG, 503, "{}".into()),
        ("GET", CATALOG, 200, json!([remote(true, 50.0)]).to_string()),
    ])?;
    configure(&f, &server.base)?;
    configure_connect(&f, &server.base)?;
    invoke(
        &f,
        &["sync", "--start", "2026-10-20", "--end", "2026-10-24"],
    )?;
    let s = BenchmarkService::new(Arc::new(SqliteBenchmarks::open(
        f.settings.database.clone(),
    )?));
    s.propose(proposal(&s.context("BARBELL_ROW", window()?)?))?;
    for _ in 0..2 {
        assert!(
            !f.command()
                .args(["benchmark", "save", "--id", "1", "--revision", "1"])
                .output()?
                .status
                .success()
        );
    }
    assert_eq!(s.draft(1)?.state, DraftState::Saving);
    assert_eq!(
        invoke(&f, &["reconcile", "--id", "1"])?["state"],
        "verified"
    );
    assert_eq!(
        server
            .writes
            .lock()
            .map_err(|_| anyhow::anyhow!("writes lock"))?
            .len(),
        1
    );
    server.finish()
}
#[test]
fn failed_sync_preserves_catalog_and_previous_evidence_atomically() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let (s, _) = seed(&f, true)?;
    let before = s.context("BARBELL_ROW", window()?)?;
    let mut invalid = sets();
    invalid["activityId"] = json!(999);
    let server = Server::start(vec![
        ("POST", TOKEN_PATH, 200, TOKEN.into()),
        ("GET", CATALOG, 200, json!([remote(false, 0.0)]).to_string()),
        ("GET", ACTIVITIES, 200, activity().to_string()),
        ("GET", SETS, 200, invalid.to_string()),
    ])?;
    configure(&f, &server.base)?;
    configure_connect(&f, &server.base)?;
    assert!(
        !f.command()
            .env("GARMIN_PERSONAL_API_KEY", KEY)
            .args([
                "benchmark",
                "sync",
                "--start",
                "2026-10-20",
                "--end",
                "2026-10-24"
            ])
            .output()?
            .status
            .success()
    );
    assert_eq!(
        s.context("BARBELL_ROW", window()?)?.evidence.revision,
        before.evidence.revision
    );
    assert_eq!(s.catalog()?[0].id, Some(91));
    server.finish()
}
#[test]
fn benchmark_migration_preserves_existing_workout_drafts() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    crate::workouts::seed_draft(&f)?;
    let db = rusqlite::Connection::open(&f.settings.database.path)?;
    // A real schema-5 database has neither the benchmark tables nor the MCP request journal.
    db.execute_batch("DROP INDEX workout_library_standalone; DROP TABLE coach_snapshots; ALTER TABLE workout_drafts DROP COLUMN explanation; DROP INDEX activities_sport_chronological; DROP TABLE benchmark_drafts; DROP TABLE strength_imports; DROP TABLE strength_sessions; DROP TABLE benchmark_catalog; ALTER TABLE workout_drafts DROP COLUMN mcp_request; ALTER TABLE workout_deliveries DROP COLUMN mcp_request; PRAGMA user_version=5;")?;
    f.init()?;
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
        10
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM workout_drafts", [], |r| r
            .get::<_, i64>(0))?,
        1
    );
    Ok(())
}
