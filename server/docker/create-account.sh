#!/usr/bin/env bash
# Creates a shadNet account from this server, for admins: players register from the launcher.
#
#   sudo bash create-account.sh <OnlineID> <email>
#
# OnlineID: 3-16 characters (letters, digits, _ and -), the name other players see.
# email: unique per account; no mail is sent.
# The password is asked for without echo. The registration key is read from its file (readable
# by root and the backend's container user) and reaches the tool through the environment, never
# the screen or the process list.
set -euo pipefail
. "$(dirname "$(readlink -f "$0")")/common.sh"
become_root "$@"

if [ $# -ne 2 ]; then
    echo "Usage: sudo bash create-account.sh <OnlineID> <email>" >&2
    exit 1
fi
[ -s "$KEY" ] || { echo "$KEY is missing: run install.sh first." >&2; exit 1; }

# A preset FZH_ACCOUNT_PASSWORD is for automated tests only.
if [ -z "${FZH_ACCOUNT_PASSWORD:-}" ]; then
    read -r -s -p "Password for $1: " FZH_ACCOUNT_PASSWORD
    echo
    read -r -s -p "Repeat the password: " repeated
    echo
    if [ -z "$FZH_ACCOUNT_PASSWORD" ] || [ "$FZH_ACCOUNT_PASSWORD" != "$repeated" ]; then
        echo "The passwords differ or are empty." >&2
        exit 1
    fi
fi
FZH_REGISTRATION_KEY="$(cat "$KEY")"
export FZH_ACCOUNT_PASSWORD FZH_REGISTRATION_KEY

# Runs in the backend container and reaches shadNet on the internal network.
compose exec -T -e FZH_ACCOUNT_PASSWORD -e FZH_REGISTRATION_KEY backend \
    python /app/create_account.py shadnet 31313 "$1" "$2"
