#!/usr/bin/env bash
# The modern renderer's measurements: stage an instrumented copy of
# the tree (modern_stage.py; the repo crate is never edited), then
#
#   tools/perf/modern_bench.sh build [SOURCE]      the headless bench test binary
#   tools/perf/modern_bench.sh client [SOURCE]     a perf client (build.sh with the
#                                               engine profiler) whose modern
#                                               renderer writes MODERN_PERF_OUT rows
#   tools/perf/modern_bench.sh run OUTDIR TAG      run the bench (env: MODERN_BENCH_*,
#                                               see modern_perf_bench.rs)
#   tools/perf/modern_bench.sh views OUTDIR TAG    the offline views as raw RGBA frames
#                                               (modern_views: byte comparisons between
#                                               two builds, the anti-aliasing change)
#   tools/perf/modern_bench.sh settings-views OUTDIR TAG
#                                               one settled frame per scene and settings
#                                               combination (modern_views_diff.py compares)
#   tools/perf/modern_bench.sh shadowviews OUTDIR TAG
#                                               every shadow quality with point
#                                               shadows off and on, settled and
#                                               10 frames later, then a moving
#                                               and a turning camera (raw RGBA)
#   tools/perf/modern_bench.sh ambient OUTDIR TAG the per-square ambient capture's cost
#                                               (verified look): per frame the faces
#                                               captured, CPU and GPU times, until settled
#   tools/perf/modern_bench.sh hitches OUTDIR TAG  cold start, region and settings
#                                               changes: worst frames (lane P5)
#   tools/perf/modern_bench.sh check OUTDIR TAG BASELINE.tsv [TOL]
#                                               the regression guard (modern_bench_sum.py)
#
# SOURCE defaults to this worktree. Needs CARGO_TARGET_DIR. The bench binary:
# $CARGO_TARGET_DIR/perf-bin/modern-bench (MODERN_BENCH_BIN names another binary in
# perf-bin, e.g. a copy of the base build's); the client: perf-bin/client910-modernperf.
set -euo pipefail
: "${CARGO_TARGET_DIR:?set CARGO_TARGET_DIR}"
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
CMD=${1:?build|client|run|views|shadowviews|ambient|check}
DEST="$CARGO_TARGET_DIR/perf-src/modernperf"
BIN_ROOT="$CARGO_TARGET_DIR/perf-bin"
mkdir -p "$BIN_ROOT"
case $CMD in
  build)
    SRC=${2:-$REPO}
    python3 "$HERE/modern_stage.py" "$SRC" "$DEST"
    EXE=$(cd "$DEST/tools" && CARGO_TARGET_DIR="$CARGO_TARGET_DIR/perf-target/modernperf" \
      cargo test --release --offline -q -p rs910-render-modern --lib --no-run --message-format=json 2>/dev/null \
      | python3 -c "import sys,json; print([m['executable'] for m in map(json.loads, sys.stdin) if m.get('reason')=='compiler-artifact' and m.get('executable') and m['target']['name']=='rs910_render_modern'][-1])")
    cp "$EXE" "$BIN_ROOT/modern-bench"
    # The tests find the pack from their build copy's manifest directory.
    echo "$BIN_ROOT/modern-bench"
    ;;
  client)
    SRC=${2:-$REPO}
    python3 "$HERE/modern_stage.py" "$SRC" "$DEST"
    PERF_FEATURES=profile "$HERE/build.sh" modernperf "$DEST"
    ;;
  run)
    OUT=${2:?OUTDIR}; TAG=${3:?TAG}
    mkdir -p "$OUT"
    OUT="$(cd "$OUT" && pwd)"
    load=$(sysctl -n vm.loadavg)
    echo "load before: $load" | tee -a "$OUT/$TAG.log"
    (cd "$DEST/tools/client910/crates/rs910-render-modern" && \
      MODERN_BENCH_OUT="$OUT" MODERN_BENCH_TAG="$TAG" perl -e 'alarm 7200; exec @ARGV' \
      "$BIN_ROOT/${MODERN_BENCH_BIN:-modern-bench}" frame::perf_bench::modern_matrix --exact --ignored --nocapture --test-threads=1) \
      2>&1 | tee -a "$OUT/$TAG.log"
    ;;
  views)
    OUT=${2:?OUTDIR}; TAG=${3:?TAG}
    mkdir -p "$OUT"
    OUT="$(cd "$OUT" && pwd)"
    (cd "$DEST/tools/client910/crates/rs910-render-modern" && \
      MODERN_BENCH_OUT="$OUT" MODERN_BENCH_TAG="$TAG" perl -e 'alarm 7200; exec @ARGV' \
      "$BIN_ROOT/${MODERN_BENCH_BIN:-modern-bench}" frame::perf_bench::modern_views --exact --ignored --nocapture --test-threads=1) \
      2>&1 | tee -a "$OUT/$TAG.log"
    ;;
  shadowviews)
    OUT=${2:?OUTDIR}; TAG=${3:?TAG}
    mkdir -p "$OUT"
    OUT="$(cd "$OUT" && pwd)"
    (cd "$DEST/tools/client910/crates/rs910-render-modern" && \
      MODERN_BENCH_OUT="$OUT" MODERN_BENCH_TAG="$TAG" perl -e 'alarm 7200; exec @ARGV' \
      "$BIN_ROOT/${MODERN_BENCH_BIN:-modern-bench}" frame::perf_bench::modern_shadow_views --exact --ignored --nocapture --test-threads=1) \
      2>&1 | tee -a "$OUT/$TAG.log"
    ;;
  settings-views)
    # One settled frame per scene and settings combination (modern_settings_views;
    # MODERN_VIEWS_COMBOS filters); compare with modern_views_diff.py.
    OUT=${2:?OUTDIR}; TAG=${3:?TAG}
    mkdir -p "$OUT"
    OUT="$(cd "$OUT" && pwd)"
    (cd "$DEST/tools/client910/crates/rs910-render-modern" && \
      MODERN_BENCH_OUT="$OUT" MODERN_BENCH_TAG="$TAG" perl -e 'alarm 7200; exec @ARGV' \
      "$BIN_ROOT/${MODERN_BENCH_BIN:-modern-bench}" frame::perf_bench::modern_settings_views --exact --ignored --nocapture --test-threads=1) \
      2>&1 | tee -a "$OUT/$TAG.log"
    ;;
  ambient)
    # The captured ambient's cost (modern_ambient_cost): OUT/TAG-ambient-SCENE.tsv
    # and the medians of the capture frames against the settled ones in the log.
    OUT=${2:?OUTDIR}; TAG=${3:?TAG}
    mkdir -p "$OUT"
    OUT="$(cd "$OUT" && pwd)"
    (cd "$DEST/tools/client910/crates/rs910-render-modern" && \
      MODERN_BENCH_OUT="$OUT" MODERN_BENCH_TAG="$TAG" perl -e 'alarm 3600; exec @ARGV' \
      "$BIN_ROOT/${MODERN_BENCH_BIN:-modern-bench}" frame::perf_bench::modern_ambient_cost --exact --ignored --nocapture --test-threads=1) \
      2>&1 | tee -a "$OUT/$TAG.log"
    ;;
  hitches)
    # Cold start, region changes and settings changes of one renderer as the
    # client makes it (modern_hitches; MODERN_HITCH_ROUTE, MODERN_HITCH_FRAMES,
    # MODERN_BENCH_COLD=1 for a cold Metal shader cache):
    # OUT/TAG-hitches.tsv, OUT/TAG-hitch-frames.tsv.
    OUT=${2:?OUTDIR}; TAG=${3:?TAG}
    mkdir -p "$OUT"
    OUT="$(cd "$OUT" && pwd)"
    (cd "$DEST/tools/client910/crates/rs910-render-modern" && \
      MODERN_BENCH_OUT="$OUT" MODERN_BENCH_TAG="$TAG" perl -e 'alarm 1800; exec @ARGV' \
      "$BIN_ROOT/${MODERN_BENCH_BIN:-modern-bench}" frame::perf_bench::modern_hitches --exact --ignored --nocapture --test-threads=1) \
      2>&1 | tee -a "$OUT/$TAG.log"
    ;;
  check)
    OUT=${2:?OUTDIR}; TAG=${3:?TAG}; BASE=${4:?BASELINE}
    python3 "$HERE/modern_bench_sum.py" check "$OUT/$TAG-summary.tsv" "$BASE" "${5:-0.15}"
    ;;
  *) echo "unknown command $CMD" >&2; exit 2;;
esac
