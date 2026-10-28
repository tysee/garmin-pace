//! src/interface/benchmark_view.rs presents observed strength evidence and model estimates.
//! Human reviews distinguish existing Garmin values, arithmetic averages and estimated 1RM.

use crate::workouts::{
    DraftState,
    benchmarks::{
        ActivitySets, Benchmark, BenchmarkDraft, BenchmarkDraftSummary, Context, ProposalOrigin,
    },
};
use serde_json::{Value, json};

pub(super) fn name(key: &str) -> String {
    key.to_lowercase().replace('_', " ")
}
pub(super) fn state(value: DraftState) -> &'static str {
    match value {
        DraftState::Draft => "Ready for review; nothing saved to Garmin",
        DraftState::Saving => "Save outcome unknown; reconcile before any new save",
        DraftState::Saved => "Save accepted; verification incomplete. Reconcile",
        DraftState::Verified => "Saved and verified in Garmin",
        DraftState::Discarded => "Discarded locally; Garmin unchanged",
    }
}
pub(super) fn summary(draft: &BenchmarkDraftSummary) -> Value {
    json!({"id":draft.id,"revision":draft.revision,"key":draft.key,"state":draft.state.as_str()})
}
pub(super) fn benchmark(row: &Benchmark) -> String {
    let values = match (row.weight_kg, row.repetitions, row.one_rep_max_kg) {
        (Some(w), Some(r), Some(m)) => format!("{w:.1} kg x {r} reps; 1RM {m:.1} kg"),
        _ => "Not set".into(),
    };
    format!(
        "{}\n{}\nSynced: {}",
        name(&row.exercise.name),
        values,
        row.synced_at
    )
}
pub(super) fn context(data: &Context) -> String {
    let e = &data.evidence;
    let averages = match (data.mean_weight_kg, data.mean_repetitions) {
        (Some(w), Some(r)) => format!("Mean load: {w:.1} kg\nMean repetitions: {r:.1}"),
        _ => "No usable loaded sets. An estimate cannot be proposed from this evidence.".into(),
    };
    format!(
        "{}\nWindow: {} to {} UTC\nStrength sync: {}\nUsable sets: {} | Sessions: {}\n{averages}\nExcluded: {} incomplete/rest/unusable sets; {} other exercises\nAverages use the exact exercise only, with equal weight per set.",
        name(&e.source.exercise.name),
        e.window.start,
        e.window.end,
        e.synced_at
            .as_deref()
            .unwrap_or("Not synced for the complete window"),
        data.sample_count,
        data.session_count,
        data.excluded_sets,
        data.unrelated_sets
    )
}
/// One line per active set, naming the exact exercise or only Garmin's category; an absent
/// session names the import state instead of showing zero.
pub(super) fn activity_sets(data: &ActivitySets) -> String {
    let Some(session) = &data.session else {
        return match &data.import_synced_at {
            Some(synced) => format!(
                "Sets: none imported for this activity; strength data for {} was synced {synced}.",
                data.date
            ),
            None => format!(
                "Sets: not synced. Import this activity's sets, or sync strength data for a window covering {}.",
                data.date
            ),
        };
    };
    let mut lines = vec![format!("Sets (imported {}):", session.synced_at)];
    let mut rest = 0;
    for set in &session.sets {
        if !set.active {
            rest += 1;
            continue;
        }
        let label = match (&set.exercise, &set.category) {
            (Some(e), _) => name(&e.name),
            (None, Some(c)) => format!("{} (category)", name(c)),
            (None, None) => "unidentified exercise".into(),
        };
        lines.push(format!(
            "{}. {label} | {} | {}",
            set.position + 1,
            set.weight_kg
                .map_or("no load".into(), |w| format!("{w:.1} kg")),
            set.repetitions
                .map_or("no reps".into(), |r| format!("{r} reps")),
        ));
    }
    if lines.len() == 1 {
        lines.push("No active sets recorded.".into());
    }
    if rest > 0 {
        lines.push(format!("Rest sets: {rest}"));
    }
    lines.join("\n")
}
pub(super) fn review(draft: &BenchmarkDraft) -> String {
    let p = &draft.proposal;
    let origin = match &p.origin {
        ProposalOrigin::UserRequest { request } => format!("User request: {request}"),
        ProposalOrigin::MissingForWorkout {
            draft_id,
            draft_revision,
            reason,
        } => format!("Missing for workout draft #{draft_id}, revision {draft_revision}: {reason}"),
    };
    let links = draft
        .source
        .linked
        .iter()
        .map(|e| name(&e.name))
        .collect::<Vec<_>>()
        .join(", ");
    let requests = super::workout_view::mcp_requests(draft.mcp_request.as_deref());
    format!(
        "Benchmark proposal #{} / revision {}\n{}\n{origin}\n{requests}\nBEFORE\n{}\n\nPROPOSED ESTIMATE\n{:.1} kg x {} reps\nEstimated 1RM: {:.1} kg\nConfidence: {}\nMethod: {}\nReason: {}\nThis is a model estimate, not a measured personal record.\nLinked exercises: {links}\n\nOBSERVED EVIDENCE\n{}",
        draft.id,
        draft.revision,
        state(draft.state),
        benchmark(&draft.source),
        p.weight_kg,
        p.repetitions,
        p.one_rep_max_kg,
        p.confidence,
        p.method,
        p.rationale,
        context(&draft.context)
    )
}
