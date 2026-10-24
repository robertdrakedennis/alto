#!/usr/bin/env bash
# Phase 6 benchmark: per-frame stats of a perf_probe binary (build.sh) on the
# five fixed-clock offline views (FRAMES frames each, default 300) and two
# online sessions (CYCLES cycles, default 600: default preferences, and
# PREFS_AA_BLOOM when given, 4x MSAA + bloom).
#
#   tools/perf/bench.sh BIN OUTDIR PREFIX
#   python3 tools/perf/summarize.py --table OUTDIR BASEPREFIX NEWPREFIX \
#       lumb6 lumb40 east river castle online online-aa
#
# Stats rows: OUTDIR/PREFIX-<view>.tsv (summarize.py skips the first 60
# frames: the load). The CPU times are the main thread's CPU time (waits for
# the surface or the GPU do not count), the GPU time the frame's submits,
# each waited for (perf_probe.rs docs). Run base and new back to back on an
# otherwise idle machine; repeat with REPEAT=N to see the run-to-run spread.
set -uo pipefail
BIN=$1; OUT=$2; P=$3
HERE="$(cd "$(dirname "$0")" && pwd)"
FRAMES=${FRAMES:-300}; CYCLES=${CYCLES:-600}
"$HERE/views.sh" "$BIN" "$OUT" "$P" stats "$FRAMES" >/dev/null
"$HERE/online.sh" "$BIN" "$OUT" "$P-online" stats "$CYCLES"
if [ -n "${PREFS_AA_BLOOM:-}" ]; then
  "$HERE/online.sh" "$BIN" "$OUT" "$P-online-aa" stats "$CYCLES" "$PREFS_AA_BLOOM"
fi
