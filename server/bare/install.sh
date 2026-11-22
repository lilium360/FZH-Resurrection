#!/usr/bin/env bash
# Installs the three services without Docker or systemd into FZH_ROOT (config.env, default ~/fzh):
# builds shadNet (server/shadnet/build-native.sh) unless bin/shadnet exists, copies the Python
# services to app/, and writes shadnet.cfg with a new registration key on the first run.
# Safe to run again; run it after copying a new server/ folder, then stop.sh and start.sh.
#
#   bash install.sh
set -euo pipefail
. "$(dirname "$(readlink -f "$0")")/common.sh"

mkdir -p "$FZH_ROOT/app" "$FZH_ROOT/data/shadnet" "$FZH_ROOT/data/backend" "$FZH_ROOT/logs" "$FZH_ROOT/run"
chmod 700 "$FZH_ROOT/data"

if [ ! -x "$FZH_ROOT/bin/shadnet" ]; then
    bash "$SERVER/shadnet/build-native.sh" "$FZH_ROOT"
fi
for f in backend/backend.py backend/catalogue.json backend/shadnet_client.py relay/relay.py account/create_account.py; do
    cp -f "$SERVER/$f" "$FZH_ROOT/app/"
done
for f in worlds.cfg scoreboards.cfg clientversions.cfg; do
    [ -f "$FZH_ROOT/data/shadnet/$f" ] || cp "$FZH_ROOT/bin/defaults/$f" "$FZH_ROOT/data/shadnet/$f"
done
bash "$SERVER/shadnet/write-config.sh" "$FZH_ROOT/data/shadnet/shadnet.cfg" "$KEY" \
    "$GAME_PORT" "$MATCHING_PORT" "$WEBAPI_PORT"

cat <<END
Installed in $FZH_ROOT. Start with: bash start.sh
Open inbound: TCP $GAME_PORT, UDP $MATCHING_PORT, TCP $WEBAPI_PORT, TCP $BACKEND_PORT, UDP $RELAY_PORT.
END
