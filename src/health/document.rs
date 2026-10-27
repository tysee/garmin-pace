//! src/health/document.rs maps health domain values to the versioned JSON contract.
//! SQLite and transport edges share this codec; domain services never depend on it.

use super::{HealthDay, Measurement, Record, Section, Value};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Compact snapshot document without time-series arrays.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DayDocument {
    pub contract_version: u32,
    pub scope: String,
    pub date: String,
    pub synced_at: String,
    pub datasets: BTreeMap<String, DatasetDocument>,
}
/// Availability describes this synchronized dataset, not the presence of a device.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetDocument {
    pub status: String,
    pub source: String,
    pub records: Vec<RecordDocument>,
    pub sample_count: usize,
}
/// All readiness snapshots retain their timestamp and context.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordDocument {
    pub observed_at: Option<String>,
    pub values: BTreeMap<String, MeasurementDocument>,
}
/// Numeric and qualitative values use a stable, explicit unit-bearing contract.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementDocument {
    pub value: ScalarDocument,
    pub unit: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
/// JSON representation of one numeric measurement or descriptive label.
pub enum ScalarDocument {
    Number(f64),
    Text(String),
}

impl From<HealthDay> for DayDocument {
    fn from(day: HealthDay) -> Self {
        Self {
            contract_version: 1,
            scope: "local_snapshot".into(),
            date: day.date.to_string(),
            synced_at: day.synced_at,
            datasets: day
                .sections
                .into_iter()
                .map(|s| {
                    let status = if s.records.is_empty() && s.sample_count == 0 {
                        "no_data"
                    } else {
                        "available"
                    };
                    (
                        s.metric.as_str().into(),
                        DatasetDocument {
                            status: status.into(),
                            source: s.metric.source().into(),
                            sample_count: s.sample_count,
                            records: s
                                .records
                                .into_iter()
                                .map(|r| RecordDocument {
                                    observed_at: r.observed_at,
                                    values: r
                                        .values
                                        .into_iter()
                                        .map(|(name, value)| {
                                            (
                                                name,
                                                MeasurementDocument {
                                                    value: match value.value {
                                                        Value::Number(n) => {
                                                            ScalarDocument::Number(n)
                                                        }
                                                        Value::Text(t) => ScalarDocument::Text(t),
                                                    },
                                                    unit: value.unit,
                                                },
                                            )
                                        })
                                        .collect(),
                                })
                                .collect(),
                        },
                    )
                })
                .collect(),
        }
    }
}
impl TryFrom<DayDocument> for HealthDay {
    type Error = anyhow::Error;
    fn try_from(day: DayDocument) -> Result<Self> {
        ensure!(
            day.contract_version == 1 && day.scope == "local_snapshot",
            "unsupported health document"
        );
        let sections = day
            .datasets
            .into_iter()
            .map(|(name, s)| -> Result<Section> {
                Ok(Section {
                    metric: name.parse()?,
                    sample_count: s.sample_count,
                    records: s
                        .records
                        .into_iter()
                        .map(|r| Record {
                            observed_at: r.observed_at,
                            values: r
                                .values
                                .into_iter()
                                .map(|(name, m)| {
                                    (
                                        name,
                                        Measurement {
                                            value: match m.value {
                                                ScalarDocument::Number(n) => Value::Number(n),
                                                ScalarDocument::Text(t) => Value::Text(t),
                                            },
                                            unit: m.unit,
                                        },
                                    )
                                })
                                .collect(),
                        })
                        .collect(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(HealthDay {
            date: day.date.parse()?,
            synced_at: day.synced_at,
            sections,
        })
    }
}
