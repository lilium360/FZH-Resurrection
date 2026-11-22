#!/usr/bin/env bash
# Installs and starts the Firewall Zero Hour online services on this Linux machine with Docker.
# Safe to run again: Docker, the registration key and the data stay as they are.
#
#   sudo bash install.sh
#
# Steps: Docker and its compose plugin if missing (Ubuntu, Debian, Oracle Linux and relatives);
# the data folder with a new registration key and shadnet.cfg (first run only); the five ports in
# the machine's firewall if one is active; build and start the three services; show the status.
set -euo pipefail
. "$(dirname "$(readlink -f "$0")")/common.sh"
become_root "$@"

for f in backend/backend.py backend/catalogue.json backend/shadnet_client.py relay/relay.py account/create_account.py shadnet/Dockerfile shadnet/entrypoint.sh shadnet/write-config.sh; do
    if [ ! -f "$SERVER/$f" ]; then
        echo "server/$f is missing: copy the whole server/ folder to this machine." >&2
        exit 1
    fi
done

install_docker() {
    if command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1; then
        echo "Docker and compose are installed."
        return
    fi
    . /etc/os-release
    case "${ID:-}" in
        ubuntu|debian)
            export DEBIAN_FRONTEND=noninteractive
            apt-get update -q
            command -v docker >/dev/null 2>&1 || apt-get install -y -q docker.io
            # The plugin is docker-compose-v2 on Ubuntu and docker-compose on Debian 13.
            docker compose version >/dev/null 2>&1 || apt-get install -y -q docker-compose-v2 \
                || apt-get install -y -q docker-compose
            docker buildx version >/dev/null 2>&1 || apt-get install -y -q docker-buildx || true
            ;;
        ol|rhel|centos|rocky|almalinux)
            dnf -y install dnf-plugins-core
            dnf config-manager --add-repo https://download.docker.com/linux/centos/docker-ce.repo
            dnf -y install docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
            ;;
        *)
            echo "Unsupported system (${ID:-unknown}): install Docker with the compose plugin, then run this again." >&2
            exit 1
            ;;
    esac
    systemctl enable --now docker
    docker compose version >/dev/null 2>&1 || { echo "Docker compose is not installed." >&2; exit 1; }
}

prepare_data() {
    umask 077
    mkdir -p "$DATA/shadnet" "$DATA/backend"
    chmod 700 "$DATA" "$DATA/shadnet"
    # The backend container runs as user 10001 and writes only here.
    chown 10001:10001 "$DATA/backend"
    bash "$SERVER/shadnet/write-config.sh" "$DATA/shadnet/shadnet.cfg" "$KEY"
    # The backend registers accounts with the key, read-only in its container.
    chown 10001:10001 "$KEY"
}

# Inserted before the final REJECT/DROP without removing anything, and written to the file that
# restores the rules at boot (the layout of Oracle's Ubuntu images).
open_iptables() {
    local position entry port proto file=/etc/iptables/rules.v4 lines="" added=0
    position="$(iptables -L INPUT -n --line-numbers | awk '$2 == "REJECT" || $2 == "DROP" { print $1; exit }')"
    if [ -z "$position" ] && ! iptables -S INPUT | grep -q '^-P INPUT DROP'; then
        echo "No active firewall on this machine."
        return
    fi
    for entry in $PORTS; do
        port="${entry%/*}"
        proto="${entry#*/}"
        if ! iptables -C INPUT -p "$proto" -m "$proto" --dport "$port" -j ACCEPT 2>/dev/null; then
            iptables -I INPUT "${position:-1}" -p "$proto" -m "$proto" --dport "$port" -j ACCEPT
            added=$((added + 1))
        fi
        lines="$lines-A INPUT -p $proto -m $proto --dport $port -j ACCEPT
"
    done
    echo "iptables: ports open ($added rules added now)."
    if [ -f "$file" ] && grep -q -- '^-A INPUT .*-j REJECT' "$file"; then
        if grep -q -- '--dport 31313 ' "$file"; then
            echo "iptables: the rules are already in $file."
        else
            cp -p "$file" "$file.before-fzh"
            LINES="$lines" awk '!done && /^-A INPUT .*-j REJECT/ { printf "%s", ENVIRON["LINES"]; done = 1 } { print }' \
                "$file.before-fzh" > "$file"
            echo "iptables: rules written to $file (previous copy in $file.before-fzh)."
        fi
    elif command -v netfilter-persistent >/dev/null 2>&1; then
        netfilter-persistent save
        echo "iptables: rules saved with netfilter-persistent."
    else
        echo "WARNING: unknown place for persistent iptables rules: run install.sh again after a reboot."
    fi
}

open_firewall() {
    local entry
    if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q '^Status: active'; then
        for entry in $PORTS; do ufw allow "$entry" >/dev/null; done
        echo "ufw: ports open."
    elif command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
        for entry in $PORTS; do firewall-cmd --quiet --permanent --add-port="$entry"; done
        firewall-cmd --quiet --reload
        echo "firewalld: ports open."
    elif command -v iptables >/dev/null 2>&1; then
        open_iptables
    else
        echo "No firewall found on this machine."
    fi
}

echo "== 1/5 Docker"
install_docker
echo "== 2/5 Data in $DATA"
prepare_data
echo "== 3/5 Firewall"
open_firewall
echo "== 4/5 Services (the first build compiles shadNet: 5-15 minutes, longer on small ARM machines)"
compose build
compose up -d
wait_healthy
echo "== 5/5 Status"
bash "$HERE/status.sh"

cat <<END

Done. Still to do:
  - open the five ports inbound in the provider's network firewall (security list, security group):
    TCP 31313, UDP 31314, TCP 31315, TCP 1338, UDP 31316;
  - players create their accounts in the launcher (the backend registers them; the key in
    $KEY never leaves this machine); create-account.sh <OnlineID> <email> is for admins;
  - players set this machine's public address as private server in the launcher (Advanced).
END
