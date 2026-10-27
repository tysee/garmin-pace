# Review and adjust a Garmin Coach week

[Documentation](README.md) · [Project home](../README.md)

## Inspect a week

Requires a saved Connect session (`pace garmin login`). Replace these example
dates with a window of at most seven days. Sync reads Garmin and updates the local
cache; it does not edit the remote plan.

```sh
pace coach sync --start 2026-10-26 --end 2026-11-01
pace coach show --start 2026-10-26 --end 2026-11-01
pace --json coach show --start 2026-10-26 --end 2026-11-01
```

`show` reads the exact imported window. It does not fetch missing dates or combine
different cached windows. In the terminal, open **Coach**, select a session, then
open **Workout steps** or the linked activity when available locally.

## Propose adjustments from results

1. Import the Coach week and the strength evidence period. For example:
   `pace --env-file .env benchmark sync --start 2026-10-01 --end 2026-10-25`.
   Health imports are optional context; missing observations must stay explicit.
2. Ask your connected assistant to read the plan, exact exercise sets and benchmark
   context. Have it propose local changes with reasons and uncertainty.
3. Open **Coach → Review drafts** or run `pace workout drafts` and
   `pace workout review --id DRAFT_ID`. Review the source plan, affected workout
   IDs/dates, sets, repetitions, loads and before/after definitions.
4. Stop here if you only wanted a proposal. Nothing has been saved in Garmin.

Example prompt:

> Read my Push, Pull, Legs Coach plan for October 26–November 1 and my recorded
> strength results. Propose workout and load changes as local drafts. Show
> before → after, explain the evidence and missing data, and do not save to Garmin.

The assistant should not treat `light`, `moderate` or `heavy` as fixed kilograms,
invent loads from missing sets, or treat a category-only set as an exact exercise.
Changing a workout's recommended load and updating its stored 1RM benchmark are
separate operations.

## Approve a specific revision and verify it

After review, a separate command such as “Save draft 12 revision 2 to Garmin”
authorizes the granted assistant to save that revision. The equivalent CLI action
is `pace workout save --id DRAFT_ID --revision REVISION`; it **writes to Garmin**
immediately and does not open an additional confirmation prompt.

Pace checks the source, journals the write and reads Garmin back. Inspect the draft
again and require `verified`; then re-run `coach sync` for the same week and check
the plan and schedule links. A timeout is not permission to retry: follow the
[pending-save procedure](workouts.md#command-reference).

## Supported behavior and limits

The terminal Training section separates **Workouts** (standalone library entries)
from **Coach** (plans and sessions by calendar week). Coach opens the current UTC
week locally, labels unsynced windows, and lets you choose another Garmin calendar
week, move backward/forward, explicitly sync, inspect plan details and workout
steps, and open the shared draft review queue. Opening or navigating never contacts
Garmin. `pace workout list --standalone` uses the same library filter.

Coach is Garmin's calendar and workout interface; the connected model proposes
changes through Pace. `pace coach sync --start DATE --end DATE` explicitly reads
active plans and scheduled workouts for at most seven Garmin calendar days. The
snapshot and editable workouts commit together; a failed import preserves the
previous snapshot. `pace coach show --start DATE --end DATE` reads that exact local
window, including plan IDs, schedule IDs and associated activity IDs. An absent
activity link is not proof that a workout was not performed.

MCP exposes `sync_coach_v1` (explicit granted import) and `get_coach_v1` (local
snapshot). The agent reads the current plan, stored activity sets and benchmark
context, then creates a source-linked workout draft with an `explanation` describing
the evidence, recommended loads, uncertainty and changes. Review returns BEFORE
and AFTER definitions. Saving is a separate explicit user instruction using the
existing revision-checked, journaled save tool, followed by Garmin read-back.
Coach load labels (`light`, `moderate`, `heavy`) and minimum repetitions round-trip;
absolute kilograms, benchmark percentages and relative load labels are exclusive.
Changing a relative label to kilograms must be an explicit proposal, never an
inferred conversion. The source plan ID is retained and checked on read-back.

Pace edits one scheduled workout definition at a time; it does not change Coach
preferences, reschedule sessions or replace Garmin's adaptive algorithm. Repeated
sessions referencing the same workout ID share that definition. The agent must
show the affected IDs/dates before saving. Refresh the Coach snapshot after a save
to check the calendar linkage. Garmin can subsequently change adaptive workouts;
Pace never adjusts anything in the background or claims a permanent override.
