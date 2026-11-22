#!/usr/bin/env bash
# Creates a shadNet account on the local shadNet.
#
#   bash create-account.sh <OnlineID> <email>
#
# The password is asked for without echo; the registration key is read from its file.
set -euo pipefail
. "$(dirname "$(readlink -f "$0")")/common.sh"

if [ $# -ne 2 ]; then
    echo "Usage: bash create-account.sh <OnlineID> <email>" >&2
    exit 1
fi
[ -s "$KEY" ] || { echo "$KEY is missing: run install.sh first." >&2; exit 1; }
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
"$PYTHON" "$FZH_ROOT/app/create_account.py" 127.0.0.1 "$GAME_PORT" "$1" "$2"
