#!/bin/bash
# deploy/install.sh installs the bundled ARM64 release on a systemd-based Raspberry Pi.
# Its interactive entry point preserves existing state and starts verified local MCP access.
set -euo pipefail
export PATH=/usr/sbin:/usr/bin:/sbin:/bin
umask 077

SERVICE_USER=garmin-pace
STATE=/var/lib/garmin-pace
CONFIG=/etc/garmin-pace/config.toml
BINARY=/usr/local/bin/pace
UNIT=/etc/systemd/system/garmin-pace.service
BUNDLE=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
HANDOFF_DIR=
BINARY_TEMP=
HTTPS_TEMP=
HTTPS_ONLY=no

fail() { printf '\nSetup stopped: %s\n' "$*" >&2; exit 1; }
root() { if (( EUID == 0 )); then "$@"; else sudo -- "$@"; fi; }
cleanup() {
    if [[ -n "$HTTPS_TEMP" ]]; then root rm -rf -- "$HTTPS_TEMP"; fi
    if [[ -n "$BINARY_TEMP" ]]; then root rm -f -- "$BINARY_TEMP"; fi
    if [[ -n "$HANDOFF_DIR" ]]; then
        root rm -f -- "$HANDOFF_DIR/key"
        root rmdir -- "$HANDOFF_DIR"
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

preflight() {
    [[ $(uname -s) == Linux && $(uname -m) == aarch64 ]] || fail 'Use 64-bit Raspberry Pi OS (Bookworm or newer) on an ARM64 device.'
    [[ -t 0 && -t 1 ]] || fail 'Open a terminal on the Raspberry Pi, then run ./install.sh interactively.'
    [[ -d /run/systemd/system ]] || fail 'This installer needs a running systemd system.'
    for tool in systemctl runuser useradd getent install mktemp sha256sum timeout stat cp mv cut id chmod chown rm rmdir mkdir tee env sleep; do
        command -v "$tool" >/dev/null || fail "Required OS command is missing: $tool. No packages were installed."
    done
    if (( EUID != 0 )); then command -v sudo >/dev/null || fail 'Run this installer from an administrator account with sudo.'; fi
    [[ -x "$BUNDLE/pace" ]] || fail 'Extract the complete release archive first.'
    (cd -- "$BUNDLE" && sha256sum --check --status SHA256SUMS) || fail 'Release files are incomplete or damaged. Extract a fresh trusted archive.'
    "$BUNDLE/pace" --version || fail 'This release requires glibc 2.36 or newer (Raspberry Pi OS Bookworm).'
}

# Root-owned deployment parents must never resolve through a writable link or directory.
public_directory() {
    local path=$1
    [[ -d "$path" && ! -L "$path" ]] || fail "Expected a real system directory: $path"
    [[ $(stat -c %u -- "$path") == 0 ]] || fail "System directory must belong to root: $path"
    local mode
    mode=$(stat -c %a -- "$path")
    (( (8#$mode & 0022) == 0 )) || fail "System directory must not allow group/other writes: $path"
}

private_directory() {
    local path=$1
    if ! root test -e "$path" && ! root test -L "$path"; then
        if [[ "$path" == "$STATE" ]]; then
            root install -d -o "$SERVICE_USER" -g "$SERVICE_USER" -m 0700 -- "$path"
        else
            root runuser -u "$SERVICE_USER" -- mkdir -m 0700 -- "$path"
        fi
    fi
    root test -d "$path" && ! root test -L "$path" || fail "Expected a real private directory: $path"
    [[ $(root stat -c '%u:%a' -- "$path") == "$SERVICE_UID:700" ]] || fail "Private directory has unexpected ownership or permissions: $path"
}

check_system_file() {
    local path=$1
    if [[ -e "$path" || -L "$path" ]]; then
        [[ -f "$path" && ! -L "$path" ]] || fail "Expected a regular system file: $path"
        [[ $(stat -c '%u:%h' -- "$path") == 0:1 ]] || fail "System file has unexpected ownership or hard links: $path"
        local mode
        mode=$(stat -c %a -- "$path")
        (( (8#$mode & 0022) == 0 )) || fail "System file must not allow group/other writes: $path"
    fi
}

prepare_account() {
    if ! getent passwd "$SERVICE_USER" >/dev/null; then
        getent group "$SERVICE_USER" >/dev/null && fail 'A garmin-pace group already exists without its service account. Ask an administrator to review it.'
        root useradd --system --user-group --no-create-home --home-dir "$STATE" --shell /usr/sbin/nologin "$SERVICE_USER"
    fi
    local name password gid description home shell
    IFS=: read -r name password SERVICE_UID gid description home shell < <(getent passwd "$SERVICE_USER")
    [[ "$SERVICE_UID" != 0 && "$home" == "$STATE" && "$shell" == /usr/sbin/nologin ]] || fail 'Existing garmin-pace account does not match the dedicated service account.'
    [[ $(getent group "$SERVICE_USER" | cut -d: -f3) == "$gid" ]] || fail 'Service account group does not match.'
    private_directory "$STATE"
    private_directory "$STATE/profiles"
    private_directory "$STATE/profiles/$PROFILE"
}

install_release() {
    for directory in /usr /usr/local /usr/local/bin /etc /etc/systemd /etc/systemd/system /var /var/lib; do
        public_directory "$directory"
    done
    if [[ ! -e /etc/garmin-pace && ! -L /etc/garmin-pace ]]; then
        root install -d -o root -g root -m 0755 /etc/garmin-pace
    fi
    public_directory /etc/garmin-pace
    for path in "$BINARY" "$CONFIG" "$UNIT"; do check_system_file "$path"; done
    if [[ -f "$UNIT" ]]; then
        local bundled_digest installed_digest
        bundled_digest=$(sha256sum < "$BUNDLE/garmin-pace.service")
        installed_digest=$(sha256sum < "$UNIT")
        [[ "$bundled_digest" == "$installed_digest" ]] || fail 'The installed service was customized. It was preserved; ask an administrator to review the service update.'
    fi
    "$BUNDLE/pace" --config "$CONFIG" config check >/dev/null 2>&1 || {
        [[ ! -e "$CONFIG" ]] || fail 'Existing configuration is invalid. It was preserved; ask an administrator to review it.'
    }
    prepare_account
    BINARY_TEMP=$(root mktemp /usr/local/bin/.pace-install.XXXXXXXX)
    root install -o root -g root -m 0755 -- "$BUNDLE/pace" "$BINARY_TEMP"
    root mv -T -- "$BINARY_TEMP" "$BINARY"
    BINARY_TEMP=
    if [[ ! -e "$CONFIG" ]]; then root install -o root -g root -m 0644 -- "$BUNDLE/config.toml" "$CONFIG"; fi
    if [[ ! -e "$UNIT" ]]; then root install -o root -g root -m 0644 -- "$BUNDLE/garmin-pace.service" "$UNIT"; fi
}

configure_profile() {
    HANDOFF_DIR=$(root mktemp -d /tmp/pace-install.XXXXXXXX)
    # A root-owned parent prevents the service account from swapping the handoff file.
    root chmod 0711 -- "$HANDOFF_DIR"
    root install -o "$SERVICE_USER" -g "$SERVICE_USER" -m 0600 /dev/null "$HANDOFF_DIR/key"
    root runuser --pty -u "$SERVICE_USER" -- "$BINARY" --config "$PROFILE_CONFIG" setup --quick --user "$PROFILE" --server-config "$CONFIG" --key-handoff "$HANDOFF_DIR/key" || fail 'Profile setup was interrupted. Saved data was kept; run ./install.sh again to continue.'
}

start_service() {
    printf '\nStarting and checking your MCP service...\n'
    root timeout 45 systemctl daemon-reload
    root timeout 45 systemctl restart garmin-pace.service
    root runuser -u "$SERVICE_USER" -- "$BINARY" --config "$CONFIG" installation-check --profile "$PROFILE_CONFIG" || fail 'The service did not pass its access check. Autostart was not enabled by this run. Ask an administrator to inspect: journalctl -u garmin-pace.service'
    root timeout 45 systemctl enable garmin-pace.service
    root systemctl is-active --quiet garmin-pace.service
    root systemctl is-enabled --quiet garmin-pace.service
}

# Even when invoked with sudo, deliver as the original administrator, not root.
owner() {
    if (( EUID == 0 )) && [[ "$DELIVERY_UID" != 0 ]]; then
        runuser -u "$DELIVERY_USER" -- "$@"
    else
        "$@"
    fi
}

deliver_key() {
    local delivery_home delivery_gid delivery_dir delivery_file
    DELIVERY_UID=$(id -u)
    if (( EUID == 0 )) && [[ -n "${SUDO_UID:-}" ]]; then DELIVERY_UID=$SUDO_UID; fi
    [[ "$DELIVERY_UID" =~ ^[0-9]+$ ]] || fail 'Cannot identify the invoking administrator.'
    DELIVERY_USER=$(getent passwd "$DELIVERY_UID" | cut -d: -f1)
    delivery_gid=$(id -g "$DELIVERY_USER")
    delivery_home=$(getent passwd "$DELIVERY_UID" | cut -d: -f6)
    [[ -d "$delivery_home" && ! -L "$delivery_home" ]] || fail 'Cannot locate your home folder. Your key remains in the private service profile.'
    delivery_dir="$delivery_home/Pace"
    if ! owner test -e "$delivery_dir" && ! owner test -L "$delivery_dir"; then owner mkdir -m 0700 -- "$delivery_dir"; fi
    owner test -d "$delivery_dir" && ! owner test -L "$delivery_dir" || fail 'The Pace folder must be a real private directory.'
    [[ $(owner stat -c '%u:%a' -- "$delivery_dir") == "$DELIVERY_UID:700" ]] || fail 'Your Pace folder must be private (0700). The key remains in the service profile.'
    root chown --no-dereference "$DELIVERY_UID:$delivery_gid" "$HANDOFF_DIR/key"
    delivery_file=$(owner mktemp "$delivery_dir/$PROFILE-mcp.XXXXXXXX.key")
    owner cp -- "$HANDOFF_DIR/key" "$delivery_file"
    printf '\nPace is installed and running. It will start automatically after a reboot.\n'
    printf 'Your private MCP key file: %s\n' "$delivery_file"
    printf 'Keep this file private. Keys expire according to the server configuration (30 days by default).\n'
    if [[ -n "$HTTPS_URL" ]]; then printf 'Configured MCP HTTPS address: %s\n' "$HTTPS_URL"; else printf 'Use ./install.sh --https to configure Internet access later.\n'; fi
    printf 'To connect Garmin later or add another profile, run ./install.sh again.\n'
}

main() {
    case "${1:-}" in
        --https) HTTPS_ONLY=yes ;;
        "") ;;
        *) fail 'Usage: ./install.sh [--https]' ;;
    esac
    [[ $# -le 1 ]] || fail 'Usage: ./install.sh [--https]'
    preflight
    source "$BUNDLE/https.sh"
    printf '\nWelcome to Pace\n\nThis wizard installs Pace and automatic startup on this Raspberry Pi.\nYour Garmin data stays in private storage on this device.\nNo packages are downloaded. Existing data and configuration are kept.\n\n'
    read -r -p 'Profile name [owner]: ' PROFILE
    PROFILE=${PROFILE:-owner}
    [[ "$PROFILE" =~ ^[a-z][a-z0-9_-]{0,31}$ ]] || fail 'Use 1-32 lowercase letters, numbers, hyphens or underscores, starting with a letter.'
    PROFILE_CONFIG="$STATE/profiles/$PROFILE/config.toml"
    if [[ "$HTTPS_ONLY" == yes ]]; then
        if (( EUID != 0 )); then sudo -v; fi
        public_directory /etc/garmin-pace
        check_system_file "$BINARY"
        check_system_file "$CONFIG"
        configure_https
        return
    fi
    printf '\nPace will install to /usr/local/bin and store data in %s.\n' "$STATE"
    read -r -p 'Install and configure Pace? [y/N]: ' confirmation
    [[ "$confirmation" == y || "$confirmation" == Y ]] || { printf 'Cancelled. No system changes were made.\n'; exit 0; }
    if (( EUID != 0 )); then sudo -v; fi
    install_release
    configure_profile
    start_service
    configure_https
    deliver_key
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then main "$@"; fi
