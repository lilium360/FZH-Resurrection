#!/usr/bin/env bash
# Writes shadnet.cfg with a registration key generated on this machine, once.
# An existing shadnet.cfg or key file is left as it is.
#
#   bash write-config.sh <shadnet.cfg> <key file> [game port] [matching udp port] [webapi port]
set -euo pipefail

CFG="${1:?usage: write-config.sh <shadnet.cfg> <key file> [game] [udp] [webapi]}"
KEY="${2:?usage: write-config.sh <shadnet.cfg> <key file> [game] [udp] [webapi]}"
GAME_PORT="${3:-31313}"
UDP_PORT="${4:-31314}"
WEBAPI_PORT="${5:-31315}"

umask 077
if [ ! -s "$KEY" ]; then
    ( set +o pipefail; tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 40 > "$KEY" )
    if [ "$(wc -c < "$KEY")" -ne 40 ]; then
        echo "Could not generate the registration key." >&2
        exit 1
    fi
    echo "Registration key generated in $KEY."
fi
chmod 600 "$KEY"

if [ -f "$CFG" ]; then
    echo "$CFG exists: left unchanged."
    exit 0
fi
{
    cat <<CFG
[General]
Host=0.0.0.0
UnsecuredPort=$GAME_PORT
MatchingUdpPort=$UDP_PORT
WebApiPort=$WEBAPI_PORT
Matching2Enabled=true
Matching2MaxUsers=0
TrophiesEnabled=true
StatsEnabled=false
MemberApiEnabled=false
AdminApiEnabled=false
EmailValidated=false
LogFile=shadnet.log
LogMaxSizeMB=10
CFG
    printf 'RegistrationSecretKey=%s\n' "$(cat "$KEY")"
} > "$CFG"
chmod 600 "$CFG"
echo "Wrote $CFG."
