#!/usr/bin/env bash
# Content traces of the GPU capture replays (recorded UI sessions painted
# through the real GPU painter and interface models, deterministic) with a
# perf_probe test binary (PERF_TESTS=1 build.sh).
#
#   tools/perf/captures.sh TESTBIN OUTDIR PREFIX
#
# Writes OUTDIR/PREFIX-<test>.{trace,log}; compare two prefixes with
# trcmp.py. The binary is run from its build copy (CARGO_MANIFEST_DIR).
set -uo pipefail
BIN=$1; OUT=$2; P=$3
mkdir -p "$OUT"
run() { # name [VAR=value...] test
  local name=$1; shift
  local vars=()
  while [[ "$1" == *=* ]]; do vars+=("$1"); shift; done
  env ${vars[@]+"${vars[@]}"} CLIENT910_PERF_TRACE="$OUT/$P-$name.trace" "$BIN" "$1" --test-threads=1 \
    --include-ignored --exact > "$OUT/$P-$name.log" 2>&1
  local rc=$?
  echo "$name rc=$rc $(grep -E '^test result' "$OUT/$P-$name.log" | head -1)"
}
run equipment_preview ui_equipment_replay::equipment_preview_renders_and_animates
run equipment_stats ui_equipment_replay::equipment_stats_cache_replay
run equipment_armour ui_equipment_replay::equipment_armour_cache_replay
run settings_tabs CLIENT910_SETTINGS_CAPTURE=1 ui_goldens::ui_settings_replay::settings_all_tabs_replay
