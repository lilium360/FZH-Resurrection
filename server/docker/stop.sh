#!/usr/bin/env bash
# Stops the three services. Data and images stay; start again with install.sh.
#
#   sudo bash stop.sh
set -euo pipefail
. "$(dirname "$(readlink -f "$0")")/common.sh"
become_root "$@"
compose down
echo "Services stopped. The data stays in $DATA."
