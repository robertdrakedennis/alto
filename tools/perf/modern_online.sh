#!/usr/bin/env bash
# One online session for the renderer's measurements :
# fresh dev lobby and world servers on private ports with an empty player
# store, the perf client (`modern_bench.sh client`: the engine profiler, the
# perf probe and the modern hook) logged in with `--renderer modern` on the fixed
# clock, teleported to Draynor (a region change) and run to CYCLES logic
# cycles, then a screenshot and exit.
#
#   tools/perf/modern_online.sh BIN OUTDIR PREFIX CYCLES WxH [PREFS_FILE] [-- extra client args]
#
# Files: OUTDIR/PREFIX-modern.tsv (modern hook rows: draw CPU, phases, per-pass GPU
# timestamps, counters), PREFIX-prof.csv (engine profiler; profsum.py),
# PREFIX-stats.tsv (perf probe: redraw CPU, GPU, allocations), PREFIX.log,
# PREFIX.png, PREFIX-load.txt. The CLIENT910_MODERN_* switches pass through the
# environment (e.g. CLIENT910_MODERN_SHADOWS=high, CLIENT910_MODERN_FAR=4).
# PREFS_FILE seeds preferences.dat (e.g. the 4x MSAA block). Ports:
# PERF_PORT (default 48100) and +1; only the two servers started here are
# stopped (by PID). Hard timeout: 600 s for the client. PERF_TELE (default
# "3093 3250 0", Draynor) is the teleport target ("x z level").
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
exec python3 "$HERE/online_session.py" modern "$@"
