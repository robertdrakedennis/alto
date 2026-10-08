#!/bin/sh
# Record NPC ranged and magic through ordinary client operations on isolated ports.
set -eu
exec python3 "$(dirname "$0")/record.py" "$@"
