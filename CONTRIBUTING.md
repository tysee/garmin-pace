# Contributing to Garmin Pace

Thanks for your interest in Pace. Bug reports, documentation fixes and focused
pull requests are welcome. By participating you agree to follow the
[Code of Conduct](CODE_OF_CONDUCT.md).

## Reporting bugs and requesting features

Use [GitHub Issues](https://github.com/tysee/garmin-pace/issues) and pick the
matching template. For a bug, include the Pace version, OS and architecture, the
command, expected and actual behavior, and sanitized output. Remove tokens, keys,
Garmin account details and personal health data before posting.

Discuss larger changes in an issue before writing code.

Do not report security vulnerabilities in public issues; follow the
[security policy](SECURITY.md).

## Development environment

Maintainers use [mise](https://mise.jdx.dev/) for the pinned development tools and
tasks in [mise.toml](mise.toml). It is a development convenience, not a Pace runtime
requirement. With those tools already installed, `mise trust` and `mise run build`
build the debug binary; `mise exec -- pace ...` supplies its PATH and configuration.
The [Podman and installer packaging commands](docs/deployment.md) also use this
maintainer environment. Install missing tools only as an explicit setup step.

Tests use real temporary SQLite databases and run the CLI and MCP flows against
local Garmin fixtures; no Garmin account is needed. The code is organized by
feature: `history`, `health`, `workouts` and `access` own their logic and storage;
`garmin` adapts the external APIs; `interface` owns CLI, MCP, HTTP and the terminal.

## Pull requests

Open pull requests against `main`. Before opening one:

1. Read [AGENTS.md](AGENTS.md) for architecture and conventions.
2. Keep the change focused, update the affected docs and add regression tests with
   synthetic fixtures.
3. Run `mise run check` (rustfmt, cargo check, clippy with warnings denied, tests).
   Runtime changes also need a CLI/MCP process check and a Podman smoke test.
4. Never commit `.env`, local config, SQLite files, tokens or real Garmin data.

Commit messages use `<type>: <description>` with the types `feat`, `fix`,
`refactor`, `docs`, `test`, `chore`, `perf` and `ci`.

Contributions are licensed under the project's [MIT License](LICENSE).
