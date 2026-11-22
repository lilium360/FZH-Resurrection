# Sourced by the scripts of this folder.

HERE="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" && pwd)"
SERVER="$(dirname "$HERE")"
[ -f "$HERE/config.env" ] && . "$HERE/config.env"
FZH_ROOT="${FZH_ROOT:-$HOME/fzh}"
PYTHON="${PYTHON:-python3}"
BIND_HOST="${BIND_HOST:-0.0.0.0}"
GAME_PORT="${GAME_PORT:-31313}"
MATCHING_PORT="${MATCHING_PORT:-31314}"
WEBAPI_PORT="${WEBAPI_PORT:-31315}"
BACKEND_PORT="${BACKEND_PORT:-1338}"
RELAY_PORT="${RELAY_PORT:-31316}"
KEY="$FZH_ROOT/data/registration-key.txt"

# A pid file whose process is alive. Processes are tracked by pid file only, never by name.
alive() {
    [ -s "$FZH_ROOT/run/$1.pid" ] && kill -0 "$(cat "$FZH_ROOT/run/$1.pid")" 2>/dev/null
}
