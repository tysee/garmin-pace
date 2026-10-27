#!/bin/bash
# deploy/https.sh adds managed HTTPS to the installer using a dedicated Caddy identity.
# It exposes configure_https and reports verified or pending access without exposing keys.

HTTPS_CONFIG=/etc/garmin-pace/Caddyfile
HTTPS_METADATA=/etc/garmin-pace/https.toml
HTTPS_DIGEST=/etc/garmin-pace/Caddyfile.sha256
HTTPS_UNIT=/etc/systemd/system/garmin-pace-https.service
HTTPS_STATE=/var/lib/garmin-pace-https
HTTPS_BINARY=/usr/local/lib/garmin-pace/caddy
HTTPS_URL=

https_domain() {
    local previous= answer path
    for path in "$HTTPS_CONFIG" "$HTTPS_METADATA" "$HTTPS_DIGEST" "$HTTPS_UNIT" "$HTTPS_BINARY"; do check_system_file "$path"; done
    if [[ -f "$HTTPS_CONFIG" ]]; then
        [[ -f "$HTTPS_DIGEST" && -f "$HTTPS_METADATA" ]] || fail 'An unmanaged HTTPS configuration already exists. It was preserved.'
        sha256sum --check --status "$HTTPS_DIGEST" || fail 'The HTTPS configuration was edited outside the wizard. It was preserved; ask an administrator to review it.'
        previous=$("$BINARY" --config "$CONFIG" installation-https-config --saved-domain "$HTTPS_METADATA" --domain-only) || fail 'Saved HTTPS domain is invalid; ask an administrator to review it.'
    fi
    printf '\nInternet / HTTPS\nUse a public domain you control, for example mcp.your-domain.com.\n'
    printf 'DNS A/AAAA records must point to this device. Forward TCP 80 and 443 on your router.\n'
    printf 'Behind CGNAT, obtain a public address or arrange a tunnel first. Port 8080 stays private.\n'
    while :; do
        read -r -p "Domain${previous:+ [$previous]} (empty to skip): " HTTPS_DOMAIN
        HTTPS_DOMAIN=${HTTPS_DOMAIN:-$previous}
        [[ -n "$HTTPS_DOMAIN" ]] || return 1
        if "$BINARY" --config "$CONFIG" installation-https-config --domain "$HTTPS_DOMAIN" >/dev/null; then break; fi
    done
    HTTPS_DOMAIN=${HTTPS_DOMAIN,,}
    printf 'Pace will run its own HTTPS proxy for https://%s and manage certificates automatically.\n' "$HTTPS_DOMAIN"
    read -r -p 'DNS and router are ready; configure HTTPS now? [y/N]: ' answer
    [[ "$answer" == y || "$answer" == Y ]]
}

https_account() {
    if ! getent passwd pace-https >/dev/null; then
        getent group pace-https >/dev/null && fail 'A pace-https group already exists without its service account.'
        root useradd --system --user-group --no-create-home --home-dir "$HTTPS_STATE" --shell /usr/sbin/nologin pace-https
    fi
    local name password uid gid description home shell
    IFS=: read -r name password uid gid description home shell < <(getent passwd pace-https)
    [[ "$uid" != 0 && "$home" == "$HTTPS_STATE" && "$shell" == /usr/sbin/nologin ]] || fail 'Unexpected existing HTTPS service account.'
    [[ $(getent group pace-https | cut -d: -f3) == "$gid" ]] || fail 'HTTPS service account group does not match.'
    if ! root test -e "$HTTPS_STATE" && ! root test -L "$HTTPS_STATE"; then
        root install -d -o pace-https -g pace-https -m 0700 -- "$HTTPS_STATE"
    fi
    root test -d "$HTTPS_STATE" && ! root test -L "$HTTPS_STATE" || fail 'HTTPS state must be a real private directory.'
    [[ $(root stat -c '%u:%a' -- "$HTTPS_STATE") == "$uid:700" ]] || fail 'HTTPS state has unexpected ownership or permissions.'
}

https_assets() {
    local installed bundled
    public_directory /usr/local/lib
    if [[ ! -e /usr/local/lib/garmin-pace && ! -L /usr/local/lib/garmin-pace ]]; then
        root install -d -o root -g root -m 0755 /usr/local/lib/garmin-pace
    fi
    public_directory /usr/local/lib/garmin-pace
    if [[ -f "$HTTPS_UNIT" ]]; then
        installed=$(sha256sum < "$HTTPS_UNIT")
        bundled=$(sha256sum < "$BUNDLE/garmin-pace-https.service")
        [[ "$installed" == "$bundled" ]] || fail 'The HTTPS service unit was customized. It was preserved.'
    fi
    https_account
    BINARY_TEMP=$(root mktemp /usr/local/lib/garmin-pace/.caddy-install.XXXXXXXX)
    root install -o root -g root -m 0755 "$BUNDLE/caddy" "$BINARY_TEMP"
    root mv -T "$BINARY_TEMP" "$HTTPS_BINARY"
    BINARY_TEMP=
    if [[ ! -e "$HTTPS_UNIT" ]]; then root install -o root -g root -m 0644 "$BUNDLE/garmin-pace-https.service" "$HTTPS_UNIT"; fi
}

https_firewall() {
    local ufw_status= firewalld_active=no answer zone
    if command -v ufw >/dev/null; then ufw_status=$(root env LC_ALL=C ufw status); fi
    if command -v firewall-cmd >/dev/null && systemctl is-active --quiet firewalld; then firewalld_active=yes; fi
    if [[ "$ufw_status" == 'Status: active'* && "$firewalld_active" == yes ]]; then
        fail 'Both UFW and firewalld are active. Ask an administrator to review the firewall.'
    fi
    if [[ "$ufw_status" == 'Status: active'* || "$firewalld_active" == yes ]]; then
        read -r -p 'Allow incoming TCP 80 and 443 in the active device firewall? [y/N]: ' answer
        if [[ "$answer" != y && "$answer" != Y ]]; then
            printf 'Firewall unchanged. HTTPS needs TCP 80 and 443 to be allowed.\n'
            return
        fi
        if [[ "$firewalld_active" == yes ]]; then
            zone=$(root firewall-cmd --get-default-zone)
            root firewall-cmd --zone="$zone" --add-service=http --add-service=https
            root firewall-cmd --permanent --zone="$zone" --add-service=http --add-service=https
            printf 'HTTP/HTTPS allowed in firewalld zone: %s\n' "$zone"
        else
            root ufw allow 80/tcp
            root ufw allow 443/tcp
        fi
    else
        printf 'No active UFW/firewalld detected. Custom firewall rules and the router must allow TCP 80/443.\n'
    fi
}

https_prepare_config() {
    HTTPS_TEMP=$(root mktemp -d /etc/garmin-pace/.https-install.XXXXXXXX)
    root chmod 0755 "$HTTPS_TEMP"
    "$BINARY" --config "$CONFIG" installation-https-config --domain "$HTTPS_DOMAIN" | root tee "$HTTPS_TEMP/Caddyfile" >/dev/null
    root chmod 0644 "$HTTPS_TEMP/Caddyfile"
    root runuser -u pace-https -- env XDG_DATA_HOME="$HTTPS_STATE" XDG_CONFIG_HOME="$HTTPS_STATE" \
        "$HTTPS_BINARY" validate --config "$HTTPS_TEMP/Caddyfile" --adapter caddyfile
    if [[ -f "$HTTPS_CONFIG" ]]; then root cp -- "$HTTPS_CONFIG" "$HTTPS_TEMP/previous"; fi
}

https_start() {
    local was_active=no
    if systemctl is-active --quiet garmin-pace-https.service; then was_active=yes; fi
    root mv -T "$HTTPS_TEMP/Caddyfile" "$HTTPS_CONFIG"
    root timeout 45 systemctl daemon-reload
    if ! { root timeout 45 systemctl restart garmin-pace-https.service && sleep 1 && root systemctl is-active --quiet garmin-pace-https.service; }; then
        if [[ -f "$HTTPS_TEMP/previous" ]]; then
            root mv -T "$HTTPS_TEMP/previous" "$HTTPS_CONFIG"
        else
            root rm -f -- "$HTTPS_CONFIG"
        fi
        if [[ "$was_active" == yes ]]; then
            if ! { root timeout 45 systemctl restart garmin-pace-https.service && sleep 1 && root systemctl is-active --quiet garmin-pace-https.service; }; then
                printf 'Previous HTTPS configuration could not be restarted; inspect its journal.\n' >&2
            fi
        else
            root timeout 25 systemctl stop garmin-pace-https.service
        fi
        fail 'HTTPS could not start. Another web server may own ports 80/443. Existing sites were not stopped. Inspect: journalctl -u garmin-pace-https.service'
    fi
    printf 'domain = "%s"\n' "$HTTPS_DOMAIN" | root tee "$HTTPS_TEMP/https.toml" >/dev/null
    sha256sum "$HTTPS_CONFIG" | root tee "$HTTPS_TEMP/Caddyfile.sha256" >/dev/null
    root chmod 0644 "$HTTPS_TEMP/https.toml" "$HTTPS_TEMP/Caddyfile.sha256"
    root mv -T "$HTTPS_TEMP/https.toml" "$HTTPS_METADATA"
    root mv -T "$HTTPS_TEMP/Caddyfile.sha256" "$HTTPS_DIGEST"
    root timeout 45 systemctl enable garmin-pace-https.service
    root systemctl is-active --quiet garmin-pace-https.service
}

configure_https() {
    local answer
    if [[ "$HTTPS_ONLY" != yes ]]; then
        read -r -p 'Configure Internet access with HTTPS? [y/N]: ' answer
        [[ "$answer" == y || "$answer" == Y ]] || return 0
    fi
    "$BINARY" installation-https-config --help >/dev/null 2>&1 || fail 'Run ./install.sh once to update Pace before configuring HTTPS.'
    root runuser -u garmin-pace -- "$BINARY" --config "$CONFIG" installation-check --profile "$PROFILE_CONFIG"
    if ! https_domain; then printf 'HTTPS setup skipped; existing services are unchanged.\n'; return; fi
    https_assets
    https_prepare_config
    https_firewall
    https_start
    HTTPS_URL="https://$HTTPS_DOMAIN/api/v1/mcp"
    printf '\nWaiting for a trusted certificate and checking MCP access...\n'
    if root runuser -u garmin-pace -- "$BINARY" --config "$CONFIG" installation-check --profile "$PROFILE_CONFIG" --https-domain "$HTTPS_DOMAIN"; then
        printf '\nHTTPS verified locally. MCP URL: %s\nTest this URL from another network before sharing it.\n' "$HTTPS_URL"
    else
        printf '\nHTTPS is configured but NOT VERIFIED. Certificate issuance or the DNS path may still be pending.\nCaddy remains enabled and retries automatically. Check DNS, TCP 80/443, and its journal.\nRun ./install.sh --https to retry the checks. Intended URL: %s\n' "$HTTPS_URL" >&2
    fi
}
