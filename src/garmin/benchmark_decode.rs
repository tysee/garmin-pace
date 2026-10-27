//! src/garmin/benchmark_decode.rs normalizes benchmark loads and recorded strength sets.
//! Workout/benchmark loads use their named unit; recorded activity set weights are grams.

use crate::workouts::benchmarks::{Benchmark, ExerciseKey, ObservedSet, valid_key};
use anyhow::{Context, Result, ensure};
use serde_json::Value;

pub(super) fn benchmark(value: &Value, now: &str) -> Result<Benchmark> {
    let exercise = ExerciseKey {
        category: key(&value["benchmarkCategoryKey"])?,
        name: key(&value["benchmarkKey"])?,
    };
    let id = if value["id"].is_null() || value["id"] == 0 {
        None
    } else {
        let id = value["id"].as_u64().context("invalid benchmark ID")?;
        ensure!(id > 0 && id <= i64::MAX as u64, "invalid benchmark ID");
        Some(id)
    };
    let (weight_kg, repetitions, one_rep_max_kg) = if id.is_some() {
        let factor = match value["weightUnit"]["unitKey"].as_str() {
            Some("kilogram") => 1.0,
            Some("pound") => 0.45359237,
            _ => anyhow::bail!("unsupported benchmark weight unit"),
        };
        let weight = number(&value["maxWeight"])? * factor;
        let max = number(&value["oneRepMax"])? * factor;
        let reps = value["reps"]
            .as_u64()
            .context("invalid benchmark repetitions")?;
        ensure!(
            (0.1..=453.6).contains(&weight)
                && (1..=20).contains(&reps)
                && max >= weight
                && max <= 1000.0,
            "invalid Garmin benchmark values"
        );
        (Some(weight), Some(u32::try_from(reps)?), Some(max))
    } else {
        ensure!(
            value["maxWeight"].is_null() && value["oneRepMax"].is_null() && value["reps"].is_null(),
            "unset benchmark contains inconsistent values"
        );
        (None, None, None)
    };
    let linked = if id.is_none() && value["linkedBenchmarkExercises"].is_null() {
        vec![exercise.clone()]
    } else {
        let rows = value["linkedBenchmarkExercises"]
            .as_array()
            .context("missing benchmark exercise links")?;
        ensure!(rows.len() <= 128, "too many linked benchmark exercises");
        rows.iter()
            .map(|row| {
                Ok(ExerciseKey {
                    category: key(&row["categoryKey"])?,
                    name: key(&row["exerciseKey"])?,
                })
            })
            .collect::<Result<Vec<_>>>()?
    };
    Ok(Benchmark {
        id,
        exercise,
        weight_kg,
        repetitions,
        one_rep_max_kg,
        linked,
        revision: super::workout_decode::revision(value)?,
        synced_at: now.into(),
    })
}
pub(super) fn sets(value: &Value, id: u64) -> Result<Vec<ObservedSet>> {
    ensure!(
        value["activityId"].as_u64() == Some(id),
        "exercise sets belong to a different activity"
    );
    let rows = value["exerciseSets"]
        .as_array()
        .context("missing exercise set list")?;
    ensure!(rows.len() <= 500, "strength activity exceeds 500 sets");
    rows.iter()
        .enumerate()
        .map(|(position, row)| {
            let active = match row["setType"].as_str() {
                Some("ACTIVE") => true,
                Some("REST") => false,
                _ => anyhow::bail!("unsupported strength set type"),
            };
            let exercises = row["exercises"]
                .as_array()
                .context("missing recorded exercise identities")?;
            ensure!(
                exercises.len() <= 32,
                "too many candidate exercise identities"
            );
            let exercise = if exercises.len() == 1
                && !exercises[0]["name"].is_null()
                && !exercises[0]["category"].is_null()
            {
                Some(ExerciseKey {
                    category: key(&exercises[0]["category"])?,
                    name: key(&exercises[0]["name"])?,
                })
            } else {
                None
            };
            // Without one exact identity, keep the category when every candidate shares it.
            let category = if exercise.is_some() {
                None
            } else {
                let mut categories = exercises
                    .iter()
                    .filter(|candidate| !candidate["category"].is_null())
                    .map(|candidate| key(&candidate["category"]))
                    .collect::<Result<Vec<_>>>()?;
                categories.sort_unstable();
                categories.dedup();
                if categories.len() == 1 {
                    categories.pop()
                } else {
                    None
                }
            };
            let weight_kg = if row["weight"].is_null() {
                None
            } else {
                let grams = number(&row["weight"])?;
                ensure!(
                    (-1.0..=2_000_000.0).contains(&grams),
                    "invalid recorded set load"
                );
                if grams < 0.0 {
                    None
                } else {
                    Some(grams / 1000.0)
                }
            };
            let repetitions = if row["repetitionCount"].is_null() {
                None
            } else {
                let count = row["repetitionCount"]
                    .as_u64()
                    .context("invalid set repetition count")?;
                ensure!(count <= 10000, "recorded repetitions exceed bound");
                Some(u32::try_from(count)?)
            };
            Ok(ObservedSet {
                position,
                active,
                exercise,
                category,
                weight_kg,
                repetitions,
            })
        })
        .collect()
}
fn key(value: &Value) -> Result<String> {
    let value = value.as_str().context("missing exercise key")?;
    ensure!(valid_key(value), "invalid exercise key");
    Ok(value.into())
}
fn number(value: &Value) -> Result<f64> {
    let value = value.as_f64().context("missing numeric strength value")?;
    ensure!(value.is_finite(), "non-finite strength value");
    Ok(value)
}
