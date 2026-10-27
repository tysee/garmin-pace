//! src/workouts/domain/equivalence.rs compares a reviewed candidate with a saved definition.
//! It allows only representation rounding in physical values; steps, names, targets and
//! benchmark and Coach-relative loads must agree exactly.

use super::{Duration, Exercise, Step, Target, Workout};

impl Workout {
    /// Match all semantics while tolerating decimal precision lost by a remote serializer.
    pub fn equivalent(&self, other: &Self) -> bool {
        self.name == other.name
            && self.description == other.description
            && self.sport == other.sport
            && option_number(self.pool_length_meters, other.pool_length_meters, 0.000001)
            && self.segments.len() == other.segments.len()
            && self
                .segments
                .iter()
                .zip(&other.segments)
                .all(|(a, b)| a.sport == b.sport && steps(&a.steps, &b.steps))
    }
}
fn close(a: f64, b: f64, precision: f64) -> bool {
    (a - b).abs() <= precision
}
fn option_number(a: Option<f64>, b: Option<f64>, precision: f64) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => close(a, b, precision),
        (None, None) => true,
        _ => false,
    }
}
fn duration(a: &Duration, b: &Duration) -> bool {
    match (a, b) {
        (Duration::Seconds(a), Duration::Seconds(b))
        | (Duration::Meters(a), Duration::Meters(b)) => close(*a, *b, 0.000001),
        _ => a == b,
    }
}
fn target(a: &Target, b: &Target) -> bool {
    match (a, b) {
        (
            Target::Range {
                unit: a,
                minimum: amin,
                maximum: amax,
            },
            Target::Range {
                unit: b,
                minimum: bmin,
                maximum: bmax,
            },
        ) => {
            // Pace is stored as speed rounded to four decimal places by Garmin's designer.
            let precision = if a == "pace_seconds_per_km" {
                0.0001
            } else {
                0.000001
            };
            let value = |n: f64| {
                if a == "pace_seconds_per_km" {
                    1000.0 / n
                } else {
                    n
                }
            };
            a == b
                && close(value(*amin), value(*bmin), precision)
                && close(value(*amax), value(*bmax), precision)
        }
        _ => a == b,
    }
}
fn secondary(a: &Option<Target>, b: &Option<Target>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => target(a, b),
        (None, None) => true,
        (Some(Target::None), None) | (None, Some(Target::None)) => true,
        _ => false,
    }
}
fn exercise(a: &Option<Box<Exercise>>, b: &Option<Box<Exercise>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            a.category == b.category
                && a.name == b.name
                && option_number(a.weight_kg, b.weight_kg, 0.001)
                && a.benchmark == b.benchmark
                && a.relative_load == b.relative_load
        }
        (None, None) => true,
        _ => false,
    }
}
fn steps(a: &[Step], b: &[Step]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| match (a, b) {
            (
                Step::Repeat {
                    count: a,
                    skip_last_rest: ar,
                    steps: ac,
                },
                Step::Repeat {
                    count: b,
                    skip_last_rest: br,
                    steps: bc,
                },
            ) => a == b && ar == br && steps(ac, bc),
            (
                Step::Exercise {
                    kind: ak,
                    description: an,
                    duration: ad,
                    target: at,
                    secondary_target: ast,
                    exercise: ae,
                },
                Step::Exercise {
                    kind: bk,
                    description: bn,
                    duration: bd,
                    target: bt,
                    secondary_target: bst,
                    exercise: be,
                },
            ) => {
                ak == bk
                    && an == bn
                    && duration(ad, bd)
                    && target(at, bt)
                    && secondary(ast, bst)
                    && exercise(ae, be)
            }
            _ => false,
        })
}
