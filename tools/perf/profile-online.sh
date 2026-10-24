#!/usr/bin/env bash
# Engine-profiler run of an online session with two region changes (lane
# E-A4; the A/B evidence for region streaming and the job pool): online.sh's
# session (fresh dev servers on private ports, fixed clock, the client exits
# after its screenshot at CYCLES) with a `--features profile` client,
# CLIENT910_PROFILE_OUT set and two server teleports (Lumbridge -> Varrock ->
# Lumbridge, each a REBUILD_NORMAL), then profsum.py's report.
#
#   CARGO_TARGET_DIR=... cargo build --release -p client910 --features profile
#   tools/perf/profile-online.sh BIN OUTDIR PREFIX [CYCLES]
#
# Files: OUTDIR/PREFIX.csv (the dump), PREFIX.txt (the report), PREFIX.log,
# PREFIX.png. Ports: PERF_PORT (online.sh; default 39914) and +1. The
# teleports are the 4th and 5th server commands (one per 100 cycles once
# the map is ready), so CYCLES (default 900) must leave room for both.
set -uo pipefail
if [ $# -lt 3 ]; then
  sed -n '2,17p' "$0"
  exit 2
fi
BIN=$1; OUT=$2; P=$3; CYCLES=${4:-900}
[ -x "$BIN" ] || { echo "not an executable: $BIN" >&2; exit 2; }
case $CYCLES in ''|*[!0-9]*) echo "CYCLES must be a number: $CYCLES" >&2; exit 2;; esac
case $P in ''|*/*) echo "PREFIX must be a plain name: $P" >&2; exit 2;; esac
HERE="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
rm -f "$OUT/$P.csv"
CLIENT910_PROFILE_OUT="$OUT/$P.csv" "$HERE/online.sh" "$BIN" "$OUT" "$P" shot "$CYCLES" -- \
  --server-command "tele 3212 3424 0" --server-command "tele 3222 3218 0"
[ -s "$OUT/$P.csv" ] || { echo "no dump written (a --features profile build?)" >&2; exit 1; }
python3 "$HERE/profsum.py" "$OUT/$P.csv" > "$OUT/$P.txt"
echo "report: $OUT/$P.txt"
