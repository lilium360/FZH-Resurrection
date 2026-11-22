#!/usr/bin/env bash
# Which services run, their listening sockets and the last lines of their output.
#
#   bash status.sh
set -uo pipefail
. "$(dirname "$(readlink -f "$0")")/common.sh"

for name in shadnet backend relay; do
    if alive "$name"; then
        echo "$name: running (pid $(cat "$FZH_ROOT/run/$name.pid"))"
    else
        echo "$name: STOPPED"
    fi
done
echo "--- Listening (expected: tcp $GAME_PORT $WEBAPI_PORT $BACKEND_PORT, udp $MATCHING_PORT $RELAY_PORT)"
if command -v ss >/dev/null 2>&1; then
    ss -H -lntu | awk '{ print $1, $5 }' \
        | grep -E ":($GAME_PORT|$MATCHING_PORT|$WEBAPI_PORT|$BACKEND_PORT|$RELAY_PORT)\$" | sort -u | sed 's/^/  /'
fi
for name in shadnet backend relay; do
    echo "[$name]"
    tail -n 4 "$FZH_ROOT/logs/$name.out" 2>/dev/null | cut -c1-160 | sed 's/^/  /'
done
