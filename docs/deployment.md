# Run Pace on Raspberry Pi or Podman

[Documentation](README.md) · [Project home](../README.md)

## Raspberry Pi

Target: 64-bit Raspberry Pi OS Bookworm or newer (glibc 2.36+, systemd). The same
installer archive exists for x86_64 Debian or Ubuntu hosts with glibc 2.36+. Build
on your development machine, not on the device, or download a tagged release.

Validation so far: on 2026-10-27 the installer archive was unpacked on a
Raspberry Pi (Debian 13 userland) and `./install.sh` was run from scratch. It
created the service account, installed the binary, configuration and unit, ran
the guided setup, started and enabled `garmin-pace.service` and delivered the key
file. Health answered 200 with the key and 401 without, documentation and OpenAPI
answered 200, MCP `initialize` answered, and the journal stayed clean. Resident
memory was 11.3 MB (VmRSS and VmHWM) with two threads while idle after those
requests; sync and MCP tool calls were not measured. CardputerZero remains
unvalidated on hardware.

These packaging commands use the maintainer's mise tasks on the build machine,
with Podman running; mise is not needed on the device:

```sh
mise run installer:fetch-https aarch64   # pinned, checksum-verified Caddy (deploy/release.conf)
mise run installer:package aarch64       # dist/pace-linux-aarch64.tar.gz + .sha256
```

Pass `x86_64` instead of `aarch64` for an x86_64 host; that build needs an x86_64
build host or Rosetta, since `rustc` crashes under QEMU emulation on Apple Silicon,
so use the release workflow there. Pushing a `v<version>` tag on a `main` commit,
matching the version in `Cargo.toml`, runs
[the release workflow](../.github/workflows/release.yml), which packages both
architectures with these scripts on native GitHub runners and attaches the archives
to the GitHub release, so other systems download a finished archive instead of
building:

```
https://github.com/tysee/garmin-pace/releases/download/v<version>/pace-linux-<arch>.tar.gz
https://github.com/tysee/garmin-pace/releases/latest/download/pace-linux-<arch>.tar.gz
```

Each archive has a `.sha256` file beside it. Tags on other branches and tags that
do not match the crate version stop the workflow before any build.

Copy the archive and checksum to the device, verify, unpack and run `./install.sh`
inside `pace-linux-<arch>/`. The easy setup asks for a profile name and sudo, then
installs the binary, creates a dedicated service account with private state, runs
the guided setup (including the key lifetime), checks authenticated MCP access and
enables the systemd service. It downloads nothing. Your MCP key lands in a private
file under `~/Pace/`, and the installer offers to open the menu.

Answer `advanced` to the setup type question to choose the program directory
(`bin/` and `lib/garmin-pace/` go there, default `/usr/local`) and the data directory
(registry, profiles, keys; default `/var/lib/garmin-pace`), and to pick between the
system service and a user-only install. The chosen mode and directories are recorded
in `install.env` (`/etc/garmin-pace/` or the data directory) and later runs keep them;
a flag that contradicts the record stops the installer instead of moving files.

<details>
<summary>Non-interactive install for agents</summary>

Every question has a flag, so an AI agent or a provisioning script can install
without a terminal:

```sh
./install.sh --yes --system --key-lifetime none            # service account + systemd, needs root
./install.sh --yes --user --key-lifetime 30d               # this user only, no root, no service
./install.sh --yes --user --prefix "$HOME/pace" --data "$HOME/pace/data" --key-lifetime 180d
```

`--yes` requires an explicit mode and key lifetime (`30d`, `180d` or `none`, stored
as 100 years); `--profile` defaults to `owner`. `--system` needs root or passwordless
sudo and a running systemd, and fails at once otherwise instead of degrading: give
the agent root (run as root, a `NOPASSWD` sudoers entry, or a root container) or use
`--user`. In user mode the binary and launcher go under `~/.local`, the data under
`~/.local/share/garmin-pace`, nothing is registered with systemd, and the agent keeps
`pace serve --transport http` running itself; it does not survive a reboot.
`--user` cannot add HTTPS ingress. The run proves the install with a temporary
server and `installation-check`, then prints `key=value` lines:
`pace_install_mode`, `pace_install_prefix`, `pace_install_data`,
`pace_install_key_file`, `pace_install_mcp_url`, `pace_install_menu_command` and,
in user mode, `pace_install_serve_command`. Garmin credentials are not part of a
non-interactive run; connect them later through the menu or `garmin` CLI commands.
Under the hood the run uses `pace setup --quick --batch --key-days N`, which
provisions the profile, registry, key and installation receipt exactly like the
wizard.

</details>

Running the installer again keeps config, history and credentials, issues another
key and leaves a modified service unit alone. The binary lives in
`<prefix>/lib/garmin-pace/pace`; the `pace` command in `<prefix>/bin` is a launcher
that runs every command against the last installed profile, so `pace` opens its menu
and `pace garmin sync ...` needs neither `--config` nor `--env-file`
from any directory. `--config` or `GARMIN_PACE_CONFIG` still selects another
configuration. In system mode the launcher runs as the service account through
`sudo runuser` (non-interactively with `sudo -n` when there is no terminal). The
installer offers to open the menu as its last step. If `<prefix>/bin` is not on your
`PATH`, the final message prints the full launcher path; add the directory to `PATH`
in your shell profile to type `pace` after a new login.

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

The menu's **Configuration → Internet access (HTTPS)** shows the configured domain,
MCP URL and `garmin-pace-https.service` state, and re-runs the certificate and
DNS-path check. It validates a new domain but cannot apply it: the menu runs as the
service account, so an administrator with sudo applies the change with
`./install.sh --https` from the release folder.

Manual deployment: [systemd unit](../deploy/garmin-pace.service),
[server config](../deploy/config.toml), [Caddyfile](../deploy/Caddyfile).

</details>

## Podman development

Requires the pinned Podman and, on macOS, a running Podman Machine. Base image
digests pin multi-arch manifest indexes; `--platform` selects Linux ARM64 or x86_64.

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
