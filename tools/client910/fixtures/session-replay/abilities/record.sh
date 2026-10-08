#!/bin/sh
# Use the ordinary client input/network recorder on this lane's owned ports.
set -eu
exec python3 "$(dirname "$0")/record.py" "$@"
