//! src/garmin/workout_tests.rs checks vendor units, nested ordering and lossless-edit boundaries.
//! Fixtures represent Garmin wire fields independently from the public model document.

use super::{workout_decode::snapshot, workout_encode::payload};
use crate::workouts::{BenchmarkLoad, Duration, Exercise, Segment, Step, Target, Workout};
use anyhow::Context;
use serde_json::{Value, json};

fn exercise() -> Step {
    Step::Exercise {
        kind: "interval".into(),
        description: String::new(),
        duration: Duration::Repetitions(8),
        target: Target::None,
        secondary_target: None,
        exercise: Some(Box::new(Exercise {
            category: "SQUAT".into(),
            name: "GOBLET_SQUAT".into(),
            weight_kg: Some(6.0),
            benchmark: None,
            relative_load: None,
        })),
    }
}
fn benchmark_lift() -> Step {
    Step::Exercise {
        kind: "interval".into(),
        description: String::new(),
        duration: Duration::Repetitions(5),
        target: Target::None,
        secondary_target: None,
        exercise: Some(Box::new(Exercise {
            category: "DEADLIFT".into(),
            name: "BARBELL_DEADLIFT".into(),
            weight_kg: None,
            relative_load: None,
            benchmark: Some(BenchmarkLoad {
                percentage: 80,
                key: "BARBELL_DEADLIFT".into(),
            }),
        })),
    }
}
fn workout() -> Workout {
    Workout {
        name: "Strength circuit".into(),
        description: String::new(),
        sport: "multi_sport".into(),
        pool_length_meters: None,
        segments: vec![
            Segment {
                sport: "strength_training".into(),
                steps: vec![Step::Repeat {
                    count: 3,
                    skip_last_rest: false,
                    steps: vec![exercise()],
                }],
            },
            Segment {
                sport: "strength_training".into(),
                steps: vec![exercise()],
            },
        ],
    }
}
#[test]
fn global_step_order_and_kilogram_load_match_wire_contract() -> anyhow::Result<()> {
    let source = workout();
    let mut wire = payload(&source)?;
    wire["workoutId"] = json!(7);
    assert_eq!(
        wire["workoutSegments"][0]["workoutSteps"][0]["stepOrder"],
        1
    );
    let child = &wire["workoutSegments"][0]["workoutSteps"][0]["workoutSteps"][0];
    assert_eq!(child["stepOrder"], 2);
    assert_eq!(child["weightValue"], 6.0);
    assert_eq!(child["weightUnit"]["unitKey"], "kilogram");
    assert_eq!(
        wire["workoutSegments"][1]["workoutSteps"][0]["stepOrder"],
        3
    );
    assert_eq!(snapshot(&wire, 7)?.definition, Some(source));
    Ok(())
}
#[test]
fn unsupported_semantics_never_disappear_on_an_edit() -> anyhow::Result<()> {
    for field in ["futureWorkoutFeature", "strokeType", "endConditionCompare"] {
        let mut wire = payload(&workout())?;
        wire["workoutId"] = json!(7);
        wire["workoutSegments"][0]["workoutSteps"][0]["workoutSteps"][0][field] =
            json!({"enabled":true});
        let row = snapshot(&wire, 7)?;
        assert!(row.definition.is_none(), "{field}");
        assert!(row.read_only_reason.is_some());
    }
    let mut wire = payload(&workout())?;
    wire["workoutId"] = json!(7);
    wire["workoutSegments"][0]["workoutSteps"][0]["endCondition"]["conditionTypeKey"] =
        json!("time");
    assert!(snapshot(&wire, 7)?.definition.is_none());
    Ok(())
}
#[test]
fn verification_accepts_representation_rounding_but_not_different_exercises() {
    let source = workout();
    let mut rounded = source.clone();
    let Step::Exercise {
        exercise: Some(exercise),
        ..
    } = &mut rounded.segments[1].steps[0]
    else {
        panic!("fixture")
    };
    exercise.weight_kg = Some(5.9999);
    assert!(source.equivalent(&rounded));
    let Step::Exercise {
        exercise: Some(exercise),
        ..
    } = &mut rounded.segments[1].steps[0]
    else {
        panic!("fixture")
    };
    exercise.name = "BARBELL_BACK_SQUAT".into();
    assert!(!source.equivalent(&rounded));
}
fn strength_wire(benchmark: bool) -> Value {
    // Field shapes observed in Garmin Connect strength workouts: empty comparisons,
    // negative "no load" sentinels and benchmark-relative loads.
    let mut lift = json!({"type":"ExecutableStepDTO","stepOrder":3,"stepType":{"stepTypeKey":"interval"},
        "endCondition":{"conditionTypeKey":"reps"},"endConditionValue":10.0,"endConditionCompare":"",
        "targetType":{"workoutTargetTypeKey":"no.target"},"category":"DEADLIFT","exerciseName":"BARBELL_DEADLIFT",
        "weightValue":null,"weightUnit":{"unitKey":"kilogram"}});
    if benchmark {
        lift["benchmarkPercentage"] = json!(80);
        lift["benchmarkKey"] = json!("BARBELL_DEADLIFT");
    }
    json!({"workoutId":9,"workoutName":"Back day","sportType":{"sportTypeKey":"strength_training"},
    "workoutSegments":[{"segmentOrder":1,"sportType":{"sportTypeKey":"strength_training"},"workoutSteps":[
        {"type":"ExecutableStepDTO","stepOrder":1,"stepType":{"stepTypeKey":"warmup"},
         "endCondition":{"conditionTypeKey":"lap.button"},"endConditionValue":0.0,"endConditionCompare":"",
         "targetType":{"workoutTargetTypeKey":"no.target"},"targetValueTwo":0.0,
         "strokeType":{"strokeTypeId":0},"equipmentType":{"equipmentTypeId":0},
         "category":"CARDIO","exerciseName":"","weightValue":-1.0,"weightUnit":{"unitKey":"kilogram"}},
        {"type":"RepeatGroupDTO","stepOrder":2,"stepType":{"stepTypeKey":"repeat"},"numberOfIterations":3,
         "endCondition":{"conditionTypeKey":"iterations"},"endConditionValue":3.0,"smartRepeat":false,"skipLastRestStep":false,
         "workoutSteps":[lift,
            {"type":"ExecutableStepDTO","stepOrder":4,"stepType":{"stepTypeKey":"rest"},
             "endCondition":{"conditionTypeKey":"lap.button"},"endConditionValue":0.0,"endConditionCompare":"",
             "targetType":{"workoutTargetTypeKey":"no.target"},"weightValue":-1.0,"weightUnit":{"unitKey":"kilogram"}}]}
    ]}]})
}
fn lift_of(workout: &Workout) -> &Exercise {
    let Step::Repeat {
        steps, count: 3, ..
    } = &workout.segments[0].steps[1]
    else {
        panic!("repeat group")
    };
    let Step::Exercise {
        description,
        duration: Duration::Repetitions(10),
        exercise: Some(exercise),
        ..
    } = &steps[0]
    else {
        panic!("lift")
    };
    assert!(description.is_empty());
    exercise
}
#[test]
fn garmin_sentinels_and_benchmark_loads_decode_as_editable() -> anyhow::Result<()> {
    for benchmark in [false, true] {
        let row = snapshot(&strength_wire(benchmark), 9)?;
        let definition = row.definition.unwrap_or_else(|| {
            panic!(
                "sentinel fields are not features: {:?}",
                row.read_only_reason
            )
        });
        assert!(row.outline.is_none() && row.read_only_reason.is_none());
        let Step::Exercise { exercise, .. } = &definition.segments[0].steps[0] else {
            panic!("warmup")
        };
        assert_eq!(
            exercise
                .as_ref()
                .map(|e| (e.category.as_str(), e.weight_kg)),
            Some(("CARDIO", None))
        );
        let lift = lift_of(&definition);
        assert_eq!(lift.name, "BARBELL_DEADLIFT");
        assert_eq!(lift.weight_kg, None);
        assert_eq!(
            lift.benchmark,
            benchmark.then(|| BenchmarkLoad {
                percentage: 80,
                key: "BARBELL_DEADLIFT".into(),
            })
        );
    }
    Ok(())
}
#[test]
fn benchmark_percentage_roundtrips_and_excludes_kilograms() -> anyhow::Result<()> {
    let mut source = workout();
    source.segments[1].steps[0] = benchmark_lift();
    let mut wire = payload(&source)?;
    wire["workoutId"] = json!(7);
    let step = &wire["workoutSegments"][1]["workoutSteps"][0];
    assert_eq!(step["benchmarkPercentage"], 80);
    assert_eq!(step["benchmarkKey"], "BARBELL_DEADLIFT");
    assert!(step["weightValue"].is_null());
    assert_eq!(snapshot(&wire, 7)?.definition, Some(source.clone()));

    let mut rounded = source.clone();
    let Step::Exercise {
        exercise: Some(exercise),
        ..
    } = &mut rounded.segments[1].steps[0]
    else {
        panic!("fixture")
    };
    exercise.benchmark = Some(BenchmarkLoad {
        percentage: 79,
        key: "BARBELL_DEADLIFT".into(),
    });
    assert!(!source.equivalent(&rounded));

    let Step::Exercise {
        exercise: Some(exercise),
        ..
    } = &mut source.segments[1].steps[0]
    else {
        panic!("fixture")
    };
    exercise.weight_kg = Some(60.0);
    assert!(source.validate().is_err());
    assert!(payload(&source).is_err());

    // A remote step carrying both loads stays read-only with an outline instead of losing one.
    let mut wire = strength_wire(true);
    wire["workoutSegments"][0]["workoutSteps"][1]["workoutSteps"][0]["weightValue"] = json!(60.0);
    let row = snapshot(&wire, 9)?;
    assert!(row.definition.is_none());
    assert!(
        row.read_only_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("not both"))
    );
    let mut wire = strength_wire(true);
    wire["workoutSegments"][0]["workoutSteps"][1]["workoutSteps"][0]["benchmarkPercentage"] =
        Value::Null;
    assert!(snapshot(&wire, 9)?.definition.is_none());
    assert!(row.outline.is_none());
    Ok(())
}
#[test]
fn other_unsupported_features_still_keep_a_readable_outline() -> anyhow::Result<()> {
    let mut wire = strength_wire(true);
    wire["workoutSegments"][0]["workoutSteps"][1]["workoutSteps"][0]["strokeType"] =
        json!({"strokeTypeId":4});
    let row = snapshot(&wire, 9)?;
    assert!(row.definition.is_none());
    let Some(outline) = row.outline else {
        panic!("read-only workouts keep an outline")
    };
    assert_eq!(
        lift_of(&outline).benchmark,
        Some(BenchmarkLoad {
            percentage: 80,
            key: "BARBELL_DEADLIFT".into(),
        })
    );
    Ok(())
}

#[test]
fn coach_loads_minimum_reps_and_plan_identity_survive_round_trip() -> anyhow::Result<()> {
    let mut wire = payload(&workout())?;
    wire["workoutId"] = json!(7);
    wire["trainingPlanId"] = json!(42);
    let step = &mut wire["workoutSegments"][0]["workoutSteps"][0]["workoutSteps"][0];
    step["weightValue"] = Value::Null;
    step["weightUnit"] = Value::Null;
    step["relativeWeight"] = json!("HEAVY");
    step["isMinReps"] = json!(true);
    let row = snapshot(&wire, 7)?;
    let definition = row
        .definition
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("{:?}", row.read_only_reason))?;
    let encoded = payload(definition)?;
    let step = &encoded["workoutSegments"][0]["workoutSteps"][0]["workoutSteps"][0];
    assert_eq!(step["relativeWeight"], "HEAVY");
    assert_eq!(step["isMinReps"], true);
    Ok(())
}

#[test]
fn coach_unknown_load_and_ambiguous_loads_remain_read_only() -> anyhow::Result<()> {
    for load in ["HEAVY", "UNKNOWN"] {
        let mut wire = payload(&workout())?;
        wire["workoutId"] = json!(7);
        let step = &mut wire["workoutSegments"][0]["workoutSteps"][0]["workoutSteps"][0];
        step["relativeWeight"] = json!(load);
        assert!(snapshot(&wire, 7)?.definition.is_none());
    }
    Ok(())
}

#[test]
fn coach_verification_distinguishes_minimum_from_exact_repetitions() -> anyhow::Result<()> {
    let mut wire = payload(&workout())?;
    wire["workoutId"] = json!(7);
    let exact = snapshot(&wire, 7)?
        .definition
        .ok_or_else(|| anyhow::anyhow!("missing exact definition"))?;
    wire["workoutSegments"][0]["workoutSteps"][0]["workoutSteps"][0]["isMinReps"] = json!(true);
    let minimum = snapshot(&wire, 7)?
        .definition
        .ok_or_else(|| anyhow::anyhow!("missing minimum definition"))?;
    assert!(!exact.equivalent(&minimum));
    Ok(())
}

#[test]
fn coach_null_targets_preserve_the_complete_percentage_workout() -> anyhow::Result<()> {
    let wire: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/coach_push_5x5.json"))?;
    let row = snapshot(&wire, 777)?;
    assert_eq!(row.read_only_reason, None);
    assert_eq!(row.coach_plan_id, Some(42));
    let source = row.definition.context("editable Coach workout")?;
    let mut encoded = payload(&source)?;
    encoded["workoutId"] = json!(777);
    assert_eq!(snapshot(&encoded, 777)?.definition, Some(source));
    assert_eq!(
        encoded["workoutSegments"][0]["workoutSteps"][0]["targetType"]["workoutTargetTypeKey"],
        "no.target"
    );
    Ok(())
}

#[test]
fn null_target_with_values_or_malformed_type_stays_read_only() -> anyhow::Result<()> {
    for field in ["targetValueOne", "targetValueTwo", "zoneNumber"] {
        let mut wire = strength_wire(true);
        let step = &mut wire["workoutSegments"][0]["workoutSteps"][0];
        step["targetType"] = Value::Null;
        step["targetValueTwo"] = Value::Null;
        step[field] = json!(0);
        assert!(snapshot(&wire, 9)?.definition.is_none(), "{field}");
    }
    for malformed in [
        json!({}),
        json!({"workoutTargetTypeId":4}),
        json!("no.target"),
    ] {
        let mut wire = strength_wire(true);
        wire["workoutSegments"][0]["workoutSteps"][0]["targetType"] = malformed;
        assert!(snapshot(&wire, 9)?.definition.is_none());
    }
    let mut wire = strength_wire(true);
    wire["workoutSegments"][0]["workoutSteps"][0]
        .as_object_mut()
        .context("step")?
        .remove("targetType");
    assert!(snapshot(&wire, 9)?.definition.is_none());
    Ok(())
}
