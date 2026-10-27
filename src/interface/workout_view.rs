//! src/interface/workout_view.rs presents library entries, complete draft reviews and
//! flattened step rows for tables. CLI, terminal and MCP share these edge projections;
//! units remain explicit in every view.

use crate::workouts::{
    Draft, DraftState, Duration, Exercise, LibraryWorkout, Step, Target, Workout, WorkoutSummary,
    document::{LibraryDocument, WorkoutDocument},
};
use serde_json::{Value, json};

/// Describe the completed send action, never the current contents of Garmin's queue.
pub(super) fn delivery_label(accepted: bool) -> &'static str {
    if accepted {
        "Sent to watch"
    } else {
        "Outcome unknown"
    }
}

/// Receipt is a separate fact from Garmin accepting a send request.
pub(super) const DELIVERY_RECEIPT_NOTE: &str =
    "Garmin accepted the send request; receipt on the watch is not verified.";

pub(super) fn summary(row: &WorkoutSummary) -> Value {
    json!({"id":row.id,"name":row.name,"sport":row.sport,"revision":row.revision,
        "synced_at":row.synced_at,"editable":row.editable})
}
pub(super) fn draft_summary(draft: &Draft) -> Value {
    json!({"id":draft.id,"revision":draft.revision,"name":draft.workout.name,
        "sport":draft.workout.sport,"state":draft.state.as_str(),
        "source_id":draft.source.as_ref().map(|s| s.id),"remote_id":draft.remote_id,
        "mcp_request":draft.mcp_request,"explanation":draft.explanation})
}
pub(super) fn draft_json(draft: &Draft) -> Value {
    let mut value = draft_summary(draft);
    value["workout"] = json!(WorkoutDocument::from(draft.workout.clone()));
    value["source"] = json!(draft.source.clone().map(LibraryDocument::from));
    value["review"] = json!(review(draft));
    value
}
pub(super) fn state(state: DraftState) -> &'static str {
    match state {
        DraftState::Draft => "Ready for review; nothing saved to Garmin",
        DraftState::Saving => {
            "Save outcome unknown; inspect Garmin and reconcile. Do not save again."
        }
        DraftState::Saved => "Saved; verification incomplete. Reconcile with Garmin.",
        DraftState::Verified => "Saved and verified in Garmin; watch delivery is separate",
        DraftState::Discarded => "Discarded locally; Garmin unchanged",
    }
}
pub(super) fn library(row: &LibraryWorkout) -> String {
    let mut text = format!(
        "{}\nGarmin ID: {}\nSport: {}\nSynced: {}\n",
        row.name, row.id, row.sport, row.synced_at
    );
    if let Some(plan) = row.coach_plan_id {
        text.push_str(&format!("Coach plan: {plan}\n"));
    }
    if let Some(workout) = &row.definition {
        text.push_str(&super::workout_overview::summary(workout));
        text.push_str(&definition(workout));
    }
    if let Some(reason) = &row.read_only_reason {
        let reason: String = reason.chars().flat_map(char::escape_debug).collect();
        text.push_str(&format!("\nRead-only: {reason}\nEdit this workout in Garmin Connect to preserve unsupported features.\n"));
        match &row.outline {
            Some(outline) => {
                text.push_str("\nSTEPS (read-only outline)\n");
                text.push_str(&definition(outline));
            }
            None => text
                .push_str("\nNo step outline is stored; sync the library again to read its steps."),
        }
    }
    text
}
/// Journaled user instructions behind MCP-initiated Garmin writes, one line each.
pub(super) fn mcp_requests(journal: Option<&str>) -> String {
    journal
        .map(|text| {
            text.lines()
                .map(|line| {
                    let line: String = line.chars().flat_map(char::escape_debug).collect();
                    format!("Saved via MCP on user request: {line}\n")
                })
                .collect()
        })
        .unwrap_or_default()
}
pub(super) fn review(draft: &Draft) -> String {
    let operation = match &draft.source {
        Some(s) => format!("UPDATE Garmin workout {} (keeps its ID)", s.id),
        None => "CREATE a new Garmin workout".into(),
    };
    let mut text = format!(
        "Draft {} / revision {}\n{}\n{operation}\n",
        draft.id,
        draft.revision,
        state(draft.state)
    );
    if let Some(id) = draft.remote_id {
        text.push_str(&format!("Garmin ID: {id}\n"));
    }
    text.push_str(&mcp_requests(draft.mcp_request.as_deref()));
    text.push_str(&format!(
        "\nCHANGE OVERVIEW\n{}\n\nSUMMARY\n{}",
        super::workout_overview::changes(draft),
        super::workout_overview::summary(&draft.workout)
    ));
    if let Some(reason) = &draft.explanation {
        text.push_str(&format!("\nEXPLANATION\n{reason}\n"));
    }
    if let Some(source) = &draft.source {
        text.push_str(&format!("\nBEFORE\n{}\n", library(source)));
    }
    text.push_str(&format!(
        "\n{}\n{}",
        if draft.source.is_some() {
            "AFTER"
        } else {
            "PROPOSED WORKOUT"
        },
        definition(&draft.workout)
    ));
    text
}
pub(super) fn definition(workout: &Workout) -> String {
    let mut lines = vec![format!("{} | {}", workout.name, workout.sport)];
    if !workout.description.is_empty() {
        lines.push(workout.description.clone());
    }
    if let Some(length) = workout.pool_length_meters {
        lines.push(format!("Pool: {length} m"));
    }
    for (index, segment) in workout.segments.iter().enumerate() {
        lines.push(format!("Segment {}: {}", index + 1, segment.sport));
        steps(&segment.steps, 0, &mut lines);
    }
    lines.join("\n")
}
fn steps(items: &[Step], depth: usize, lines: &mut Vec<String>) {
    let indent = "  ".repeat(depth);
    for (index, step) in items.iter().enumerate() {
        match step {
            Step::Repeat {
                count,
                skip_last_rest,
                steps: children,
            } => {
                lines.push(format!(
                    "{indent}{}. Repeat {count} times{}",
                    index + 1,
                    if *skip_last_rest {
                        "; skip final rest"
                    } else {
                        ""
                    }
                ));
                steps(children, depth + 1, lines);
            }
            Step::Exercise {
                kind,
                description,
                duration,
                target,
                secondary_target,
                exercise,
            } => {
                let duration = duration_text(duration);
                lines.push(format!(
                    "{indent}{}. {kind}: {duration}; {}",
                    index + 1,
                    intensity(target)
                ));
                if let Some(target) = secondary_target {
                    lines.push(format!("{indent}   Secondary: {}", intensity(target)));
                }
                if let Some(e) = exercise {
                    lines.push(format!("{indent}   {}", exercise_text(e)));
                }
                if !description.is_empty() {
                    lines.push(format!("{indent}   {description}"));
                }
            }
        }
    }
}
fn intensity(target: &Target) -> String {
    match target {
        Target::None => "no intensity target".into(),
        Target::Zone { kind, number } => format!("{} zone {number}", kind.replace('_', " ")),
        Target::SwimCssOffset(seconds) => format!("CSS {seconds:+} sec"),
        Target::Range {
            unit,
            minimum,
            maximum,
        } if unit == "pace_seconds_per_km" => {
            let pace = |value: f64| {
                let seconds = value.round() as u64;
                format!("{}:{:02}", seconds / 60, seconds % 60)
            };
            format!("{}-{} min/km", pace(*minimum), pace(*maximum))
        }
        Target::Range {
            unit,
            minimum,
            maximum,
        } => {
            let label = match unit.as_str() {
                "speed_mps" => "m/s",
                "heart_rate_bpm" => "bpm",
                "power_watts" => "W",
                "cadence_per_minute" => "per min",
                other => other,
            };
            format!("{minimum}-{maximum} {label}")
        }
    }
}

/// Category, exercise name when Garmin has one, and load.
pub(super) fn exercise_text(e: &Exercise) -> String {
    let mut text = e.category.clone();
    if !e.name.is_empty() {
        text.push_str(&format!(" / {}", e.name));
    }
    if let Some(load) = &e.relative_load {
        text.push_str(&format!(" | {load} relative load"));
    }
    if let Some(kg) = e.weight_kg {
        text.push_str(&format!(" | {kg} kg"));
    }
    if let Some(b) = &e.benchmark {
        text.push_str(&format!(" | {}% of benchmark {}", b.percentage, b.key));
    }
    text
}
pub(super) fn duration_text(duration: &Duration) -> String {
    match duration {
        Duration::Seconds(n) => format!("{n} sec"),
        Duration::Meters(n) => format!("{n} m"),
        Duration::Repetitions(n) => format!("{n} reps"),
        Duration::MinimumRepetitions(n) => format!("at least {n} reps"),
        Duration::LapButton => "until Lap button".into(),
    }
}

// --- step rows for tables ---

/// One flattened step: repeat groups number their children 2.1, 2.2 and so on.
pub(super) struct StepRow {
    pub number: String,
    pub kind: String,
    pub duration: String,
    pub target: String,
    pub exercise: String,
    pub note: String,
}
pub(super) fn step_rows(workout: &Workout) -> Vec<StepRow> {
    let mut rows = Vec::new();
    for (index, segment) in workout.segments.iter().enumerate() {
        let prefix = if workout.segments.len() > 1 {
            format!("{}.", index + 1)
        } else {
            String::new()
        };
        flatten(&segment.steps, &prefix, &mut rows);
    }
    rows
}
fn flatten(items: &[Step], prefix: &str, rows: &mut Vec<StepRow>) {
    for (index, step) in items.iter().enumerate() {
        let number = format!("{prefix}{}", index + 1);
        match step {
            Step::Repeat {
                count,
                skip_last_rest,
                steps,
            } => {
                rows.push(StepRow {
                    number: number.clone(),
                    kind: format!("repeat x{count}"),
                    duration: format!("{} steps", steps.len()),
                    target: String::new(),
                    exercise: String::new(),
                    note: if *skip_last_rest {
                        "skip final rest".into()
                    } else {
                        String::new()
                    },
                });
                flatten(steps, &format!("{number}."), rows);
            }
            Step::Exercise {
                kind,
                description,
                duration,
                target,
                secondary_target,
                exercise,
            } => {
                let mut target_text = match target {
                    Target::None => String::new(),
                    other => intensity(other),
                };
                if let Some(secondary) = secondary_target {
                    if !target_text.is_empty() {
                        target_text.push_str("; ");
                    }
                    target_text.push_str(&intensity(secondary));
                }
                rows.push(StepRow {
                    number,
                    kind: kind.clone(),
                    duration: match duration {
                        Duration::LapButton => "lap button".into(),
                        other => duration_text(other),
                    },
                    target: target_text,
                    exercise: exercise
                        .as_ref()
                        .map_or(String::new(), |e| exercise_text(e)),
                    note: description.clone(),
                });
            }
        }
    }
}
/// Every field of one step, for a detail screen.
pub(super) fn step_details(row: &StepRow) -> String {
    let field = |value: &str| {
        if value.is_empty() {
            "none".to_string()
        } else {
            value.to_string()
        }
    };
    format!(
        "Step {}\nKind: {}\nDuration: {}\nTarget: {}\nExercise: {}\nNote: {}",
        row.number,
        row.kind,
        row.duration,
        field(&row.target),
        field(&row.exercise),
        field(&row.note)
    )
}
