#!/usr/bin/env bash
# Containers, listening ports, number of accounts, last log lines.
#
#   sudo bash status.sh
set -uo pipefail
. "$(dirname "$(readlink -f "$0")")/common.sh"
become_root "$@"

echo "--- Services"
for s in shadnet backend relay; do
    printf '  %-8s %s\n' "$s" "$(health "$s")"
done
compose ps --format '  {{.Service}}: {{.Status}}'

echo "--- Listening ports (expected: tcp 31313 31315 1338, udp 31314 31316)"
if command -v ss >/dev/null 2>&1; then
    ss -H -lntu | awk '{ print $1, $5 }' | grep -E ':(31313|31314|31315|1338|31316)$' | sort -u | sed 's/^/  /'
else
    compose ps --format '  {{.Service}}: {{.Ports}}'
fi

echo "--- shadNet accounts"
if [ -f "$DATA/shadnet/db/shadnet.db" ]; then
    # Read-only, with the backend image's Python and no network.
    docker run --rm --network none --user 0 -v "$DATA/shadnet/db:/db" "$PROJECT-backend" python -c "
import sqlite3
c = sqlite3.connect('file:/db/shadnet.db?mode=ro', uri=True)
print('  %d' % c.execute('select count(*) from account').fetchone()[0])" 2>/dev/null \
        || echo "  (not readable now)"
else
    echo "  (no database yet)"
fi

echo "--- Disk used by the data ($DATA)"
du -sh "$DATA/shadnet" "$DATA/backend" 2>/dev/null | sed 's/^/  /'

echo "--- Last log lines"
for s in shadnet backend relay; do
    echo "[$s]"
    compose logs --no-log-prefix --tail 6 "$s" 2>/dev/null | sed -e 's/\x1b\[[0-9;]*[A-Za-z]//g' -e 's/^/  /'
done
if [ -f "$DATA/backend/backend-requests.log" ]; then
    echo "[backend requests]"
    grep -E '^[0-9]{2}:[0-9]{2}:[0-9]{2} |^=== ' "$DATA/backend/backend-requests.log" | tail -n 6 | cut -c1-160 | sed 's/^/  /'
fi
