#!/usr/bin/env bash
# Rebuilds the images and restarts the services after a new server/ folder was copied here.
# The data in FZH_DATA stays.
#
#   sudo bash update.sh
set -euo pipefail
. "$(dirname "$(readlink -f "$0")")/common.sh"
become_root "$@"

# The request log only grows: past 20 MB it starts again, keeping the previous one.
log="$DATA/backend/backend-requests.log"
if [ -f "$log" ] && [ "$(stat -c %s "$log")" -gt 20000000 ]; then
    mv -f "$log" "$log.1"
    echo "Backend request log moved to $log.1"
fi

compose build --pull
compose up -d --remove-orphans
wait_healthy
bash "$HERE/status.sh"
