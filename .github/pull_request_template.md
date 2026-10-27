## Summary

<!-- What changes and why. Link the issue it resolves, e.g. "Closes #123". -->

## Verification

- [ ] `mise run check` passes (rustfmt, cargo check, clippy `-D warnings`, tests)
- [ ] Regression tests added or updated with synthetic fixtures
- [ ] Runtime change: CLI/MCP process check and Podman smoke test run
- [ ] Affected docs updated

## Checklist

- [ ] The change is focused and follows [AGENTS.md](https://github.com/tysee/garmin-pace/blob/main/AGENTS.md)
- [ ] No `.env`, local config, SQLite files, tokens or real Garmin data committed
