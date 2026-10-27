//! src/garmin/workout_encode.rs converts validated workout definitions into Garmin write DTOs.
//! It owns vendor IDs and unit conversions, including inverse pace ranges and strength loads
//! in kilograms, as a benchmark percentage or as a Coach intensity label.

use crate::workouts::{Duration, Step, Target, Workout};
use anyhow::{Result, bail};
use serde_json::{Value, json};

pub(super) fn sport(key: &str) -> Result<Value> {
    let id = match key {
        "running" => 1,
        "cycling" => 2,
        "other" => 3,
        "swimming" => 4,
        "strength_training" => 5,
        "cardio_training" => 6,
        "yoga" => 7,
        "pilates" => 8,
        "hiit" => 9,
        "multi_sport" => 10,
        "mobility" => 11,
        _ => bail!("unsupported workout sport"),
    };
    Ok(json!({"sportTypeId":id,"sportTypeKey":key,"displayOrder":id}))
}
pub(super) fn payload(workout: &Workout) -> Result<Value> {
    workout.validate()?;
    let mut order = 0;
    let segments = workout.segments.iter().enumerate().map(|(i, segment)| -> Result<Value> {
        Ok(json!({"segmentOrder":i+1,"sportType":sport(&segment.sport)?,"workoutSteps":steps(&segment.steps, &mut order)?}))
    }).collect::<Result<Vec<_>>>()?;
    let mut value = json!({"workoutName":workout.name,"description":workout.description,
        "sportType":sport(&workout.sport)?,"workoutSegments":segments});
    if let Some(length) = workout.pool_length_meters {
        value["poolLength"] = json!(length);
        value["poolLengthUnit"] = json!({"unitId":1,"unitKey":"meter","factor":100.0});
    }
    Ok(value)
}
fn steps(source: &[Step], order: &mut usize) -> Result<Vec<Value>> {
    source.iter().map(|step| {
        *order += 1;
        let current = *order;
        match step {
            Step::Repeat { count, skip_last_rest, steps: children } => Ok(json!({
                "type":"RepeatGroupDTO","stepOrder":current,"stepType":{"stepTypeId":6,"stepTypeKey":"repeat","displayOrder":6},
                "numberOfIterations":count,"endCondition":condition(7,"iterations"),"endConditionValue":count,
                "smartRepeat":false,"skipLastRestStep":skip_last_rest,"workoutSteps":steps(children,order)?})),
            Step::Exercise { kind, description, duration, target, secondary_target, exercise } => {
                let id = match kind.as_str() { "warmup"=>1,"cooldown"=>2,"interval"=>3,"recovery"=>4,"rest"=>5,"other"=>7,"main"=>8,_=>bail!("unsupported step type") };
                let (condition_id, key, amount) = match duration {
                    Duration::Seconds(n) => (2,"time",json!(n)), Duration::Meters(n) => (3,"distance",json!(n)),
                    Duration::Repetitions(n) | Duration::MinimumRepetitions(n) => (10,"reps",json!(n)), Duration::LapButton => (1,"lap.button",Value::Null),
                };
                let mut value = json!({"type":"ExecutableStepDTO","stepOrder":current,
                    "stepType":{"stepTypeId":id,"stepTypeKey":kind,"displayOrder":id},
                    "endCondition":condition(condition_id,key),"endConditionValue":amount,"description":description});
                if matches!(duration, Duration::MinimumRepetitions(_)) { value["isMinReps"] = json!(true); }
                write_target(&mut value, target, false)?;
                if let Some(target) = secondary_target { write_target(&mut value,target,true)?; }
                if let Some(exercise) = exercise {
                    value["category"] = json!(exercise.category); value["exerciseName"] = json!(exercise.name);
                    if let Some(load) = &exercise.relative_load { value["relativeWeight"] = json!(load.to_ascii_uppercase()); }
                    if let Some(weight) = exercise.weight_kg {
                        value["weightValue"] = json!(weight);
                        value["weightUnit"] = json!({"unitId":8,"unitKey":"kilogram","factor":1000.0});
                    }
                    if let Some(benchmark) = &exercise.benchmark {
                        // Garmin Connect writes a null absolute load next to a benchmark share.
                        value["weightValue"] = Value::Null;
                        value["benchmarkPercentage"] = json!(benchmark.percentage);
                        value["benchmarkKey"] = json!(benchmark.key);
                    }
                }
                Ok(value)
            }
        }
    }).collect()
}
fn condition(id: u32, key: &str) -> Value {
    json!({"conditionTypeId":id,"conditionTypeKey":key,"displayOrder":id,"displayable":id != 7})
}
fn write_target(value: &mut Value, target: &Target, secondary: bool) -> Result<()> {
    let (id, key, one, two, zone) = match target {
        Target::None => (1, "no.target", None, None, None),
        Target::SwimCssOffset(seconds) => (17, "swim.css.offset", Some(*seconds), Some(0.0), None),
        Target::Zone { kind, number } => match kind.as_str() {
            "heart_rate" => (4, "heart.rate.zone", None, None, Some(*number)),
            "power" => (2, "power.zone", None, None, Some(*number)),
            _ => bail!("unsupported zone target"),
        },
        Target::Range {
            unit,
            minimum,
            maximum,
        } => {
            let (id, key, one, two) = match unit.as_str() {
                "pace_seconds_per_km" => (6, "pace.zone", 1000.0 / minimum, 1000.0 / maximum),
                "speed_mps" => (5, "speed.zone", *minimum, *maximum),
                "heart_rate_bpm" => (4, "heart.rate.zone", *minimum, *maximum),
                "power_watts" => (2, "power.zone", *minimum, *maximum),
                "cadence_per_minute" => (3, "cadence", *minimum, *maximum),
                _ => bail!("unsupported target unit"),
            };
            (id, key, Some(one), Some(two), None)
        }
    };
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
    value[fields[0]] =
        json!({"workoutTargetTypeId":id,"workoutTargetTypeKey":key,"displayOrder":id});
    if let Some(one) = one {
        value[fields[1]] = json!(one);
    }
    if let Some(two) = two {
        value[fields[2]] = json!(two);
    }
    if let Some(zone) = zone {
        value[fields[3]] = json!(zone);
    }
    Ok(())
}
