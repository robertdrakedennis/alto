#!/bin/sh
# Record ordinary client reclaim operations on isolated local ports.
set -eu
exec python3 "$(dirname "$0")/record.py" "$@"
