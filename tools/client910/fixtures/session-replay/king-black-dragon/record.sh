#!/bin/sh
# Record the cached KBD setup, an ordinary full-health fight and owned loot.
set -eu
exec python3 "$(dirname "$0")/record.py" "$@"
