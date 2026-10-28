#!/bin/bash
# deploy/install.sh installs the bundled Linux release (aarch64 or x86_64) either system-wide
# under a dedicated service account with a systemd unit, or for the current user without root.
# Interactive runs keep the easy wizard; --yes answers everything from flags for agents.
# Both modes preserve existing state, verify authenticated MCP access and never download anything.
set -euo pipefail
# The caller's PATH only decides how the final message names the pace command.
CALLER_PATH=$PATH
export PATH=/usr/sbin:/usr/bin:/sbin:/bin
umask 077

SERVICE_USER=garmin-pace
BUNDLE=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
MODE=
PREFIX=
DATA=
PROFILE=
KEY_DAYS=
YES=no
HTTPS_ONLY=no
HANDOFF_DIR=
BINARY_TEMP=
HTTPS_TEMP=
SERVE_PID=
USAGE='Usage: ./install.sh [--https]
       ./install.sh --yes (--system | --user) --key-lifetime 30d|180d|none [--profile NAME] [--prefix DIR] [--data DIR]'

fail() { printf '\nSetup stopped: %s\n' "$*" >&2; exit 1; }
# Privileged steps run through sudo in system mode; user mode never escalates.
root() { if [[ "$MODE" == user ]] || (( EUID == 0 )); then "$@"; else sudo -- "$@"; fi; }
# Steps that touch the profile run as the service account in system mode, as the user otherwise.
service() { if [[ "$MODE" == user ]]; then "$@"; else root runuser -u "$SERVICE_USER" -- "$@"; fi; }
service_pty() { if [[ "$MODE" == user ]]; then "$@"; else root runuser --pty -u "$SERVICE_USER" -- "$@"; fi; }
cleanup() {
    if [[ -n "$SERVE_PID" ]]; then kill "$SERVE_PID" 2>/dev/null || true; wait "$SERVE_PID" 2>/dev/null || true; fi
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

# --- arguments ---

parse_arguments() {
    while (( $# > 0 )); do
        case "$1" in
            --https) HTTPS_ONLY=yes ;;
            --yes) YES=yes ;;
            --system) MODE=system ;;
            --user) MODE=user ;;
            --profile) PROFILE=${2:-}; shift ;;
            --prefix) PREFIX=${2:-}; shift ;;
            --data) DATA=${2:-}; shift ;;
            --key-lifetime)
                case "${2:-}" in
                    30d) KEY_DAYS=30 ;;
                    180d) KEY_DAYS=180 ;;
                    none) KEY_DAYS=36500 ;;
                    *) fail "--key-lifetime takes 30d, 180d or none. $USAGE" ;;
                esac
                shift ;;
            *) fail "$USAGE" ;;
        esac
        shift
    done
    if [[ "$YES" == yes ]]; then
        [[ "$HTTPS_ONLY" == no ]] || fail 'HTTPS setup asks for a domain and runs interactively; omit --yes.'
        [[ -n "$MODE" ]] || fail "--yes needs an explicit --system or --user. $USAGE"
        [[ -n "$KEY_DAYS" ]] || fail "--yes needs --key-lifetime. $USAGE"
    fi
    [[ "$HTTPS_ONLY" == no || "$MODE" != user ]] || fail 'HTTPS ingress needs the system install; user mode has no service.'
    for path in "$PREFIX" "$DATA"; do
        [[ -z "$path" || "$path" == /* ]] || fail "Directories must be absolute paths: $path"
    done
}

# --- preflight ---

preflight() {
    [[ $(uname -s) == Linux && ( $(uname -m) == aarch64 || $(uname -m) == x86_64 ) ]] || fail 'Use 64-bit Linux (Raspberry Pi OS or Debian Bookworm or newer) on an ARM64 or x86_64 device.'
    if [[ "$YES" == no ]]; then
        [[ -t 0 && -t 1 ]] || fail 'Open a terminal on the device and run ./install.sh interactively, or pass --yes with --system or --user.'
    fi
    for tool in getent install mktemp sha256sum timeout stat cp mv cut id chmod chown rm rmdir mkdir tee env sleep sed; do
        command -v "$tool" >/dev/null || fail "Required OS command is missing: $tool. No packages were installed."
    done
    [[ -x "$BUNDLE/pace" ]] || fail 'Extract the complete release archive first.'
    (cd -- "$BUNDLE" && sha256sum --check --status SHA256SUMS) || fail 'Release files are incomplete or damaged. Extract a fresh trusted archive.'
    "$BUNDLE/pace" --version || fail "This release does not run here: use the pace-linux-$(uname -m) archive on glibc 2.36 or newer (Debian or Raspberry Pi OS Bookworm)."
}

# System mode needs systemd, the account tools and root; an agent cannot type a sudo password.
preflight_system() {
    [[ -d /run/systemd/system ]] || fail 'The system install needs a running systemd. Use --user for an install without a service.'
    for tool in systemctl runuser useradd; do
        command -v "$tool" >/dev/null || fail "Required OS command is missing: $tool. No packages were installed."
    done
    if (( EUID != 0 )); then
        command -v sudo >/dev/null || fail 'The system install needs root or sudo. Use --user for an install without root.'
        if [[ "$YES" == yes ]]; then
            sudo -n true 2>/dev/null || fail 'The system install needs root without a password prompt: run as root, grant this account passwordless sudo, or use --user.'
        fi
    fi
}

# Interactive runs ask for the sudo password only after the administrator confirmed the install.
escalate() { if [[ "$MODE" == system ]] && (( EUID != 0 )); then sudo -v; fi; }

# --- paths ---

# PREFIX holds bin/pace and lib/garmin-pace/; DATA holds the server registry and profiles.
resolve_paths() {
    if [[ "$MODE" == system ]]; then
        PREFIX=${PREFIX:-/usr/local}
        DATA=${DATA:-/var/lib/garmin-pace}
        CONFIG=/etc/garmin-pace/config.toml
        INSTALL_ENV=/etc/garmin-pace/install.env
        UNIT=/etc/systemd/system/garmin-pace.service
        OWNER_UID=0
        case "$DATA" in
            /home/*|/root|/root/*) fail 'The system service cannot reach home directories (ProtectHome). Choose a data directory outside /home and /root.' ;;
        esac
    else
        PREFIX=${PREFIX:-$HOME/.local}
        DATA=${DATA:-$HOME/.local/share/garmin-pace}
        CONFIG=$DATA/config.toml
        INSTALL_ENV=$DATA/install.env
        UNIT=
        OWNER_UID=$EUID
    fi
    STATE=$DATA
    LIB_DIR=$PREFIX/lib/garmin-pace
    BINARY=$LIB_DIR/pace
    LAUNCHER=$PREFIX/bin/pace
    PROFILE_CONFIG=$STATE/profiles/$PROFILE/config.toml
}

# An earlier install fixes mode and directories; later runs must not silently move them.
load_install_env() {
    local file
    for file in /etc/garmin-pace/install.env "${DATA:-$HOME/.local/share/garmin-pace}/install.env"; do
        [[ -f "$file" ]] || continue
        local saved_mode saved_prefix saved_data
        saved_mode=$(sed -n 's/^PACE_MODE=//p' "$file")
        saved_prefix=$(sed -n 's/^PACE_PREFIX=//p' "$file")
        saved_data=$(sed -n 's/^PACE_DATA=//p' "$file")
        [[ -n "$saved_mode" && -n "$saved_prefix" && -n "$saved_data" ]] || fail "Unreadable installation record: $file"
        [[ -z "$MODE" || "$MODE" == "$saved_mode" ]] || fail "This device already has a $saved_mode install ($file); --$MODE would create a second one."
        [[ -z "$PREFIX" || "$PREFIX" == "$saved_prefix" ]] || fail "Pace is already installed under $saved_prefix ($file); --prefix cannot move it."
        [[ -z "$DATA" || "$DATA" == "$saved_data" ]] || fail "Pace data already lives in $saved_data ($file); --data cannot move it."
        MODE=$saved_mode
        PREFIX=$saved_prefix
        DATA=$saved_data
        EXISTING_INSTALL=$file
        return
    done
    EXISTING_INSTALL=
}

save_install_env() {
    local temp
    temp=$(root mktemp "$(dirname -- "$INSTALL_ENV")/.install.XXXXXXXX")
    printf 'PACE_MODE=%s\nPACE_PREFIX=%s\nPACE_DATA=%s\n' "$MODE" "$PREFIX" "$DATA" | root tee -- "$temp" >/dev/null
    root chmod 0644 -- "$temp"
    root mv -T -- "$temp" "$INSTALL_ENV"
}

# --- filesystem guards ---

# Deployment parents belong to the install owner and never resolve through a writable link.
public_directory() {
    local path=$1
    [[ -d "$path" && ! -L "$path" ]] || fail "Expected a real directory: $path"
    [[ $(stat -c %u -- "$path") == "$OWNER_UID" ]] || fail "Directory has an unexpected owner: $path"
    local mode
    mode=$(stat -c %a -- "$path")
    (( (8#$mode & 0022) == 0 )) || fail "Directory must not allow group/other writes: $path"
}

# install(1) with the deployment owner: root in system mode, the user themselves otherwise.
install_public() {
    if [[ "$MODE" == user ]]; then install "$@"; else root install -o root -g root "$@"; fi
}

ensure_public_directory() {
    local path=$1
    if [[ ! -e "$path" && ! -L "$path" ]]; then install_public -d -m 0755 -- "$path"; fi
    public_directory "$path"
}

private_directory() {
    local path=$1
    if ! root test -e "$path" && ! root test -L "$path"; then
        if [[ "$MODE" == user ]]; then
            mkdir -m 0700 -- "$path"
        elif [[ "$path" == "$STATE" ]]; then
            root install -d -o "$SERVICE_UID" -g "$SERVICE_GID" -m 0700 -- "$path"
        else
            service mkdir -m 0700 -- "$path"
        fi
    fi
    root test -d "$path" && ! root test -L "$path" || fail "Expected a real private directory: $path"
    [[ $(root stat -c '%u:%a' -- "$path") == "$SERVICE_UID:700" ]] || fail "Private directory has unexpected ownership or permissions: $path"
}

check_system_file() {
    local path=$1
    if [[ -e "$path" || -L "$path" ]]; then
        [[ -f "$path" && ! -L "$path" ]] || fail "Expected a regular file: $path"
        [[ $(stat -c '%u:%h' -- "$path") == "$OWNER_UID:1" ]] || fail "File has unexpected ownership or hard links: $path"
        local mode
        mode=$(stat -c %a -- "$path")
        (( (8#$mode & 0022) == 0 )) || fail "File must not allow group/other writes: $path"
    fi
}

# --- accounts and directories ---

prepare_account() {
    if [[ "$MODE" == user ]]; then
        SERVICE_UID=$EUID
        SERVICE_GID=$(id -g)
    else
        if ! getent passwd "$SERVICE_USER" >/dev/null; then
            getent group "$SERVICE_USER" >/dev/null && fail 'A garmin-pace group already exists without its service account. Ask an administrator to review it.'
            root useradd --system --user-group --no-create-home --home-dir "$STATE" --shell /usr/sbin/nologin "$SERVICE_USER"
        fi
        local name password description home shell
        IFS=: read -r name password SERVICE_UID SERVICE_GID description home shell < <(getent passwd "$SERVICE_USER")
        [[ "$SERVICE_UID" != 0 && "$home" == "$STATE" && "$shell" == /usr/sbin/nologin ]] || fail 'Existing garmin-pace account does not match the dedicated service account or data directory.'
        [[ $(getent group "$SERVICE_USER" | cut -d: -f3) == "$SERVICE_GID" ]] || fail 'Service account group does not match.'
    fi
    local parent
    parent=$(dirname -- "$STATE")
    if [[ "$MODE" == user && ! -e "$parent" ]]; then mkdir -p -- "$parent"; fi
    private_directory "$STATE"
    private_directory "$STATE/profiles"
    private_directory "$STATE/profiles/$PROFILE"
}

# --- generated files ---

# The bundled unit is written for the default paths; rewrite them for this install.
generated_unit() {
    local unit
    unit=$(sed -e "s#/usr/local/lib/garmin-pace/pace#$BINARY#g" -e "s#/var/lib/garmin-pace#$DATA#g" -- "$BUNDLE/garmin-pace.service")
    if [[ "$DATA" != /var/lib/garmin-pace ]]; then unit=$(printf '%s\n' "$unit" | sed '/^StateDirectory/d'); fi
    printf '%s\n' "$unit"
}

generated_config() {
    sed -e "s#/var/lib/garmin-pace#$DATA#g" -- "$BUNDLE/config.toml"
}

install_file() {
    local destination=$1 mode=$2
    BINARY_TEMP=$(root mktemp "$(dirname -- "$destination")/.pace-install.XXXXXXXX")
    root tee -- "$BINARY_TEMP" >/dev/null
    root chmod "$mode" -- "$BINARY_TEMP"
    root mv -T -- "$BINARY_TEMP" "$destination"
    BINARY_TEMP=
}

# The PATH command runs every command, the menu included, against the last installed profile;
# --config or GARMIN_PACE_CONFIG still selects another. System mode runs as the service account.
install_launcher() {
    local run
    if [[ "$MODE" == user ]]; then
        run="exec $BINARY \"\$@\""
    else
        run="[[ \$(id -un) == $SERVICE_USER ]] && exec $BINARY \"\$@\"
pty=; [[ -t 0 && -t 1 ]] && pty=--pty
if (( EUID == 0 )); then exec runuser \$pty -u $SERVICE_USER -- env GARMIN_PACE_CONFIG=\"\$GARMIN_PACE_CONFIG\" $BINARY \"\$@\"; fi
prompt=; [[ -t 0 ]] || prompt=-n
exec sudo \$prompt runuser \$pty -u $SERVICE_USER -- env GARMIN_PACE_CONFIG=\"\$GARMIN_PACE_CONFIG\" $BINARY \"\$@\""
    fi
    printf '%s\n' '#!/bin/bash' \
        "# $LAUNCHER runs Pace for the installed profile; --config or GARMIN_PACE_CONFIG selects another." \
        "export GARMIN_PACE_CONFIG=\"\${GARMIN_PACE_CONFIG:-$PROFILE_CONFIG}\"" \
        "$run" | install_file "$LAUNCHER" 0755
}

install_release() {
    local directory
    for directory in "$(dirname -- "$PREFIX")" "$PREFIX" "$PREFIX/bin" "$PREFIX/lib" "$LIB_DIR"; do
        ensure_public_directory "$directory"
    done
    # The server configuration lives in /etc in system mode and inside the private data directory otherwise.
    if [[ "$MODE" == system ]]; then ensure_public_directory "$(dirname -- "$CONFIG")"; fi
    for path in "$BINARY" "$LAUNCHER" "$CONFIG" "$INSTALL_ENV" $UNIT; do check_system_file "$path"; done
    if [[ -n "$UNIT" && -f "$UNIT" ]]; then
        [[ $(generated_unit | sha256sum) == $(sha256sum < "$UNIT") ]] || fail 'The installed service was customized. It was preserved; ask an administrator to review the service update.'
    fi
    "$BUNDLE/pace" --config "$CONFIG" config check >/dev/null 2>&1 || {
        [[ ! -e "$CONFIG" ]] || fail 'Existing configuration is invalid. It was preserved; ask an administrator to review it.'
    }
    prepare_account
    install_public -m 0755 -- "$BUNDLE/pace" "$BINARY.new"
    BINARY_TEMP=$BINARY.new
    root mv -T -- "$BINARY_TEMP" "$BINARY"
    BINARY_TEMP=
    install_launcher
    if [[ ! -e "$CONFIG" ]]; then generated_config | install_file "$CONFIG" 0644; fi
    if [[ -n "$UNIT" && ! -e "$UNIT" ]]; then generated_unit | install_file "$UNIT" 0644; fi
    save_install_env
}

# --- profile, service and delivery ---

configure_profile() {
    HANDOFF_DIR=$(root mktemp -d /tmp/pace-install.XXXXXXXX)
    # A root-owned parent prevents the service account from swapping the handoff file.
    root chmod 0711 -- "$HANDOFF_DIR"
    if [[ "$MODE" == user ]]; then install -m 0600 /dev/null "$HANDOFF_DIR/key"; else root install -o "$SERVICE_UID" -g "$SERVICE_GID" -m 0600 /dev/null "$HANDOFF_DIR/key"; fi
    if [[ "$YES" == yes ]]; then
        service "$BINARY" --config "$PROFILE_CONFIG" setup --quick --batch --user "$PROFILE" --server-config "$CONFIG" --key-handoff "$HANDOFF_DIR/key" --key-days "$KEY_DAYS" >/dev/null || fail 'Profile setup failed. Saved data was kept; fix the reported problem and run the installer again.'
    else
        service_pty "$BINARY" --config "$PROFILE_CONFIG" setup --quick --user "$PROFILE" --server-config "$CONFIG" --key-handoff "$HANDOFF_DIR/key" || fail 'Profile setup was interrupted. Saved data was kept; run ./install.sh again to continue.'
    fi
}

start_service() {
    printf '\nStarting and checking your MCP service...\n'
    if [[ "$MODE" == user ]]; then
        # No supervisor in user mode: prove the install with a temporary server, then stop it.
        "$BINARY" --config "$CONFIG" serve --transport http >/dev/null 2>&1 &
        SERVE_PID=$!
        "$BINARY" --config "$CONFIG" installation-check --profile "$PROFILE_CONFIG" || fail "The server did not pass its access check. Start it by hand and inspect its output: $BINARY --config $CONFIG serve --transport http"
        kill "$SERVE_PID"
        wait "$SERVE_PID" 2>/dev/null || true
        SERVE_PID=
        return
    fi
    root timeout 45 systemctl daemon-reload
    root timeout 45 systemctl restart garmin-pace.service
    service "$BINARY" --config "$CONFIG" installation-check --profile "$PROFILE_CONFIG" || fail 'The service did not pass its access check. Autostart was not enabled by this run. Ask an administrator to inspect: journalctl -u garmin-pace.service'
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
    local delivery_home delivery_gid delivery_dir
    DELIVERY_UID=$(id -u)
    if (( EUID == 0 )) && [[ -n "${SUDO_UID:-}" ]]; then DELIVERY_UID=$SUDO_UID; fi
    [[ "$DELIVERY_UID" =~ ^[0-9]+$ ]] || fail 'Cannot identify the invoking administrator.'
    DELIVERY_USER=$(getent passwd "$DELIVERY_UID" | cut -d: -f1)
    delivery_gid=$(id -g "$DELIVERY_USER")
    delivery_home=$(getent passwd "$DELIVERY_UID" | cut -d: -f6)
    [[ -d "$delivery_home" && ! -L "$delivery_home" ]] || fail 'Cannot locate your home folder. Your key remains in the private profile.'
    delivery_dir="$delivery_home/Pace"
    if ! owner test -e "$delivery_dir" && ! owner test -L "$delivery_dir"; then owner mkdir -m 0700 -- "$delivery_dir"; fi
    owner test -d "$delivery_dir" && ! owner test -L "$delivery_dir" || fail 'The Pace folder must be a real private directory.'
    [[ $(owner stat -c '%u:%a' -- "$delivery_dir") == "$DELIVERY_UID:700" ]] || fail 'Your Pace folder must be private (0700). The key remains in the profile.'
    root chown --no-dereference "$DELIVERY_UID:$delivery_gid" "$HANDOFF_DIR/key"
    DELIVERY_FILE=$(owner mktemp "$delivery_dir/$PROFILE-mcp.XXXXXXXX.key")
    owner cp -- "$HANDOFF_DIR/key" "$DELIVERY_FILE"
}

report() {
    local menu=pace
    case ":$CALLER_PATH:" in *":$PREFIX/bin:"*) ;; *) menu=$LAUNCHER ;; esac
    if [[ "$YES" == yes ]]; then
        printf 'pace_install_mode=%s\npace_install_prefix=%s\npace_install_data=%s\npace_install_key_file=%s\npace_install_mcp_url=http://127.0.0.1:8080/api/v1/mcp\npace_install_menu_command=%s\n' "$MODE" "$PREFIX" "$DATA" "$DELIVERY_FILE" "$menu"
        if [[ "$MODE" == user ]]; then printf 'pace_install_serve_command=%s --config %s serve --transport http\n' "$BINARY" "$CONFIG"; fi
        return
    fi
    if [[ "$MODE" == user ]]; then
        printf '\nPace is installed for this user; no system service was created.\n'
        printf 'Start the MCP server when needed (it stops with the terminal or at reboot):\n  %s --config %s serve --transport http\n' "$BINARY" "$CONFIG"
    else
        printf '\nPace is installed and running. It will start automatically after a reboot.\n'
    fi
    printf 'Your private MCP key file: %s\n' "$DELIVERY_FILE"
    printf 'Keep this file private. The key stays valid for the lifetime you chose during setup.\n'
    printf 'Open the Pace menu (users, keys, Garmin, sync) any time by typing: %s\n' "$menu"
    if [[ "$MODE" == system ]]; then
        if [[ -n "$HTTPS_URL" ]]; then printf 'Configured MCP HTTPS address: %s\n' "$HTTPS_URL"; else printf 'Use ./install.sh --https to configure Internet access later.\n'; fi
    fi
    printf 'To connect Garmin later or add another profile, run ./install.sh again.\n'
}

# --- interactive questions ---

ask_profile() {
    if [[ -z "$PROFILE" ]]; then
        if [[ "$YES" == yes ]]; then
            PROFILE=owner
        else
            read -r -p 'Profile name [owner]: ' PROFILE
            PROFILE=${PROFILE:-owner}
        fi
    fi
    [[ "$PROFILE" =~ ^[a-z][a-z0-9_-]{0,31}$ ]] || fail 'Use 1-32 lowercase letters, numbers, hyphens or underscores, starting with a letter.'
}

ask_layout() {
    local answer
    if [[ -n "$EXISTING_INSTALL" ]]; then
        printf 'Existing %s install found (%s): program under %s, data in %s. These stay as they are.\n' "$MODE" "$EXISTING_INSTALL" "$PREFIX" "$DATA"
        return
    fi
    read -r -p 'Setup type: easy (system service, standard folders) or advanced (choose folders)? [easy/advanced]: ' answer
    case "${answer:-easy}" in
        easy|e|E) MODE=${MODE:-system} ;;
        advanced|a|A)
            if [[ -z "$MODE" ]]; then
                read -r -p 'Install as a system service with its own account (needs sudo) or for this user only? [system/user]: ' answer
                case "${answer:-system}" in
                    system|s|S) MODE=system ;;
                    user|u|U) MODE=user ;;
                    *) fail 'Answer system or user.' ;;
                esac
            fi
            local default_prefix default_data
            if [[ "$MODE" == system ]]; then default_prefix=/usr/local; default_data=/var/lib/garmin-pace; else default_prefix=$HOME/.local; default_data=$HOME/.local/share/garmin-pace; fi
            read -r -p "Program directory (bin/ and lib/ go here) [${PREFIX:-$default_prefix}]: " answer
            PREFIX=${answer:-${PREFIX:-$default_prefix}}
            read -r -p "Data directory (database, registry, keys) [${DATA:-$default_data}]: " answer
            DATA=${answer:-${DATA:-$default_data}}
            for answer in "$PREFIX" "$DATA"; do [[ "$answer" == /* ]] || fail "Directories must be absolute paths: $answer"; done
            ;;
        *) fail 'Answer easy or advanced.' ;;
    esac
}

main() {
    parse_arguments "$@"
    preflight
    if [[ "$YES" == no ]]; then
        printf '\nWelcome to Pace\n\nThis wizard installs Pace and, as a system service, automatic startup on this device.\nYour Garmin data stays in private storage on this device.\nNo packages are downloaded. Existing data and configuration are kept.\n\n'
    fi
    ask_profile
    load_install_env
    if [[ "$HTTPS_ONLY" == yes ]]; then
        [[ -n "$EXISTING_INSTALL" ]] || fail 'Run ./install.sh once before configuring HTTPS.'
        resolve_paths
        preflight_system
        escalate
        source "$BUNDLE/https.sh"
        public_directory "$(dirname -- "$CONFIG")"
        check_system_file "$BINARY"
        check_system_file "$CONFIG"
        configure_https
        return
    fi
    if [[ "$YES" == no ]]; then ask_layout; fi
    resolve_paths
    if [[ "$MODE" == system ]]; then preflight_system; source "$BUNDLE/https.sh"; else HTTPS_URL=; fi
    if [[ "$YES" == no ]]; then
        printf '\nPace will install the pace command to %s/bin and store data in %s.\n' "$PREFIX" "$DATA"
        read -r -p 'Install and configure Pace? [y/N]: ' confirmation
        [[ "$confirmation" == y || "$confirmation" == Y ]] || { printf 'Cancelled. No system changes were made.\n'; exit 0; }
        escalate
    fi
    install_release
    configure_profile
    start_service
    if [[ "$MODE" == system && "$YES" == no ]]; then configure_https; fi
    deliver_key
    report
    if [[ "$YES" == no ]]; then
        read -r -p 'Open the Pace menu now? [Y/n]: ' answer
        if [[ "${answer:-y}" == y || "${answer:-y}" == Y ]]; then "$LAUNCHER"; fi
    fi
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then main "$@"; fi
