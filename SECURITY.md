# Security Policy

## Supported versions

Pace is in early development. Security fixes land on `main` and in the next
release; only the latest release is supported.

| Version | Supported |
| --- | --- |
| Latest release | Yes |
| Older releases | No |

## Reporting a vulnerability

Do not open a public issue, discussion or pull request for a suspected
vulnerability.

Report it privately through GitHub:
[Security → Report a vulnerability](https://github.com/tysee/garmin-pace/security/advisories/new).

Include, where possible:

- the Pace version, OS and architecture, and how Pace is deployed (local stdio,
  HTTP MCP, installer with HTTPS proxy, Podman);
- the affected component (for example access keys, HTTP origin checks, MCP tools,
  Garmin session storage, installer);
- steps to reproduce and the impact you observed.

Never include real access keys, Garmin credentials, session files or personal
health data. Use synthetic values.

## What to expect

This is a volunteer-maintained project. The maintainer aims to acknowledge a report
within 7 days, keeps you informed while a fix is prepared, and publishes a GitHub
security advisory with credit unless you prefer to stay anonymous.

## Scope

In scope: the `pace` binary, its MCP and HTTP interfaces, the access registry,
local credential and SQLite storage, and the files under `deploy/`.

Out of scope: Garmin's own services and APIs, third-party dependencies and the
bundled Caddy (report those upstream), and deployments that bypass the documented
HTTPS ingress and firewall requirements.
