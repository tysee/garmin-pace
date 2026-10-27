#!/bin/bash
# deploy/fetch-caddy.sh fetches the pinned Caddy release asset for one Linux architecture
# (aarch64 or x86_64) into dist/vendor, verified against deploy/release.conf. No host service is installed.
set -euo pipefail
umask 077
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
# shellcheck source=deploy/release.conf
source "$ROOT/deploy/release.conf"
: "${PACE_CADDY_VERSION:?deploy/release.conf must pin PACE_CADDY_VERSION}"
ARCH=${1:-}
case "$ARCH" in
    aarch64) CADDY_ARCH=arm64; EXPECTED=${PACE_CADDY_LINUX_ARM64_SHA256:?deploy/release.conf must pin PACE_CADDY_LINUX_ARM64_SHA256} ;;
    x86_64) CADDY_ARCH=amd64; EXPECTED=${PACE_CADDY_LINUX_AMD64_SHA256:?deploy/release.conf must pin PACE_CADDY_LINUX_AMD64_SHA256} ;;
    *) printf 'Usage: fetch-caddy.sh aarch64|x86_64\n' >&2; exit 2 ;;
esac
[[ $# -eq 1 ]] || { printf 'Usage: fetch-caddy.sh aarch64|x86_64\n' >&2; exit 2; }
CACHE="$ROOT/dist/vendor/caddy-$PACE_CADDY_VERSION-$ARCH"
TEMP=$(mktemp -d)
trap 'rm -rf -- "$TEMP"' EXIT
curl --fail --location --proto '=https' --connect-timeout 10 --max-time 120 --retry 2 \
    "https://github.com/caddyserver/caddy/releases/download/v$PACE_CADDY_VERSION/caddy_${PACE_CADDY_VERSION}_linux_${CADDY_ARCH}.tar.gz" -o "$TEMP/caddy.tar.gz"
if command -v sha256sum >/dev/null; then digest=$(sha256sum < "$TEMP/caddy.tar.gz"); else digest=$(shasum -a 256 < "$TEMP/caddy.tar.gz"); fi
[[ ${digest%% *} == "$EXPECTED" ]] || { printf 'Caddy archive checksum mismatch.\n' >&2; exit 1; }
tar -xzf "$TEMP/caddy.tar.gz" -C "$TEMP" caddy LICENSE
mkdir -p -- "$CACHE"
install -m 0755 "$TEMP/caddy" "$CACHE/caddy"
install -m 0644 "$TEMP/LICENSE" "$CACHE/LICENSE"
printf 'Verified Caddy %s %s cached for packaging.\n' "$PACE_CADDY_VERSION" "$ARCH"
