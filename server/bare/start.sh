#!/usr/bin/env bash
# Starts the services that are not running. Output goes to $FZH_ROOT/logs/<name>.out.
#
#   bash start.sh
set -euo pipefail
. "$(dirname "$(readlink -f "$0")")/common.sh"

export LD_LIBRARY_PATH="$FZH_ROOT/bin/lib"
export QT_PLUGIN_PATH="$FZH_ROOT/bin/plugins"
export LANG=C.UTF-8
[ -f "$FZH_ROOT/data/shadnet/shadnet.cfg" ] || { echo "Run install.sh first." >&2; exit 1; }

launch() { # name, working folder, command...
    local name="$1" dir="$2"
    shift 2
    if alive "$name"; then
        echo "$name already running (pid $(cat "$FZH_ROOT/run/$name.pid"))"
        return
    fi
    (
        cd "$dir" || exit 1
        nohup setsid "$@" >> "$FZH_ROOT/logs/$name.out" 2>&1 < /dev/null &
        echo $! > "$FZH_ROOT/run/$name.pid"
    )
    echo "$name started (pid $(cat "$FZH_ROOT/run/$name.pid"))"
}

# shadNet works in the folder of its executable.
alive shadnet || cp -f "$FZH_ROOT/bin/shadnet" "$FZH_ROOT/data/shadnet/shadnet"
launch shadnet "$FZH_ROOT/data/shadnet" "$FZH_ROOT/data/shadnet/shadnet"
sleep 2
export FZH_GAME_LOGIN FZH_SESSION_HOURS FZH_REGISTER_PER_DAY FZH_REGISTER_COOLDOWN \
    FZH_REGISTER_GLOBAL_PER_HOUR FZH_RESERVED_NAMES FZH_TRUSTED_PROXY_HEADER FZH_TRUSTED_PROXIES \
    FZH_RELAY_LIST
launch backend "$FZH_ROOT/data/backend" "$PYTHON" -u "$FZH_ROOT/app/backend.py" \
    --host "$BIND_HOST" --port "$BACKEND_PORT" \
    --data "$FZH_ROOT/data/backend/backend-data.json" --log "$FZH_ROOT/data/backend/backend-requests.log" \
    --shadnet-webapi "http://127.0.0.1:$WEBAPI_PORT" --shadnet-game "127.0.0.1:$GAME_PORT" \
    --registration-key-file "$KEY"
launch relay "$FZH_ROOT" "$PYTHON" -u "$FZH_ROOT/app/relay.py" --host "$BIND_HOST" --port "$RELAY_PORT"
