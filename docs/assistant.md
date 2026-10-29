# Connect an AI assistant over MCP

[Documentation](README.md) · [Project home](../README.md)

## Local stdio

Register the binary with your client from the repository root. The client starts
Pace itself; no separately running server or access key is needed. Initialize the
profile first, then build the release binary used in these examples:

```sh
mise exec -- cargo build --release --locked --bin pace
```

```sh
# Codex
codex mcp add garmin-pace -- \
  "$PWD/target/release/pace" --config "$PWD/config.toml" serve --transport stdio

# Claude Code
claude mcp add --transport stdio garmin-pace -- \
  "$PWD/target/release/pace" --config "$PWD/config.toml" serve --transport stdio
```

Rebuild with `mise exec -- cargo build --release --locked --bin pace` after source changes
and reconnect the server in your client. Use the same config for sync and MCP so new imports are visible.
The current catalogue has 39 tools, including `get_coach_v1`, `sync_coach_v1` and
`reschedule_coach_session_v1`.
If an existing Codex connection still shows 34, reconnect `garmin-pace` in the
client's MCP settings to reload its tool definitions. Verify both Coach tools are
available; `get_capabilities_v1` alone checks the running server, not the client's
cached catalogue.
Client docs: [Codex MCP](https://learn.chatgpt.com/docs/extend/mcp?surface=cli),
[Claude Code MCP](https://code.claude.com/docs/en/mcp).

## MCP tools

Agent workflows also expose `get_capabilities_v1`, `search_activities_v1` (optional
inclusive UTC dates, sport and name substring, limit and offset; returns the matching total), and `get_activity_v1` (stored Garmin ID).
The original `list_activities` contract stays available for existing clients.
`list_workouts_v1` accepts `standalone: true` to exclude Coach plan workouts.
`get_workout_deliveries_v1` reads the local request journal (one device, or every device
when `device_id` is omitted) with revision, device,
`sending`/`queued` state and pagination. The additive `status_label` displays
`queued` as **Sent to watch**, meaning Garmin accepted the send request, not a live
queue observation. The v1 state codes remain stable for existing MCP clients.
No matching row means no recorded request. Watch receipt is always `unconfirmed`.
`search_exercises_v1` searches exact identities from editable library snapshots and
benchmark catalogues, including linked exercises. An empty result means sync those
sources or supply a supported exercise; it is not a complete Garmin catalogue.

With the existing Garmin-writes grant, explicit agent requests can call
`sync_activities_v1` (one bounded page, optionally scoped to at most 31 Garmin local dates),
`sync_health_v1` (one Garmin calendar day), `sync_strength_v1` (up to 31 UTC days),
`sync_activity_sets_v1` (one stored strength activity's sets by Garmin ID, Connect
session only, no activity search),
`sync_workouts_v1` (one library page), and `refresh_workout_v1` (one remote ID).
Use returned offsets to continue imports. Completed pages/days remain available
after a later failure. Personal API credentials must be in private `garmin.env`
beside that profile's database; HTTP never uses the server environment's key.
These calls share the Garmin concurrency gate with writes and may outlive an HTTP
timeout. Check capabilities for a busy server and read local snapshots before
continuing; never repeat an uncertain save or delivery. For a lost create response,
sync the library, inspect the matching definition and use its ID for reconciliation.
Pending-delivery closure, pending-save abandonment and clearing an unresolved Coach
move remain explicit CLI actions after the user checks Garmin; the agent cannot confirm
physical watch receipt.

stdio and HTTP expose the same tools. The original read-only tools remain available:

| Tool | Arguments | Result |
| --- | --- | --- |
| `get_status` | — | Activity count and sync status |
| `list_activities` | `limit` (optional) | Saved activities, newest first |
| `get_activity_sets_v1` | `garmin_id` | Exercise, load and reps per set of one stored strength activity |
| `get_daily_health` | `date` | Daily health and all readiness snapshots |
| `list_health_metrics` | `start`, `end` | Daily summaries plus missing dates |
| `get_health_samples` | `date`, `metric`, `limit`, `offset` (optional) | Paginated samples with units |

Dates are `YYYY-MM-DD` in Garmin's calendar; sleep belongs to the wake-up day.
Sample metrics: `heart_rate`, `body_battery`, `hrv`, `spo2`. Timestamps are UTC.
A day that was never imported is `not_synced`; an imported day without
measurements is `no_data`. Garmin may return an empty sleep record with
device flags and a sleep-need recommendation; this is `no_data` for observed sleep,
not zero sleep. A response containing undated sleep observations is rejected.
Body Battery's timestamp/null placeholders with null descriptors and no summary
are also `no_data`; populated samples still require valid column descriptors.
Activity sets use the same statuses and exist locally
only after `benchmark sync` covered the activity's UTC date or that activity's sets
were imported by ID. Readiness snapshots
keep their original time and context. Body Battery samples are interval extrema (720 intervals per day by
default), not raw sensor readings.

See [workout and benchmark tools](workouts.md) and [Coach workflows](coach.md).
`get_coach_v1` reads a local week; `sync_coach_v1` explicitly imports up to seven
Garmin calendar days under the same access grant. Workout proposal tools write
local drafts. With an explicit grant your assistant can also save,
reconcile and deliver them on your instruction; see
[Garmin writes from your assistant](assistant.md#garmin-writes-from-your-assistant).

## HTTP and multiple users

Use Streamable HTTP when Pace runs on another machine. `/api/v1/mcp` and
`/api/v1/health` both require a per-user Bearer key. Public access needs HTTPS.

Open [Swagger UI](http://127.0.0.1:8080/api/v1/docs) on a running local server,
or `/api/v1/docs` on your HTTPS deployment. The documentation and
[/api/v1/openapi.json](http://127.0.0.1:8080/api/v1/openapi.json) are public and
contain no profile data. Click **Authorize**, enter your per-user key, and use
**Try it out** to check health or send an MCP JSON-RPC request. The MCP examples
cover initialization, tool discovery and reading local status; other tools are
discovered through `tools/list`, not separate REST endpoints.

Swagger UI loads version-pinned, integrity-checked assets from jsDelivr, so the
browser needs Internet access to render it. The local OpenAPI schema and API keep
working without the CDN. Authorization is not persisted across page reloads;
requests and schema validation are never sent to an external validator.

<details>
<summary>Provision a profile for HTTP</summary>

```sh
pace users init
pace users add owner --profile "$PWD/config.toml"
pace users issue-key owner --output "$PWD/data/owner-mcp.key"
pace serve --transport http
```

The CLI writes the key to a new private file and never prints it. In the terminal
menu, Onboarding step 8 lists MCP users with their key state and shows a new key
once on screen for copying instead; Configuration → MCP users lists every user and
key. Pace stores only key hashes. Configure the client to
send `Authorization: Bearer <key>` to `http://127.0.0.1:8080/api/v1/mcp` locally,
or `https://your-domain/api/v1/mcp` remotely. Keep the key out of URLs and shell
arguments.

Each additional user gets its own config, database and Connect session, registered
against the server config:

```sh
pace --config /path/to/server/config.toml users add alice \
  --profile /path/to/alice/config.toml
```

Every request maps its key to one registered profile; there is no default user.
The registry stores key hashes, expiry and revocation. The menu and the installer
ask for the lifetime of each new key: 30 days, 6 months or no expiry, which is
recorded as 100 years. The CLI defaults to `key_lifetime_days` from the
configuration (30 days) and accepts `--days 1..36500`. Inspect with `users keys USER`, revoke with
`users revoke-key KEY_ID`, suspend with `users disable USER`; let a user's assistant
write to Garmin with `users allow-garmin-writes USER` and take it back with
`users deny-garmin-writes USER`; changes apply on the next request. There is no OAuth: clients must support a configured Bearer header.
Browser requests must use the server's own origin; cross-origin requests are
rejected. The managed HTTPS proxy validates its public HTTPS origin before
forwarding to loopback. The native listener binds loopback by default.

</details>

## Garmin writes from your assistant

By default an assistant connected over MCP can only read local data and write local
drafts. An administrator can let one user's assistant finish the flow from chat:

```sh
pace users allow-garmin-writes owner   # undo with: pace users deny-garmin-writes owner
```

Over local stdio, where there is no registry, set `stdio_garmin_writes = true` under
`[runtime]` in that profile's config instead. The grant enables the explicit
sync/refresh tools described here, including `sync_coach_v1`, and these eight Garmin operations:
`save_workout_draft_to_garmin_v1`, `reconcile_workout_draft_v1`,
`send_workout_to_device_v1`, `list_garmin_devices_v1`,
`save_benchmark_draft_to_garmin_v1`, `reconcile_benchmark_draft_v1`,
`reschedule_coach_session_v1` and `reconcile_coach_reschedule_v1`. Without the
grant they answer "not enabled" and contact nothing. A Coach move takes the schedule
ID from `get_coach_v1` and one new Garmin calendar date; see [Coach](coach.md).

Every write takes a `user_request`: your instruction, quoted by the assistant
(1 to 1000 bytes, no control characters). Pace cannot check that you really said
it; the grant is the boundary, the quote is the audit trail. It is stored with the
draft or delivery before Garmin is contacted and shown by `workout review`,
`benchmark review` and the menu as "Saved via MCP on user request".

One Garmin operation runs at a time per server; a second call answers "another
Garmin operation is running". A save may outlast the HTTP request timeout, and the
assistant then sees a timeout while the save still completes. The draft state is the
truth: `draft` means nothing was applied, `saving` means in flight or unresolved,
`saved` means accepted but unverified, `verified` means done. Read the draft before
calling again and never save twice. Watch receipt is never confirmed, exactly as in
the menu. The example configs raise `http.request_timeout_seconds` to 60 and
`http.connection_lifetime_seconds` to 90 for this.
