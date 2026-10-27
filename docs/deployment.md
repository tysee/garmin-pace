# Run Pace on Raspberry Pi or Podman

[Documentation](README.md) · [Project home](../README.md)

## Raspberry Pi

Target: 64-bit Raspberry Pi OS Bookworm or newer (glibc 2.36+, systemd). Build on
your development machine, not on the device.

Validation so far: on 2026-10-24 the ARM64 release binary was copied to a
Raspberry Pi and run by hand from the home directory with the example config
(`init`, access registry, user and key, `serve --transport http`). Health answered
200 with the key and 401 without, documentation and OpenAPI answered 200, and the
log stayed clean. Resident memory was 9.2 MB (VmRSS and VmHWM) while idle after
those requests, with one thread; sync and MCP tool calls were not measured.
The installer, systemd unit and CardputerZero remain unvalidated on hardware.

These packaging commands use the maintainer's mise tasks on the build machine,
with Podman running; mise is not needed on the device:

```sh
mise run installer:fetch-https   # pinned, checksum-verified Caddy for optional HTTPS
mise run installer:package       # dist/pace-raspberry-pi-linux-arm64.tar.gz + .sha256
```

Copy the archive and checksum to the device, verify, unpack and run `./install.sh`
inside `pace-raspberry-pi/`. The installer asks for a profile name and sudo, then
installs the binary, creates a dedicated service account with private state, runs
the guided setup, checks authenticated MCP access and enables the systemd service.
It downloads nothing. Your MCP key lands in a private file under `~/Pace/`.

Running the installer again keeps config, history and credentials, issues another
key and leaves a modified service unit alone.

<details>
<summary>Optional public HTTPS</summary>

Choose **Internet / HTTPS** during installation or run `./install.sh --https`
later. The wizard asks for a public domain and runs a separate `garmin-pace-https`
service with bundled Caddy and its own certificate storage. It leaves other web
servers alone and stops if ports 80 or 443 are taken.

The domain must point at the device's public address; forward TCP 80 and 443 if
needed and keep port 8080 private. The wizard can open the ports in an active
UFW/firewalld after confirmation. DNS, router and CGNAT setup is yours. Caddy issues
and renews certificates; errors are reported, not hidden. A passing local check
does not prove Internet reachability, so test from another network.

Manual deployment: [systemd unit](../deploy/garmin-pace.service),
[server config](../deploy/config.toml), [Caddyfile](../deploy/Caddyfile).

</details>

## Podman development

Requires the pinned Podman and, on macOS, a running Podman Machine. Image digests
are pinned for Linux ARM64.

```sh
mise run container:build
mise run container:init
mise exec -- podman run --rm --network none -v garmin-pace-data:/data \
  localhost/garmin-pace:dev users init
mise run container:run
mise run container:menu
mise run container:status
```

`container:run` keeps the MCP server running in a detached container;
`container:menu` opens the main terminal menu inside it with `podman exec`, with
Onboarding as its first entry. Closing the menu leaves MCP running and keys issued
there work at once. Onboarding's Start MCP step checks that the server answers.
If the menu is the container's own main process (`podman run --rm -it ... setup`),
MCP would stop with it, and the step prints the detached commands instead. On a
Linux install, the step shows `garmin-pace.service` and can enable and start it
with `sudo`.

The container runs as non-root, binds `127.0.0.1:8080` and stores state in the
`garmin-pace-data` volume under `/data/private/`. The 128 MiB memory limit is a
cap, not a measurement. HTTP rejects requests until a user and key exist:

```sh
mise exec -- podman exec garmin-pace-dev /app/pace --config /app/config.toml \
  users add owner --profile /app/config.toml
mise exec -- podman exec garmin-pace-dev /app/pace --config /app/config.toml \
  users issue-key owner --output /data/private/owner-mcp.key
```

`mise run container:sync` imports 20 activities using the ignored `.env`. For the
terminal menu in a container, add `--env-file .env` to `podman run` to supply the
key from the environment, or paste it once: it is saved to
`/data/private/garmin.env` in the volume. Host
SQLite and the volume are separate stores. `mise run container:export` writes
`dist/pace-linux-aarch64`; `mise run container:stop` removes the container and
keeps the volume.
