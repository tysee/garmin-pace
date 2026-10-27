# Configuration, privacy and upgrades

[Documentation](README.md) · [Project home](../README.md)

[config.example.toml](../config.example.toml) documents every setting and resource
bound. Secrets live in the environment or a private dotenv file
([.env.example](../.env.example)); file values override environment values. The
terminal menu also reads `garmin.env` in the database directory.

Storage directories must be mode `0700`; databases, sidecars, keys, dotenv files
and Garmin sessions must be `0600` and owned by the process user. Symlinks, hard
links and looser permissions are rejected. Back up SQLite with its backup API or
with all writers stopped, keeping WAL/SHM files together.

Connect tokens are stored in plaintext inside the private session file because
refresh needs them. `garmin logout` deletes that file but does not revoke the
session at Garmin. Pace does not log credentials or protocol payloads. Anything
your assistant reads is shared with that client and its model provider.

Garmin requests have timeouts and size limits. Reads retry a bounded number of
times; logins, rate limits and writes with an unknown outcome stop and tell you.
Saved data stays readable after a failed sync, with its last successful sync time.

<details>
<summary>Upgrading an older development install</summary>

`GARMIN_PACE_HTTP_TOKEN` is gone. Add `[access]` plus the HTTP `max_connections`
and `connection_lifetime_seconds` settings from the current example, run
`pace users init`, issue per-user keys, fix ownership and modes, then run
`pace init`. These commands apply the current registry and profile migrations.
Profile schema 10 includes Coach storage and the standalone library index. Use
the `http` timeouts from the current example if the assistant should save to Garmin.

For an old Podman volume, stop everything and move the database, its WAL/SHM files
and `garmin-auth` together into `/data/private/` with private modes. Image builds
do not migrate volumes, and an empty database at the new path does not contain your
history.

</details>

## Garmin API access

- **Personal API key** reads activities and the seven Personal API health datasets.
- **Connect session** reads training readiness, strength sets, workouts, Coach and
  devices, and writes workouts and benchmarks. `pace garmin login` signs in with
  your email, password and MFA code,
  then stores refreshable tokens. Your password is not stored. This uses unofficial
  Connect endpoints, which Garmin may change or block.

Health sync needs both credentials because it imports all eight datasets. The
official [Training API](https://developer.garmin.com/gc-developer-program/enterprise/training-api/)
is not implemented; a Personal API key cannot write workouts.
