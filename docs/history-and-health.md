# Review activity, health and strength history

[Documentation](README.md) · [Project home](../README.md)

## Import a period, then read it locally

Complete [setup](getting-started.md) first. The examples assume `pace` is on PATH,
`GARMIN_PACE_CONFIG` points to your profile, and `.env` is your private Personal API
key file. Replace the example dates with your own period.

```sh
# Import everything for a period: activities, strength sets, health, Coach.
pace sync --start 2026-10-19 --end 2026-10-25
# After a failure or a stop, continue where it ended.
pace sync --resume

# Or import one dataset: the local activity cache only.
pace --env-file .env garmin sync --start 2026-10-19 --end 2026-10-25

# Search saved history without contacting Garmin.
pace activities search --start 2026-10-19 --end 2026-10-25 --limit 20
pace activities search --sport strength_training --limit 20
pace activities search --name "Morning" --limit 20
pace activities list --limit 20 --offset 20

# Read Garmin health and readiness; both Garmin credentials are required.
pace --env-file .env health sync --start 2026-10-19 --end 2026-10-25

# Inspect the imported days and a page of stored samples.
pace health list --start 2026-10-19 --end 2026-10-25
pace health show --date 2026-10-25
pace health samples --date 2026-10-25 --metric heart_rate --limit 100
pace --json health show --date 2026-10-25
```

Activity imports select Garmin local start dates; local activity searches use
inclusive UTC dates. Health dates use Garmin's calendar, with sleep assigned to
the wake-up day. Near midnight these scopes may differ.

Health imports cover at most 31 days per CLI call and commit each day atomically.
An activity period imports page by page. Completed pages and days survive a later
failure.

`pace sync` (menu: **Sync everything**) runs the steps in order over windows aligned
to the period start: activities in 31-day windows, strength sets in 31-day windows,
health one day at a time and Coach in 7-day windows, for up to 366 days. Activities
need the Personal API key; health and strength need the key and the Connect login;
Coach needs the login. A step whose credential is missing is skipped with the reason.
The first failed window ends its own step, and the later steps still run. Each window
is journaled in the profile database as it finishes, so the report says per step
whether it is complete, partial (with the failing window and cause), failed or
skipped, and the command exits non-zero unless every step is complete.
`pace sync --resume` continues the latest unfinished run and skips the windows it
already imported; a plain `pace sync` always fetches the whole period again, so
today's data is refreshed. Reads retry within the Garmin request bounds; writes
are never part of a sync. In the terminal, **Sync everything**, **Sync activities**
and **Health → Import health** offer date shortcuts and explicit confirmation;
viewing saved data never starts a sync.

## Understand freshness and missing data

- `not_synced`: no local import covers the requested day or evidence window.
- `no_data`: the import completed, but Garmin supplied no observations.
- An import timestamp says when Pace fetched data, not when it was measured.
- Readiness keeps each snapshot's time and context; an old score is not today's score.
- Body Battery is Garmin's energy estimate, not the watch battery level.

An empty sleep record with device flags or a sleep-need recommendation is still
missing observed sleep. Body Battery timestamp placeholders with null values are
not measured samples. Missing values are never replaced with zero.

## Review actual strength sets

```sh
pace --env-file .env benchmark sync --start 2026-10-01 --end 2026-10-25
pace benchmark sync-activity --id 501
pace benchmark list
pace benchmark context --key BARBELL_BENCH_PRESS --start 2026-10-01 --end 2026-10-25
```

Open **Activities**, choose a stored strength activity, and inspect its sets or
import them from Garmin. `benchmark sync-activity --id ID` (menu: **Import sets from
Garmin**) reads only that activity's sets with the Connect session: no Personal API
token, activity search or benchmark catalogue. It does not mark the date as covered
for other activities.
An assistant uses `get_activity_sets_v1` with that activity's Garmin ID, and with the
Garmin grant can import them with `sync_activity_sets_v1`. Sets are
available only after a strength import covered its UTC date or imported that activity. Category-only records
remain visible but cannot establish an exact exercise's benchmark.

See [benchmark proposals](workouts.md#benchmark-exercises) for the separate
review-and-save workflow.

## Ask your assistant

> Compare my activities, sleep and morning readiness for October 19–25. State the
> dates of the observations and flag missing data. Read local data only.

If newer data is needed, explicitly request an import. Agent imports require the
[Garmin access grant](assistant.md#garmin-writes-from-your-assistant).

## Supported data

| Data | Fields | Source |
| --- | --- | --- |
| Activities | ID, name, sport, start time, duration, distance | Personal API |
| Daily summary | Steps, floors, calories, intensity minutes, stress, hydration, respiration | Personal API |
| Heart rate | Resting, minimum, maximum, samples | Personal API |
| Sleep | Duration, stages, awakenings, score, quality | Personal API |
| HRV | Overnight summary, baseline, status, samples | Personal API |
| Body Battery | Summary, charge and drain, interval minimum/maximum samples | Personal API |
| SpO₂ | Summary and samples | Personal API |
| Weight | Weight and body composition | Personal API |
| Training readiness | Every snapshot with timestamp, context and factors | Connect session |

What you get depends on your device and account. Missing data is reported as
missing, never as zero. Body Battery is Garmin's energy estimate, not the watch
battery.
