//! src/workouts/domain/validation.rs bounds complexity and checks unit-aware values.
//! Workout::validate is shared by model proposals, files and decoded Garmin definitions.

use super::{Duration, Step, Target, Workout, WorkoutError};
use crate::workouts::benchmarks::valid_key;

fn require(valid: bool, message: &'static str) -> Result<(), WorkoutError> {
    if valid {
        Ok(())
    } else {
        Err(WorkoutError::InvalidInput(message))
    }
}
fn text(value: &str, limit: usize) -> bool {
    value.len() <= limit && !value.chars().any(char::is_control)
}
fn positive(value: f64, maximum: f64) -> bool {
    value.is_finite() && value > 0.0 && value <= maximum
}
fn sport(value: &str) -> bool {
    matches!(
        value,
        "running"
            | "cycling"
            | "other"
            | "swimming"
            | "strength_training"
            | "cardio_training"
            | "yoga"
            | "pilates"
            | "hiit"
            | "multi_sport"
            | "mobility"
    )
}
impl Workout {
    /// Reject incomplete, unbounded or ambiguous definitions before any write.
    pub fn validate(&self) -> Result<(), WorkoutError> {
        require(
            !self.name.trim().is_empty() && text(&self.name, 80),
            "workout name must contain 1..=80 bytes without control characters",
        )?;
        require(
            text(&self.description, 2000),
            "workout description exceeds 2000 bytes or contains control characters",
        )?;
        require(sport(&self.sport), "unsupported workout sport")?;
        require(
            !self.segments.is_empty() && self.segments.len() <= 8,
            "workout needs 1..=8 segments",
        )?;
        if let Some(length) = self.pool_length_meters {
            require(
                positive(length, 150.0),
                "pool length must be positive and at most 150 meters",
            )?;
        }
        let mut count = 0;
        for segment in &self.segments {
            require(
                sport(&segment.sport) && segment.sport != "multi_sport",
                "invalid segment sport",
            )?;
            require(
                self.sport == "multi_sport" || self.sport == segment.sport,
                "segment sport must match workout sport",
            )?;
            if segment.sport == "swimming" {
                require(
                    self.pool_length_meters.is_some(),
                    "swimming requires an explicit pool length in meters",
                )?;
            }
            validate_steps(&segment.steps, 0, &mut count)?;
        }
        Ok(())
    }
}
fn validate_steps(steps: &[Step], depth: usize, count: &mut usize) -> Result<(), WorkoutError> {
    require(
        !steps.is_empty() && depth <= 3,
        "steps must be nonempty with at most three repeat nesting levels",
    )?;
    for step in steps {
        *count += 1;
        require(*count <= 100, "workout exceeds 100 stored steps")?;
        match step {
            Step::Repeat {
                count: repeats,
                steps,
                ..
            } => {
                require((1..=100).contains(repeats), "repeat count must be 1..=100")?;
                validate_steps(steps, depth + 1, count)?;
            }
            Step::Exercise {
                kind,
                description,
                duration,
                target,
                secondary_target,
                exercise,
            } => {
                require(
                    matches!(
                        kind.as_str(),
                        "warmup" | "cooldown" | "interval" | "recovery" | "rest" | "other" | "main"
                    ),
                    "unsupported step kind",
                )?;
                require(
                    text(description, 500),
                    "step description exceeds 500 bytes or contains control characters",
                )?;
                let valid = match duration {
                    Duration::Seconds(n) => positive(*n, 86_400.0),
                    Duration::Meters(n) => positive(*n, 1_000_000.0),
                    Duration::Repetitions(n) | Duration::MinimumRepetitions(n) => {
                        (1..=10_000).contains(n)
                    }
                    Duration::LapButton => true,
                };
                require(valid, "invalid workout step duration")?;
                validate_target(target, false)?;
                if let Some(target) = secondary_target {
                    validate_target(target, true)?;
                }
                if let Some(exercise) = exercise {
                    require(
                        !exercise.category.is_empty()
                            && text(&exercise.category, 100)
                            && text(&exercise.name, 100),
                        "exercise category and name are required and bounded to 100 bytes",
                    )?;
                    if let Some(load) = &exercise.relative_load {
                        require(
                            matches!(load.as_str(), "light" | "moderate" | "heavy"),
                            "relative load must be light, moderate or heavy",
                        )?;
                        require(
                            exercise.weight_kg.is_none() && exercise.benchmark.is_none(),
                            "relative load excludes kilograms and benchmark percentages",
                        )?;
                    }
                    if let Some(weight) = exercise.weight_kg {
                        require(
                            positive(weight, 2000.0),
                            "exercise load must be positive kilograms, at most 2000",
                        )?;
                    }
                    if let Some(benchmark) = &exercise.benchmark {
                        require(
                            exercise.weight_kg.is_none(),
                            "exercise load is either kilograms or a benchmark percentage, not both",
                        )?;
                        require(
                            benchmark.percentage != 0 && !benchmark.key.is_empty(),
                            "a benchmark load needs both a percentage and a key",
                        )?;
                        require(
                            (1..=100).contains(&benchmark.percentage),
                            "benchmark percentage must be 1..=100",
                        )?;
                        require(
                            valid_key(&benchmark.key),
                            "benchmark key must be an uppercase Garmin exercise key",
                        )?;
                    }
                }
            }
        }
    }
    Ok(())
}
fn validate_target(target: &Target, secondary: bool) -> Result<(), WorkoutError> {
    match target {
        Target::None => Ok(()),
        Target::Zone { kind, number } => require(
            matches!(kind.as_str(), "heart_rate" | "power")
                && (1..=7).contains(number)
                && (kind != "heart_rate" || *number <= 5),
            "invalid heart-rate or power zone",
        ),
        Target::SwimCssOffset(value) => require(
            secondary && value.is_finite() && value.abs() <= 300.0,
            "CSS offset is a secondary target in seconds, bounded to +/-300",
        ),
        Target::Range {
            unit,
            minimum,
            maximum,
        } => {
            let cap = match unit.as_str() {
                "pace_seconds_per_km" => 7200.0,
                "speed_mps" => 100.0,
                "heart_rate_bpm" => 250.0,
                "power_watts" => 5000.0,
                "cadence_per_minute" => 400.0,
                _ => return Err(WorkoutError::InvalidInput("unsupported target unit")),
            };
            require(
                positive(*minimum, cap) && positive(*maximum, cap) && minimum <= maximum,
                "target range must be positive, ordered and within its unit's bounds",
            )
        }
    }
}
