# Pace usage guides

[Project home](../README.md)

Choose a workflow. All commands use the same `pace` binary and profile; examples
assume it is on PATH and `GARMIN_PACE_CONFIG` is set, or that you pass `--config`.
Dates and uppercase ID/revision placeholders must be replaced with your own values.

| I want to… | Guide |
| --- | --- |
| Connect Garmin and import my first data | [Getting started](getting-started.md) |
| Use the menu or script the CLI | [CLI and terminal](cli.md) |
| Review activities, health and recorded strength sets | [History and health](history-and-health.md) |
| Connect an AI assistant locally or remotely | [MCP setup and permissions](assistant.md) |
| Draft a workout, review it and send it to a watch | [Workouts and benchmarks](workouts.md) |
| Inspect my Coach week and approve specific adjustments | [Coach workflow](coach.md) |
| Run Pace in Podman or on a Raspberry Pi | [Deployment](deployment.md) |
| Manage configuration, private storage and upgrades | [Configuration and privacy](configuration.md) |

## What changes where?

| Action | Effect |
| --- | --- |
| List, show, search, review | Reads local snapshots; no Garmin request |
| Sync or refresh | Reads Garmin and updates the local cache |
| Create or revise a draft | Changes a local proposal only |
| Save | Writes the reviewed revision to Garmin and verifies it by reading back |
| Send | Requests delivery; acceptance does not confirm watch receipt |

Pace runs no model internally. Your connected assistant proposes changes; Pace
stores drafts, checks revisions and handles explicit Garmin operations. Agent
network tools require an administrator grant and an explicit user request.
