//! tests/cases/health.rs verifies CLI health sync, SQLite atomicity and MCP local reads.
//! Garmin alone uses a local HTTP stand-in; persistence and application processes are real.

use crate::{
    common::Fixture,
    garmin::{Server, configure, configure_connect},
};
use anyhow::{Context, Result};
use garmin_pace::health::{
    Dataset, Date, DayImport, HealthError, HealthService, HealthSource, Limits, Measurement,
    Metric, Record, Sample, Section, SqliteHealth, Value as Scalar,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

const DATE: &str = "2026-10-24";
const TOKEN: (&str, &str, u16) = (
    "POST",
    "/personalapitoken-service/personal-api/personal-api-key/token",
    200,
);
fn script(readiness: Value) -> Vec<(&'static str, &'static str, u16, String)> {
    vec![
        (TOKEN.0, TOKEN.1, TOKEN.2, "fixture-access-token-never-real".into()),
        ("GET", "/usersummary-service/usersummary/daily?calendarDate=2026-10-24", 200, json!({"calendarDate":DATE,"totalSteps":4321,"restingHeartRate":55,"hydrationValueInML":1500}).to_string()),
        ("GET", "/sleep-service/sleep/dailySleep?date=2026-10-24", 200, json!({"calendarDate":DATE,"sleepTimeSeconds":27000,"deepSleepSeconds":5000,"sleepScores":{"overall":{"value":85}}}).to_string()),
        ("GET", "/hrv-service/hrv/2026-10-24", 204, String::new()),
        ("GET", "/wellness-service/wellness/dailyHeartRate?date=2026-10-24", 200, json!({"calendarDate":DATE,"restingHeartRate":55,"heartRateValueDescriptors":[{"index":1,"key":"timestamp"},{"index":0,"key":"heartrate"}],"heartRateValues":[[62,1792800000000i64],[null,1792800060000i64]]}).to_string()),
        ("GET", "/wellness-service/wellness/bodyBattery/reports/daily?startDate=2026-10-24&endDate=2026-10-24&chunks=720", 200, "[]".into()),
        ("GET", "/wellness-service/wellness/daily/spo2acclimation/2026-10-24", 200, json!({"calendarDate":DATE,"averageSpO2":97}).to_string()),
        ("GET", "/weight-service/weight/range/2026-10-24/2026-10-24?includeAll=true", 200, json!({"dailyWeightSummaries":[]}).to_string()),
        ("GET", "/metrics-service/metrics/trainingreadiness/2026-10-24", 200, readiness.to_string()),
    ]
}
/// One day of minimal Garmin health answers, after the window's token exchange.
pub(super) fn day_script(date: &str) -> Vec<(&'static str, &'static str, u16, String)> {
    // The fixture server matches static paths; leaking a few test strings is bounded.
    let get = |path: String, body: String| -> (&'static str, &'static str, u16, String) {
        ("GET", Box::leak(path.into_boxed_str()), 200, body)
    };
    let day = json!({"calendarDate": date}).to_string();
    vec![
        get(
            format!("/usersummary-service/usersummary/daily?calendarDate={date}"),
            json!({"calendarDate":date,"totalSteps":1000}).to_string(),
        ),
        get(
            format!("/sleep-service/sleep/dailySleep?date={date}"),
            day.clone(),
        ),
        (
            "GET",
            Box::leak(format!("/hrv-service/hrv/{date}").into_boxed_str()),
            204,
            String::new(),
        ),
        get(
            format!("/wellness-service/wellness/dailyHeartRate?date={date}"),
            day.clone(),
        ),
        get(
            format!(
                "/wellness-service/wellness/bodyBattery/reports/daily?startDate={date}&endDate={date}&chunks=720"
            ),
            "[]".into(),
        ),
        get(
            format!("/wellness-service/wellness/daily/spo2acclimation/{date}"),
            day,
        ),
        get(
            format!("/weight-service/weight/range/{date}/{date}?includeAll=true"),
            json!({"dailyWeightSummaries":[]}).to_string(),
        ),
        get(
            format!("/metrics-service/metrics/trainingreadiness/{date}"),
            "[]".into(),
        ),
    ]
}
fn sync(f: &Fixture) -> Result<std::process::Output> {
    Ok(f.command()
        .env("GARMIN_PERSONAL_API_KEY", "fixture-api-key-never-real")
        .args(["health", "sync", "--start", DATE, "--end", DATE])
        .output()?)
}
fn read(f: &Fixture, args: &[&str]) -> Result<Value> {
    let output = f.command().args(args).output()?;
    anyhow::ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

#[test]
fn health_sync_reads_all_sources_persists_gaps_and_preserves_failed_day() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.init()?;
    let readiness = json!([
        {"calendarDate":DATE,"timestamp":"2026-10-24T06:00:00","score":70,"inputContext":"AFTER_WAKEUP_RESET","recoveryTime":90},
        {"calendarDate":DATE,"timestamp":"2026-10-24T12:00:00Z","score":65,"inputContext":"UPDATE","hrvFactorPercent":80}
    ]);
    let mut exchanges = script(readiness);
    exchanges.extend(script(
        json!({"calendarDate":"2026-10-23","timestamp":"2026-10-23T06:00:00Z","score":10}),
    ));
    let server = Server::start(exchanges)?;
    configure(&fixture, &server.base)?;
    configure_connect(&fixture, &server.base)?;
    let synced = sync(&fixture)?;
    assert!(
        synced.status.success(),
        "{}",
        String::from_utf8_lossy(&synced.stderr)
    );
    let day = read(&fixture, &["health", "show", "--date", DATE])?;
    assert_eq!(
        day["datasets"]["daily"]["records"][0]["values"]["steps"]["value"],
        4321.0
    );
    assert_eq!(
        day["datasets"]["sleep"]["records"][0]["values"]["score"]["value"],
        85.0
    );
    assert_eq!(day["datasets"]["hrv"]["status"], "no_data");
    assert_eq!(
        day["datasets"]["training_readiness"]["records"]
            .as_array()
            .context("records")?
            .len(),
        2
    );
    assert_eq!(
        day["datasets"]["training_readiness"]["records"][0]["observed_at"],
        "2026-10-24T06:00:00.000Z"
    );
    let points = read(
        &fixture,
        &[
            "health",
            "samples",
            "--date",
            DATE,
            "--metric",
            "heart_rate",
            "--limit",
            "1",
        ],
    )?;
    assert_eq!(points["total"], 2);
    assert_eq!(points["next_offset"], 1);
    assert_eq!(points["samples"][0]["value"], 62.0);
    let gap = read(
        &fixture,
        &[
            "health",
            "samples",
            "--date",
            DATE,
            "--metric",
            "heart_rate",
            "--limit",
            "1",
            "--offset",
            "1",
        ],
    )?;
    assert!(gap["samples"][0]["value"].is_null());
    assert!(gap["next_offset"].is_null());
    let failed = sync(&fixture)?;
    assert!(!failed.status.success());
    let error = String::from_utf8(failed.stderr)?;
    assert!(
        error.contains("previous data preserved") && error.contains("calendar date differs"),
        "{error}"
    );
    assert_eq!(read(&fixture, &["health", "show", "--date", DATE])?, day);
    let missing = read(&fixture, &["health", "show", "--date", "2026-10-23"])?;
    assert_eq!(missing["status"], "not_synced");
    for args in [
        vec!["health", "show", "--date", "2026-02-30"],
        vec!["health", "list", "--start", "2026-01-01", "--end", DATE],
        vec![
            "health",
            "samples",
            "--date",
            DATE,
            "--metric",
            "heart_rate",
            "--limit",
            "0",
        ],
    ] {
        assert!(!fixture.command().args(args).output()?.status.success());
    }
    server.finish()
}

pub(super) fn seed_health(fixture: &Fixture, steps: f64) -> Result<()> {
    struct Source(f64);
    impl HealthSource for Source {
        fn day(&self, _: Date) -> Result<DayImport, HealthError> {
            Ok(DayImport {
                datasets: Metric::ALL.map(|metric| Dataset {
                    section: Section {
                        metric,
                        records: if metric == Metric::Daily {
                            vec![Record {
                                observed_at: None,
                                values: BTreeMap::from([(
                                    "steps".into(),
                                    Measurement {
                                        value: Scalar::Number(self.0),
                                        unit: Some("steps".into()),
                                    },
                                )]),
                            }]
                        } else {
                            vec![]
                        },
                        sample_count: usize::from(metric == Metric::HeartRate),
                    },
                    samples: if metric == Metric::HeartRate {
                        vec![Sample {
                            timestamp: "2026-10-24T08:00:00.000Z".into(),
                            value: Some(60.0),
                            unit: "bpm".into(),
                        }]
                    } else {
                        vec![]
                    },
                }),
            })
        }
    }
    let repository = SqliteHealth::open(fixture.settings.database.clone())?;
    let limits = Limits {
        max_days: 31,
        max_samples_per_day: 20000,
        max_sample_page: 500,
        max_records_per_metric: 64,
    };
    let service = HealthService::new(Arc::new(repository), limits);
    service.sync_day(&Source(steps), DATE.parse()?, || {
        "2026-10-24T12:00:00.000Z".into()
    })?;
    Ok(())
}

#[test]
fn health_coverage_counts_days_with_data_per_metric() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let service = HealthService::new(
        Arc::new(SqliteHealth::open(f.settings.database.clone())?),
        f.settings.health.limits(),
    );
    let empty = service.coverage()?;
    assert_eq!(
        empty.iter().map(|c| c.metric).collect::<Vec<_>>(),
        Metric::ALL.to_vec()
    );
    assert!(
        empty
            .iter()
            .all(|c| c.day_count == 0 && c.latest.is_none() && c.sample_count == 0)
    );
    seed_health(&f, 100.0)?;
    let coverage = service.coverage()?;
    let date: Date = DATE.parse()?;
    let daily = &coverage[0];
    assert_eq!(
        (
            daily.day_count,
            daily.earliest,
            daily.latest,
            daily.sample_count
        ),
        (1, Some(date), Some(date), 0)
    );
    let heart = coverage
        .iter()
        .find(|c| c.metric == Metric::HeartRate)
        .context("heart rate coverage")?;
    assert_eq!(
        (heart.day_count, heart.latest, heart.sample_count),
        (1, Some(date), 1)
    );
    assert_eq!(coverage.iter().filter(|c| c.day_count == 0).count(), 6);
    Ok(())
}

#[test]
fn health_transaction_rolls_back_samples_and_document_together() -> Result<()> {
    let f = Fixture::new()?;
    f.init()?;
    let service = HealthService::new(
        Arc::new(SqliteHealth::open(f.settings.database.clone())?),
        f.settings.health.limits(),
    );
    let empty = service.summary()?;
    assert_eq!((empty.day_count, empty.latest), (0, None));
    seed_health(&f, 100.0)?;
    let old = read(&f, &["health", "show", "--date", DATE])?;
    let connection = rusqlite::Connection::open(&f.settings.database.path)?;
    connection.execute_batch("CREATE TRIGGER fail_samples BEFORE INSERT ON health_samples BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;")?;
    assert!(seed_health(&f, 999.0).is_err());
    assert_eq!(read(&f, &["health", "show", "--date", DATE])?, old);
    let summary = service.summary()?;
    assert_eq!(
        (summary.day_count, summary.latest),
        (1, Some(DATE.parse()?))
    );
    assert_eq!(
        read(
            &f,
            &[
                "health",
                "samples",
                "--date",
                DATE,
                "--metric",
                "heart_rate",
                "--limit",
                "1"
            ]
        )?["total"],
        1
    );
    let plan: String = connection.query_row("EXPLAIN QUERY PLAN SELECT timestamp FROM health_samples WHERE date=?1 AND metric=?2 ORDER BY sequence LIMIT 10", [DATE,"heart_rate"], |r| r.get(3))?;
    assert!(plan.contains("PRIMARY KEY"), "{plan}");
    Ok(())
}
