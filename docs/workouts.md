# Draft workouts and benchmark changes

[Documentation](README.md) · [Project home](../README.md)

Pace keeps a local copy of your Garmin workout library, separate from completed
activities. An assistant connected over MCP reads that library and proposes a new
workout or a revision of an existing one as a **draft**. You review the draft in
the terminal or CLI, then save it to Garmin and optionally send it to a watch.
Pace does not run a model itself and writes to Garmin only when you save in the
terminal or CLI, or when your assistant, with an administrator grant, calls a write
tool quoting your instruction.

A draft is a v1 JSON document: name, description, sport, segments, steps and repeat
groups. Steps end by time, distance, repetitions, minimum repetitions or lap button and carry explicit
targets (seconds per km, bpm, watts or cadence) and optional exercise details. A
strength exercise names its Garmin category and exercise and carries at most one
load: `weight_kg`, or `benchmark_percentage` (1-100) together with the
`benchmark_key` whose one-rep max it refers to (a key from `list_benchmarks_v1`,
such as `BARBELL_DEADLIFT`), or a `relative_load` label (`light`, `moderate`,
`heavy`). These load modes are mutually exclusive. Workouts that use features the editor does not
understand stay visible but read-only, with the reason shown. Their steps are
still readable as a display-only `outline` (in `get_workout_v1`, `workout show`
and the menu), which is never written back.

Garmin's explicit `targetType: null` with empty target values and zone means no
intensity target, as used by Coach warmups and cooldowns. Pace imports these steps
as editable and writes the equivalent `no.target` representation. A missing or
malformed type, or target values without a type, remains read-only; benchmark
percentages, repetitions and repeat groups keep their independent meaning.

MCP tools: `list_workouts_v1`, `get_workout_v1`, `create_workout_draft_v1`,
`save_workout_draft_to_garmin_v1`, `reconcile_workout_draft_v1`,
`send_workout_to_device_v1`, `list_garmin_devices_v1` (the last four need the grant),
`list_workout_drafts_v1`, `get_workout_draft_v1`, `revise_workout_draft_v1`.

## Draft, review, save, then deliver

1. Run `pace workout sync --start 0 --limit 20` to import a library page.
2. Ask your connected assistant to draft a workout locally, or use
   `pace workout draft --file workout.json` with a v1 workout document.
3. Run `pace workout drafts`, then `pace workout review --id DRAFT_ID` to inspect
   the full before/after definition and current revision. Replace uppercase
   placeholders with values from the output.
4. **Writes to Garmin:** only when you approve that exact revision, run
   `pace workout save --id DRAFT_ID --revision REVISION`.
5. Check that the result is `verified`. Saving alone does not request delivery.
6. Optionally run `pace garmin devices`, then **request delivery** with
   `pace workout send --id WORKOUT_ID --device-id DEVICE_ID`. Use the Garmin
   workout ID returned after saving, not the local draft ID.
7. Inspect `pace workout deliveries --id WORKOUT_ID` and check the watch after sync.

Example assistant request:

> Create a local strength workout draft using my recorded exercise history.
> Explain the recommended loads and show before → after. Do not save to Garmin.

Editing an existing workout requires its source ID and revision from
`pace --json workout show --id WORKOUT_ID`; the assistant reads the same fields
through MCP. [Coach drafts](coach.md) additionally require an explanation.

## Command reference

Prefix each command below with `pace`. IDs and revisions come from list and review output.

| Command | Purpose |
| --- | --- |
| `workout sync [--start N --limit N]` | Import a page of the Garmin library |
| `workout list [--standalone]`, `workout show --id ID` | Browse the local library |
| `workout draft --file FILE [--source-id ID --source-revision REV] [--explanation TEXT]` | Import a draft from a file |
| `workout drafts`, `workout review --id ID` | List drafts, show before/after |
| `workout revise --id ID --revision N --file FILE [--explanation TEXT]` | Replace an unsaved draft |
| `workout save --id ID --revision N` | Create or update the Garmin workout, then verify |
| `workout reconcile --id ID --workout-id ID` | Resolve a save with an unknown outcome |
| `workout discard --id ID --revision N` | Drop an unsaved draft |
| `workout abandon --id ID --revision N` | Close a pending save you have checked yourself |
| `workout send --id ID --device-id ID` | Ask Garmin to deliver a workout to a device |
| `workout clear-delivery --id ID --device-id ID` | Drop a pending delivery you have checked yourself |

Saving re-reads Garmin first and refuses if the source changed since the draft was
made. After a write, Pace reads the workout back and verifies it. Writes are
journaled and never retried automatically. If Garmin refuses a write outright
(HTTP 4xx), the draft returns to `draft` and a delivery reservation is released. If
the outcome is unknown, the draft or delivery stays pending until you `reconcile`
it, or close it with `abandon` / `clear-delivery` after checking Garmin and the
watch yourself. Garmin accepting a delivery does not prove the watch received it;
check the watch after it syncs.

## Benchmark exercises

Garmin benchmarks are per-exercise one-rep-max estimates. Pace imports the
benchmark catalogue and your recorded strength sets for a date window of up to
31 days. An assistant can then estimate a benchmark from the observed sets for that
exact exercise and store the estimate as a local **proposal** with its method,
rationale and confidence. You review and save it, exactly like a workout draft.

Rules the service enforces:

- A filled benchmark changes only when you asked for it. An unset benchmark may be
  proposed without a request only when a specific unsaved workout draft needs it.
- A proposal needs real loaded sets for that exercise in a fully imported window.
  No usable sets means no estimate, not a zero.
- Only active sets with an external load and 1–20 reps count; rest, bodyweight,
  ambiguous and other exercises are reported as excluded.

MCP tools: `list_benchmarks_v1`, `get_benchmark_context_v1`, `propose_benchmark_v1`,
`list_benchmark_drafts_v1`, `get_benchmark_draft_v1`, `revise_benchmark_draft_v1`,
`get_activity_sets_v1`, and with the grant `save_benchmark_draft_to_garmin_v1` and
`reconcile_benchmark_draft_v1`.
The imported sets of one activity are also readable per activity with
`get_activity_sets_v1`, and the Activities menu shows them in a strength
activity's details. A set Garmin recorded without one exact exercise keeps only
its category; such sets are shown but never count as benchmark evidence.

CLI: `benchmark sync --start DATE --end DATE`, `benchmark sync-activity --id ID`,
`benchmark list`,
`benchmark context --key KEY --start DATE --end DATE`, `benchmark propose --file FILE`,
`benchmark drafts`, `benchmark review --id ID`,
`benchmark revise --id ID --revision N --file FILE`,
`benchmark save --id ID --revision N`, `benchmark reconcile --id ID`,
`benchmark discard --id ID --revision N`, `benchmark abandon --id ID --revision N`.
Sync lists strength activities through the Personal API and reads their sets and
the benchmarks through the Connect session; saving uses the Connect session and
verifies the read-back. As with workouts, a Garmin
refusal returns the proposal to `draft`, and `abandon` closes a save whose outcome
you have checked yourself.
