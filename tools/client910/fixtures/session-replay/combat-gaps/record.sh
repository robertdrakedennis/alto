#!/bin/sh
# Record Abyss, living rock combat and fatal thieving through ordinary client operations on isolated ports.
set -eu
exec python3 "$(dirname "$0")/record.py" "$@"
