#!/bin/sh
# Record the ordinary client UI/network loop on isolated 48xxx ports.
# The Python supervisor waits for and SIGTERMs only its own children.
set -eu
exec python3 "$(dirname "$0")/record.py" "$@"
