//! src/workouts/overview.rs computes bounded workout totals without expanding repeat blocks.
//! overview reports known timer time separately from repetitions and open-ended steps.

use super::{Duration, Exercise, Step, Workout};

/// One stored working exercise, with repeat multiplicity and its original completion rule.
#[derive(Debug)]
pub struct ExerciseOverview {
    pub exercise: Exercise,
    pub sets: u64,
    pub duration: Duration,
}
/// Exact programmed totals; no assumed repetition speed or transition time.
#[derive(Debug, Default)]
pub struct WorkoutOverview {
    pub working_sets: u64,
    pub repetitions: u64,
    pub has_minimum_repetitions: bool,
    pub timed_seconds: f64,
    pub rest_seconds: f64,
    pub untimed_steps: u64,
    pub exercises: Vec<ExerciseOverview>,
}
/// Summarize a validated definition in space proportional to its stored steps.
pub fn overview(workout: &Workout) -> WorkoutOverview {
    let mut result = WorkoutOverview::default();
    for segment in &workout.segments {
        accumulate(&segment.steps, 1, 0, &mut result);
    }
    result
}
fn accumulate(steps: &[Step], multiplier: u64, skipped_final: u64, total: &mut WorkoutOverview) {
    for (index, step) in steps.iter().enumerate() {
        match step {
            Step::Repeat {
                count,
                skip_last_rest,
                steps,
            } => {
                accumulate(
                    steps,
                    multiplier * u64::from(*count),
                    if *skip_last_rest { multiplier } else { 0 },
                    total,
                );
            }
            Step::Exercise {
                kind,
                duration,
                exercise,
                ..
            } => {
                let rest = matches!(kind.as_str(), "rest" | "recovery");
                let times = if rest && index + 1 == steps.len() {
                    multiplier - skipped_final
                } else {
                    multiplier
                };
                match duration {
                    Duration::Seconds(seconds) => {
                        total.timed_seconds += seconds * times as f64;
                        if rest {
                            total.rest_seconds += seconds * times as f64;
                        }
                    }
                    Duration::Repetitions(reps) => total.repetitions += u64::from(*reps) * times,
                    Duration::MinimumRepetitions(reps) => {
                        total.repetitions += u64::from(*reps) * times;
                        total.has_minimum_repetitions |= times > 0;
                    }
                    Duration::Meters(_) | Duration::LapButton => total.untimed_steps += times,
                }
                if matches!(kind.as_str(), "interval" | "main")
                    && let Some(exercise) = exercise
                {
                    total.working_sets += times;
                    total.exercises.push(ExerciseOverview {
                        exercise: *exercise.clone(),
                        sets: times,
                        duration: duration.clone(),
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workouts::{Segment, Target};

    fn step(kind: &str, duration: Duration) -> Step {
        Step::Exercise {
            kind: kind.into(),
            description: String::new(),
            duration,
            target: Target::None,
            secondary_target: None,
            exercise: (kind == "interval").then(|| {
                Box::new(Exercise {
                    category: "bench_press".into(),
                    name: "barbell_bench_press".into(),
                    weight_kg: Some(40.0),
                    benchmark: None,
                    relative_load: None,
                })
            }),
        }
    }
    fn workout(steps: Vec<Step>) -> Workout {
        Workout {
            name: "Test".into(),
            description: String::new(),
            sport: "strength_training".into(),
            pool_length_meters: None,
            segments: vec![Segment {
                sport: "strength_training".into(),
                steps,
            }],
        }
    }
    #[test]
    fn nested_repeats_skip_only_the_final_direct_rest() {
        let group = Step::Repeat {
            count: 3,
            skip_last_rest: true,
            steps: vec![
                step("interval", Duration::Repetitions(10)),
                step("rest", Duration::Seconds(60.0)),
            ],
        };
        let result = overview(&workout(vec![
            step("warmup", Duration::Seconds(300.0)),
            Step::Repeat {
                count: 2,
                skip_last_rest: false,
                steps: vec![group],
            },
        ]));
        assert_eq!(result.working_sets, 6);
        assert_eq!(result.repetitions, 60);
        assert_eq!(result.rest_seconds, 240.0);
        assert_eq!(result.timed_seconds, 540.0);
        assert_eq!(result.exercises[0].sets, 6);
    }
    #[test]
    fn unknown_time_and_warmup_reps_do_not_invent_working_sets() {
        let result = overview(&workout(vec![
            step("warmup", Duration::Repetitions(8)),
            step("interval", Duration::LapButton),
            step("cooldown", Duration::Meters(100.0)),
        ]));
        assert_eq!(result.working_sets, 1);
        assert_eq!(result.repetitions, 8);
        assert_eq!(result.untimed_steps, 2);
        assert_eq!(result.timed_seconds, 0.0);
    }
    #[test]
    fn minimum_repetitions_are_a_lower_bound() {
        let result = overview(&workout(vec![step(
            "interval",
            Duration::MinimumRepetitions(8),
        )]));
        assert_eq!(result.repetitions, 8);
        assert!(result.has_minimum_repetitions);
    }
    #[test]
    fn skipped_rest_is_not_a_skipped_work_step() {
        let result = overview(&workout(vec![Step::Repeat {
            count: 1,
            skip_last_rest: true,
            steps: vec![step("interval", Duration::Repetitions(10))],
        }]));
        assert_eq!(result.repetitions, 10);
    }
}
