# Project conventions

Follow the user's engineering rules and local tooling policy. Run development
tools through mise. Do not install tools or dependencies without authorization.

## Deployment and scope

- Project/package: `garmin-pace`; executable: `pace`; Rust library: `garmin_pace`.
  Project mise config adds the development binary directory to PATH and supplies
  `GARMIN_PACE_CONFIG`; explicit `--config` overrides it. HTTP credentials are issued with `pace users issue-key` and checked against the
  local access registry on every request. The shared environment token is retired. Configuration remains required outside that environment.

- Rust executable with embedded SQLite; Linux ARM64 on CardputerZero / Raspberry
  Pi Zero 2 W (512 MB RAM) is the deployment target. Podman on macOS is for local
  development only. No browser runtime, Python sidecar, or database server.
- This scaffold implements configuration, database initialization, local history
  reads, CLI diagnostics, and authenticated MCP over HTTP or local stdio.
- Garmin activity reads use the official Personal API. Strength exercise sets and
  Connect workout writes require a separate native OAuth session created by interactive `garmin login`. Report only observed synchronization
  and delivery states; FIT ingestion remains deferred.
- CLI technical output, terminal menus, and API contracts use English. `pace`
  without a subcommand opens a TTY-only interactive menu; `pace ui --compact`
  forces its small-screen layout. The browser surface is limited to Swagger API documentation.

## Boundaries

- `src/history/`: pure domain and use cases in `domain.rs` and `service.rs`;
  SQLite adapter in `sqlite.rs`; other features use its public `mod.rs` API.
  Local search supports optional inclusive UTC dates, exact sport and literal name
  substring (ASCII case-insensitive), with bounded pages and a matching row count.
  Count and page are read in one SQLite snapshot; no entire archive is materialized.
- `src/garmin/`: external Garmin HTTP adapters and boundary validation; history
  declares the activity source and transaction ports consumed by its sync use case.
  One client factory serves every Garmin host: optional `garmin.proxy_url` (no
  credentials) replaces the environment's HTTPS_PROXY/ALL_PROXY/NO_PROXY, and
  `garmin.extra_ca_file` merges PEM roots into the platform store; TLS verification is
  never disabled. Network failures are classified (DNS, proxy unreachable, tunnel
  refused, untrusted certificate, timeout, refused) with host and redacted route only;
  the environment route is decided per URL by reqwest's own hyper-util matcher (NO_PROXY).
  `pace doctor` probes each Garmin host once without keys through the same client.
  `pace config network` and the menu's Garmin network screen edit only those two `[garmin]`
  lines in place (`src/config/network.rs`, no TOML editing dependency): a re-parse must show
  no other change, the candidate loads and builds the client, then replaces the file atomically.
- `src/health/`: pure metric/date types and bounded sync/read services; SQLite and
  document adapters. Eight datasets commit atomically per day, failed days retain
  previous snapshots. MCP reads local data and explicit granted sync tools refresh it; all health reads share the
  authenticated profile used for activities. Readiness requires native Connect auth.
  Garmin's explicit empty sleep DTO (`id` and `calendarDate` null, no observation
  fields populated) is `no_data`; device flags and `sleepNeed` recommendations do
  not establish an observed sleep record. Undated observations remain errors.
  An empty Body Battery report may contain only timestamp/null placeholders with
  explicitly null descriptors and summary fields; it is `no_data`, not measured
  samples. Populated samples still require column descriptors.
- `src/sync/`: full sync over a period (at most 366 days): activities, strength sets, health and
  Coach in that fixed order over windows aligned to the period start (31, 31, 1 and 7 days).
  Step adapters in `src/interface/full_sync.rs` wrap the existing use cases; the feature owns
  ordering, outcomes and the SQLite run journal (`sync_runs`, windows and step results; newest
  20 runs kept). A missing credential skips its step with the reason; the first failed window
  ends only its step. `--resume` continues the latest unfinished run, skipping done windows;
  a new run always refetches. CLI `pace sync` and the menu's Sync everything (and onboarding's
  first full download) share it; MCP keeps its bounded one-dataset sync tools.
- `src/workouts/`: pure workout definitions, library/draft use cases and repository
  ports; document and SQLite adapters. Models author versioned local drafts through
  MCP; CLI/menu explicitly sync, save, reconcile and deliver them. MCP may save,
  reconcile and deliver only for a profile whose administrator granted Garmin writes
  (registry `garmin_writes`; `runtime.stdio_garmin_writes` over stdio), always with the
  user's quoted instruction journaled in `mcp_request` before the network call and shown
  in review output. Updates preserve
  remote identity, reject changed sources, and journal writes before execution.
  A definite Garmin refusal (HTTP 4xx) returns the draft to `draft` or releases the
  delivery reservation; only uncertain outcomes stay pending, and the user closes them
  explicitly with abandon/clear-delivery. Unsupported remote features stay read-only.
  The old test ledger is historical only.
- `src/workouts/coach/`: pure plan/window types, bounded import/read use cases and
  storage ports; Garmin DTOs stay in `src/garmin/coach.rs`. Coach snapshots and their
  workout library entries commit atomically. Imports cover at most seven Garmin
  calendar days and 100 plans/sessions. Failed imports retain previous snapshots.
  Coach uses Garmin as the schedule interface and the external model as the proposal
  author; never performs autonomous adaptation. Source-linked Coach drafts require
  an explanation. Relative strength load and minimum repetitions are preserved;
  load modes are exclusive. Saves retain the plan identity and verify it on read-back.
  One scheduled session can be moved to another Garmin calendar date by its schedule
  ID: the session must exist in an imported local window, the move is journaled in
  `coach_reschedules` before Garmin is contacted, the adapter reads the calendar entry,
  refuses a changed date or workout (conflict), PUTs only the new date and workout ID,
  and reads the entry back. HTTP 4xx closes the row as `not_applied`; an uncertain outcome
  stays `moving` until an explicit reconcile (GET only) or the CLI/menu clear; an entry
  Garmin no longer returns is recorded as `missing`. Garmin decides which plans allow a
  move and how far; Pace enforces no date policy beyond distinct valid dates. Shifting
  the rest of a plan, Coach preferences and the adaptive algorithm remain outside this
  scope. A moved session leaves both affected local windows stale until they are synced
  again. Repeated schedule entries may share a workout definition. The terminal
  separates standalone Workouts from Coach weeks; navigation is local and only
  explicit Sync contacts Garmin. The initial week is labelled as UTC; users can
  choose the dates in Garmin calendar terms. The standalone library filter has a
  partial SQLite index for bounded pagination.
- `src/access/`: user/key domain and service ports, SQLite registry and cryptographic
  key adapter. Store only SHA-256 hashes of randomly generated 256-bit keys.
  Each registered profile owns a distinct canonical database and Garmin session path,
  plus a `garmin_writes` grant that defaults to off and is set only by the local CLI.
  Every key has a definite expiry chosen when it is issued: the menu and the installer
  offer 30 days, 6 months or no expiry, stored as 100 years (`MAX_KEY_LIFETIME_DAYS`);
  the CLI `--days` and `access.key_lifetime_days` accept 1..=36500.
- `src/private_fs.rs`: shared owner-only filesystem boundary for credentials and SQLite.
- `src/workouts/benchmarks/`: benchmark catalogue, observed strength sets,
  evidence summaries and model proposals. Models estimate through MCP using real
  per-exercise history; local proposals require explicit CLI/menu review/save, or the
  granted `save_benchmark_draft_to_garmin_v1` with the user's quoted instruction.
  One activity's imported sets are readable by activity ID and UTC date, with the
  covering strength import reported explicitly (`available`/`no_data`/`not_synced`);
  the Activities menu and `get_activity_sets_v1` read them, never Garmin. A stored strength
  activity's sets can also be imported alone by ID (`benchmark sync-activity`, the menu's
  Import sets, granted `sync_activity_sets_v1`): one Connect `exerciseSets` read, no Personal
  API token, search or catalogue, and no window coverage recorded for its date. A set without
  one exact exercise keeps Garmin's agreed category for display only.
  Filled benchmarks require an actual user request. Proactive proposals are allowed
  only for an unset benchmark needed by a specific current unsaved workout draft.
  Strength imports advance by raw Garmin page size before filtering sibling sports;
  one import is bounded to 1,000 candidate activities and 100 strength activities.
- `src/installation/`: pure HTTPS domain/config rules, private installation receipts, and bounded HTTP/TLS readiness
  adapter; the terminal wizard composes existing profile, sync and access use cases.
- `src/interface/`: CLI, MCP, HTTP, and response DTOs. No business decisions.
- Workout overview totals and draft attention counts belong to `src/workouts/`;
  terminal summaries, positional change reviews and freshness are local read views.
  Main-menu state hints and quick sync shortcuts do not alter onboarding.
- `src/interface/terminal/`: terminal lifecycle, keyboard input, menu state and
  rendering; invokes the same CLI dispatcher and public feature boundaries.
  Network work is explicit and one-shot, with no idle network polling. `pace setup`
  and the Onboarding menu reuse CLI commands for profile preparation, initial imports
  and access administration. Start MCP first probes the server config's listener once without a key.
  With the installer's systemd unit it shows the unit state and can enable it via sudo;
  in Podman it directs to a detached server container with the menu opened through
  `podman exec`; elsewhere a confirmed launch hands the same process to foreground HTTP
  serving. Onboarding marks each step from a local,
  network-free self-check that never initializes missing storage; the Status screen shows
  the same findings grouped by area (local data, Garmin, MCP) with a remedy per row
  (`~` marks a key not yet used by an import, an expired access token or an unfinished full sync),
  and the main menu summarizes stored activities and the last successful sync in one line. CLI commands and the menu
  resolve the key file alike (`secrets::profile_key_file`): an explicit --env-file, else
  `garmin.env` beside the profile database, else the environment; a missing key is
  pasted hidden (only its length is shown), checked with Garmin and saved to a new
  private `garmin.env`. Terminal control uses the already-locked `rustix`.
- `src/config.rs` and `src/main.rs`: validated configuration and composition.
- `migrations/`: embedded, forward-only SQLite migrations.
- Public MCP endpoint: `/api/v1/mcp`. Health: `/api/v1/health`.
  Public, read-only API documentation is served at `/api/v1/docs` with its
  `/api/v1/openapi.json` contract. Swagger UI loads pinned, integrity-checked CDN
  assets in the visitor's browser; CDN failure affects only the UI, with a visible
  message and a local schema link. No Rust dependency or server-side fetch is needed.
  Swagger authorization stays in memory; external validation is disabled.
  MCP and health require an active, unexpired, administrator-issued per-user key.
  The access registry maps each key to exactly one canonical database; HTTP never
  falls back to the local default history. User/key administration is local CLI only. Local stdio relies
  on process and filesystem permissions. Browser requests must match the HTTP
  listener's origin (Host including port); cross-origin access is rejected without
  CORS. The managed HTTPS proxy validates its exact public HTTPS Origin before
  translating it to the loopback origin. Forwarded headers never authorize origins.
  Documentation uses the same host/origin and resource bounds but requires no key;
  executing API requests still requires the per-user Bearer key.
- SQLite failure is critical and explicit. Garmin failures preserve
  local history with an explicit last-successful-sync timestamp. Sync is a bounded
  one-shot operation with no background polling. MCP contacts Garmin only through the
  granted tools (sync, refresh, save, reconcile, send, device list, Coach reschedule and its
  reconcile) under the same authenticated
  profile: one Garmin operation per process at a time, on a dedicated thread outside the
  blocking pool that outlives the HTTP request timeout, never automatically and never on
  retry; the draft state and local delivery journal are the source of truth after a timeout.
  Sync calls require an explicit request, use only `garmin.env` beside the authenticated
  profile database, and never inherit a server-wide Personal API key. Health imports are
  one day per call, strength windows at most 31 days, a single activity's sets one ID per
  call, and activity/library imports one
  bounded page per call; callers advance the returned offset. Local history search uses
  inclusive UTC dates, sport and bounded pagination. Delivery reads distinguish absent,
  unresolved and accepted requests by remote revision; UI labels accepted requests
  "Sent to watch" while receipt remains unconfirmed. The v1 `queued` code stays stable
  for existing clients and gains an additive `status_label`; no queue polling or
  receipt inference from a device-sync timestamp.
  A remote workout can be explicitly refreshed by ID or discovered by library sync for
  reconciliation. Known exercise search returns only exact identities in imported
  editable workouts and benchmark catalogues; it is not Garmin's complete exercise catalogue.
  Native token refresh and login hold the session file lock; another process waits at most
  30 s for it (so a concurrent refresh is reused, not repeated) and then fails explicitly.
  Reading the saved session's expiry needs no lock.
  A persistent circuit breaker is deferred until there is a long-running caller.
  Adapters own timeouts, response caps and bounded GET retries. Every Garmin request is
  paced per host for the whole process; reads wait out HTTP 429 within configured bounds
  and slow later requests, and the waits are reported (menu footer or stderr). Writes fail clearly
  and are never automatically retried after an uncertain outcome.

## Resource and verification discipline

- Bound page sizes, SQLite cache, HTTP bodies, in-flight requests, and threads.
- Do not load an entire history or FIT archive into memory.
- Before completion: rustfmt, clippy with warnings denied, cargo check, real
  SQLite tests, CLI/MCP process tests, and a Podman runtime smoke test.
- Report measured memory separately from estimates. A successful container test
  does not prove operation on the physical target device.

## Production access and supervision

- Administrator-managed opaque keys are the HTTP auth contract; OAuth login/discovery
  is intentionally absent. Each user is bound to a unique canonical profile database
  and session path. Only local CLI can register users, issue/revoke keys or grant Garmin
  writes (`users allow-garmin-writes`, default off, re-read on every request). The CLI
  delivers keys to new private files; the terminal menu reveals a new key once on screen
  and stores nothing but its hash.
- Access registry failure is critical: reject the request, never use a default user.
  There are no network calls in this authorization path and no grant cache.
- Private Unix storage is required for SQLite and credentials. Do not log protocol
  payloads: only application tracing targets are enabled, including at debug/trace.
- `deploy/package.sh <arch>` builds `dist/pace-linux-<arch>.tar.gz` for aarch64 and
  x86_64 from the Containerfile, with Caddy pins in `deploy/release.conf`.
  `.github/workflows/release.yml` runs the same scripts on native runners for `v*` tags
  only (the tag must sit on `main` and equal the crate version) and attaches both
  archives to the GitHub release; there is no manual or branch build. The x86_64
  archive is the same installer for Debian/Ubuntu servers; it is not a device validation.
- `deploy/` contains Linux systemd and HTTPS proxy configuration. Native MCP binds
  loopback; public access requires HTTPS ingress and a firewall excluding port 8080.
  `deploy/install.sh` is the Linux installer. Easy setup keeps the fixed destinations
  (`/usr/local`, `/var/lib/garmin-pace`, `/etc/garmin-pace`), a dedicated service user,
  automatic profile paths and authenticated readiness before enabling boot startup;
  advanced setup chooses the program and data directories and a system or user-only
  install, and `--yes --system|--user --key-lifetime ...` answers everything from flags
  for agents (`setup --quick --batch` provisions without a terminal). The mode and
  directories are recorded in `install.env` and never moved by a later run. System mode
  requires root or passwordless sudo plus systemd and fails fast otherwise; user mode has
  no service account, no unit and no HTTPS, and the caller keeps `serve` running. The
  binary goes to `<prefix>/lib/garmin-pace/pace`; the generated `<prefix>/bin/pace`
  launcher exports `GARMIN_PACE_CONFIG` (unless already set) for the last installed profile
  and runs every command, the menu included, with it; system mode runs it as the service
  account (`sudo -n` without a terminal). Loopback readiness checks do not need system trust roots. It installs bundled assets only, never downloads packages.
  Optional HTTPS uses pinned bundled Caddy as a separate `pace-https` OS user and
  `garmin-pace-https.service`, with private certificate storage separate from Garmin.
  The wizard owns its generated proxy configuration, validates public domain input,
  and checks trusted TLS locally before sending a key. DNS-path checks send no keys.
  The menu's Internet access screen reads the recorded domain and unit state, runs the
  same check and validates a new domain; only the installer applies it (the menu has no
  root and gains no sudoers grant).
  ACME certificate pending/failure remains explicit; public reachability from an
  outside network is not inferred from a successful local check. Caddy owns certificate
  renewal/retry policy. DNS/router/CGNAT configuration remains outside the installer.
  Privileged steps reject symlinks, unexpected ownership and changed unit files.
  Device, public certificate issuance and external reachability remain deployment work; configured resource
  limits are not measurements.
