#!/usr/bin/env bash
# Interleaved A/B benchmark of two perf_probe binaries: REPEAT rounds (default
# 3) of `A then B` over the five offline views (and the online session with
# ONLINE=1, the 4x MSAA + bloom one too with PREFS_AA_BLOOM), so machine drift
# hits both alike. Then:
#   python3 tools/perf/summarize.py --ab OUTDIR A B lumb6 lumb40 east river castle [online online-aa]
# prints, per view and metric, the median over rounds of each round's
# per-frame median (times) or mean (counts).
#
#   tools/perf/abtest.sh BIN_A NAME_A BIN_B NAME_B OUTDIR
set -uo pipefail
A=$1; NA=$2; B=$3; NB=$4; OUT=$5
HERE="$(cd "$(dirname "$0")" && pwd)"
for r in $(seq 1 "${REPEAT:-3}"); do
  for pair in "$A $NA" "$B $NB"; do
    set -- $pair
    "$HERE/views.sh" "$1" "$OUT" "$2$r" stats "${FRAMES:-300}" >/dev/null
    if [ "${ONLINE:-0}" = 1 ]; then
      "$HERE/online.sh" "$1" "$OUT" "$2$r-online" stats "${CYCLES:-600}" >/dev/null
      if [ -n "${PREFS_AA_BLOOM:-}" ]; then
        "$HERE/online.sh" "$1" "$OUT" "$2$r-online-aa" stats "${CYCLES:-600}" "$PREFS_AA_BLOOM" >/dev/null
      fi
    fi
  done
done
echo "done: $OUT"
