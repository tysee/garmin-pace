# Use the CLI and terminal menu

[Documentation](README.md) · [Project home](../README.md)

Output is human-readable; add `--json` for scripts (`pace --json status`).
`pace --help` lists the command groups; `pace help --all` prints every command with its flags,
defaults and accepted values, and needs no `--config`.

| Command | Purpose |
| --- | --- |
| `pace`, `pace ui --compact` | Terminal menu, optionally in the small-screen layout |
| `pace setup` | Guided Garmin and MCP setup |
| `pace config check`, `pace config show` | Validate or print the non-secret config |
| `pace config network [--proxy-url URL \| --clear-proxy] [--extra-ca-file PATH \| --clear-ca]` | Show or change only the Garmin proxy and extra CA file |
| `pace status` | Local history and last sync |
| `pace doctor` | Check HTTPS to every Garmin host through Pace's own client, without keys |
| `pace sync --start DATE [--end DATE]` | Import activities, strength sets, health and Coach for a period in one journaled run |
| `pace sync --resume` | Continue the latest unfinished full sync, skipping windows it already imported |
| `pace garmin sync --limit 20` | Import a page of recent activities |
| `pace garmin sync --start DATE [--end DATE]` | Import every activity of a period, page by page |
| `pace garmin login`, `pace garmin auth-status`, `pace garmin logout` | Manage the local Connect session |
| `pace health sync --start DATE --end DATE` | Import health and readiness for a date range |
| `pace serve --transport stdio`, `pace serve --transport http` | Serve the saved data over MCP |

## Navigate the terminal

The CLI opens directly in Actions. Every row shows local state and a next step;
Tab expands the finding, purpose and action. Findings refresh when returning to
the menu, without background network requests. Draft counts include all active
records, even beyond the first page; library counts are explicitly bounded.
Session files do not establish live Garmin connectivity, and device delivery is
not inferred. Onboarding and administration retain their existing flows.

Workout and draft cards offer Summary: working sets, repetitions, loads, total
timed steps and timed rest, counting repeated blocks and omitted final rests.
Coach minimum repetitions remain a lower bound and relative load labels retain their meaning.
Repetition and Lap/distance steps have no assumed pace; the timed subtotal is not
a promise of total session duration. Change overview compares the immutable source
snapshot with the proposed draft by segment/step position, showing changed fields,
additions and removals; the complete before/after review remains available.

Data freshness separates the latest stored observation date from its import time.
Empty health snapshots do not count as observed metric data. Today, Yesterday and
Last 7 days shortcuts show exact inclusive dates before explicit confirmation;
today is based on UTC, while Garmin selects its own calendar dates. Custom periods
and recent-activity counts remain available. **Sync everything** runs the same full
sync as `pace sync` with these shortcuts or a custom period, and offers to continue
an unfinished run; Sync activities and Health → Import health import one dataset.
Nothing syncs on opening a screen.

The menu needs a TTY of at least 32×10 characters. Actions are grouped under
History, Garmin, Training and Configuration headings; Onboarding's first step
creates the configuration and initializes the database. Status lists local data,
Garmin access and MCP readiness with a mark per row: `√` ready, `~` usable but
not fully established (a saved Garmin key no import has used since it was written,
an expired access token that renews on the next Garmin request, a full sync that
did not cover its period) and `×` needs action. Enter on a row explains the finding
and names the action that changes it. Arrows or `j`/`k`
move, Enter selects, `/` searches the flat list, Esc goes back, Ctrl-C exits.
Network actions run only when you pick them; saving and delivery ask for
confirmation. Activities, Workouts, Workout drafts, Benchmark exercises, Garmin
devices and MCP users are tables, a page at a time: Enter opens a row (a workout
or draft opens as a table of its steps, a step shows its fields), Tab shows the
selected row's columns, and Next page / Previous page rows follow the table. On a
narrow screen the rightmost columns are dropped first.

Use PgUp/PgDn to scroll lists and complete reviews. Review text wraps to the
visible frame even on wide terminals. If a prompt's instructions do not fit,
Tab opens the full instructions while keeping the input visible. Text fields
and active search accept bracketed paste; pasted Enter never submits. When the
window is smaller than 32×10, resize it to continue; Ctrl-C still exits.

Activities offers a separate Search all history action: optional inclusive UTC
dates, exact sport and a literal name substring filter all saved rows before
pagination. The result count describes local history only. Name matching ignores
ASCII case; other scripts match literally. `/` continues to filter the visible
page and its actions. The CLI exposes the same filters with `activities search`.

After a verified workout save, the menu offers Send to watch or Back to drafts.
Each workout's Delivery status reads the local journal across devices and versions
without contacting Garmin. Accepted requests display **Sent to watch**, with a
clarification that Garmin accepted the request and watch receipt is not verified.
This is a completed send action, not a live queue status. Uncertain outcomes stay
**Outcome unknown**. An empty journal means no recorded request.
Neither an empty Garmin pending-message queue nor a device-sync timestamp proves
receipt of a specific workout revision. Older versions are labelled separately.
`pace workout deliveries --id ID [--device-id ID]` reads the same journal from
the command line, with `--limit` and `--offset` for bounded pages.

Ordinary menus retain their position after action errors and preserve non-secret
prompt values within the selected main-menu action. Pending saves reload their
durable state and offer reconciliation rather than another save. Delivery failures
open the journal for inspection; clearing an unresolved reservation requires an
explicit review after checking Garmin and the watch. No failed write is retried
automatically. These flows do not change onboarding.

Health lists the eight saved datasets with the days each one covers and holds
the menu's Import health. Open a dataset to walk its days in windows of
`health.max_days`, newest first, with headline values or "Not synced" per day;
open a day for its records and, for heart rate, HRV, Body Battery and SpO2, its
stored samples page by page. Everything there reads the local database only.

## Workouts or Coach?

| Menu | Use it for |
| --- | --- |
| **Activities** | Completed sessions and imported strength sets |
| **Workouts** | Reusable standalone workout definitions |
| **Coach** | Plan details, scheduled sessions and workout steps for a calendar week |
| **Workout drafts** | The shared queue of proposed changes, including Coach changes |
| **Benchmark exercises** | Stored one-rep-max estimates and evidence-backed proposals |

```sh
pace workout list --standalone
pace coach show --start 2026-10-26 --end 2026-11-01
pace coach reschedule --schedule-id SCHEDULE_ID --date 2026-11-02
pace coach reconcile-reschedule --schedule-id SCHEDULE_ID
pace coach clear-reschedule --schedule-id SCHEDULE_ID
pace workout drafts
```

`coach reschedule` writes to Garmin immediately after journaling the move; the two
follow-ups only read Garmin or close a journal row you checked yourself. See
[Move one session](coach.md#move-one-session-to-another-date).

Without `--standalone`, `workout list` includes imported Coach definitions too.
The Coach menu opens the current UTC week; choose another date if your Garmin
calendar week differs. A missing week stays marked as unsynced until you import it.

## Use the running Podman profile

```sh
mise run container:menu
mise exec -- podman exec garmin-pace-dev /app/pace --config /app/config.toml status
mise exec -- podman exec garmin-pace-dev /app/pace --config /app/config.toml \
  coach show --start 2026-10-26 --end 2026-11-01
```

The container volume and a host profile are separate stores. Use this prefix for
CLI examples when your account is configured inside Podman. See
[Podman setup](deployment.md#podman-development) for the initial container setup.
