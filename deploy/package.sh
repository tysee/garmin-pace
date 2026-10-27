#!/bin/bash
# deploy/package.sh exports its exact ARM64 build into a self-contained installer archive.
# Run via mise run installer:package; only the explicit release assets enter dist/.
set -euo pipefail
umask 077
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
TEMP=$(mktemp -d)
CONTAINER=
cleanup() {
    if [[ -n "$CONTAINER" ]]; then podman rm "$CONTAINER" >/dev/null; fi
    rm -rf -- "$TEMP"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if command -v sha256sum >/dev/null; then
    hash() { sha256sum "$@"; }
else
    command -v shasum >/dev/null || { printf 'A SHA-256 tool is required. No tools were installed.\n' >&2; exit 1; }
    hash() { shasum -a 256 "$@"; }
fi
: "${PACE_CADDY_VERSION:?Run through mise run installer:package}"
CADDY_CACHE="$ROOT/dist/vendor/caddy-$PACE_CADDY_VERSION"
[[ -f "$CADDY_CACHE/caddy" && -f "$CADDY_CACHE/LICENSE" ]] || { printf 'Caddy is not cached. Explicitly run mise run installer:fetch-https first.\n' >&2; exit 1; }
podman build --platform linux/arm64 --iidfile "$TEMP/image-id" -f "$ROOT/Containerfile" "$ROOT"
CONTAINER=$(podman create --network none "$(cat "$TEMP/image-id")")
RELEASE="$TEMP/pace-raspberry-pi"
mkdir -- "$RELEASE"
podman cp "$CONTAINER:/app/pace" "$RELEASE/pace"
podman rm "$CONTAINER" >/dev/null
CONTAINER=
for asset in install.sh https.sh config.toml garmin-pace.service garmin-pace-https.service; do cp -- "$ROOT/deploy/$asset" "$RELEASE/$asset"; done
cp -- "$CADDY_CACHE/caddy" "$RELEASE/caddy"
cp -- "$CADDY_CACHE/LICENSE" "$RELEASE/CADDY-LICENSE"
cp -- "$ROOT/LICENSE" "$RELEASE/LICENSE"
chmod 0755 "$RELEASE/pace" "$RELEASE/install.sh" "$RELEASE/https.sh" "$RELEASE/caddy"
chmod 0644 "$RELEASE/config.toml" "$RELEASE/garmin-pace.service" "$RELEASE/garmin-pace-https.service" "$RELEASE/CADDY-LICENSE" "$RELEASE/LICENSE"
(cd -- "$RELEASE" && hash pace caddy CADDY-LICENSE LICENSE install.sh https.sh config.toml garmin-pace.service garmin-pace-https.service > SHA256SUMS)
chmod 0644 "$RELEASE/SHA256SUMS"
mkdir -p -- "$ROOT/dist"
ARCHIVE=pace-raspberry-pi-linux-arm64.tar.gz
COPYFILE_DISABLE=1 tar -czf "$TEMP/$ARCHIVE" -C "$TEMP" pace-raspberry-pi
mv -- "$TEMP/$ARCHIVE" "$ROOT/dist/$ARCHIVE"
(cd -- "$ROOT/dist" && hash "$ARCHIVE" > "$ARCHIVE.sha256")
printf 'Installer archive: %s/dist/%s\n' "$ROOT" "$ARCHIVE"
