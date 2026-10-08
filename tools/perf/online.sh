#!/usr/bin/env bash
# One online session with a perf_probe binary (build.sh): fresh dev lobby and
# world servers on private ports with an empty player store, the client
# logged in with fresh preferences on the fixed clock, CYCLES logic cycles,
# then a screenshot and exit.
#
#   tools/perf/online.sh BIN OUTDIR PREFIX MODE [CYCLES] [PREFS_FILE] [-- extra client args]
#
# MODE is `stats` (CLIENT910_PERF_STATS), `trace` (CLIENT910_PERF_TRACE) or
# `shot`. CYCLES defaults to 400. PREFS_FILE seeds preferences.dat (e.g. one
# with 4x MSAA and bloom). Ports: PERF_PORT (default 48110) and +1. Only the
# two server processes this script starts are stopped (by PID).
# The client runs the faithful renderer unless RENDERER (modern, faithful-gpu,
# null) says otherwise, so the numbers stay comparable with earlier runs; do
# not pass `--renderer` in the extra arguments.
# Online sessions are not run-to-run deterministic (server timing), so use
# them for the per-frame numbers, not for exact trace equality.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
exec python3 "$HERE/online_session.py" faithful "$@"
