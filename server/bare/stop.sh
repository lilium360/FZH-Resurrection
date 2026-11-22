#!/usr/bin/env bash
# Stops the services started by start.sh.
#
#   bash stop.sh
set -uo pipefail
. "$(dirname "$(readlink -f "$0")")/common.sh"

for name in relay backend shadnet; do
    if alive "$name"; then
        kill "$(cat "$FZH_ROOT/run/$name.pid")" && echo "$name stopped"
    fi
    rm -f "$FZH_ROOT/run/$name.pid"
done
