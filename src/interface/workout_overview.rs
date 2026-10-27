//! src/interface/workout_overview.rs renders workout totals and positional draft changes.
//! summary and changes are local projections shared by terminal cards and human CLI reviews.

use super::workout_view::{duration_text, exercise_text, step_rows};
use crate::workouts::{Draft, Workout, overview};

pub(super) fn summary(workout: &Workout) -> String {
    let totals = overview(workout);
    let mut text = format!(
        "{}\nWorking sets: {}\nTotal repetitions: {}{} (including warmup/cooldown)\nTimed steps: {:.0} sec ({:.1} min)\nTimed rest/recovery: {:.0} sec\n",
        workout.name,
        totals.working_sets,
        if totals.has_minimum_repetitions {
            "at least "
        } else {
            ""
        },
        totals.repetitions,
        totals.timed_seconds,
        totals.timed_seconds / 60.0,
        totals.rest_seconds
    );
    if totals.repetitions > 0 || totals.untimed_steps > 0 {
        text.push_str(&format!("Total duration: unknown; add repetition time and {} distance/Lap steps to the timed subtotal.\n", totals.untimed_steps));
    }
    text.push_str("Transitions and unprogrammed pauses are not included.\n\n");
    for row in totals.exercises {
        text.push_str(&format!(
            "{} | {} sets x {}\n",
            exercise_text(&row.exercise),
            row.sets,
            duration_text(&row.duration)
        ));
    }
    text
}

pub(super) fn changes(draft: &Draft) -> String {
    let Some(source) = &draft.source else {
        return "CREATE new workout; no previous version.".into();
    };
    let Some(before) = &source.definition else {
        return "Source definition unavailable; a complete comparison is not possible.".into();
    };
    let after = &draft.workout;
    let mut lines = vec![format!(
        "UPDATE Garmin ID {}\nCompared with saved source revision {}.\nSteps are compared by position; insertions can shift later positions.",
        source.id, source.revision
    )];
    change(&mut lines, "Name", &before.name, &after.name);
    change(&mut lines, "Sport", &before.sport, &after.sport);
    change(
        &mut lines,
        "Description",
        &before.description,
        &after.description,
    );
    change(
        &mut lines,
        "Pool (m)",
        &format!("{:?}", before.pool_length_meters),
        &format!("{:?}", after.pool_length_meters),
    );
    for index in 0..before.segments.len().max(after.segments.len()) {
        change(
            &mut lines,
            &format!("Segment {} sport", index + 1),
            before
                .segments
                .get(index)
                .map_or("absent", |s| s.sport.as_str()),
            after
                .segments
                .get(index)
                .map_or("absent", |s| s.sport.as_str()),
        );
    }
    // Stable segment prefixes are essential when a segment is added or removed.
    let rows = |workout: &Workout| {
        workout
            .segments
            .iter()
            .enumerate()
            .flat_map(|(index, segment)| {
                let single = Workout {
                    segments: vec![segment.clone()],
                    ..workout.clone()
                };
                step_rows(&single).into_iter().map(move |mut row| {
                    row.number = format!("{}.{}", index + 1, row.number);
                    row
                })
            })
            .collect::<Vec<_>>()
    };
    let old = rows(before);
    let new = rows(after);
    for row in &old {
        let Some(next) = new.iter().find(|next| next.number == row.number) else {
            lines.push(format!(
                "Step {} removed: {} / {} / {}",
                row.number, row.kind, row.duration, row.exercise
            ));
            continue;
        };
        for (label, previous, proposed) in [
            ("kind", &row.kind, &next.kind),
            ("duration", &row.duration, &next.duration),
            ("target", &row.target, &next.target),
            ("exercise/load", &row.exercise, &next.exercise),
            ("note", &row.note, &next.note),
        ] {
            change(
                &mut lines,
                &format!("Step {} {label}", row.number),
                previous,
                proposed,
            );
        }
    }
    for row in &new {
        if !old.iter().any(|old| old.number == row.number) {
            lines.push(format!(
                "Step {} added: {} / {} / {}",
                row.number, row.kind, row.duration, row.exercise
            ));
        }
    }
    if lines.len() == 1 {
        lines.push("No definition changes.".into());
    }
    lines.join("\n")
}
fn change(lines: &mut Vec<String>, label: &str, before: &str, after: &str) {
    if before != after {
        lines.push(format!(
            "{label}: {} -> {}",
            if before.is_empty() { "(empty)" } else { before },
            if after.is_empty() { "(empty)" } else { after }
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workouts::{DraftState, Duration, Exercise, LibraryWorkout, Segment, Step, Target};

    fn workout() -> Workout {
        Workout {
            name: "Coach strength".into(),
            description: String::new(),
            sport: "strength_training".into(),
            pool_length_meters: None,
            segments: vec![Segment {
                sport: "strength_training".into(),
                steps: vec![Step::Exercise {
                    kind: "interval".into(),
                    description: "Controlled effort".into(),
                    duration: Duration::MinimumRepetitions(8),
                    target: Target::None,
                    secondary_target: None,
                    exercise: Some(Box::new(Exercise {
                        category: "bench_press".into(),
                        name: "barbell_bench_press".into(),
                        weight_kg: None,
                        benchmark: None,
                        relative_load: Some("heavy".into()),
                    })),
                }],
            }],
        }
    }
    fn draft(workout: Workout) -> Draft {
        Draft {
            explanation: Some("Keep plan identity".into()),
            id: 1,
            revision: 1,
            source: Some(LibraryWorkout {
                coach_plan_id: Some(42),
                id: 7,
                name: workout.name.clone(),
                sport: workout.sport.clone(),
                revision: "source".into(),
                synced_at: "2026-10-27T00:00:00Z".into(),
                definition: Some(workout.clone()),
                read_only_reason: None,
                outline: None,
            }),
            workout,
            state: DraftState::Draft,
            remote_id: None,
            mcp_request: None,
        }
    }
    #[test]
    fn coach_summary_and_diff_preserve_load_and_minimum_semantics() {
        let mut draft = draft(workout());
        let text = summary(&draft.workout);
        assert!(text.contains("Total repetitions: at least 8"));
        assert!(text.contains("heavy"));
        assert!(changes(&draft).contains("No definition changes."));
        if let Step::Exercise {
            duration,
            exercise: Some(exercise),
            ..
        } = &mut draft.workout.segments[0].steps[0]
        {
            *duration = Duration::Repetitions(10);
            exercise.relative_load = None;
            exercise.weight_kg = Some(40.0);
        }
        let text = changes(&draft);
        assert!(text.contains("at least 8 reps -> 10 reps"));
        assert!(text.contains("heavy") && text.contains("40 kg"));
        assert!(text.contains("exercise/load"));
    }
    #[test]
    fn changed_repeat_structure_and_removed_segments_are_visible() {
        let mut draft = draft(workout());
        draft.workout.segments[0].steps = vec![Step::Repeat {
            count: 3,
            skip_last_rest: true,
            steps: draft.workout.segments[0].steps.clone(),
        }];
        let text = changes(&draft);
        assert!(text.contains("repeat x3"));
        assert!(text.contains("skip final rest"));
        assert!(text.contains("Step 1.1.1 added"));
        if let Some(source) = draft.source.as_mut() {
            source.definition = Some(draft.workout.clone());
        }
        draft.workout = workout();
        assert!(changes(&draft).contains("Step 1.1.1 removed"));
    }
}
