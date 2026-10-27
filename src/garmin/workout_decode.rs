//! src/garmin/workout_decode.rs validates Garmin snapshots and computes opaque revision tokens.
//! Unsupported semantic fields make a workout read-only instead of disappearing on update; a
//! read-only workout still gets a display-only outline that notes those fields in step text.

use crate::workouts::{
    BenchmarkLoad, Duration, Exercise, LibraryWorkout, Segment, Step, Target, Workout,
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

pub(super) fn revision(value: &Value) -> Result<String> {
    let bytes = serde_json::to_vec(value)?;
    Ok(ring::digest::digest(&ring::digest::SHA256, &bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
pub(super) fn snapshot(value: &Value, id: u64) -> Result<LibraryWorkout> {
    ensure!(
        value["workoutId"].as_u64() == Some(id) && id > 0 && id <= i64::MAX as u64,
        "invalid Garmin workout identity"
    );
    let name = string(&value["workoutName"])?;
    ensure!(
        name.len() <= 500 && !name.chars().any(char::is_control),
        "invalid Garmin workout name"
    );
    let sport = string(&value["sportType"]["sportTypeKey"])?;
    ensure!(
        sport.len() <= 80 && !sport.chars().any(char::is_control),
        "invalid Garmin sport label"
    );
    let (definition, reason, outline) = match definition(value, Mode::Strict) {
        Ok(value) => (Some(value), None, None),
        // The outline is read with the same parser; unsupported semantics become step notes.
        Err(error) => (
            None,
            Some(error.to_string()),
            definition(value, Mode::Outline).ok(),
        ),
    };
    Ok(LibraryWorkout {
        id,
        coach_plan_id: match value.get("trainingPlanId").filter(|v| !v.is_null()) {
            None => None,
            Some(id) => Some(
                id.as_u64()
                    .filter(|n| *n > 0 && *n <= i64::MAX as u64)
                    .context("invalid Coach plan ID")?,
            ),
        },
        name,
        sport,
        revision: revision(value)?,
        synced_at: chrono::Utc::now().to_rfc3339(),
        definition,
        read_only_reason: reason,
        outline,
    })
}

/// Strict reads produce editable definitions; outline reads describe read-only workouts.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Strict,
    Outline,
}

fn definition(value: &Value, mode: Mode) -> Result<Workout> {
    // Preserve server metadata on PUT; only represent features the editor can round-trip.
    ensure!(
        mode == Mode::Outline
            || value["isSessionTransitionEnabled"].is_null()
            || value["isSessionTransitionEnabled"] == false,
        "multisport transitions are read-only"
    );
    let segments = value["workoutSegments"]
        .as_array()
        .context("missing workout segments")?;
    ensure!(segments.len() <= 8, "workout exceeds segment bound");
    let first = segments.first().context("missing workout segment")?["workoutSteps"]
        .as_array()
        .and_then(|s| s.first())
        .context("missing first workout step")?;
    let order_base = first["stepOrder"]
        .as_u64()
        .filter(|n| *n <= i64::MAX as u64)
        .and_then(|n| n.checked_sub(1))
        .context("invalid first workout step order")?;
    let mut total = 0;
    let segments = segments
        .iter()
        .map(|segment| -> Result<Segment> {
            supported_fields(
                segment,
                &[
                    "workoutSegmentId",
                    "segmentOrder",
                    "sportType",
                    "workoutSteps",
                ],
                mode,
            )?;
            Ok(Segment {
                sport: string(&segment["sportType"]["sportTypeKey"])?,
                steps: steps(&segment["workoutSteps"], 0, &mut total, mode, order_base)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let pool = if value["poolLength"].is_null() {
        None
    } else {
        ensure!(
            value["poolLengthUnit"]["unitKey"] == "meter",
            "non-meter pool length is read-only"
        );
        Some(number(&value["poolLength"])?)
    };
    let workout = Workout {
        name: string(&value["workoutName"])?,
        description: optional_text(&value["description"])?,
        sport: string(&value["sportType"]["sportTypeKey"])?,
        pool_length_meters: pool,
        segments,
    };
    workout.validate()?;
    Ok(workout)
}
fn steps(
    value: &Value,
    depth: usize,
    total: &mut usize,
    mode: Mode,
    order_base: u64,
) -> Result<Vec<Step>> {
    ensure!(depth <= 3, "repeat nesting exceeds editor bound");
    let values = value.as_array().context("missing workout steps")?;
    values
        .iter()
        .map(|value| {
            *total += 1;
            ensure!(*total <= 100, "workout exceeds editor step bound");
            ensure!(
                mode == Mode::Outline
                    || value["stepOrder"].as_u64() == Some(order_base + *total as u64),
                "non-sequential workout steps are read-only"
            );
            match value["type"].as_str() {
                Some("RepeatGroupDTO") => {
                    supported_fields(
                        value,
                        &[
                            "type",
                            "stepId",
                            "stepOrder",
                            "stepType",
                            "childStepId",
                            "smartRepeat",
                            "skipLastRestStep",
                            "numberOfIterations",
                            "endCondition",
                            "endConditionValue",
                            "workoutSteps",
                        ],
                        mode,
                    )?;
                    ensure!(
                        value["endCondition"]["conditionTypeKey"] == "iterations",
                        "non-counted repeats are read-only"
                    );
                    ensure!(
                        mode == Mode::Outline
                            || value["smartRepeat"].is_null()
                            || value["smartRepeat"] == false,
                        "smart repeats are read-only"
                    );
                    let count = value["numberOfIterations"]
                        .as_u64()
                        .context("missing repeat count")?
                        .try_into()?;
                    let skip_last_rest = match value["skipLastRestStep"].as_bool() {
                        Some(value) => value,
                        None if value["skipLastRestStep"].is_null() => false,
                        None => bail!("invalid repeat rest flag"),
                    };
                    Ok(Step::Repeat {
                        count,
                        skip_last_rest,
                        steps: steps(&value["workoutSteps"], depth + 1, total, mode, order_base)?,
                    })
                }
                Some("ExecutableStepDTO") => executable(value, mode),
                _ => bail!("unsupported Garmin workout step type"),
            }
        })
        .collect()
}
fn executable(value: &Value, mode: Mode) -> Result<Step> {
    supported_fields(
        value,
        &[
            "type",
            "stepId",
            "stepOrder",
            "stepType",
            "childStepId",
            "endCondition",
            "endConditionValue",
            "endConditionCompare",
            "targetType",
            "targetValueOne",
            "targetValueTwo",
            "zoneNumber",
            "secondaryTargetType",
            "secondaryTargetValueOne",
            "secondaryTargetValueTwo",
            "secondaryZoneNumber",
            "strokeType",
            "drillType",
            "equipmentType",
            "description",
            "category",
            "exerciseName",
            "weightValue",
            "weightUnit",
            "benchmarkPercentage",
            "benchmarkKey",
            "relativeWeight",
            "isMinReps",
        ],
        mode,
    )?;
    // Garmin writes an empty comparison and a non-positive load where it means "none".
    let weight = value["weightValue"].as_f64().filter(|weight| *weight > 0.0);
    if mode == Mode::Strict {
        ensure!(
            value["endConditionCompare"].is_null() || value["endConditionCompare"] == "",
            "compared completion conditions are read-only"
        );
        ensure!(
            !value["category"].is_null()
                || (weight.is_none()
                    && value["exerciseName"].is_null()
                    && value["benchmarkPercentage"].is_null()
                    && value["relativeWeight"].is_null()),
            "exercise data without a category is read-only"
        );
        ensure!(
            value["benchmarkKey"].is_null() || !value["benchmarkPercentage"].is_null(),
            "a benchmark key without a percentage is read-only"
        );
        for (field, id) in [
            ("strokeType", "strokeTypeId"),
            ("equipmentType", "equipmentTypeId"),
            ("drillType", "drillTypeId"),
        ] {
            ensure!(
                value[field].is_null() || value[field][id].as_u64() == Some(0),
                "stroke, drill or equipment-specific steps are read-only"
            );
        }
        for field in ["exerciseId", "weightPercent", "targetValueType"] {
            ensure!(
                value[field].is_null(),
                "workout uses an unsupported exercise or target feature"
            );
        }
    }
    ensure!(
        value["isMinReps"].is_null() || value["isMinReps"].is_boolean(),
        "invalid minimum repetition flag"
    );
    ensure!(
        value["isMinReps"] != true || value["endCondition"]["conditionTypeKey"] == "reps",
        "minimum repetitions require a repetition condition"
    );
    let duration = match value["endCondition"]["conditionTypeKey"].as_str() {
        Some("time") => Duration::Seconds(number(&value["endConditionValue"])?),
        Some("distance") => Duration::Meters(number(&value["endConditionValue"])?),
        Some("reps") => {
            let n = number(&value["endConditionValue"])?;
            ensure!(
                n.fract() == 0.0 && (1.0..=10000.0).contains(&n),
                "invalid repetition count"
            );
            if value["isMinReps"] == true {
                Duration::MinimumRepetitions(n as u32)
            } else {
                Duration::Repetitions(n as u32)
            }
        }
        Some("lap.button") => Duration::LapButton,
        _ => bail!("unsupported completion condition"),
    };
    let exercise = if value["category"].is_null() {
        None
    } else {
        if weight.is_some() {
            ensure!(
                value["weightUnit"]["unitKey"] == "kilogram",
                "unsupported strength load unit"
            );
        }
        Some(Box::new(Exercise {
            category: string(&value["category"])?,
            name: optional_text(&value["exerciseName"])?,
            weight_kg: weight,
            relative_load: if value["relativeWeight"].is_null() {
                None
            } else {
                Some(string(&value["relativeWeight"])?.to_ascii_lowercase())
            },
            benchmark: benchmark(value)?,
        }))
    };
    let secondary_target = if value["secondaryTargetType"].is_null() {
        None
    } else {
        match target(value, true)? {
            Target::None => None,
            other => Some(other),
        }
    };
    Ok(Step::Exercise {
        kind: string(&value["stepType"]["stepTypeKey"])?,
        description: optional_text(&value["description"])?,
        duration,
        target: target(value, false)?,
        secondary_target,
        exercise,
    })
}
/// A load given as a whole percentage of a benchmark; Workout::validate bounds it and
/// rejects a step that also carries kilograms.
fn benchmark(value: &Value) -> Result<Option<BenchmarkLoad>> {
    if value["benchmarkPercentage"].is_null() {
        return Ok(None);
    }
    let percentage = number(&value["benchmarkPercentage"])?;
    ensure!(
        percentage.fract() == 0.0 && (1.0..=100.0).contains(&percentage),
        "invalid benchmark percentage"
    );
    Ok(Some(BenchmarkLoad {
        percentage: percentage as u8,
        key: string(&value["benchmarkKey"])?,
    }))
}
fn target(value: &Value, secondary: bool) -> Result<Target> {
    let fields = if secondary {
        [
            "secondaryTargetType",
            "secondaryTargetValueOne",
            "secondaryTargetValueTwo",
            "secondaryZoneNumber",
        ]
    } else {
        [
            "targetType",
            "targetValueOne",
            "targetValueTwo",
            "zoneNumber",
        ]
    };
    // Coach warmups/cooldowns explicitly omit intensity, independently of duration/load.
    // Do not infer a target when Garmin sent values or an incomplete type object.
    if value.get(fields[0]) == Some(&Value::Null) {
        ensure!(
            fields[1..].iter().all(|field| value[*field].is_null()),
            "target values without a target type are read-only"
        );
        return Ok(Target::None);
    }
    let key = value[fields[0]]["workoutTargetTypeKey"]
        .as_str()
        .context("missing target type")?;
    if key == "no.target" {
        return Ok(Target::None);
    }
    if key == "swim.css.offset" {
        return Ok(Target::SwimCssOffset(number(&value[fields[1]])?));
    }
    if !value[fields[3]].is_null() {
        let kind = match key {
            "heart.rate.zone" => "heart_rate",
            "power.zone" => "power",
            _ => bail!("unsupported zone target"),
        };
        let number = value[fields[3]]
            .as_u64()
            .context("invalid zone")?
            .try_into()?;
        return Ok(Target::Zone {
            kind: kind.into(),
            number,
        });
    }
    let one = number(&value[fields[1]])?;
    let two = number(&value[fields[2]])?;
    let (unit, minimum, maximum) = match key {
        "pace.zone" => {
            ensure!(one > 0.0 && two > 0.0, "invalid pace speed");
            ("pace_seconds_per_km", 1000.0 / one, 1000.0 / two)
        }
        "speed.zone" => ("speed_mps", one, two),
        "heart.rate.zone" => ("heart_rate_bpm", one, two),
        "power.zone" => ("power_watts", one, two),
        "cadence" => ("cadence_per_minute", one, two),
        _ => bail!("unsupported intensity target"),
    };
    Ok(Target::Range {
        unit: unit.into(),
        minimum,
        maximum,
    })
}
fn string(value: &Value) -> Result<String> {
    Ok(value.as_str().context("missing workout text field")?.into())
}
fn optional_text(value: &Value) -> Result<String> {
    if value.is_null() {
        Ok(String::new())
    } else {
        string(value)
    }
}
fn number(value: &Value) -> Result<f64> {
    value.as_f64().context("missing numeric workout field")
}

fn supported_fields(value: &Value, allowed: &[&str], mode: Mode) -> Result<()> {
    let object = value.as_object().context("expected a workout object")?;
    if mode == Mode::Outline {
        return Ok(());
    }
    for (key, field) in object {
        ensure!(
            field.is_null() || allowed.contains(&key.as_str()),
            "unsupported workout field: {key}"
        );
    }
    Ok(())
}
