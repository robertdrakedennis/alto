#!/usr/bin/env bash
# The five fixed-clock offline views (Lumbridge at frames 6 and 40, east,
# river, castle) with a perf_probe binary (build.sh).
#
#   tools/perf/views.sh BIN OUTDIR PREFIX MODE [FRAMES] [-- extra client args]
#
# MODE
#   shot   the canonical frames, PNG only (pixel comparisons)
#   trace  the canonical frames, PNG + CLIENT910_PERF_TRACE (trcmp.py)
#   stats  FRAMES frames (default 300) per view, CLIENT910_PERF_STATS rows
#          (summarize.py); the PNG is written at the last frame
# The client runs the faithful renderer unless RENDERER (modern, faithful-gpu)
# says otherwise, so the numbers stay comparable with earlier runs; do not
# pass `--renderer` in the extra arguments.
# Files: OUTDIR/PREFIX-VIEW.{png,log,trace,tsv}. Runs from tools/client910 of
# the repository this script lives in (the pack is server/data/pack).
set -uo pipefail
BIN=$1; OUT=$2; P=$3; MODE=$4; shift 4
FRAMES=300
if [ $# -gt 0 ] && [ "$1" != "--" ]; then FRAMES=$1; shift; fi
[ "${1:-}" = "--" ] && shift
HERE="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$OUT"
cd "$HERE/../client910" || exit 2
for V in ${VIEWS:-lumb6 lumb40 east river castle}; do
  case $V in
    lumb6) F=6; A=();;
    lumb40) F=40; A=();;
    east) F=30; A=(--cam-target 3240 3218 --cam-yaw 4000);;
    river) F=20; A=(--cam-target 3243 3240 --cam-zoom 1.6 --cam-yaw 12000);;
    castle) F=12; A=(--cam-target 3208 3218 --cam-pitch 2400 --cam-zoom 0.7);;
    *) echo "unknown view $V"; exit 2;;
  esac
  unset CLIENT910_PERF_TRACE CLIENT910_PERF_STATS
  case $MODE in
    shot) ;;
    trace) export CLIENT910_PERF_TRACE="$OUT/$P-$V.trace";;
    stats) export CLIENT910_PERF_STATS="$OUT/$P-$V.tsv"; F=$FRAMES;;
    *) echo "unknown mode $MODE"; exit 2;;
  esac
  perl -e 'alarm 600; exec @ARGV' "$BIN" --offline --renderer "${RENDERER:-faithful-gpu}" --fixed-clock 1700000000000 \
    --screenshot "$OUT/$P-$V.png" --screenshot-frame "$F" ${A[@]+"${A[@]}"} "$@" \
    > "$OUT/$P-$V.log" 2>&1 || echo "FAIL $V (see $OUT/$P-$V.log)"
  echo "$(md5 -q "$OUT/$P-$V.png" 2>/dev/null) $P-$V.png"
done
