#!/bin/sh
# shadNet works in the folder of its executable, so it runs from /data to keep its
# configuration, database and logs in the volume.
set -e
cd /data
cp -f /opt/shadnet/shadnet /data/shadnet
for f in worlds.cfg scoreboards.cfg clientversions.cfg; do
    [ -f "$f" ] || cp "/opt/shadnet/defaults/$f" "$f"
done
# Without shadnet.cfg shadNet would start with registration open to anyone.
if [ ! -f shadnet.cfg ]; then
    echo "/data/shadnet.cfg is missing: run server/docker/install.sh first." >&2
    exit 1
fi
exec /data/shadnet "$@"
