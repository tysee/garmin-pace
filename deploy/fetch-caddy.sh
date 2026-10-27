#!/bin/bash
# deploy/fetch-caddy.sh fetches an explicitly requested, pinned Caddy release asset.
# The mise task supplies the version and archive digest; no host service is installed.
set -euo pipefail
umask 077
: "${PACE_CADDY_VERSION:?Run through mise run installer:fetch-https}"
: "${PACE_CADDY_SHA256:?A pinned archive digest is required}"
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
CACHE="$ROOT/dist/vendor/caddy-$PACE_CADDY_VERSION"
TEMP=$(mktemp -d)
trap 'rm -rf -- "$TEMP"' EXIT
curl --fail --location --proto '=https' --connect-timeout 10 --max-time 120 --retry 2 \
    "https://github.com/caddyserver/caddy/releases/download/v$PACE_CADDY_VERSION/caddy_${PACE_CADDY_VERSION}_linux_arm64.tar.gz" -o "$TEMP/caddy.tar.gz"
if command -v sha256sum >/dev/null; then digest=$(sha256sum < "$TEMP/caddy.tar.gz"); else digest=$(shasum -a 256 < "$TEMP/caddy.tar.gz"); fi
[[ ${digest%% *} == "$PACE_CADDY_SHA256" ]] || { printf 'Caddy archive checksum mismatch.\n' >&2; exit 1; }
tar -xzf "$TEMP/caddy.tar.gz" -C "$TEMP" caddy LICENSE
mkdir -p -- "$CACHE"
install -m 0755 "$TEMP/caddy" "$CACHE/caddy"
install -m 0644 "$TEMP/LICENSE" "$CACHE/LICENSE"
printf 'Verified Caddy %s ARM64 cached for packaging.\n' "$PACE_CADDY_VERSION"
