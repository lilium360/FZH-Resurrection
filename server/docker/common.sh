# Sourced by the scripts of this folder.

HERE="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" && pwd)"
SERVER="$(dirname "$HERE")"
if [ -f "$HERE/.env" ]; then
    set -a
    . "$HERE/.env"
    set +a
fi
export FZH_DATA="${FZH_DATA:-/srv/fzh}"
DATA="$FZH_DATA"
KEY="$DATA/registration-key.txt"
PROJECT="${COMPOSE_PROJECT_NAME:-fzh}"
PORTS="31313/tcp 31314/udp 31315/tcp 1338/tcp 31316/udp"

compose() {
    docker compose --project-directory "$HERE" -f "$HERE/docker-compose.yml" "$@"
}

become_root() {
    if [ "$(id -u)" -ne 0 ]; then
        exec sudo --preserve-env=FZH_DATA,COMPOSE_PROJECT_NAME,DOCKER_DEFAULT_PLATFORM,FZH_ACCOUNT_PASSWORD bash "$0" "$@"
    fi
}

# healthy, starting, unhealthy or stopped.
health() {
    local id
    id="$(compose ps -q "$1" 2>/dev/null | head -n 1)"
    [ -n "$id" ] || { echo stopped; return; }
    docker inspect -f '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' "$id" 2>/dev/null || echo stopped
}

wait_healthy() {
    local i
    echo "Waiting for the services to answer..."
    for i in $(seq 1 30); do
        [ "$(health shadnet) $(health backend) $(health relay)" = "healthy healthy healthy" ] && return 0
        sleep 3
    done
    return 0
}
