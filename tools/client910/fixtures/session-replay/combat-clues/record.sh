#!/bin/sh
# Record combat clues through ordinary client operations on isolated ports.
set -eu
exec python3 "$(dirname "$0")/record.py" "$@"
